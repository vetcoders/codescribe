import SwiftUI
import AppKit

// View model for the dictation overlay, backed by the redesign hotkey/controller
// bridge (`CodescribeHotkeys` / `CsTranscriptionListener`).
//
// The view talks only to the thin `DictationEngine` protocol below, so #Preview
// renders standalone against `MockDictationEngine`.
//
// TRANSCRIPT MODEL (new bridge semantics):
//   on_preview    → interim utterance; replace active text or commit+append when
//                   the stream advances to a new spoken fragment.
//   on_correction → targeted replacement when previous_text matches; otherwise
//                   preserve visible text and append the corrected fragment.
//   on_final      → completed VAD-bounded utterance → commit + clear preview.
//   on_vad_active → speech start/stop → drives the WaveformView pulse.
//   on_no_speech / on_error → transient toast.
//
// AMPLITUDE GAP unchanged: the FFI exposes no audio-level callback, so the
// waveform is ambient (synthetic eq) and merely gated on VAD activity.

// MARK: - Engine seam (orchestrator injects the real adapter in App.swift)

/// Minimal slice of the controller-backed dictation surface the overlay needs.
/// Kept as a protocol so the view-model + preview compile without a live Rust core.
protocol DictationEngine: AnyObject {
    func setListener(_ listener: CsTranscriptionListener)
    func startRecording(language: CsLanguage?) async throws
    func stopRecording() async throws -> String
    func isRecording() async -> Bool
    func initModel() async throws
    func isModelLoaded() -> Bool
    func isFormattingAvailable() -> Bool
    func formatText(text: String, language: CsLanguage?) async throws -> String
    func transcribeFile(path: String) async throws -> CsTranscription
}

/// Two-state machine mirrored from the mock: live dictation vs the finalized
/// transcript returned by `stopRecording`.
enum OverlayMode: Equatable {
    case listening
    case formatted
}

@MainActor
final class OverlayState: ObservableObject {

    // MARK: Published state
    @Published var mode: OverlayMode = .listening
    @Published var preview: String = ""        // current utterance interim
    @Published var committedUtterances: [String] = [] // accumulated finals, one item per utterance
    @Published var formattedText: String = ""  // finalized transcript after stop
    @Published var vadActive: Bool = false     // drives the WaveformView pulse
    @Published var toast: String?              // transient no-speech / error notice
    @Published var errorMessage: String?
    @Published var isFormatting: Bool = false

    // MARK: Injected collaborators (all optional so #Preview renders standalone)
    /// The recording core. Injected by the orchestrator. Do NOT instantiate here.
    var engine: DictationEngine?
    /// Handoff to the agent surface — wired by the orchestrator (routes the text
    /// into AgentChat, which streams it through `CodescribeAgent.streamReply`).
    var onSendToAgent: ((String) -> Void)?
    /// Dismiss the floating window — wired by the orchestrator.
    var onClose: (() -> Void)?
    var onRecordingStarted: (() -> Void)?
    var onRecordingStopped: (() -> Void)?

    /// Strong ref so the Rust-side callback (held via the UniFFI handle map) and
    /// our hop-to-main bridge stay alive for the lifetime of the overlay.
    private lazy var listener: CsTranscriptionListener = DictationListener(state: self)

    private var recording = false
    private var toastTask: Task<Void, Never>?
    private var mockRevealTask: Task<Void, Never>?

    init() {}

    func attach() {
        engine?.setListener(listener)
    }

    // MARK: Derived display (one source of truth for the view)

    var statusText: String { mode == .listening ? "recording" : "Idle" }
    var statusColor: Color { mode == .listening ? CSColor.terracotta : CSColor.oliveLight }
    var statusRippling: Bool { mode == .listening }

    var tagText: String { mode == .listening ? "DICTATION" : "FINAL" }
    var tagColor: Color { mode == .listening ? CSColor.terracottaLight : CSColor.oliveLight }

    var metaText: String { mode == .listening ? "live preview · raw" : "final · transcript" }
    var footerRight: String {
        if isFormatting { return "formatting" }
        return mode == .listening ? "vad-gated preview" : "editable"
    }

