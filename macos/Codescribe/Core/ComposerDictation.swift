import Foundation
import OSLog

/// Diagnostic breadcrumbs for the composer voice-note path. Filter with:
///   log show --predicate 'subsystem == "com.vetcoders.codescribe"' --info
private let dictationLog = Logger(
    subsystem: Bundle.main.bundleIdentifier ?? "com.vetcoders.codescribe",
    category: "composer-dictation"
)

/// Real composer dictation adapter: a thin driver over the `CodescribeDictation`
/// UniFFI bridge (the SAME streaming recorder + Whisper the hotkey/tray dictation
/// uses, exposed through an independent recorder handle). Click-to-start,
/// click-to-stop; on stop the accumulated transcript is appended to the composer
/// draft — never auto-sent. Streaming callbacks update a separate live preview
/// in the composer; `stopRecording()` remains the only source that commits text
/// into the editable draft.
///
/// Deliberately a SEPARATE recorder from `CodescribeHotkeys`: the composer voice
/// note is an independent product path and must not share the overlay/hotkey
/// RecordingController. To avoid two recorders fighting over the microphone, a
/// live hotkey session (surfaced via `store.dictationBlocked`) disables this mic.
///
/// The final transcript is read from `stopRecording()`'s return value — the
/// bridge's `CsEventSink` does not emit `onFinalTranscriptReady`, so no
/// delivery-grade LocalFinalPass formatting is applied on this path; the returned
/// text is the composed presentation-buffer transcript.
@MainActor
final class RealComposerDictation: ComposerDictating {
    private let dictation = CodescribeDictation()
    private weak var store: AgentChatStore?
    /// Strong ref so the foreign listener outlives the Rust-side `Arc` handoff.
    private var listener: ComposerDictationListener?
    /// Whisper is idempotently loaded once, then reused for later notes.
    private var modelReady = false
    /// Guards against re-entrant toggles while an async start/stop is in flight.
    private var transitioning = false

    init(store: AgentChatStore) {
        self.store = store
    }

    func toggle() {
        guard let store, !transitioning else { return }
        switch store.dictationPhase {
        case .recording:
            stop()
        case .idle, .failed:
            start()
        case .preparing:
            break  // mid-transition — ignore until it settles
        }
    }

    private func start() {
        guard let store else { return }
        // Collision guard: a hotkey/tray/overlay dictation session owns the mic.
        if store.dictationBlocked {
            store.reportDictationFailure("Microphone is busy with a shortcut dictation.")
            return
        }
        transitioning = true
        store.clearDictationPreview()
        store.setDictationPhase(.preparing)
        Task { @MainActor in
            defer { transitioning = false }
            guard await Self.ensureMicPermission() else {
                store.reportDictationFailure(
                    "Microphone access is off — enable it in System Settings › Privacy & Security.")
                return
            }
            // Register a fresh listener (held strongly here) before starting; the
            // bridge rejects `startRecording` without one.
            let listener = ComposerDictationListener(store: store)
            self.listener = listener
            dictation.setListener(listener: listener)
            do {
                if !modelReady {
                    try await dictation.initModel()
                    modelReady = true
                }
                try await dictation.startRecording(language: nil)  // auto-detect language
                store.setDictationPhase(.recording)
                dictationLog.info("composer dictation: recording started")
            } catch {
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
            defer { transitioning = false }
            do {
                let transcript = try await dictation.stopRecording()
                let trimmed = transcript.trimmingCharacters(in: .whitespacesAndNewlines)
                if trimmed.isEmpty {
                    dictationLog.info("composer dictation: stopped with empty transcript")
                    store.clearDictationPreview()
                    store.reportDictationFailure("No speech detected.")
                } else {
                    store.appendDictatedTranscript(trimmed)
                    store.setDictationPhase(.idle)
                    dictationLog.info("composer dictation: inserted \(trimmed.count, privacy: .public) chars")
                }
            } catch {
                dictationLog.error("composer dictation stop failed: \(error.localizedDescription, privacy: .public)")
                store.clearDictationPreview()
                store.reportDictationFailure("Couldn't finish recording: \(error.localizedDescription)")
            }
        }
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
    private let lock = NSLock()
    private var committedSegments: [(utteranceId: UInt64, text: String)] = []
    private var activePreview = ""

    init(store: AgentChatStore) {
        self.store = store
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
    func onFinal(utteranceId: UInt64, text: String) {
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
    func onSessionFinalised(sessionId: String, layerSummary: CsLayerSummary) {}
    func onFinalTranscriptReady(text: String) {
        publishFinalPreview(text)
    }
    func onVadActive(active: Bool) {}
    func onNoSpeech(reason: String) {
        dictationLog.info("composer dictation: no speech (\(reason, privacy: .public))")
    }
    func onError(message: String) {
        dictationLog.error("composer dictation engine warning: \(message, privacy: .public)")
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
            store?.updateDictationPreview(trimmed)
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
