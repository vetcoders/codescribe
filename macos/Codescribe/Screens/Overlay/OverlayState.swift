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
//   on_audio_level → capture RMS per block → real waveform amplitude (U22;
//                   closes the old AMPLITUDE GAP — ambient eq is now only the
//                   fallback when no live level arrives).
//   on_no_speech → dedicated `.noSpeech` outcome body (Close only).
//   on_error     → transient toast.

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
    func currentOverlayPolicy() -> OverlayPolicySnapshot?
    func setAutoPasteEnabled(_ enabled: Bool)
    func formatText(
        text: String,
        language: CsLanguage?,
        level: FormattingPolicyOption
    ) async throws -> String
    func pasteText(text: String) async throws -> CsPasteOutcome
    func copyTaggedTranscript(text: String) async throws
    func pasteTargetAppName() async -> String?
    func sendAssistiveTranscript(text: String) async throws -> Bool
    func transcribeFile(path: String) async throws -> CsTranscription
}

struct OverlayPolicySnapshot: Equatable {
    let autoPasteEnabled: Bool
    let autoFormatLevel: FormattingPolicyOption
}

enum OverlayActionPresentation {
    static let manualFormatLevels = FormattingPolicyOption.editablePrompts
    static let formatTitle = "Format"
    static let formatHelp = "Format transcript once as Correction, Smart, or Max"
    static let sendTitle = "To Agent"
    static let sendHelp = "Send transcript to the agent"
}

struct OverlayInsertActionPresentation: Equatable {
    let targetAppName: String?
    let title: String
    let help: String

    init(targetAppName: String?) {
        let normalized = targetAppName?.trimmingCharacters(in: .whitespacesAndNewlines)
        let target = normalized.flatMap { $0.isEmpty ? nil : $0 }
        self.targetAppName = target
        if let target {
            title = "Insert → \(target)"
            help = "Insert at the cursor in \(target)"
        } else {
            title = "Insert"
            help = "Insert at the cursor in the previous app"
        }
    }
}

/// State machine mirrored from the mock: live dictation, the finalized
/// transcript returned by `stopRecording`, or a session that ended without any
/// usable text (VAD silence / all speech rejected). `.noSpeech` is a dedicated
/// terminal outcome so the overlay never lands in `.formatted` with an empty
/// editable FINAL that reads like a crash. `.error` is the terminal outcome for
/// engine/controller failures so they are not flattened into "no speech".
enum OverlayMode: Equatable {
    case listening
    case formatted
    case noSpeech
    case error
}

@MainActor
final class OverlayState: ObservableObject {

    // MARK: Published state
    @Published var mode: OverlayMode = .listening
    @Published var preview: String = ""        // current utterance interim
    @Published var committedUtterances: [String] = [] // accumulated finals, one item per utterance
    @Published var formattedText: String = ""  // finalized transcript after stop
    @Published var vadActive: Bool = false     // drives the WaveformView pulse
    /// Live capture level for the waveform. NOT @Published on purpose — the
    /// waveform's TimelineView reads it every frame; see `AudioLevelMeter`.
    let levelMeter = AudioLevelMeter()
    /// Distinguishes a measured microphone feed from the explicit ambient
    /// fallback used by legacy/disconnected engines before any RMS arrives.
    @Published private(set) var hasMeasuredAudioLevel = false
    @Published var audioReady: Bool = false    // recorder confirmed; STT/VAD may still be warming
    @Published var warmingUp: Bool = false     // true after user intent, before audio/VAD proves life
    /// Stop was requested and we are awaiting the final transcript. Distinct from
    /// recording: the waveform must NOT keep pulsing like capture, and the status
    /// reads "transcribing" so the user can tell recording ended vs. hung. Set only
    /// on the Swift-observable stop (`runStop`); cleared by finalize / error / reset
    /// / close so it can never stick. See `WaveformView(transcribing:)`.
    @Published var transcribing: Bool = false
    @Published var toast: String?              // transient error notice
    @Published var errorMessage: String?
    @Published var isFormatting: Bool = false
    @Published var formatFailureStatus: String?
    /// Prompt-free policy snapshot from C02's persisted settings owner. These
    /// values are replaced only by a fresh engine read, never by optimistic UI.
    @Published private(set) var autoPasteEnabled = true
    @Published private(set) var autoFormatLevel: FormattingPolicyOption = .correction
    /// Assistive sessions never expose delivery controls. The controller owns
    /// that authoritative session gate and updates this presentation fence.
    @Published private(set) var autoPasteControlAvailable = true
    /// Destination name latched once at overlay session entry. The action row
    /// reads this snapshot; it never polls the bridge during rendering.
    @Published private(set) var pasteTargetAppName: String?
    /// Final pass phase (AI formatting / authoritative assembly after stop).
    /// Set on `applySessionFinalised`, cleared on controller finish or reset.
    /// Drives "final pass" status while the user still sees the live assembly.
    @Published var isFinalPass: Bool = false
    /// Human-facing notice shown in the `.noSpeech` outcome body. Set when a
    /// session finalizes without usable text; refined by `on_no_speech`'s reason
    /// so VAD silence and quality-gate rejection read differently.
    @Published var noSpeechNotice: String = OverlayState.defaultNoSpeechNotice
    @Published private(set) var indicatorMode: CsIndicatorMode = .hold

