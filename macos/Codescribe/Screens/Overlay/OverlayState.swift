import SwiftUI
import AppKit

// View model for the dictation overlay, backed by the redesign hotkey/controller
// bridge (`CodescribeHotkeys` / `CsTranscriptionListener`).
//
// The view talks only to the thin `DictationEngine` protocol below, so #Preview
// renders standalone against `MockDictationEngine`.
//
// TRANSCRIPT MODEL (new bridge semantics):
//   on_preview    → interim text; accepts both utterance-local chunks and
//                   cumulative session previews without duplicating committed text.
//   on_correction → targeted replacement when previous_text matches; otherwise
//                   preserve visible text and append the corrected fragment.
//   on_final      → completed VAD-bounded utterance → commit + clear preview.
//   on_vad_active → speech start/stop → drives the WaveformView pulse.
//   on_no_speech / on_error → transient toast.
//
// AMPLITUDE GAP unchanged: the FFI exposes no audio-level callback, so the
// waveform is ambient (synthetic eq) and merely gated on VAD activity.

// MARK: - Engine seam (orchestrator injects the real adapter in App.swift)

private struct OverlayTranscriptAnnotation: Equatable {
    var position: Int
    var text: String
}

private struct OverlayTranscriptSegment: Equatable {
    var utteranceId: UInt64?
    var text: String
    var annotations: [OverlayTranscriptAnnotation] = []

    var renderedText: String {
        guard !annotations.isEmpty else { return text }
        var rendered = text
        for annotation in annotations.sorted(by: { $0.position > $1.position }) {
            let bounded = min(max(annotation.position, 0), rendered.count)
            let index = rendered.index(rendered.startIndex, offsetBy: bounded)
            rendered.insert(contentsOf: " [\(annotation.text)]", at: index)
        }
        return rendered
    }

    mutating func replaceRange(start: UInt64, end: UInt64, replacement: String) -> Bool {
        guard start <= end,
              let startOffset = Int(exactly: start),
              let endOffset = Int(exactly: end),
              endOffset <= text.count else { return false }
        let startIndex = text.index(text.startIndex, offsetBy: startOffset)
        let endIndex = text.index(text.startIndex, offsetBy: endOffset)
        text.replaceSubrange(startIndex..<endIndex, with: replacement)
        annotations = annotations.filter { $0.position <= text.count }
        return true
    }