    /// committed finals + the current interim preview, space-joined.
    var liveText: String {
        (committedUtterances + [preview])
            .filter { !$0.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty }
            .joined(separator: " ")
    }

    /// Text shown in the listening body, with the mock's "listening…" placeholder.
    var listeningDisplay: String { liveText.isEmpty ? "listening…" : liveText }

    /// Whatever the action row should copy/send for the current state.
    var activeText: String { mode == .listening ? liveText : formattedText }

    var canFormat: Bool {
        mode == .formatted
            && !isFormatting
            && engine?.isFormattingAvailable() == true
            && !formattedText.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
    }

    // MARK: Recording lifecycle (engine-backed; no-op when engine is absent)

    /// Start mic dictation. Gated on `micPermissionGranted()`; requests access
    /// once when undetermined. Fires the async bridge work in a Task so the view
    /// can call it from a synchronous context (onAppear / hotkey).
    func start(language: CsLanguage? = nil) {
        guard engine != nil, !recording else { return }
        Task { @MainActor in await self.runStart(language: language) }
    }

    /// Stop the mic and flip to the finalized transcript returned by the core.
    func stop() {
        guard engine != nil, recording else { return }
        Task { @MainActor in await self.runStop() }
    }

    private func runStart(language: CsLanguage?) async {
        guard let engine else { return }
        guard micPermissionGranted() || requestMicPermission() else {
            showToast("Microphone access denied")
            return
        }
        engine.setListener(listener)
        mode = .listening
        preview = ""
        committedUtterances = []
        formattedText = ""
        isFormatting = false
        errorMessage = nil
        recording = true
        do {
            if !engine.isModelLoaded() { try await engine.initModel() }
            try await engine.startRecording(language: language)
        } catch {
            recording = false
            errorMessage = "Couldn't start recording: \(error)"
            showToast("Couldn't start recording")
        }
    }

    func formatTranscript() {
        guard let engine, canFormat else { return }
        let source = formattedText
        isFormatting = true
        Task { @MainActor in
            defer { self.isFormatting = false }
            do {
                let formatted = try await engine.formatText(text: source, language: nil)
                self.formattedText = formatted.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty ? source : formatted
                self.mode = .formatted
            } catch {
                self.errorMessage = "Couldn't format transcript: \(error)"
                self.showToast("Couldn't format transcript")
            }
        }
    }

    private func runStop() async {
        guard let engine else { return }
        do {
            let raw = try await engine.stopRecording()
            recording = false
            vadActive = false
            formattedText = raw.isEmpty ? liveText : raw
            mode = .formatted
        } catch {
            recording = false
            errorMessage = "Couldn't finalize transcript: \(error)"
            showToast("Couldn't finalize transcript")
        }
    }

    // MARK: Action row

    func copyToPasteboard() {
        let pb = NSPasteboard.general
        pb.clearContents()
        pb.setString(activeText, forType: .string)
    }

    func sendToAgent() {
        onSendToAgent?(activeText)
    }

    func close() {
        mockRevealTask?.cancel()
        toastTask?.cancel()
        if recording, let engine {
            recording = false
            Task { @MainActor in _ = try? await engine.stopRecording() }
        }
        vadActive = false
        onClose?()
    }

    func handleRecordingStarted() {
        let wasRecording = recording
        mode = .listening
        if !wasRecording {
            preview = ""
            committedUtterances = []
            formattedText = ""
            isFormatting = false
            errorMessage = nil
        }
        recording = true
        onRecordingStarted?()
    }

    func finishControllerRecording() {
        recording = false
        vadActive = false
        formattedText = liveText
        mode = .formatted
    }

    // MARK: Listener-driven mutations (called on the main actor by DictationListener)