    // MARK: Panel placement (persisted; the window orchestrator repositions live)
    /// Anchored placement: one of six screen anchors, applied on every show().
    /// Picking an anchor exits free motion — the pick's intent is "go there".
    @Published var placementAnchor: OverlayAnchor = OverlayPlacement.anchor {
        didSet {
            guard placementAnchor != oldValue else { return }
            OverlayPlacement.anchor = placementAnchor
            if freeMotion { freeMotion = false } else { onPlacementChanged?() }
        }
    }
    /// Free motion: the panel keeps (and restores) wherever the user dragged it.
    @Published var freeMotion: Bool = OverlayPlacement.freeMotion {
        didSet {
            guard freeMotion != oldValue else { return }
            OverlayPlacement.freeMotion = freeMotion
            onPlacementChanged?()
        }
    }
    /// Wired by the orchestrator: re-derive the visible panel's origin now.
    var onPlacementChanged: (() -> Void)?

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

    static let defaultNoSpeechNotice = "No speech detected"

    private var recording = false
    /// Reason from `on_no_speech`, captured before the terminal stop so
    /// `finalizeTranscript` can pick the right notice when it resolves to empty.
    private var pendingNoSpeechMessage: String?
    private var committedSegments: [OverlayTranscriptSegment] = []
    /// Authoritative post-stop transcript pushed by the Rust controller
    /// (`on_final_transcript_ready` → LocalFinalPass `final_formatted_text`) — the
    /// SAME text the delivery/paste and tray "Copy" use. When present it is the
    /// FINAL the overlay shows, instead of the raw per-utterance streaming assembly.
    private var authoritativeFinalText: String?
    /// The delivered (pre-user-edit) text at the moment we entered .formatted.
    /// Captured for P0-D quality loop: diff delivered→edited on Copy/Send/close.
    private var deliveredText: String = ""
    /// Best-effort raw STT transcript text (pre-AI formatting / postprocess) for
    /// quality records. D-05: wired from authoritative final / STT assembly so
    /// lexicon v2 and quality analytics get the real misheard text, not only
    /// the (possibly formatted) delivered. Cleared on reset like deliveredText.
    private var sttRawText: String = ""
    /// Canonical provenance for the text currently shown in FINAL. Starts from
    /// persisted Auto Format truth and is replaced only by a successful manual
    /// format. Revert restores the previous level together with the exact bytes.
    private var qualityFormattingLevel: FormattingPolicyOption = .off
    /// One-step manual-format undo. A successful changed result replaces this
    /// source; failures, empty results, and identical no-ops leave it untouched.
    private var preFormatText: String?
    private var preFormatLevel: FormattingPolicyOption?
    /// Once a session is finalized (mode `.formatted` / Idle), the transcript is
    /// FROZEN. Late streaming events (Preview/Correction/UtteranceFinal/VAD) that the
    /// engine may still emit during/after teardown are DROPPED instead of mutating
    /// `@Published` state — otherwise each late apply re-invalidates the hosting view
    /// (TextEditor re-layout) and spins the SwiftUI render graph at 100% CPU in Idle.
    /// The authoritative `FinalTranscript` is the only post-finalize update allowed.
    private var finalized = false
    private var agentSessionArmed = false
    private var agentFinalTranscriptAppeared = false
    private var agentAutoSendCancelled = false
    private var agentDeliveryStarted = false
    private var toastTask: Task<Void, Never>?
    private var mockRevealTask: Task<Void, Never>?
    /// Belt-and-suspenders guard against an orphaned optimistic "starting" overlay.
    /// The Rust bridge now guarantees a terminal event for every preparing it shows
    /// (`compensate_orphaned_preparing`); this watchdog is the second layer: if no
    /// started/activity/stopped/finish arrives within `warmupWatchdogNanos`, the
    /// overlay dismisses itself instead of hanging on "starting" forever.
    private var warmupWatchdogTask: Task<Void, Never>?
    private var pasteTargetRefreshTask: Task<Void, Never>?
    private static let warmupWatchdogNanos: UInt64 = 4_000_000_000

    // MARK: Activity-anchored auto-hide for terminal outcomes
    private var autoHideTask: Task<Void, Never>?
    private var autoHideDeadline: TimeInterval?
    private var isPointerHovering = false
    private let nowProvider: () -> TimeInterval
    /// Single source of truth for the operator-dictated terminal lifetime.
    /// Five seconds is the comfortable end of the requested 3–5 second range.
    static let autoHideDelaySeconds: TimeInterval = 5

