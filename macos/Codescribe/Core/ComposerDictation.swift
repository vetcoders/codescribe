import Foundation
import OSLog

/// Diagnostic breadcrumbs for the composer voice-note path. Filter with:
///   log show --predicate 'subsystem == "com.vetcoders.codescribe"' --info
private let dictationLog = Logger(
    subsystem: Bundle.main.bundleIdentifier ?? "com.vetcoders.codescribe",
    category: "composer-dictation"
)

/// Real composer dictation adapter: a thin driver over the `CodescribeDictation`
/// UniFFI bridge (the SAME streaming recorder + Whisper stack the overlay uses,
/// on an independent recorder handle). Click-to-start / click-to-stop inserts
/// into the draft without auto-send. Assistive hotkeys route here via
/// `handle(_:)` and may auto-send on stop unless the user edits the preview
/// (`resolveDictationDelivery` cancels auto-send for edited text).
///
/// Process-wide capture ownership lives in the bridge (`CAPTURE_OWNER`
/// none/overlay/agent via `setAgentCaptureActive`). Claim fails closed when the
/// overlay owns the mic; overlay start fails closed while agent owns capture.
/// `store.dictationBlocked` is an additional UI guard for a live overlay session.
///
/// Live partials stream into `dictationPreview`; `stopRecording()` is the only
/// path that commits into the editable draft. When final text regresses against
/// a longer live canvas, delivery keeps live and preserves the final alternative.
@MainActor
final class RealComposerDictation: ComposerDictating {
    private let dictation = CodescribeDictation()
    private let hotkeys = CodescribeHotkeys()
    private weak var store: AgentChatStore?
    /// Strong ref so the foreign listener outlives the Rust-side `Arc` handoff.
    private var listener: ComposerDictationListener?
    /// Whisper is idempotently loaded once, then reused for later notes.
    private var modelReady = false
    /// Guards against re-entrant toggles while an async start/stop is in flight.
    private var transitioning = false
    private var autoSendOnStop = false
    private var pendingStopAfterStart = false

    init(store: AgentChatStore) {
        self.store = store
    }

    func toggle() {
        guard let store, !transitioning else { return }
        switch store.dictationPhase {
        case .recording:
            stop()
        case .idle, .failed:
            start(autoSend: false)
        case .preparing:
            break  // mid-transition — ignore until it settles
        }
    }

    func handle(_ command: ComposerCaptureCommand) {
        guard let store else { return }
        if transitioning {
            switch command {
            case .stopAssistive, .toggleAssistive:
                pendingStopAfterStart = true
            case .startAssistive:
                break
            }
            return
        }
        switch command {
        case .startAssistive:
            if store.dictationPhase != .recording { start(autoSend: true) }
        case .stopAssistive:
            if store.dictationPhase == .recording { stop() }
        case .toggleAssistive:
            if store.dictationPhase == .recording { stop() } else { start(autoSend: true) }
        }
    }

    private func start(autoSend: Bool) {
        guard let store else { return }
        // Collision guard: a hotkey/tray/overlay dictation session owns the mic.
        if store.dictationBlocked {
            store.reportDictationFailure("Microphone is busy with a shortcut dictation.")
            return
        }
        transitioning = true
        autoSendOnStop = autoSend
        store.beginDictationPreviewSession()
        store.setDictationPhase(.preparing)
        guard hotkeys.setAgentCaptureActive(active: true) else {
            transitioning = false
            store.reportDictationFailure("Transcription overlay already owns the microphone")
            return
        }
        Task { @MainActor in
            defer {
                transitioning = false
                if pendingStopAfterStart {
                    pendingStopAfterStart = false
                    if store.dictationPhase == .recording {
                        Task { @MainActor [weak self] in self?.stop() }
                    }
                }
            }
            guard await Self.ensureMicPermission() else {
                _ = hotkeys.setAgentCaptureActive(active: false)
                store.reportDictationFailure(
                    "Microphone access is off — enable it in System Settings › Privacy & Security.")
                return
            }
            // Register a fresh listener (held strongly here) before starting; the
            // bridge rejects `startRecording` without one.
            let listener = ComposerDictationListener(store: store) { [weak self] message in
                Task { @MainActor [weak self] in
                    self?.handleEngineError(message: message)
                }
            }
            self.listener = listener
            dictation.setListener(listener: listener)
            do {
                // Optional Whisper warm: Apple-live must start even when weights are
                // missing (gap-fill degraded for the session). Bridge initModel is
                // soft-fail for Apple; keep recording start unblocked either way.
                if !modelReady {
                    do {
                        try await dictation.initModel()
                        modelReady = true
                    } catch {
                        dictationLog.warning(
                            "composer dictation: Whisper warm skipped (degraded gap-fill): \(error.localizedDescription, privacy: .public)"
                        )
                        // Leave modelReady false so a later session can retry.
                    }
                }
                try await dictation.startRecording(language: nil)  // auto-detect language
                store.setDictationPhase(.recording)
                dictationLog.info("composer dictation: recording started")
            } catch {
                _ = hotkeys.setAgentCaptureActive(active: false)
                dictationLog.error("composer dictation start failed: \(error.localizedDescription, privacy: .public)")
                store.clearDictationPreview()
                store.reportDictationFailure("Couldn't start recording: \(error.localizedDescription)")
            }
        }
    }