    func applyPreview(_ text: String) {
        let next = text.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !next.isEmpty else { return }
        mode = .listening
        if preview.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
            preview = next
            return
        }
        if isSamePreviewUtterance(current: preview, next: next) {
            preview = next
            return
        }
        commitPreviewIfNeeded()
        preview = next
    }

    func applyCorrection(_ text: String, previousText: String) {
        let corrected = text.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !corrected.isEmpty else { return }

        mode = .listening
        let previous = previousText.trimmingCharacters(in: .whitespacesAndNewlines)
        if replacesActivePreview(previous: previous, corrected: corrected) {
            return
        }
        if replacesCommittedUtterance(previous: previous, corrected: corrected) {
            return
        }

        if !preview.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
            if isSamePreviewUtterance(current: preview, next: corrected) {
                preview = corrected
            } else {
                commitPreviewIfNeeded()
                preview = corrected
            }
            return
        }

        if committedUtterances.last.map({ normalized($0) == normalized(corrected) }) != true {
            committedUtterances.append(corrected)
        }
    }

    func applyFinal(_ text: String) {
        let trimmed = text.trimmingCharacters(in: .whitespacesAndNewlines)
        if !trimmed.isEmpty {
            if normalized(preview) == normalized(trimmed) {
                preview = ""
            }
            if committedUtterances.last.map({ normalized($0) == normalized(trimmed) }) != true {
                committedUtterances.append(trimmed)
            }
        }
        preview = ""
    }

    private func commitPreviewIfNeeded() {
        let active = preview.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !active.isEmpty else { return }
        if committedUtterances.last.map({ normalized($0) == normalized(active) }) != true {
            committedUtterances.append(active)
        }
        preview = ""
    }

    private func isSamePreviewUtterance(current: String, next: String) -> Bool {
        let currentKey = normalized(current)
        let nextKey = normalized(next)
        guard !currentKey.isEmpty, !nextKey.isEmpty else { return false }
        return nextKey.hasPrefix(currentKey)
            || currentKey.hasPrefix(nextKey)
            || substantialTokenOverlap(currentKey, nextKey)
    }

    private func substantialTokenOverlap(_ lhs: String, _ rhs: String) -> Bool {
        let left = Set(lhs.split(separator: " ").map(String.init))
        let right = Set(rhs.split(separator: " ").map(String.init))
        guard min(left.count, right.count) >= 3 else { return false }
        let shared = left.intersection(right).count
        return Double(shared) / Double(min(left.count, right.count)) >= 0.65
    }

    private func replacesActivePreview(previous: String, corrected: String) -> Bool {
        guard !preview.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else { return false }
        if previous.isEmpty {
            guard isSamePreviewUtterance(current: preview, next: corrected) else { return false }
            preview = corrected
            return true
        }
        if normalized(preview) == normalized(previous) {
            preview = corrected
            return true
        }
        return false
    }

    private func replacesCommittedUtterance(previous: String, corrected: String) -> Bool {
        guard !committedUtterances.isEmpty else { return false }
        let previousKey = normalized(previous)
        if let exact = committedUtterances.lastIndex(where: { normalized($0) == previousKey }) {
            committedUtterances[exact] = corrected
            preview = ""
            return true
        }
        if !previousKey.isEmpty,
           let suffix = committedUtterances.indices.reversed().first(where: {
               previousKey.hasSuffix(normalized(committedUtterances[$0]))
                   || normalized(committedUtterances[$0]).hasSuffix(previousKey)
           }) {
            committedUtterances[suffix] = corrected
            preview = ""
            return true
        }
        return false
    }

    private func normalized(_ text: String) -> String {
        text.lowercased()
            .components(separatedBy: CharacterSet.alphanumerics.inverted)
            .filter { !$0.isEmpty }
            .joined(separator: " ")
    }

    func applyVad(_ active: Bool) {
        vadActive = active
    }

    func showToast(_ message: String) {
        toast = message
        toastTask?.cancel()
        toastTask = Task { @MainActor [weak self] in
            try? await Task.sleep(nanoseconds: 2_600_000_000)
            guard !Task.isCancelled else { return }
            self?.toast = nil
        }
    }

    // MARK: Preview / mock helpers (no engine required)

    /// Seeded view model for #Preview in the listening state, with a typing reveal
    /// that imitates `on_preview` arriving char-by-char (mock: 46ms).
    static func previewListening() -> OverlayState {
        let s = OverlayState()
        s.mode = .listening
        s.vadActive = true
        s.beginMockReveal("add a rate limiter to the login route and write a test for it")
        return s
    }

    /// Seeded view model for #Preview in the finalized state.
    static func previewFormatted() -> OverlayState {
        let s = OverlayState()
        s.mode = .formatted
        s.formattedText = "Add a rate limiter to the login route and write a test that covers the throttle window. Keep the existing error shape."
        return s
    }

    func beginMockReveal(_ full: String, interval: Double = 0.046) {
        mockRevealTask?.cancel()
        preview = ""
        mockRevealTask = Task { @MainActor [weak self] in
            var acc = ""
            for ch in full {
                if Task.isCancelled { return }
                acc.append(ch)
                self?.preview = acc
                try? await Task.sleep(nanoseconds: UInt64(interval * 1_000_000_000))
            }
        }
    }
}