    init(nowProvider: @escaping () -> TimeInterval = { ProcessInfo.processInfo.systemUptime }) {
        self.nowProvider = nowProvider
    }

    func attach() {
        engine?.setListener(listener)
    }

    // MARK: Derived display (one source of truth for the view)

    var statusText: String {
        if mode == .error { return "failed" }
        if mode == .formatted { return "done" }
        if mode == .noSpeech { return "no speech" }
        guard mode == .listening else { return "Idle" }
        if isFinalPass { return "final pass" }
        if transcribing { return "transcribing" }
        if warmingUp { return "starting" }
        return hasMeasuredAudioLevel ? "recording" : "recording · ambient"
    }
    var statusColor: Color {
        switch mode {
        case .listening: return CSColor.terracotta
        case .formatted: return CSColor.oliveLight
        case .noSpeech: return CSColor.textMuted
        case .error: return CSColor.terracotta
        }
    }
    /// Only the live-capture pill ripples. During `transcribing` / `final pass` we swap
    /// to the static pill so its repeatForever animation tears down — a second visual
    /// cue that capture has ended and post-processing is in flight.
    var statusRippling: Bool { mode == .listening && !transcribing && !isFinalPass && (audioReady || vadActive) }

    var tagText: String {
        switch mode {
        case .listening: return "DICTATION"
        case .formatted: return "FINAL"
        case .noSpeech: return "NO SPEECH"
        case .error: return "ERROR"
        }
    }
    var tagColor: Color {
        switch mode {
        case .listening:
            return indicatorMode == .assistive ? CSColor.assistiveLight : CSColor.terracottaLight
        case .formatted: return CSColor.oliveLight
        case .noSpeech: return CSColor.textMuted
        case .error: return CSColor.terracotta
        }
    }