    mutating func insertAnnotation(position: UInt64, text annotationText: String) -> Bool {
        guard let offset = Int(exactly: position), offset <= text.count else { return false }
        annotations.append(OverlayTranscriptAnnotation(position: offset, text: annotationText))
        return true
    }
}

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
    @Published var audioReady: Bool = false    // recorder confirmed; STT/VAD may still be warming
    @Published var warmingUp: Bool = false     // true after user intent, before audio/VAD proves life
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
    var onRecordingPreparing: (() -> Void)?
    var onRecordingStarted: (() -> Void)?
    var onRecordingStopped: (() -> Void)?

    /// Strong ref so the Rust-side callback (held via the UniFFI handle map) and
    /// our hop-to-main bridge stay alive for the lifetime of the overlay.
    private lazy var listener: CsTranscriptionListener = DictationListener(state: self)

    private var recording = false
    private var committedSegments: [OverlayTranscriptSegment] = []
    /// Authoritative post-stop transcript pushed by the Rust controller
    /// (`on_final_transcript_ready` → LocalFinalPass `final_formatted_text`) — the
    /// SAME text the delivery/paste and tray "Copy" use. When present it is the
    /// FINAL the overlay shows, instead of the raw per-utterance streaming assembly.
    private var authoritativeFinalText: String?
    /// Once a session is finalized (mode `.formatted` / Idle), the transcript is
    /// FROZEN. Late streaming events (Preview/Correction/UtteranceFinal/VAD) that the
    /// engine may still emit during/after teardown are DROPPED instead of mutating
    /// `@Published` state — otherwise each late apply re-invalidates the hosting view
    /// (TextEditor re-layout) and spins the SwiftUI render graph at 100% CPU in Idle.
    /// The authoritative `FinalTranscript` is the only post-finalize update allowed.
    private var finalized = false
    private var toastTask: Task<Void, Never>?
    private var mockRevealTask: Task<Void, Never>?
    /// Belt-and-suspenders guard against an orphaned optimistic "starting" overlay.
    /// The Rust bridge now guarantees a terminal event for every preparing it shows
    /// (`compensate_orphaned_preparing`); this watchdog is the second layer: if no
    /// started/activity/stopped/finish arrives within `warmupWatchdogNanos`, the
    /// overlay dismisses itself instead of hanging on "starting" forever.
    private var warmupWatchdogTask: Task<Void, Never>?
    private static let warmupWatchdogNanos: UInt64 = 4_000_000_000

    init() {}

    func attach() {
        engine?.setListener(listener)
    }

    // MARK: Derived display (one source of truth for the view)

    var statusText: String {
        guard mode == .listening else { return "Idle" }
        return warmingUp ? "starting" : "recording"
    }
    var statusColor: Color { mode == .listening ? CSColor.terracotta : CSColor.oliveLight }
    var statusRippling: Bool { mode == .listening && (audioReady || vadActive) }

    var tagText: String { mode == .listening ? "DICTATION" : "FINAL" }
    var tagColor: Color { mode == .listening ? CSColor.terracottaLight : CSColor.oliveLight }

    var metaText: String { mode == .listening ? "live preview · raw" : "final · transcript" }
    var footerRight: String {
        if isFormatting { return "formatting" }
        if mode == .listening && warmingUp { return "warming up" }
        if mode == .listening && audioReady && liveText.isEmpty { return "audio live" }
        return mode == .listening ? "vad-gated preview" : "editable"
    }

    /// committed finals + the current interim preview, space-joined.
    var liveText: String {
        (committedUtterances + [preview])
            .filter { !$0.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty }
            .joined(separator: " ")
    }

    /// Text shown in the listening body, with the mock's "listening…" placeholder.
    var listeningDisplay: String {
        if !liveText.isEmpty { return liveText }
        return warmingUp ? "starting…" : "listening…"
    }

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
        warmingUp = true
        resetTranscript()
        formattedText = ""
        isFormatting = false
        errorMessage = nil
        recording = true
        do {
            if !engine.isModelLoaded() { try await engine.initModel() }
            try await engine.startRecording(language: language)
        } catch {
            recording = false
            warmingUp = false
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
            // The controller bridge returns "" here; the authoritative transcript
            // is the id-ordered assembly of `UtteranceFinal` events (see liveText).
            _ = try await engine.stopRecording()
            recording = false
            finalizeTranscript()
        } catch {
            recording = false
            warmingUp = false
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
        cancelWarmupWatchdog()
        mockRevealTask?.cancel()
        toastTask?.cancel()
        if recording, let engine {
            recording = false
            Task { @MainActor in _ = try? await engine.stopRecording() }
        }
        vadActive = false
        audioReady = false
        warmingUp = false
        onClose?()
    }

    func prepareForExternalStart() {
        handleRecordingPreparing()
    }

    func handleRecordingPreparing() {
        finalized = false
        mode = .listening
        warmingUp = true
        audioReady = false
        if !recording {
            resetTranscript()
            formattedText = ""
            isFormatting = false
            errorMessage = nil
        }
        recording = true
        onRecordingPreparing?()
        armWarmupWatchdog()
    }

    func handleRecordingStarted() {
        cancelWarmupWatchdog()
        finalized = false
        mode = .listening
        warmingUp = false
        audioReady = true
        if !recording {
            if liveText.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
                resetTranscript()
            }
            formattedText = ""
            isFormatting = false
            errorMessage = nil
        }
        recording = true
        onRecordingStarted?()
    }

    func finishControllerRecording() {
        cancelWarmupWatchdog()
        recording = false
        finalizeTranscript()
    }

    // MARK: Warmup watchdog (orphaned "starting" overlay recovery)

    /// Arm (or re-arm) the warmup watchdog. Called every time an optimistic
    /// "preparing" overlay is shown; a re-arm cancels any prior pending fire so
    /// rapid repeated preparing events collapse to a single 4s window.
    private func armWarmupWatchdog() {
        warmupWatchdogTask?.cancel()
        warmupWatchdogTask = Task { @MainActor [weak self] in
            try? await Task.sleep(nanoseconds: OverlayState.warmupWatchdogNanos)
            guard !Task.isCancelled else { return }
            self?.fireWarmupWatchdog()
        }
    }

    /// Cancel the pending watchdog. Called from every path that proves the session
    /// progressed (started / streaming activity / vad) or terminated (stop /
    /// finalize / close), so a genuine session never trips the fallback dismiss.
    private func cancelWarmupWatchdog() {
        warmupWatchdogTask?.cancel()
        warmupWatchdogTask = nil
    }

    /// Fallback dismiss for a stuck optimistic overlay. Only fires if we are STILL
    /// in the "starting" state (`warmingUp`, not finalized) — if any real event
    /// already progressed us, `warmingUp` is false and this is a no-op.
    private func fireWarmupWatchdog() {
        warmupWatchdogTask = nil
        guard warmingUp, !finalized else { return }
        recording = false
        warmingUp = false
        audioReady = false
        vadActive = false
        resetTranscript()
        mode = .listening
        onClose?()
    }

    // MARK: Listener-driven mutations (called on the main actor by DictationListener)

    /// `Preview` is utterance-LOCAL cumulative: each event carries the full
    /// interim for the current (not-yet-finalised) utterance, and the bridge
    /// clears it on every `UtteranceFinal`. So we simply mirror it — no prefix
    /// matching, no commit-on-mismatch.
    func applyPreview(_ text: String) {
        guard !finalized else { return }
        let next = text.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !next.isEmpty else { return }
        markTranscriptActivity()
        preview = next
        refreshFormattedTranscriptIfNeeded()
    }

    /// `Correction` targets the current utterance. Scope it to the live preview;
    /// if the preview was already finalised, patch only the most-recent committed
    /// segment (and only when `previousText` matches it). Never a free normalized
    /// search across all committed slots.
    func applyCorrection(_ text: String, previousText: String) {
        guard !finalized else { return }
        let corrected = text.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !corrected.isEmpty else { return }
        markTranscriptActivity()

        if !preview.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
            preview = corrected
            refreshFormattedTranscriptIfNeeded()
            return
        }

        let previous = previousText.trimmingCharacters(in: .whitespacesAndNewlines)
        if let lastIndex = committedSegments.indices.last,
           previous.isEmpty || normalized(committedSegments[lastIndex].text) == normalized(previous)
        {
            committedSegments[lastIndex].text = corrected
            committedSegments[lastIndex].annotations = []
            syncCommittedUtterances()
            return
        }

        // No live preview and nothing to patch: surface it as the current interim.
        preview = corrected
        refreshFormattedTranscriptIfNeeded()
    }

    /// `UtteranceFinal` is one completed VAD-bounded utterance, delivered in FIFO
    /// order with a stable `utteranceId`. Key segments by that id and append in id
    /// order — the authoritative ordering the bridge already provides. No lossy
    /// normalized matching, no text-dedup (a legitimately repeated token must not
    /// be dropped).
    func applyFinal(utteranceId: UInt64, _ text: String) {
        guard !finalized else { return }
        markTranscriptActivity()
        // A1 contract sensor (debug-only): Rust trims at source and computes
        // ReplaceRange/InsertAnnotation offsets over that exact string. A Swift-side
        // trim here would silently shift those offsets, so we store the text
        // byte-for-byte and only assert the guarantee.
        assert(
            text == text.trimmingCharacters(in: .whitespacesAndNewlines),
            "UtteranceFinal text not trimmed at source (A1 contract) — ReplaceRange offsets would misalign"
        )
        if !text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
            upsertFinalSegment(utteranceId: utteranceId, text: text)
        }
        preview = ""
        refreshFormattedTranscriptIfNeeded()
    }

    func applyReplaceRange(utteranceId: UInt64, start: UInt64, end: UInt64, text: String) {
        guard !finalized else { return }
        guard let index = committedSegments.lastIndex(where: { $0.utteranceId == utteranceId }) else {
            showToast("Skipped unbound transcript patch")
            return
        }
        guard committedSegments[index].replaceRange(start: start, end: end, replacement: text) else {
            showToast("Skipped out-of-range transcript patch")
            return
        }
        syncCommittedUtterances()
    }

    func applyInsertAnnotation(utteranceId: UInt64, position: UInt64, text: String) {
        guard !finalized else { return }
        let annotation = text.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !annotation.isEmpty else { return }
        guard let index = committedSegments.lastIndex(where: { $0.utteranceId == utteranceId }) else {
            showToast("Skipped unbound transcript annotation")
            return
        }
        guard committedSegments[index].insertAnnotation(position: position, text: annotation) else {
            showToast("Skipped out-of-range transcript annotation")
            return
        }
        syncCommittedUtterances()
    }

    func applySessionFinalised() {
        finalizeTranscript()
    }

    /// The Rust controller's authoritative post-stop transcript (LocalFinalPass) —
    /// the SAME text that is delivered/pasted and shown by tray "Copy". Stored so
    /// the single `finalizeTranscript()` uses it instead of the raw streaming
    /// assembly. Emitted inside the awaited stop pipeline, so it normally arrives
    /// before the stop/finalise events; if it arrives AFTER (mode already
    /// `.formatted`), replace the FINAL immediately. Live PREVIEW is untouched —
    /// it stays raw-streaming on purpose ("live preview · raw").
    func applyFinalTranscript(_ text: String) {
        let clean = text.trimmingCharacters(in: .whitespacesAndNewlines)
        // Dedupe: this event fires once per stop, but a redundant re-emit must not
        // reassign `@Published` state (each write re-invalidates the TextEditor).
        guard !clean.isEmpty, clean != authoritativeFinalText else { return }
        authoritativeFinalText = clean
        if mode == .formatted, formattedText != clean {
            formattedText = clean
        }
    }

    /// Single authoritative finalize. `runStop`, `finishControllerRecording`, and
    /// `applySessionFinalised` all funnel here so `formattedText` is produced from
    /// ONE source rather than three paths each rewriting it from a different buffer.
    /// Preference: the controller's authoritative LocalFinalPass text (matches
    /// delivery/Copy); fall back to the id-ordered committed assembly only if that
    /// event has not arrived.
    private func finalizeTranscript() {
        let wasFinalized = finalized
        cancelWarmupWatchdog()
        warmingUp = false
        vadActive = false
        audioReady = false
        commitPreviewIfNeeded()
        let resolved = usableAuthoritativeFinalText ?? liveText
        if formattedText != resolved { formattedText = resolved }
        mode = .formatted
        // FREEZE: from here, late streaming events are dropped (see the apply guards)
        // so nothing keeps mutating @Published state and re-rendering in Idle.
        finalized = true
        // Notify the recording-lifecycle sink that the session ended. This is the
        // stop-side counterpart to `handleRecordingStarted` firing `onRecordingStarted?()`:
        // the tray otherwise only clears its "Recording" pill via the popover's one-shot
        // onAppear poll, so a hotkey stop left it stuck. Gate on the finalize transition
        // so redundant re-finalizes (finishControllerRecording + applySessionFinalised)
        // don't re-fire and churn @Published tray state.
        if !wasFinalized { onRecordingStopped?() }
    }

    private var usableAuthoritativeFinalText: String? {
        guard let text = authoritativeFinalText else { return nil }
        let trimmed = text.trimmingCharacters(in: .whitespacesAndNewlines)
        return trimmed.isEmpty ? nil : trimmed
    }

    private func resetTranscript() {
        preview = ""
        committedSegments = []
        committedUtterances = []
        authoritativeFinalText = nil
        finalized = false
    }

    private func markTranscriptActivity() {
        cancelWarmupWatchdog()
        warmingUp = false
        audioReady = true
        if recording {
            mode = .listening
        }
    }

    private func commitPreviewIfNeeded() {
        let active = preview.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !active.isEmpty else { return }
        appendCommittedSegment(active)
        preview = ""
        refreshFormattedTranscriptIfNeeded()
    }

    /// Append a committed segment, keyed by `utteranceId`. Re-finals for an id we
    /// already hold replace that slot in place (no duplicate, no drop); new ids
    /// append in arrival order = id order, the bridge's FIFO ordering.
    /// Kontrakt: tekst przychodzi już przycięty z Rusta (jedyny właściciel ofsetów);
    /// Swift przechowuje go bajt-w-bajt, bo ofsety ReplaceRange/InsertAnnotation
    /// liczone są u emitenta nad tym samym stringiem.
    private func upsertFinalSegment(utteranceId: UInt64, text: String) {
        guard !text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else { return }
        if let index = committedSegments.firstIndex(where: { $0.utteranceId == utteranceId }) {
            guard committedSegments[index].text != text else { return }
            committedSegments[index].text = text
            committedSegments[index].annotations = []
        } else {
            committedSegments.append(OverlayTranscriptSegment(utteranceId: utteranceId, text: text))
        }
        syncCommittedUtterances()
    }

    /// Append an un-keyed committed segment (trailing preview at finalize time —
    /// speech that never received its own `UtteranceFinal`).
    private func appendCommittedSegment(_ text: String) {
        let trimmed = text.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty else { return }
        committedSegments.append(OverlayTranscriptSegment(utteranceId: nil, text: trimmed))
        syncCommittedUtterances()
    }

    private func syncCommittedUtterances() {
        committedUtterances = committedSegments.map(\.renderedText)
        refreshFormattedTranscriptIfNeeded()
    }

    private func refreshFormattedTranscriptIfNeeded() {
        if mode == .formatted {
            // Once the controller's authoritative final transcript is in, it wins:
            // late streaming `UtteranceFinal` events must not clobber the FINAL with
            // the raw streaming assembly. Without it, fall back to the live assembly.
            // Dedupe the write — an identical reassignment still re-invalidates the
            // bound TextEditor and feeds the Idle render churn.
            let resolved = usableAuthoritativeFinalText ?? liveText
            if formattedText != resolved { formattedText = resolved }
        }
    }

    private func normalized(_ text: String) -> String {
        text.lowercased()
            .components(separatedBy: CharacterSet.alphanumerics.inverted)
            .filter { !$0.isEmpty }
            .joined(separator: " ")
    }

    func applyVad(_ active: Bool) {
        // Drop late VAD toggles after finalize: the waveform is gone in Idle and a
        // stray `vadActive` flip is just another needless @Published invalidation.
        guard !finalized else { return }
        vadActive = active
        if active {
            cancelWarmupWatchdog()
            warmingUp = false
            audioReady = true
        }
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
        resetTranscript()
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

    func onRecordingPreparing() {
        DispatchQueue.main.async { MainActor.assumeIsolated { self.state?.handleRecordingPreparing() } }
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
    func onFinal(utteranceId: UInt64, text: String) {
        DispatchQueue.main.async { MainActor.assumeIsolated { self.state?.applyFinal(utteranceId: utteranceId, text) } }
    }
    func onReplaceRange(utteranceId: UInt64, start: UInt64, end: UInt64, text: String, source: CsLayerSource) {
        DispatchQueue.main.async {
            MainActor.assumeIsolated {
                self.state?.applyReplaceRange(utteranceId: utteranceId, start: start, end: end, text: text)
            }
        }
    }
    func onInsertAnnotation(utteranceId: UInt64, position: UInt64, text: String, kind: CsAnnotationKind) {
        DispatchQueue.main.async {
            MainActor.assumeIsolated {
                self.state?.applyInsertAnnotation(utteranceId: utteranceId, position: position, text: text)
            }
        }
    }
    func onSessionFinalised(sessionId: String, layerSummary: CsLayerSummary) {
        DispatchQueue.main.async { MainActor.assumeIsolated { self.state?.applySessionFinalised() } }
    }
    func onFinalTranscriptReady(text: String) {
        DispatchQueue.main.async { MainActor.assumeIsolated { self.state?.applyFinalTranscript(text) } }
    }
    func onVadActive(active: Bool) {
        DispatchQueue.main.async { MainActor.assumeIsolated { self.state?.applyVad(active) } }
    }
    func onNoSpeech(reason: String) {
        // Distinguish genuine silence from "speech was present but the quality
        // gates rejected it" — the latter must not lie to the user as "No speech".
        let message: String
        switch reason {
        case "all_speech_rejected_by_quality_gate":
            message = "Speech too quiet or short — adjust mic"
        default:
            message = "No speech"
        }
        DispatchQueue.main.async { MainActor.assumeIsolated { self.state?.showToast(message) } }
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