/// Adapter for the redesign hotkey/controller path. This is the product path:
/// one `RecordingController`, one event stream, one Swift overlay surface.
final class ControllerDictationEngine: DictationEngine {
    private let hotkeys = CodescribeHotkeys()

    func setListener(_ listener: CsTranscriptionListener) {
        hotkeys.setListener(listener: listener)
    }
    func startRecording(language: CsLanguage?) async throws {
        try await hotkeys.startRecording()
    }
    func stopRecording() async throws -> String {
        try await hotkeys.stopRecording()
        return ""
    }
    func isRecording() async -> Bool {
        await hotkeys.isRecording()
    }
    func initModel() async throws {}
    func isModelLoaded() -> Bool { true }
    func isFormattingAvailable() -> Bool {
        hotkeys.isFormattingAvailable()
    }
    func formatText(text: String, language: CsLanguage?) async throws -> String {
        try await hotkeys.formatText(text: text, language: language)
    }
    func transcribeFile(path: String) async throws -> CsTranscription {
        throw NSError(domain: "CodescribeRedesign", code: 1, userInfo: [
            NSLocalizedDescriptionKey: "File transcription is not available through the hotkey controller."
        ])
    }
}

// MARK: - Listener bridge (Rust callbacks → main actor → OverlayState)

/// Bridges Rust-side `CsTranscriptionListener` callbacks (fired from the core's
/// transcription thread) onto the main actor, driving `OverlayState`. Mirrors the
/// hop pattern used by `StreamListener` in RealChatEngine.
final class DictationListener: CsTranscriptionListener, @unchecked Sendable {
    private weak var state: OverlayState?

    init(state: OverlayState) {
        self.state = state
    }

    func onRecordingStarted() {
        DispatchQueue.main.async { MainActor.assumeIsolated { self.state?.handleRecordingStarted() } }
    }
    func onRecordingStopped() {
        DispatchQueue.main.async { MainActor.assumeIsolated { self.state?.finishControllerRecording() } }
    }
    func onPreview(text: String) {
        DispatchQueue.main.async { MainActor.assumeIsolated { self.state?.applyPreview(text) } }
    }
    func onCorrection(text: String, previousText: String) {
        DispatchQueue.main.async { MainActor.assumeIsolated { self.state?.applyCorrection(text, previousText: previousText) } }
    }
    func onFinal(text: String) {
        DispatchQueue.main.async { MainActor.assumeIsolated { self.state?.applyFinal(text) } }
    }
    func onVadActive(active: Bool) {
        DispatchQueue.main.async { MainActor.assumeIsolated { self.state?.applyVad(active) } }
    }
    func onNoSpeech(reason: String) {
        DispatchQueue.main.async { MainActor.assumeIsolated { self.state?.showToast("No speech: \(reason)") } }
    }
    func onError(message: String) {
        DispatchQueue.main.async {
            MainActor.assumeIsolated {
                self.state?.errorMessage = message
                self.state?.showToast(message)
            }
        }
    }
}

// MARK: - Mock engine for #Preview

#if DEBUG
final class MockDictationEngine: DictationEngine {
    func setListener(_ listener: CsTranscriptionListener) {}
    func startRecording(language: CsLanguage?) async throws {}
    func stopRecording() async throws -> String { "" }
    func isRecording() async -> Bool { false }
    func initModel() async throws {}
    func isModelLoaded() -> Bool { true }
    func isFormattingAvailable() -> Bool { false }
    func formatText(text: String, language: CsLanguage?) async throws -> String { text }
    func transcribeFile(path: String) async throws -> CsTranscription {
        CsTranscription(text: "", language: "en")
    }
}
#endif