    var metaText: String {
        if isFinalPass { return "final pass · formatting" }
        switch mode {
        case .listening: return transcribing ? "finalizing · transcript" : "live preview · raw"
        case .formatted: return "final · transcript"
        case .noSpeech: return "no speech · nothing captured"
        case .error: return "error · recording stopped"
        }
    }
    var footerRight: String {
        if isFormatting { return "formatting" }
        if isFinalPass { return "final pass" }
        if mode == .noSpeech { return "no speech" }
        if mode == .error { return "error" }
        if mode == .listening && transcribing { return "transcribing" }
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

    /// Text shown in the listening body, in the SAME prominent slot that renders
    /// "listening…"/"starting…" during capture. The transcribing phase wins over any
    /// committed text so the post-capture state surfaces "transcribing…" here (not the
    /// raw streaming assembly) — the main-status counterpart to the header pill.
    /// During final pass we keep the assembled transcript visible (user sees result
    /// while AI formatting runs) — status/footer communicate the phase.
    var listeningDisplay: String {
        if isFinalPass {
            // Keep the captured assembly visible during final pass / AI formatting.
            return !liveText.isEmpty ? liveText : "final pass…"
        }
        if transcribing { return "transcribing…" }
        if !liveText.isEmpty { return liveText }
        return warmingUp ? "starting…" : "listening…"
    }

    /// Whatever the action row should copy/send for the current state.
    var activeText: String {
        switch mode {
        case .listening: return liveText
        case .formatted: return formattedText
        case .noSpeech, .error: return ""
        }
    }

    var canFormat: Bool {
        mode == .formatted
            && !isFormatting
            && engine?.isFormattingAvailable() == true
            && !formattedText.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
    }

    var canRevert: Bool {
        preFormatText != nil && !isFormatting
    }

    var insertActionPresentation: OverlayInsertActionPresentation {
        OverlayInsertActionPresentation(targetAppName: pasteTargetAppName)
    }

    var autoPasteAccessibilityValue: String {
        autoPasteEnabled ? "On" : "Off"
    }

    var manualFormatHelp: String {
        let automatic = autoFormatLevel == .off
            ? "Auto Format is Off."
            : "Auto Format is \(autoFormatLevel.visibleName)."
        return "\(automatic) \(OverlayActionPresentation.formatHelp)."
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
    /// Ignored while already transcribing so a second Finish tap during the
    /// awaited `stopRecording()` cannot re-enter and hit "no active recording".
    func stop() {
        guard engine != nil, recording, !transcribing else { return }
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
        formatFailureStatus = nil
        errorMessage = nil
        recording = true
        do {
            if !engine.isModelLoaded() { try await engine.initModel() }
            try await engine.startRecording(language: language)
        } catch {
            presentTerminalError(
                message: "Couldn't start recording: \(error)",
                toast: "Couldn't start recording"
            )
        }
    }

    func formatTranscript(level: FormattingPolicyOption) {
        guard let engine,
              canFormat,
              OverlayActionPresentation.manualFormatLevels.contains(level) else { return }
        let source = formattedText
        let sourceLevel = qualityFormattingLevel
        isFormatting = true
        // Format deliberately suspends passive dismissal. Its result stays until
        // another user activity explicitly starts a fresh countdown.
        cancelAutoHide()
        Task { @MainActor in
            defer { self.isFormatting = false }
            do {
                let formatted = try await engine.formatText(
                    text: source,
                    language: nil,
                    level: level
                )
                let isUsableChange = !formatted
                    .trimmingCharacters(in: .whitespacesAndNewlines)
                    .isEmpty && formatted != source
                if isUsableChange {
                    self.preFormatText = source
                    self.preFormatLevel = sourceLevel
                    self.formattedText = formatted
                    self.qualityFormattingLevel = level
                }
                self.formatFailureStatus = nil
                self.mode = .formatted
                self.cancelAutoHide()  // User acted (Format); do not auto-hide the result.
            } catch {
                self.formattedText = source
                self.formatFailureStatus = "raw — formatting failed"
                self.mode = .formatted
                self.cancelAutoHide()
                self.errorMessage = "Couldn't format transcript: \(error)"
                self.showToast("Couldn't format transcript")
            }
        }
    }

    /// Restore the exact source of the most recent successful changed format.
    /// The slot is consumed once and this explicit user activity starts a fresh
    /// terminal lifetime from the injected monotonic clock.
    func revertFormat() {
        guard !isFormatting, let source = preFormatText else { return }
        let sourceLevel = preFormatLevel ?? .off
        preFormatText = nil
        preFormatLevel = nil
        formattedText = source
        qualityFormattingLevel = sourceLevel
        formatFailureStatus = nil
        mode = .formatted
        restartAutoHideCountdown()
    }

    private func runStop() async {
        guard let engine else { return }
        // Enter the explicit "transcribing" phase for the whole awaited stop: the
        // waveform stops pulsing like capture and the status reads "transcribing"
        // instead of leaving the recording UI up while the final pass runs.
        transcribing = true
        warmingUp = false
        levelMeter.reset()
        do {
            // The controller bridge returns "" here; the authoritative transcript
            // is the id-ordered assembly of `UtteranceFinal` events (see liveText).
            _ = try await engine.stopRecording()
            recording = false
            isFinalPass = false
            finalizeTranscript() // clears `transcribing` as it flips to `.formatted`
        } catch {
            presentTerminalError(
                message: "Couldn't finalize transcript: \(error)",
                toast: "Couldn't finalize transcript"
            )
        }
    }

    // MARK: Action row

    func copyToPasteboard() {
        // P0-D: capture user correction on FINAL for quality loop + lexicon learning.
        captureQualityIfEdited(action: "copy")
        let pb = NSPasteboard.general
        pb.clearContents()
        pb.setString(activeText, forType: .string)
        restartAutoHideCountdown()
    }

    func sendToAgent() {
        // P0-D: capture user correction on FINAL for quality loop + lexicon learning.
        captureQualityIfEdited(action: "send")
        deliverAgentTranscript()
    }

    /// Caret-truth probe for the Insert self-paste guard. The overlay is a
    /// non-activating panel that can become key WITHOUT the app being
    /// frontmost (Spotlight-style), so a synthetic Cmd+V follows OUR key
    /// window whenever a Codescribe text view holds the caret — the frontmost
    /// app check on the Rust side cannot see that. Injectable so tests can
    /// simulate both worlds.
    var insertCaretInCodescribeProbe: () -> Bool = {
        guard let keyWindow = NSApp.keyWindow else { return false }
        return keyWindow.firstResponder is NSTextView
    }

    func pasteToPreviousApp() {
        captureQualityIfEdited(action: "paste")
        // Do not let the previous deadline fire while the async delivery is in
        // flight. A successful or failed attempt gets a fresh full countdown.
        cancelAutoHide()
        let text = activeText
        Task { @MainActor in
            defer { self.restartAutoHideCountdown() }
            do {
                if self.insertCaretInCodescribeProbe() {
                    // The caret sits inside Codescribe (e.g. the overlay's own
                    // editable FINAL) — a synthetic Cmd+V would paste the
                    // transcript right back into the overlay. Degrade to a
                    // tagged clipboard copy and say so.
                    try await engine?.copyTaggedTranscript(text: text)
                    self.showToast("Caret is in Codescribe — copied with tags")
                    return
                }
                let outcome = try await engine?.pasteText(text: text)
                if outcome == .copiedToClipboard {
                    self.showToast("Target app not focused — copied with tags")
                }
            } catch {
                self.errorMessage = "Couldn't paste transcript: \(error)"
                self.showToast("Couldn't paste transcript")
            }
        }
    }

    /// Persist through C02's single config seam, then immediately replace local
    /// state with a fresh disk-backed snapshot. A rejected write therefore snaps
    /// back to durable truth instead of leaving an optimistic switch behind.
    func setAutoPasteEnabled(_ enabled: Bool) {
        guard autoPasteControlAvailable, let engine else { return }
        engine.setAutoPasteEnabled(enabled)
        refreshOverlayPolicyTruth()
        restartAutoHideCountdown()
    }

    func setAutoPasteControlAvailable(_ available: Bool) {
        autoPasteControlAvailable = available
    }

    func close() {
        // P0-D: capture user correction on FINAL for quality loop + lexicon learning.
        captureQualityIfEdited(action: "close")
        cancelWarmupWatchdog()
        cancelAutoHide()
        mockRevealTask?.cancel()
        toastTask?.cancel()
        pasteTargetRefreshTask?.cancel()
        if recording, let engine {
            recording = false
            Task { @MainActor in _ = try? await engine.stopRecording() }
        }
        vadActive = false
        audioReady = false
        warmingUp = false
        transcribing = false
        isFinalPass = false
        onClose?()
    }

    private func refreshPasteTargetAppName(reset: Bool) {
        pasteTargetRefreshTask?.cancel()
        if reset {
            pasteTargetAppName = nil
        }
        guard let engine else { return }
        pasteTargetRefreshTask = Task { @MainActor [weak self] in
            let target = await engine.pasteTargetAppName()
            guard !Task.isCancelled, let self else { return }
            self.pasteTargetAppName = OverlayInsertActionPresentation(
                targetAppName: target
            ).targetAppName
        }
    }

    private func refreshOverlayPolicyTruth() {
        guard let truth = engine?.currentOverlayPolicy() else { return }
        autoPasteEnabled = truth.autoPasteEnabled
        autoFormatLevel = truth.autoFormatLevel
    }

    /// TextEditor writes through this seam so only actual user edits — never a
    /// programmatic format/final update — re-anchor the terminal lifetime.
    func userEditedTranscript(_ text: String) {
        if agentSessionArmed, agentFinalTranscriptAppeared, text != formattedText {
            agentAutoSendCancelled = true
        }
        formattedText = text
        restartAutoHideCountdown()
    }

    /// Consume the canonical Rust indicator mode. Agent arm is a one-shot
    /// session latch; the accepted orange processing phase must not disarm it.
    func applyIndicatorMode(_ mode: CsIndicatorMode) {
        indicatorMode = mode
        if mode == .assistive {
            agentSessionArmed = true
            autoPasteControlAvailable = false
        }
    }

    /// AppKit reports window motion separately from SwiftUI content events.
    func userDraggedOverlay() {
        restartAutoHideCountdown()
    }

    /// A live edge-drag resize is activity and therefore receives a fresh window.
    func userResizedOverlay() {
        restartAutoHideCountdown()
    }

    /// Hover pauses dismissal entirely; leaving starts a new full five seconds.
    func setPointerHovering(_ hovering: Bool) {
        guard hovering != isPointerHovering else { return }
        isPointerHovering = hovering
        guard isTerminalMode else { return }
        if hovering {
            cancelAutoHide()
        } else {
            restartAutoHideCountdown()
        }
    }

    // MARK: P0-D quality loop (user edits on FINAL → record + lexicon candidate)

    private func captureQualityIfEdited(action: String) {
        guard mode == .formatted else { return }
        let delivered = deliveredText.trimmingCharacters(in: .whitespacesAndNewlines)
        let edited = formattedText.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !edited.isEmpty, delivered != edited else { return }
        // Bridge FFI (generated by uniffi) appends the quality JSONL and feeds safe
        // candidates to lexicon.custom.jsonl. That is blocking disk I/O, so it runs
        // off the main actor — Copy/Send/Close must never wait on the disk.
        // Raw is best-effort for MVP.
        // D-05 over-correct: use sttRawText (wired from applyFinalTranscript / STT finals)
        // as raw_text when available so quality records carry the real pre-formatting
        // STT text for lexicon v2 consumers. Falls back to delivered (still better than "").
        let rawForRecord = !sttRawText.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
            ? sttRawText
            : delivered
        let formattingLevel = qualityFormattingLevel.rawValue
        Task.detached(priority: .utility) {
            // Pass action through to meta (over-correct P2-03). try? because FFI throws on err but
            // quality write is best-effort; never block UI action.
            try? commitOverlayQualityRecord(
                rawText: rawForRecord,
                deliveredText: delivered,
                editedText: edited,
                action: action,
                formattingLevel: formattingLevel
            )
        }
    }

    func prepareForExternalStart() {
        handleRecordingPreparing()
    }

    func handleRecordingPreparing() {
        agentSessionArmed = indicatorMode == .assistive
        autoPasteControlAvailable = !agentSessionArmed
        finalized = false
        isFinalPass = false
        mode = .listening
        warmingUp = true
        audioReady = false
        hasMeasuredAudioLevel = false
        levelMeter.reset()
        if !recording {
            resetTranscript()
            formattedText = ""
            isFormatting = false
            errorMessage = nil
        }
        recording = true
        refreshOverlayPolicyTruth()
        refreshPasteTargetAppName(reset: true)
        onRecordingPreparing?()
        armWarmupWatchdog()
    }

    func handleRecordingStarted() {
        cancelWarmupWatchdog()
        finalized = false
        isFinalPass = false
        mode = .listening
        warmingUp = false
        audioReady = true
        if !recording {
            hasMeasuredAudioLevel = false
            levelMeter.reset()
            if liveText.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
                resetTranscript()
            }
            formattedText = ""
            isFormatting = false
            formatFailureStatus = nil
            errorMessage = nil
        }
        recording = true
        refreshOverlayPolicyTruth()
        refreshPasteTargetAppName(reset: false)
        onRecordingStarted?()
    }

    func finishControllerRecording() {
        cancelWarmupWatchdog()
        recording = false
        isFinalPass = false
        finalizeTranscript()
    }

    /// Native hold-release / toggle stop: the controller entered `Busy` (final
    /// transcription pass) but no Swift-side `runStop` ran, so nothing had flipped
    /// us out of the live-capture UI. Enter the same "transcribing" phase the
    /// Finish button uses (waveform stops pulsing like capture, status reads
    /// "transcribing"). The terminal `on_recording_stopped` (→ `finalizeTranscript`)
    /// clears it, as do error / close / reset. Cancels the warmup watchdog because
    /// reaching finalisation proves the session progressed. Idempotent: a repeated
    /// `Busy` broadcast (or one arriving after finalize) is a no-op.
    func handleRecordingFinalising() {
        guard recording, !finalized, !transcribing else { return }
        cancelWarmupWatchdog()
        warmingUp = false
        transcribing = true
        levelMeter.reset()
        hasMeasuredAudioLevel = false
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
        abortRecordingSession(resetTranscript: true)
        mode = .listening
        onClose?()
    }

    private var isTerminalMode: Bool {
        mode == .formatted || mode == .noSpeech || mode == .error
    }

    private func restartAutoHideCountdown() {
        guard isTerminalMode, !isPointerHovering else {
            cancelAutoHide()
            return
        }
        cancelAutoHide()
        autoHideDeadline = nowProvider() + OverlayState.autoHideDelaySeconds
        scheduleAutoHideWake(after: OverlayState.autoHideDelaySeconds)
    }

    private func scheduleAutoHideWake(after delay: TimeInterval) {
        let nanoseconds = UInt64(max(0, delay) * 1_000_000_000)
        autoHideTask = Task { @MainActor [weak self] in
            try? await Task.sleep(nanoseconds: nanoseconds)
            guard !Task.isCancelled else { return }
            self?.evaluateAutoHideDeadline(rescheduleIfEarly: true)
        }
    }

    private func evaluateAutoHideDeadline(rescheduleIfEarly: Bool) {
        autoHideTask = nil
        guard isTerminalMode, !isPointerHovering, let deadline = autoHideDeadline else { return }
        let remaining = deadline - nowProvider()
        if remaining > 0 {
            if rescheduleIfEarly { scheduleAutoHideWake(after: remaining) }
            return
        }
        autoHideDeadline = nil
        if agentSessionArmed, agentFinalTranscriptAppeared {
            if !agentAutoSendCancelled {
                deliverAgentTranscript()
            }
            return
        }
        onClose?()
    }

    /// Deterministic XCTest seam: tests inject a monotonic clock, advance it,
    /// and evaluate the same deadline logic without wall-clock sleeps.
    func fireAutoHideNowForTests() {
        autoHideTask?.cancel()
        autoHideTask = nil
        evaluateAutoHideDeadline(rescheduleIfEarly: false)
    }

    private func cancelAutoHide() {
        autoHideTask?.cancel()
        autoHideTask = nil
        autoHideDeadline = nil
    }

    private func deliverAgentTranscript() {
        let text = activeText.trimmingCharacters(in: .whitespacesAndNewlines)
        guard agentSessionArmed, !agentDeliveryStarted, !text.isEmpty, let engine else { return }
        agentDeliveryStarted = true
        cancelAutoHide()
        Task { @MainActor [weak self] in
            guard let self else { return }
            do {
                if try await engine.sendAssistiveTranscript(text: text) {
                    onSendToAgent?(text)
                    onClose?()
                } else {
                    agentDeliveryStarted = false
                    showToast("Agent delivery is no longer available")
                }
            } catch {
                agentDeliveryStarted = false
                showToast("Couldn't send to Agent")
            }
        }
    }

    private func abortRecordingSession(resetTranscript shouldResetTranscript: Bool = false) {
        let shouldNotifyStopped =
            !finalized && (recording || warmingUp || transcribing || audioReady || vadActive)
        cancelWarmupWatchdog()
        cancelAutoHide()
        recording = false
        warmingUp = false
        transcribing = false
        audioReady = false
        vadActive = false
        isFinalPass = false
        levelMeter.reset()
        hasMeasuredAudioLevel = false
        if shouldResetTranscript {
            resetTranscript()
        }
        if shouldNotifyStopped {
            finalized = true
            onRecordingStopped?()
        }
    }

    func handleError(message: String) {
        presentTerminalError(message: message, toast: message)
    }

    private func presentTerminalError(message: String, toast: String) {
        abortRecordingSession()
        preview = ""
        committedSegments = []
        committedUtterances = []
        authoritativeFinalText = nil
        pendingNoSpeechMessage = nil
        noSpeechNotice = OverlayState.defaultNoSpeechNotice
        formattedText = ""
        isFormatting = false
        isFinalPass = false
        errorMessage = message
        mode = .error
        finalized = true
        showToast(toast)
        restartAutoHideCountdown()
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
        guard !finalized else { return }
        markTranscriptActivity()
        // Enter final pass phase (the post-stop AI formatting / authoritative
        // assembly). Status shows "final pass", transcript assembly remains
        // visible; the controller finish will surface the resolved .formatted.
        isFinalPass = true
        transcribing = false
        // Do not call finalizeTranscript here — that is driven by
        // finishControllerRecording (or equivalent terminal) so the phase
        // is observable to the user.
    }

    /// `on_no_speech` — the engine adjudicated the session with no usable speech.
    /// Fires BEFORE the terminal `on_recording_stopped`, so we only record the
    /// user-facing reason here; `finalizeTranscript` treats it as the engine's
    /// no-usable-speech adjudication (unless an authoritative final arrives) and
    /// flips into the dedicated `.noSpeech` outcome. If the reason arrives AFTER
    /// an already-empty finalize (late), upgrade the FINAL in place.
    func applyNoSpeech(reason: String) {
        let message: String
        switch reason {
        case "all_speech_rejected_by_quality_gate":
            message = "Speech too quiet or short — adjust the mic and try again"
        default:
            message = OverlayState.defaultNoSpeechNotice
        }
        pendingNoSpeechMessage = message
        if finalized, mode == .formatted,
           formattedText.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
            noSpeechNotice = message
            mode = .noSpeech
            restartAutoHideCountdown()
        } else if mode == .noSpeech {
            noSpeechNotice = message
        }
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
        formatFailureStatus = nil
        if mode == .formatted, formattedText != clean {
            formattedText = clean
        } else if mode == .noSpeech {
            // Real text arrived after we finalised to no-speech (empty at the
            // time): recover it as the normal FINAL rather than losing it.
            formattedText = clean
            mode = .formatted
            restartAutoHideCountdown()
        }
        if deliveredText.isEmpty, !clean.isEmpty {
            deliveredText = clean
        }
        if agentSessionArmed {
            agentFinalTranscriptAppeared = true
        }
        if sttRawText.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty, !clean.isEmpty {
            sttRawText = clean
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
        transcribing = false
        vadActive = false
        audioReady = false
        levelMeter.reset()
        hasMeasuredAudioLevel = false
        let shouldShowNoSpeechOutcome =
            pendingNoSpeechMessage != nil && usableAuthoritativeFinalText == nil
        if shouldShowNoSpeechOutcome {
            preview = ""
        } else {
            commitPreviewIfNeeded()
        }
        let resolved = shouldShowNoSpeechOutcome ? "" : (usableAuthoritativeFinalText ?? liveText)
        if resolved.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
            // Nothing usable was captured — VAD silence, or all speech rejected by
            // the quality gate. Surface a dedicated no-speech outcome instead of a
            // blank editable FINAL (Copy/Format/Send acting on an empty string).
            // When `on_no_speech` did not fire (empty final without an explicit
            // event) we treat the empty finalize as no-speech — an honest
            // approximation, since the user has nothing to act on either way.
            if formattedText != "" { formattedText = "" }
            noSpeechNotice = pendingNoSpeechMessage ?? OverlayState.defaultNoSpeechNotice
            mode = .noSpeech
        } else {
            if formattedText != resolved { formattedText = resolved }
            if deliveredText.isEmpty { deliveredText = resolved }
            qualityFormattingLevel = autoFormatLevel
            if sttRawText.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
                // Best effort: if no STT raw from per-utterance finals yet, fall back to the
                // resolved assembly (still the raw-streaming path, not AI formatted).
                sttRawText = resolved
            }
            mode = .formatted
            if agentSessionArmed {
                agentFinalTranscriptAppeared = true
            }
        }
        // FREEZE: from here, late streaming events are dropped (see the apply guards)
        // so nothing keeps mutating @Published state and re-rendering in Idle.
        finalized = true
        isFinalPass = false
        // Notify the recording-lifecycle sink that the session ended. This is the
        // stop-side counterpart to `handleRecordingStarted` firing `onRecordingStarted?()`:
        // the tray otherwise only clears its "Recording" pill via the popover's one-shot
        // onAppear poll, so a hotkey stop left it stuck. Gate on the finalize transition
        // so redundant re-finalizes (finishControllerRecording + applySessionFinalised)
        // don't re-fire and churn @Published tray state.
        if !wasFinalized { onRecordingStopped?() }