    private func stop() {
        guard let store else { return }
        transitioning = true
        store.setDictationPhase(.preparing)
        Task { @MainActor in
            defer {
                transitioning = false
                _ = hotkeys.setAgentCaptureActive(active: false)
            }
            do {
                let transcript = try await dictation.stopRecording()
                let resolution = store.resolveDictationDelivery(
                    final: transcript,
                    autoSend: autoSendOnStop
                )
                let trimmed = resolution.text
                if trimmed.isEmpty {
                    dictationLog.info("composer dictation: stopped with empty transcript")
                    store.clearDictationPreview()
                    store.reportDictationFailure("No speech detected.")
                } else {
                    store.setDictationPhase(.idle)
                    var deliveredViaVoiceLane = false
                    if resolution.autoSend {
                        // Voice lane first: the controller attaches the
                        // trigger-time selection context and the context bucket
                        // (HOTKEYS_CONTRACT "captured in the trigger handler"),
                        // and the turn streams as a core-owned voice turn — the
                        // composer FIFO already skips threads with an active
                        // voice turn. A plain `store.send()` here delivered the
                        // spoken text alone (review P0-02).
                        deliveredViaVoiceLane =
                            (try? await hotkeys.sendAssistiveTranscript(text: trimmed)) ?? false
                    }
                    if deliveredViaVoiceLane {
                        store.clearDictationPreview()
                        dictationLog.info(
                            "composer dictation: assistive turn delivered via voice lane")
                    } else {
                        store.appendDictatedTranscript(trimmed)
                        if resolution.autoSend {
                            store.send()
                        }
                        dictationLog.info(
                            "composer dictation: inserted \(trimmed.count, privacy: .public) chars")
                    }
                    // Analytics must never delay transcript delivery or agent send.
                    Task { @MainActor in
                        _ = await ActivationPing.shared.recordFirstSuccessfulDictation()
                    }
                }
            } catch {
                dictationLog.error("composer dictation stop failed: \(error.localizedDescription, privacy: .public)")
                store.clearDictationPreview()
                store.reportDictationFailure("Couldn't finish recording: \(error.localizedDescription)")
            }
        }
    }

    private func handleEngineError(message: String) {
        dictationLog.error("composer dictation engine error: \(message, privacy: .public)")
        guard let store else { return }
        guard transitioning || store.dictationPhase == .preparing || store.dictationPhase == .recording else { return }

        transitioning = false
        listener = nil
        _ = hotkeys.setAgentCaptureActive(active: false)
        store.reportDictationFailure("Dictation stopped: \(message)")
    }

    /// Check (and, if undetermined, request) microphone access. The request wrapper
    /// blocks on the system prompt, so it runs off the main actor.
    private static func ensureMicPermission() async -> Bool {
        if micPermissionGranted() { return true }
        return await Task.detached { requestMicPermission() }.value
    }
}

/// Foreign dictation listener for the composer path. Preview callbacks carry the
/// current uncommitted snapshot, while `onFinal` commits an utterance to the
/// listener's live display buffer. The composer still reads the authoritative
/// final transcript from `stopRecording()` before mutating the draft.
final class ComposerDictationListener: CsTranscriptionListener, @unchecked Sendable {
    private weak var store: AgentChatStore?
    private let onError: (String) -> Void
    private let lock = NSLock()
    private var committedSegments: [(utteranceId: UInt64, text: String)] = []
    private var activePreview = ""

    init(store: AgentChatStore, onError: @escaping (String) -> Void) {
        self.store = store
        self.onError = onError
    }

    func onRecordingPreparing() {}
    func onRecordingStarted() {}
    func onRecordingStopped() {}
    func onRecordingFinalising() {}
    func onPreview(text: String) {
        publishPreview {
            activePreview = text
        }
    }
    func onCorrection(text: String, previousText: String) {}
    func onFinal(
        utteranceId: UInt64, text: String, avgLogprob: Float?, speechPct: Float?,
        confidenceFlags: [String]
    ) {
        publishPreview {
            activePreview = ""
            let trimmed = text.trimmingCharacters(in: .whitespacesAndNewlines)
            guard !trimmed.isEmpty else { return }
            if let index = committedSegments.firstIndex(where: { $0.utteranceId == utteranceId }) {
                committedSegments[index].text = trimmed
            } else {
                committedSegments.append((utteranceId: utteranceId, text: trimmed))
            }
        }
    }
    func onReplaceRange(utteranceId: UInt64, start: UInt64, end: UInt64, text: String, source: CsLayerSource) {}
    func onInsertAnnotation(utteranceId: UInt64, position: UInt64, text: String, kind: CsAnnotationKind) {}
    func onContextMarker(position: UInt64, marker: String) {}
    func onSessionFinalised(sessionId: String, layerSummary: CsLayerSummary) {}
    func onFinalTranscriptReady(text: String) {
        publishFinalPreview(text)
    }
    func onVadActive(active: Bool) {
        Task { @MainActor [weak store] in
            store?.setDictationVadActive(active)
        }
    }
    func onAudioLevel(rms: Float) {}
    func onNoSpeech(reason: String) {
        dictationLog.info("composer dictation: no speech (\(reason, privacy: .public))")
    }
    func onError(message: String) {
        onError(message)
    }

    private func publishPreview(_ update: () -> Void) {
        lock.lock()
        update()
        let snapshot = mergedPreviewLocked()
        lock.unlock()
        Task { @MainActor [weak store] in
            store?.updateDictationPreview(snapshot)
        }
    }

    private func publishFinalPreview(_ text: String) {
        let trimmed = text.trimmingCharacters(in: .whitespacesAndNewlines)
        Task { @MainActor [weak store] in
            store?.noteDictationFinalPreview(trimmed)
        }
    }

    private func mergedPreviewLocked() -> String {
        var parts = committedSegments.map(\.text)
        let active = activePreview.trimmingCharacters(in: .whitespacesAndNewlines)
        if !active.isEmpty {
            parts.append(active)
        }
        return parts.joined(separator: " ")
    }
}