        // Every terminal outcome gets the same activity-anchored lifetime.
        restartAutoHideCountdown()
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
        deliveredText = ""
        sttRawText = ""
        qualityFormattingLevel = .off
        preFormatText = nil
        preFormatLevel = nil
        formatFailureStatus = nil
        pendingNoSpeechMessage = nil
        noSpeechNotice = OverlayState.defaultNoSpeechNotice
        finalized = false
        agentFinalTranscriptAppeared = false
        agentAutoSendCancelled = false
        agentDeliveryStarted = false
        transcribing = false
        isFinalPass = false
        // A hidden panel may not emit a pointer-exit event. Never carry a paused
        // hover latch into the next recording session.
        isPointerHovering = false
        cancelAutoHide()
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
    /// Contract: Rust already sends trimmed text and is the sole owner of offsets;
    /// Swift stores it byte-for-byte because ReplaceRange/InsertAnnotation offsets
    /// are computed by the emitter against this same string.
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

    /// `on_audio_level` — capture RMS per audio block. Only feeds the meter
    /// during live capture: once the session is transcribing/finalised the
    /// waveform is frozen or gone, and a late block must not wiggle it.
    func applyAudioLevel(_ rms: Float) {
        guard recording,
              (warmingUp || audioReady || vadActive),
              !finalized,
              !transcribing,
              !isFinalPass,
              mode == .listening else { return }
        levelMeter.push(rms: rms)
        if levelMeter.gain != nil { hasMeasuredAudioLevel = true }
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

    /// Seeded view model for #Preview in the post-capture transcribing phase.
    static func previewTranscribing() -> OverlayState {
        let s = OverlayState()
        s.mode = .listening
        s.transcribing = true
        s.audioReady = true
        s.committedUtterances = ["add a rate limiter to the login route and write a test for it"]
        return s
    }

    /// Seeded view model for #Preview in the no-speech outcome (session ended
    /// without any usable text).
    static func previewNoSpeech() -> OverlayState {
        let s = OverlayState()
        s.mode = .noSpeech
        s.noSpeechNotice = OverlayState.defaultNoSpeechNotice
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
    private let config = CodescribeConfig()

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
    func currentOverlayPolicy() -> OverlayPolicySnapshot? {
        let toggles = config.trayToggles()
        guard let formatLevel = FormattingPolicyOption(rawValue: toggles.formattingLevel) else {
            return nil
        }
        return OverlayPolicySnapshot(
            autoPasteEnabled: toggles.autoPasteEnabled,
            autoFormatLevel: formatLevel
        )
    }
    func setAutoPasteEnabled(_ enabled: Bool) {
        _ = try? config.setAutoPasteEnabled(enabled: enabled)
    }
    func formatText(
        text: String,
        language: CsLanguage?,
        level: FormattingPolicyOption
    ) async throws -> String {
        try await hotkeys.formatTextForLevel(
            text: text,
            language: language,
            level: level.rawValue
        )
    }
    func pasteText(text: String) async throws -> CsPasteOutcome {
        try await hotkeys.pasteText(text: text)
    }
    func copyTaggedTranscript(text: String) async throws {
        try await hotkeys.copyTextTagged(text: text)
    }
    func pasteTargetAppName() async -> String? {
        await hotkeys.pasteTargetAppName()
    }
    func sendAssistiveTranscript(text: String) async throws -> Bool {
        try await hotkeys.sendAssistiveTranscript(text: text)
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
    func onRecordingFinalising() {
        DispatchQueue.main.async { MainActor.assumeIsolated { self.state?.handleRecordingFinalising() } }
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
    func onAudioLevel(rms: Float) {
        DispatchQueue.main.async { MainActor.assumeIsolated { self.state?.applyAudioLevel(rms) } }
    }
    func onNoSpeech(reason: String) {
        // Route the reason into the dedicated no-speech OUTCOME (a persistent
        // body + Close), not a transient toast that fades and leaves an empty
        // editable FINAL behind. `applyNoSpeech` maps the reason to a user-facing
        // notice (genuine silence vs. quality-gate rejection).
        DispatchQueue.main.async { MainActor.assumeIsolated { self.state?.applyNoSpeech(reason: reason) } }
    }
    func onError(message: String) {
        DispatchQueue.main.async {
            MainActor.assumeIsolated {
                self.state?.handleError(message: message)
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
    func currentOverlayPolicy() -> OverlayPolicySnapshot? {
        OverlayPolicySnapshot(autoPasteEnabled: true, autoFormatLevel: .correction)
    }
    func setAutoPasteEnabled(_ enabled: Bool) {}
    func formatText(
        text: String,
        language: CsLanguage?,
        level: FormattingPolicyOption
    ) async throws -> String { text }
    func pasteText(text: String) async throws -> CsPasteOutcome { .pasted }
    func copyTaggedTranscript(text: String) async throws {}
    func pasteTargetAppName() async -> String? { nil }
    func sendAssistiveTranscript(text: String) async throws -> Bool { true }
    func transcribeFile(path: String) async throws -> CsTranscription {
        CsTranscription(text: "", language: "en")
    }
}
#endif
