import AppKit
import SwiftUI

// View model for the dictation overlay, backed by the redesign hotkey/controller
// bridge (`CodescribeHotkeys` / `CsTranscriptionListener`).
//
// The view talks only to the thin `DictationEngine` protocol below, so #Preview
// renders standalone against `MockDictationEngine`.
//
// TRANSCRIPT MODEL (one-throne bridge semantics):
//   on_transcript_projection → complete Rust-reduced document plus acoustic
//                              receipts; the sole Swift text-admission path.
//   legacy preview/correction/final/patch callbacks remain protocol stubs and
//   carry no transcript authority.
//   on_vad_active → speech start/stop → drives the WaveformView pulse.
//   on_audio_level → capture RMS per block → real waveform amplitude (U22;
//                   closes the old AMPLITUDE GAP — ambient eq is now only the
//                   fallback when no live level arrives).
//   on_no_speech → dedicated `.noSpeech` outcome body (Close only).
//   on_error     → transient toast.

// MARK: - Engine seam (orchestrator injects the real adapter in App.swift)

private struct OverlayContextMarker: Equatable {
  var position: Int
  var marker: String
  var order: Int
}

/// Read-only evidence copied from the bridge projection. Overlay code must not
/// reinterpret it as admission or finality authority.
private struct OverlayProjectedAcousticReceipt: Equatable {
  let acousticSerialVersion: UInt16
  let acousticSerial: String
  let sessionId: String
  let captureEpoch: UInt64
  let sampleStart: UInt64
  let sampleEnd: UInt64
  let durationMs: UInt64
  let energyIntegral: Double
  let meanRmsDbfs: Float
  let peakDbfs: Float
  let vadOpenSample: UInt64
  let vadCloseSample: UInt64
  let evidenceCalibrationVersion: String
  let wordEvidenceReceipts: [String]
  let layerDecisionReceipts: [String]
  let sealReceipt: String?
  let manualEditReceipt: String?
}

/// One immutable reducer projection for display. It is an event value, not a
/// Swift-owned committed document, and has no admit/reconcile/seal operation.
///
/// W2 input: `CsTranscriptProjectionEvent`. W2 output: visible overlay text and
/// evidence affordances. The listener callback and UI application are
/// intentionally unresolved until W2/W3.
private struct OverlayTranscriptProjection: Equatable {
  let schema: String
  let sequence: UInt64
  let emittedAt: String
  let sessionId: String
  let mode: String
  let reducerRevision: UInt64
  let reducerAction: String
  let occurrenceSessionId: String
  let captureEpoch: UInt64
  let sampleStart: UInt64
  let sampleEnd: UInt64
  let documentIndex: UInt64
  let label: String
  let renderedText: String
  let acousticReceipts: [OverlayProjectedAcousticReceipt]
}

/// Explicit human-edit receipt projection. W2 must send the edit to the
/// acoustic ledger and populate this only from its accepted receipt; Swift may
/// request and display the edit but cannot supersede a sealed label by itself.
private struct OverlayManualEditReceipt: Equatable {
  let receiptId: String
  let occurrenceSessionId: String
  let captureEpoch: UInt64
  let sampleStart: UInt64
  let sampleEnd: UInt64
  let baseRevision: UInt64
  let acceptedRevision: UInt64
  let previousLabel: String
  let replacementLabel: String
  let predecessorReceipt: String?
  let editedAt: String
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
  func currentOverlayPolicy() -> OverlayPolicySnapshot?
  func setAutoPasteEnabled(_ enabled: Bool)
  func pasteText(text: String) async throws -> CsPasteResult
  func deferText(text: String) async throws -> CsPasteResult
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
  static let sendTitle = "To Agent"
  static let sendHelp = "Send transcript to the agent"
  static let finishTitle = "Finish"
  static let finishHelp = "Stop capture and seal the take"
}

/// Compact chrome primary act for the slim overlay. Secondary actions live in
/// the attached menu; CloseDot remains the always-visible dismiss control.
enum OverlayPrimaryActionKind: Equatable {
  case finish
  case insert
}

/// Dictionary/history helper follows Settings `asr_mode`. Apple-only has no helper.
func helperRetranscribePass(asrMode: String) -> FileRetranscribePass? {
  switch asrMode.lowercased() {
  case "local_power": return .fullHq
  case "cloud": return .cloud
  default: return nil
  }
}

enum HelperFilePassRefusal: Equatable, Error {
  case noHelper
  case noArchivedAudio
}

/// Bind a Dictionary row to an explicit file pass. Never invent `last_session.wav`.
enum HelperFilePass {
  static func request(asrMode: String, archivedAudio: URL?) -> Result<
    (FileRetranscribePass, String), HelperFilePassRefusal
  > {
    guard let pass = helperRetranscribePass(asrMode: asrMode) else {
      return .failure(.noHelper)
    }
    guard let archived = archivedAudio else {
      return .failure(.noArchivedAudio)
    }
    return .success((pass, "\(pass.rawValue):\(archived.path)"))
  }

  static func compare(daily: String, helper: String, pass: FileRetranscribePass) -> String {
    let left = daily.trimmingCharacters(in: .whitespacesAndNewlines)
    let right = helper.trimmingCharacters(in: .whitespacesAndNewlines)
    if left == right {
      return "Helper \(pass.visibleName) matches daily."
    }
    return
      "DAILY\n\(left)\n\nHELPER \(pass.visibleName.uppercased())\n\(right)\n\nDaily is unchanged until you save a correction."
  }
}

enum FileRetranscribePass: String, CaseIterable, Identifiable {
  case fullHq = "hq"
  case cloud = "cloud"

  var id: String { rawValue }

  var visibleName: String {
    switch self {
    case .fullHq: "Full HQ file pass"
    case .cloud: "Cloud pass"
    }
  }

  var help: String {
    switch self {
    case .fullHq: "Full local Whisper pass over the selected audio file"
    case .cloud: "Cloud STT pass over the selected audio file"
    }
  }
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
  @Published var preview: String = ""  // current utterance interim
  @Published var committedUtterances: [String] = []  // accumulated finals, one item per utterance
  @Published var formattedText: String = ""  // finalized transcript after stop
  @Published var vadActive: Bool = false  // drives the WaveformView pulse
  /// Live capture level for the waveform. NOT @Published on purpose — the
  /// waveform's TimelineView reads it every frame; see `AudioLevelMeter`.
  let levelMeter = AudioLevelMeter()
  /// Distinguishes a measured microphone feed from the explicit ambient
  /// fallback used by legacy/disconnected engines before any RMS arrives.
  @Published private(set) var hasMeasuredAudioLevel = false
  @Published var audioReady: Bool = false  // recorder confirmed; STT/VAD may still be warming
  @Published var warmingUp: Bool = false  // true after user intent, before audio/VAD proves life
  /// Stop was requested and we are awaiting the final transcript. Distinct from
  /// recording: the waveform must NOT keep pulsing like capture, and the status
  /// reads "transcribing" so the user can tell recording ended vs. hung. Set only
  /// on the Swift-observable stop (`runStop`); cleared by finalize / error / reset
  /// / close so it can never stick. See `WaveformView(transcribing:)`.
  @Published var transcribing: Bool = false
  @Published var toast: String?  // transient error notice
  @Published var errorMessage: String?
  /// W13-6B highlight layer. Default follows the OFF flag; tests inject `true`.
  @Published var highlightsEnabled = false
  /// Span highlights (lexicon-corrected words + speech-gap pustki).
  @Published private(set) var highlights: [OverlayHighlight] = []
  @Published private(set) var selectedHighlightId: String?
  /// Last Teach acknowledgement for tests and the toast.
  @Published private(set) var lastTeachAcknowledgement: String?
  /// Injected Teach writer. Production uses `qualityTeachSpan`; tests replace
  /// this so XCTest never writes the operator's live lexicon.
  var teachSpan: ((OverlayHighlight) throws -> String)?
  @Published var isEditingTranscript: Bool = false
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

  // MARK: Session capture clock (UI_DIVERGENCE_AUDIT pkt 5 — overlay timer)
  /// Monotonic uptime stamp of the moment capture began for the open session.
  /// The overlay's live `00:00` counter derives from this: the user's absolute
  /// reference for audio sync, transcription lag, and stream drift.
  @Published private(set) var captureStartedAtUptime: TimeInterval?
  /// Freeze stamp — set when capture stops (Finish / native release / abort) so
  /// the counter halts at the session's true duration instead of ticking
  /// through the final pass.
  @Published private(set) var captureEndedAtUptime: TimeInterval?

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
  /// Content-free success seam. No transcript crosses this callback.
  var onSuccessfulDictation: (() -> Void)?

  /// Strong ref so the Rust-side callback (held via the UniFFI handle map) and
  /// our hop-to-main bridge stay alive for the lifetime of the overlay.
  private lazy var listener: CsTranscriptionListener = DictationListener(state: self)

  static let defaultNoSpeechNotice = "No speech detected"

  private var recording = false
  /// True after `on_vad_active(true)` until an empty-or-nonempty final consumes it.
  private var speechWasActive = false
  /// Reason from `on_no_speech`, captured before the terminal stop.
  private var pendingNoSpeechMessage: String?
  /// Global transcript markers captured by the agent combo. They remain
  /// independent from per-utterance semantic annotations so the authoritative
  /// final pass cannot erase context references.
  @Published private var contextMarkers: [OverlayContextMarker] = []
  /// The delivered (pre-user-edit) text at the moment we entered .formatted.
  /// Captured for P0-D quality loop: diff delivered→edited on Copy/Send/close.
  private var deliveredText: String = ""
  /// Armed only by a genuine TextEditor write and consumed by the first
  /// delivery action that records that edit. Delivery actions remain separate.
  private var manualHumanEditPending = false
  /// Once the reducer projects `record_ledger_terminal_seal`, later machine
  /// projections are rejected. Human edits operate on the terminal presentation;
  /// they do not rewrite `latestTranscriptProjection`.
  private var finalized = false
  /// Latest immutable projection event only; Rust `TranscriptRevision` remains
  /// the document owner and Rust `AcousticSerial` remains evidence authority.
  private var latestTranscriptProjection: OverlayTranscriptProjection?
  private var agentSessionArmed = false
  private var agentFinalTranscriptAppeared = false
  private var agentAutoSendCancelled = false
  private var agentDeliveryStarted = false
  private var toastTask: Task<Void, Never>?
  /// One-shot guard for the in-place Speech Recognition request+retry. macOS
  /// never re-prompts once the scope is determined, so a second attempt in
  /// the same app run could only loop on the terminal error.
  private var speechAuthRequestAttempted = false
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
    // Production reads the UniFFI flag (default OFF). XCTest hosts stay off
    // unless a test flips `highlightsEnabled` — never inherit a leaked env.
    if !QualityCaptureHost.isRunningTests {
      highlightsEnabled = overlayHighlightsEnabled()
    }
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

  /// Only the live-capture pill ripples. During `transcribing` / `final pass`
  /// we swap to the static pill so its
  /// repeatForever animation tears down — a second visual cue that capture
  /// has ended and post-processing is in flight, not a waveform grind.
  var statusRippling: Bool {
    mode == .listening
      && !transcribing
      && !isFinalPass
      && (audioReady || vadActive)
  }

  var tagText: String {
    if isFinalPass || transcribing { return "PROCESSING" }
    switch mode {
    case .listening:
      return indicatorMode == .assistive ? "AGENT" : "RECORDING"
    case .formatted: return "READY"
    case .noSpeech: return "NO SPEECH"
    case .error: return "ERROR"
    }
  }
  var tagColor: Color {
    if isFinalPass || transcribing {
      return CSColor.modeProcessing
    }
    switch mode {
    case .listening:
      return indicatorMode == .assistive ? CSColor.modeAgent : CSColor.modeRecording
    case .formatted: return CSColor.modeReady
    case .noSpeech: return CSColor.textMuted
    case .error: return CSColor.danger
    }
  }

  var metaText: String {
    if isFinalPass { return "final pass · formatting" }
    switch mode {
    case .listening:
      if transcribing { return "finalizing · transcript" }
      // Honesty (operator 2026-07-27 / mission B): never claim streaming
      // text the user cannot see. Apple may be shy (letter-level confidence)
      // or Previews may not have drained yet. Empty canvas = waiting cadence,
      // not "live preview · raw". Engine chip still reports Apple when live.
      let canvas = liveText.trimmingCharacters(in: .whitespacesAndNewlines)
      if canvas.isEmpty { return "live preview · waiting" }
      return "live preview · raw"
    case .formatted: return "final · transcript"
    case .noSpeech: return "no speech · nothing captured"
    case .error: return "error · recording stopped"
    }
  }
  var footerRight: String {
    if isFinalPass { return "final pass" }
    if mode == .noSpeech { return "no speech" }
    if mode == .error { return "error" }
    if mode == .listening && transcribing { return "transcribing" }
    if mode == .listening && warmingUp { return "warming up" }
    if mode == .listening && liveText.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
      // Empty canvas: do not imply a visible preview stream.
      return audioReady ? "audio live · waiting" : "waiting for audio"
    }
    return mode == .listening ? "vad-gated preview" : "editable"
  }

  /// Footer left engine chip — last stop serving label when available, else
  /// configured preference. Never a hardcoded "local whisper" (STT_CONTRACT).
  var footerEngineLabel: String {
    // Free UniFFI function (same as Settings Active STT).
    if let serving = currentServingVerdict() {
      let eng = serving.engine.trimmingCharacters(in: .whitespacesAndNewlines)
      if !eng.isEmpty {
        return Self.displayEngineChip(eng)
      }
    }
    if let pref = try? CodescribeConfig().loadSettings().sttEngine?
      .trimmingCharacters(in: .whitespacesAndNewlines),
      !pref.isEmpty
    {
      switch pref.lowercased() {
      case "apple": return "local apple"
      case "whisper", "candle": return "local whisper"
      case "auto": return "auto · apple-first"
      default: return pref
      }
    }
    return "local apple"
  }

  private static func displayEngineChip(_ engine: String) -> String {
    let e = engine.lowercased()
    if e.contains("apple") { return "local apple" }
    if e.contains("merged") && e.contains("whisper") { return "merged · whisper fill" }
    if e.contains("streaming") { return "streaming whisper" }
    if e.contains("whisper") { return "local whisper" }
    if e.contains("cloud") { return "cloud stt" }
    return engine
  }

  /// Rust-rendered transcript bytes from the latest admitted projection.
  ///
  /// The local arrays remain only for static SwiftUI previews that have no
  /// runtime listener. Once a projection exists, they can never outrank it.
  private var rawLiveText: String {
    if let projection = latestTranscriptProjection {
      return projection.renderedText
    }
    return (committedUtterances + [preview])
      .filter { !$0.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty }
      .joined(separator: " ")
  }

  var liveText: String {
    insertingContextMarkers(into: rawLiveText)
  }

  /// Text shown in the listening body, in the SAME prominent slot that renders
  /// "listening…"/"starting…" during capture.
  ///
  /// CAPTURED WORDS ALWAYS WIN OVER PHASE. The previous shape let the
  /// transcribing phase replace the live canvas with "transcribing…", so
  /// stopping a recording made the user's own words vanish behind a spinner
  /// until the final text swapped in — the operator dictated the bug report
  /// into the very canvas that then ate it (2026-08-09 20:13): "wyłączenie
  /// nagrywania zastępuje tekst … i podmienia dopiero ostateczny tekst a tego
  /// ma nie być". The overlay doctrine forbids exactly this class: never drop
  /// visible transcript. Phase placeholders render only on an EMPTY canvas;
  /// the header pill carries the phase otherwise.
  var listeningDisplay: String {
    if !liveText.isEmpty { return liveText }
    if isFinalPass { return "final pass…" }
    if transcribing { return "transcribing…" }
    return warmingUp ? "starting…" : "listening…"
  }

  /// Sealed committed utterances — engine must not rewrite these except via
  /// a human edit on the FINAL canvas. Highlighted on the live canvas.
  var listeningSealedText: String {
    let sealed =
      committedUtterances
      .map { $0.trimmingCharacters(in: .whitespacesAndNewlines) }
      .filter { !$0.isEmpty }
      .joined(separator: " ")
    return insertingContextMarkers(into: sealed)
  }

  /// Unsealed interim hypothesis. Dimmer than sealed; copy includes it.
  var listeningPreviewText: String {
    preview.trimmingCharacters(in: .whitespacesAndNewlines)
  }

  /// Live canvas: sealed truth highlighted, interim unsealed. Empty canvas
  /// keeps the phase placeholder so the body never goes blank.
  var listeningCanvas: AttributedString {
    let sealed = listeningSealedText
    let previewRun = listeningPreviewText
    if sealed.isEmpty && previewRun.isEmpty {
      var placeholder = AttributedString(listeningDisplay)
      placeholder.foregroundColor = CSColor.textBody
      return placeholder
    }
    var canvas = AttributedString()
    if !sealed.isEmpty {
      var run = AttributedString(sealed)
      run.foregroundColor = CSColor.textHigh
      run.backgroundColor = CSColor.modeReady.opacity(0.18)
      canvas.append(run)
    }
    if !sealed.isEmpty && !previewRun.isEmpty {
      canvas.append(AttributedString(" "))
    }
    if !previewRun.isEmpty {
      var run = AttributedString(previewRun)
      run.foregroundColor = CSColor.textMuted
      canvas.append(run)
    }
    return canvas
  }

  /// Copy is live from the first captured letter — listening or FINAL.
  var canCopy: Bool {
    !activeText.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
  }

  /// Timer is mandatory for any session that has started, including the
  /// frozen value after stop.
  var showsSessionTimer: Bool {
    captureStartedAtUptime != nil
  }

  /// Visual runs for the highlight canvas. Empty when the lane is off.
  var highlightCanvasRuns: [OverlayCanvasRun] {
    // Typed highlight evidence is not part of TranscriptRevision yet. Showing
    // highlights reconstructed from deleted Swift segments would create a
    // second transcript authority, so render the canonical projection plainly.
    [.text(listeningDisplay)]
  }

  /// Whatever the action row should copy/send for the current state.
  var activeText: String {
    switch mode {
    case .listening: return liveText
    case .formatted: return insertingContextMarkers(into: formattedText)
    case .noSpeech, .error: return ""
    }
  }

  /// Post-take review owns the floating panel. The formatted / no-speech
  /// surface must not yield to an Assistive tray tick — that path calls
  /// `hide()` and arms Agent auto-send.
  var blocksAssistiveOverlayHide: Bool {
    mode == .formatted || mode == .noSpeech
  }

  var insertActionPresentation: OverlayInsertActionPresentation {
    OverlayInsertActionPresentation(targetAppName: pasteTargetAppName)
  }

  var autoPasteAccessibilityValue: String {
    autoPasteEnabled ? "On" : "Off"
  }

  /// One primary act for the slim chrome combo control. `nil` for terminal
  /// outcomes that only need Close (no-speech / error).
  var primaryActionKind: OverlayPrimaryActionKind? {
    switch mode {
    case .listening: return .finish
    case .formatted: return .insert
    case .noSpeech, .error: return nil
    }
  }

  var primaryActionTitle: String {
    switch primaryActionKind {
    case .finish: return OverlayActionPresentation.finishTitle
    case .insert: return insertActionPresentation.title
    case nil: return ""
    }
  }

  var primaryActionHelp: String {
    switch primaryActionKind {
    case .finish: return OverlayActionPresentation.finishHelp
    case .insert: return insertActionPresentation.help
    case nil: return ""
    }
  }

  /// Footer may whisper canvas honesty only when it adds information the
  /// single status pill does not already carry (avoids recording/waiting stacks).
  var showsFooterHonesty: Bool {
    mode == .listening
      && !transcribing
      && !isFinalPass
      && liveText.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
  }

  var footerHonestyText: String {
    audioReady ? "waiting for words" : "waiting for audio"
  }

  // MARK: Recording lifecycle (engine-backed; no-op when engine is absent)

  /// Start mic dictation. Gated on `micPermissionGranted()`; requests access
  /// once when undetermined. Fires the async bridge work in a Task so the view
  /// can call it from a synchronous context (onAppear / hotkey).
  func start(language: CsLanguage? = nil) {
    guard engine != nil, !recording else { return }
    Task { @MainActor in await self.runStart(language: language) }
  }

  /// Whole seconds of capture for the open session; nil before any capture.
  /// Reads the frozen end stamp once capture stopped, so the final pass does
  /// not keep the clock ticking.
  func elapsedCaptureSeconds() -> Int? {
    guard let started = captureStartedAtUptime else { return nil }
    let end = captureEndedAtUptime ?? nowProvider()
    return max(0, Int(end - started))
  }

  /// `mm:ss` (or `h:mm:ss` past the hour) for the overlay's live counter.
  var sessionTimerText: String {
    let total = elapsedCaptureSeconds() ?? 0
    let (h, m, s) = (total / 3600, (total % 3600) / 60, total % 60)
    return h > 0
      ? String(format: "%d:%02d:%02d", h, m, s)
      : String(format: "%02d:%02d", m, s)
  }

  private func beginCaptureClock() {
    captureStartedAtUptime = nowProvider()
    captureEndedAtUptime = nil
  }

  private func freezeCaptureClock() {
    guard captureStartedAtUptime != nil, captureEndedAtUptime == nil else { return }
    captureEndedAtUptime = nowProvider()
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
    errorMessage = nil
    beginCaptureClock()
    recording = true
    do {
      // Whisper is optional gap-fill when Apple is live. initModel soft-fails
      // in the bridge for that path; never treat a missing Whisper model as
      // a start refusal — recording must still run (degraded: no final gap fill).
      if !engine.isModelLoaded() {
        do {
          try await engine.initModel()
        } catch {
          // Candle-live still surfaces via startRecording / later final-pass.
          // Apple-live continues; bridge already degrades quietly when it can.
          NSLog("codescribe: optional Whisper warm skipped: \(error)")
        }
      }
      try await engine.startRecording(language: language)
    } catch {
      await handleStartFailure(error, language: language)
    }
  }

  /// A start failure caused by an undetermined Speech Recognition grant is
  /// recoverable in place: fire the TCC dialog from the main app process (so
  /// the grant lands on the app's identity, which the bridge child inherits)
  /// and retry the start once when authorized. Every other failure — and a
  /// declined dialog — funnels into the terminal error path, where
  /// `speechAuthNotice` rewrites raw `speech_auth_*` markers.
  private func handleStartFailure(_ error: Error, language: CsLanguage?) async {
    let described = "\(error)"
    if described.contains("speech_auth_not_determined"), !speechAuthRequestAttempted {
      speechAuthRequestAttempted = true
      abortRecordingSession()
      let state = await withCheckedContinuation { continuation in
        SpeechRecognitionPermission.request { continuation.resume(returning: $0) }
      }
      if state == .granted {
        await runStart(language: language)
        return
      }
    }
    presentTerminalError(
      message: "Couldn't start recording: \(described)",
      toast: "Couldn't start recording"
    )
  }

  private func runStop() async {
    guard let engine else { return }
    // Enter the explicit "transcribing" phase for the whole awaited stop: the
    // waveform stops pulsing like capture and the status reads "transcribing"
    // instead of leaving the recording UI up while the final pass runs.
    transcribing = true
    warmingUp = false
    freezeCaptureClock()
    levelMeter.reset()
    do {
      // The controller bridge returns "" here; the authoritative transcript
      // is the id-ordered assembly of `UtteranceFinal` events (see liveText).
      _ = try await engine.stopRecording()
      recording = false
      isFinalPass = false
    } catch {
      presentTerminalError(
        message: "Couldn't finalize transcript: \(error)",
        toast: "Couldn't finalize transcript"
      )
    }
  }

  // MARK: Action row

  func copyToPasteboard(_ pasteboard: NSPasteboard = .general) {
    // P0-D: capture user correction on FINAL for quality loop + lexicon learning.
    captureQualityIfEdited(action: "copy")
    pasteboard.clearContents()
    pasteboard.setString(activeText, forType: .string)
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
        let result: CsPasteResult?
        if self.insertCaretInCodescribeProbe() {
          // The caret sits inside Codescribe (e.g. the overlay's own
          // editable FINAL) — a synthetic Cmd+V would paste the
          // transcript right back into the overlay. Arm the in-memory
          // Paste Here slot without touching the user's clipboard.
          result = try await engine?.deferText(text: text)
        } else {
          result = try await engine?.pasteText(text: text)
        }
        switch result?.outcome {
        case .deferredInsertArmed:
          let shortcut = result?.deferredInsertShortcut ?? "⌘⌥V"
          self.showFooterNotice(shortcut, persists: true)
        case .copiedToClipboard:
          self.showFooterNotice("copied")
        case .accessibilityPermissionNeeded:
          self.showFooterNotice("no ax")
        case .pasted, .noop, nil:
          break
        }
      } catch {
        self.errorMessage = "Couldn't paste transcript: \(error)"
        self.showFooterNotice("no paste")
      }
    }
  }

  /// Whisper a short footer chip next to `local apple`. Never a floating pill
  /// over the action row. `persists` keeps the chip until the overlay hides
  /// (Paste Here chord); otherwise it fades after the usual toast window.
  func showFooterNotice(_ message: String, persists: Bool = false) {
    toast = message
    toastTask?.cancel()
    guard !persists else { return }
    toastTask = Task { @MainActor [weak self] in
      try? await Task.sleep(nanoseconds: 2_600_000_000)
      guard !Task.isCancelled else { return }
      self?.toast = nil
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
    isEditingTranscript = false
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
      self.pasteTargetAppName =
        OverlayInsertActionPresentation(
          targetAppName: target
        ).targetAppName
    }
  }

  private func refreshOverlayPolicyTruth() {
    guard let truth = engine?.currentOverlayPolicy() else { return }
    autoPasteEnabled = truth.autoPasteEnabled
    autoFormatLevel = truth.autoFormatLevel
  }

  func beginTranscriptEdit() {
    guard mode == .formatted else { return }
    isEditingTranscript = true
    cancelAutoHide()
  }

  func endTranscriptEdit() {
    guard isEditingTranscript else { return }
    isEditingTranscript = false
  }

  /// TextEditor writes through this seam so only actual user edits — never a
  /// machine projection or file-pass output — re-anchor the terminal lifetime.
  func userEditedTranscript(_ text: String) {
    if agentSessionArmed, agentFinalTranscriptAppeared, text != formattedText {
      agentAutoSendCancelled = true
    }
    if text != formattedText {
      manualHumanEditPending = true
    }
    formattedText = text
    restartAutoHideCountdown()
  }

  /// One manual edit act may vote once. Copy followed by Close therefore keeps
  /// the second quality receipt non-voting; a later TextEditor change re-arms it.
  func consumeManualEditProvenanceForQuality(isEdited: Bool) -> String? {
    guard isEdited, manualHumanEditPending else {
      if !isEdited { manualHumanEditPending = false }
      return nil
    }
    manualHumanEditPending = false
    return "manual_human"
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
  /// Position sticks only in Free motion; anchored mode snaps back on next show.
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
    // `commitOverlayQualityRecord` is a free FFI function, not a call on the
    // injected `engine` — so a mocked engine does NOT stop it, and the XCTest
    // suite was appending two synthetic corrections ("original delivered
    // transcript here with user fix") to the OPERATOR'S live
    // ~/.codescribe/quality/corrections.jsonl on every run. 276 of 501 rows
    // in the real store came from test runs, and they surfaced in Settings ›
    // Dictionary as if the user had made them (operator screenshot
    // 2026-08-09 14:21, three seconds after a suite finished). The keychain
    // test-host gate landed earlier did not cover this path.
    guard !QualityCaptureHost.isRunningTests else { return }
    let delivered = deliveredText.trimmingCharacters(in: .whitespacesAndNewlines)
    let edited = formattedText.trimmingCharacters(in: .whitespacesAndNewlines)
    guard !edited.isEmpty else { return }
    let isEdited = delivered != edited
    // Unedited transcripts used to never reach the review queue — but "not
    // corrected on the overlay" means "no time right now", not "perfect"
    // (operator, 2026-08-09). Capture them once per session, on close, so
    // Settings › Dictionary can serve as the deferred correction desk. The
    // identical delivered/edited pair teaches the lexicon nothing (word-pair
    // extraction over a zero delta yields zero rules), so this fills the
    // queue without poisoning learning.
    guard isEdited || action == "close" else { return }
    let recordedAction = isEdited ? action : "close-unreviewed"
    let editProvenance = consumeManualEditProvenanceForQuality(isEdited: isEdited)
    // Bridge FFI (generated by uniffi) appends the quality JSONL and feeds safe
    // candidates to lexicon.custom.jsonl. That is blocking disk I/O, so it runs
    // off the main actor — Copy/Send/Close must never wait on the disk.
    // The projection exposes only rendered truth. Until its receipt carries a
    // distinct acoustic-text field, use admitted delivery bytes instead of an
    // unwritten Swift "raw" shadow.
    let rawForRecord = delivered
    // Automatic product formatting is unavailable until C15C wires the
    // occurrence-bound producer before seal; quality receipts state that truth.
    let formattingLevel = FormattingPolicyOption.off.rawValue
    // The admitted projection contract does not currently expose aggregate
    // Whisper confidence. Persist absence honestly instead of maintaining an
    // unwritten Swift confidence shadow.
    let avgLogprob: Float? = nil
    let speechPct: Float? = nil
    let confidenceFlags: [String] = []
    Task.detached(priority: .utility) { [weak self] in
      // Pass action through to meta (over-correct P2-03). try? because FFI throws on err but
      // quality write is best-effort; never block UI action.
      let result = try? commitOverlayQualityRecord(
        rawText: rawForRecord,
        deliveredText: delivered,
        editedText: edited,
        action: recordedAction,
        formattingLevel: formattingLevel,
        editProvenance: editProvenance,
        avgLogprob: avgLogprob,
        speechPct: speechPct,
        confidenceFlags: confidenceFlags
      )
      if let acknowledgement = result?.acknowledgement, !acknowledgement.isEmpty {
        await MainActor.run {
          self?.showToast(acknowledgement)
        }
      }
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
    isEditingTranscript = false
    audioReady = false
    hasMeasuredAudioLevel = false
    levelMeter.reset()
    if !recording {
      resetTranscript()
      errorMessage = nil
      beginCaptureClock()
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
      // The old guard ran the reset only when `liveText` was ALREADY empty,
      // i.e. exactly when there was nothing to clear — and skipped it whenever
      // the previous take's text was still on the canvas, which is the only
      // case that needed it. A new take always starts from an empty canvas.
      resetTranscript()
      errorMessage = nil
      beginCaptureClock()
    }
    if captureStartedAtUptime == nil {
      beginCaptureClock()
    }
    recording = true
    refreshOverlayPolicyTruth()
    refreshPasteTargetAppName(reset: false)
    onRecordingStarted?()
  }

  func finishControllerRecording() {
    let shouldNotifyStopped =
      !finalized && (recording || warmingUp || transcribing || audioReady || vadActive)
    cancelWarmupWatchdog()
    recording = false
    warmingUp = false
    transcribing = false
    audioReady = false
    vadActive = false
    isFinalPass = false
    freezeCaptureClock()
    levelMeter.reset()
    hasMeasuredAudioLevel = false

    // A terminal ledger projection normally finalizes presentation before the
    // controller returns to idle. If it did not, never leave `.listening` on
    // screen: capture is over and claiming RECORDING would be false. Preserve
    // any admitted canvas bytes, surface the missing seal, and release every
    // app-level recording latch through the same terminal callback.
    if shouldNotifyStopped {
      finalized = true
      if let noSpeech = pendingNoSpeechMessage {
        noSpeechNotice = noSpeech
        mode = .noSpeech
      } else if mode == .listening {
        errorMessage = "Recording ended before a sealed transcript was committed"
        mode = .error
      }
      onRecordingStopped?()
      restartAutoHideCountdown()
    }
  }

  /// Native hold-release / toggle stop: the controller entered `Busy` (final
  /// transcription pass) but no Swift-side `runStop` ran, so nothing had flipped
  /// us out of the live-capture UI. Enter the same "transcribing" phase the
  /// Finish button uses (waveform stops pulsing like capture, status reads
  /// "transcribing"). The terminal `on_recording_stopped`
  /// clears it, as do error / close / reset. Cancels the warmup watchdog because
  /// reaching finalisation proves the session progressed. Idempotent: a repeated
  /// `Busy` broadcast (or one arriving after finalize) is a no-op.
  func handleRecordingFinalising() {
    guard recording, !finalized, !transcribing else { return }
    cancelWarmupWatchdog()
    warmingUp = false
    transcribing = true
    freezeCaptureClock()
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
    // No `agentSessionArmed` here: the explicit Send button is live for
    // every terminal overlay (dictation and formatting included), and the
    // controller falls back to the session trigger context when no
    // assistive context was armed (review P0-03). Auto-send remains gated
    // on the armed latch by its caller.
    guard !agentDeliveryStarted, !text.isEmpty, let engine else { return }
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
    freezeCaptureClock()
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
    // Since the bridge-side warning split (`warning_is_user_terminal`), quality
    // receipts (`tail_patch_under_commit`, `layer1_lane_degraded`,
    // `apple_final_window_overlap_normalized`, ...) never reach `on_error` —
    // they are log-only in both bridges. What lands here is a user-terminal
    // failure (`transcription_failed`, start failures): the session is over.
    //
    // The content rule survives from the 2026-08-12 incident (a mislabelled
    // warning ran `presentTerminalError` and discarded 282 already-committed
    // characters): whatever the failure, a non-empty draft is sacred. But
    // "sacred" no longer means pretending the take is alive behind an
    // "Engine warning" toast while the engine is gone — that left the overlay
    // in a zombie live-capture UI with no stop parity. A failure with a draft
    // now ENDS the session exactly like a stop: engine released best-effort
    // (the same orphan-mic guard as `ComposerDictation.handleEngineError`),
    // transcript kept on screen with the normal Copy/Format/Send surface.
    //
    // Only an already admitted Rust projection can be preserved. Listener
    // preview/final callbacks are intentionally not a fallback authority.
    if let projection = latestTranscriptProjection,
      !projection.renderedText.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
    {
      if let engine {
        Task { @MainActor in _ = try? await engine.stopRecording() }
      }
      finishTerminalPresentation(
        projection: projection,
        signalsSuccessfulDictation: false,
        armsAgentAutoSend: false
      )
      showToast("Dictation failed — transcript kept")
      return
    }
    presentTerminalError(message: message, toast: message)
  }

  /// User-facing rewrite for Speech Recognition TCC failures. The engine
  /// reports raw bridge markers (`speech_auth_not_determined` / `_denied` /
  /// `_restricted`); surfacing those verbatim reads as a crash, when the fix
  /// is one System Settings toggle. Returns nil for every other error.
  static func speechAuthNotice(from message: String) -> String? {
    guard message.contains("speech_auth_") else { return nil }
    if message.contains("speech_auth_not_determined") {
      return "Apple dictation needs Speech Recognition access — "
        + "grant it in Settings › Dictation or System Settings › "
        + "Privacy & Security › Speech Recognition"
    }
    if message.contains("speech_auth_denied") || message.contains("speech_auth_restricted") {
      return "Speech Recognition is off for Codescribe — enable it in "
        + "System Settings › Privacy & Security › Speech Recognition"
    }
    return "Speech Recognition access is unavailable — check System "
      + "Settings › Privacy & Security › Speech Recognition"
  }

  /// User-facing rewrite for acoustic admission refusals. The controller
  /// refuses BEFORE opening the microphone and reports one `admission_*`
  /// code followed by its explanation and action; the toast keeps the short
  /// headline, the panel keeps the full actionable message. Nil otherwise.
  static func admissionNotice(from message: String) -> (headline: String, detail: String)? {
    guard let range = message.range(of: "admission_") else { return nil }
    let tail = String(message[range.lowerBound...])
    let code = tail.prefix { $0 == "_" || $0.isLetter }
    let detail = tail.dropFirst(code.count).drop { $0 == ":" || $0 == " " }
    let headline: String
    switch code {
    case "admission_calibration_missing", "admission_calibration_no_profile":
      headline = "Microphone not calibrated — Settings › Audio › Calibrate microphone"
    case "admission_calibration_refused", "admission_calibration_unusable":
      headline = "Stored calibration can't be used — recalibrate in Settings › Audio"
    case "admission_seal_lane_disarmed":
      headline =
        detail.contains("CODESCRIBE_SILERO_FUSION")
        ? "Seal lane is off — CODESCRIBE_SILERO_FUSION override"
        : "Seal lane is off — enable it in Settings › Audio"
    case "admission_seal_vad_unavailable":
      headline = "Silero VAD did not load — recording refused"
    case "admission_capture_device_unavailable":
      headline = "No microphone available — recording refused"
    case "admission_refused":
      // Warning-channel envelope: the real code follows in the message.
      return admissionNotice(from: String(detail))
    default:
      return nil
    }
    let detailText = detail.isEmpty ? headline : String(detail)
    return (headline, detailText)
  }

  private func presentTerminalError(message: String, toast: String) {
    let speechNotice = OverlayState.speechAuthNotice(from: message)
    let admissionNotice = OverlayState.admissionNotice(from: message)
    let message = speechNotice ?? admissionNotice?.detail ?? message
    let toast = speechNotice ?? admissionNotice?.headline ?? toast
    abortRecordingSession()
    preview = ""
    committedUtterances = []
    highlights = []
    selectedHighlightId = nil
    speechWasActive = false
    pendingNoSpeechMessage = nil
    noSpeechNotice = OverlayState.defaultNoSpeechNotice
    formattedText = ""
    isFinalPass = false
    errorMessage = message
    mode = .error
    finalized = true
    showToast(toast)
    restartAutoHideCountdown()
  }

  /// Finish UI and capture lifecycle for bytes already admitted through the
  /// Rust projection boundary. This helper never accepts free-form text.
  private func finishTerminalPresentation(
    projection: OverlayTranscriptProjection,
    signalsSuccessfulDictation: Bool,
    armsAgentAutoSend: Bool
  ) {
    let renderedText = projection.renderedText
    let hasTranscript =
      !renderedText.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
    abortRecordingSession()
    finalized = true
    isFinalPass = false
    transcribing = false
    formattedText = renderedText
    deliveredText = renderedText
    agentFinalTranscriptAppeared = armsAgentAutoSend && hasTranscript
    if hasTranscript {
      mode = .formatted
      if signalsSuccessfulDictation {
        onSuccessfulDictation?()
      }
    } else {
      noSpeechNotice = pendingNoSpeechMessage ?? OverlayState.defaultNoSpeechNotice
      mode = .noSpeech
    }
    restartAutoHideCountdown()
  }

  // MARK: Listener-driven mutations (called on the main actor by DictationListener)

  /// Apply one immutable reducer/ledger projection. Swift checks that the
  /// projected evidence is present, then displays Rust-owned rendered bytes;
  /// it never admits a label or decides whether a seal exists.
  func applyTranscriptProjection(_ event: CsTranscriptProjectionEvent) {
    // Bus sequences restart at 1 for every session, so both the terminal-seal
    // latch and the monotonic-sequence guard are only meaningful *within* one
    // session. Applied across sessions they silently drop every projection of
    // the next take and leave the previous take's sealed text on screen.
    let remembered = latestTranscriptProjection
    if let remembered, remembered.sessionId == event.sessionId {
      guard remembered.reducerAction != "record_ledger_terminal_seal" else { return }
      guard event.sequence > remembered.sequence else { return }
    }
    let acousticReceipts = event.acousticReceipts.map { receipt in
      OverlayProjectedAcousticReceipt(
        acousticSerialVersion: receipt.acousticSerialVersion,
        acousticSerial: receipt.acousticSerial,
        sessionId: receipt.sessionId,
        captureEpoch: receipt.captureEpoch,
        sampleStart: receipt.sampleStart,
        sampleEnd: receipt.sampleEnd,
        durationMs: receipt.durationMs,
        energyIntegral: receipt.energyIntegral,
        meanRmsDbfs: receipt.meanRmsDbfs,
        peakDbfs: receipt.peakDbfs,
        vadOpenSample: receipt.vadOpenSample,
        vadCloseSample: receipt.vadCloseSample,
        evidenceCalibrationVersion: receipt.evidenceCalibrationVersion,
        wordEvidenceReceipts: receipt.wordEvidenceReceipts,
        layerDecisionReceipts: receipt.layerDecisionReceipts,
        sealReceipt: receipt.sealReceipt,
        manualEditReceipt: receipt.manualEditReceipt
      )
    }
    let acousticSerials = acousticReceipts.map(\.acousticSerial)
    let transcriptRevision = event.reducerRevision
    guard transcriptRevision > 0, !acousticSerials.isEmpty,
      acousticReceipts.allSatisfy({
        !$0.acousticSerial.isEmpty && !$0.wordEvidenceReceipts.isEmpty
          && !$0.layerDecisionReceipts.isEmpty
      })
    else { return }
    let projection = OverlayTranscriptProjection(
      schema: event.schema,
      sequence: event.sequence,
      emittedAt: event.emittedAt,
      sessionId: event.sessionId,
      mode: event.mode,
      reducerRevision: transcriptRevision,
      reducerAction: event.reducerAction,
      occurrenceSessionId: event.occurrenceSessionId,
      captureEpoch: event.captureEpoch,
      sampleStart: event.sampleStart,
      sampleEnd: event.sampleEnd,
      documentIndex: event.documentIndex,
      label: event.label,
      renderedText: event.renderedText,
      acousticReceipts: acousticReceipts
    )
    latestTranscriptProjection = projection
    markTranscriptActivity()
    preview = ""
    committedUtterances = []
    formattedText = projection.renderedText
    if projection.reducerAction == "record_ledger_terminal_seal" {
      finishTerminalPresentation(
        projection: projection,
        signalsSuccessfulDictation: true,
        armsAgentAutoSend: true
      )
    }
  }

  func applyContextMarker(position: UInt64, marker: String) {
    guard !finalized, let offset = Int(exactly: position) else { return }
    let clean = marker.trimmingCharacters(in: .whitespacesAndNewlines)
    guard !clean.isEmpty else { return }
    contextMarkers.append(
      OverlayContextMarker(position: offset, marker: clean, order: contextMarkers.count)
    )
  }

  func applySessionFinalised() {
    guard !finalized else { return }
    markTranscriptActivity()
    // Enter final pass phase (the post-stop AI formatting / authoritative
    // assembly). Status shows "final pass", transcript assembly remains
    // visible; the controller finish will surface the resolved .formatted.
    isFinalPass = true
    transcribing = false
  }

  /// `on_no_speech` — the engine adjudicated the session with no usable speech.
  /// Fires BEFORE the terminal `on_recording_stopped`, so we only record the
  /// user-facing reason here. If it arrives after an empty terminal outcome,
  /// upgrade the notice in place.
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
      formattedText.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
    {
      noSpeechNotice = message
      mode = .noSpeech
      restartAutoHideCountdown()
    } else if mode == .noSpeech {
      noSpeechNotice = message
    }
  }

  private func resetTranscript() {
    // The canvas reads `rawLiveText`, which returns the projection's rendered
    // text whenever one exists and only falls back to the local arrays. Clearing
    // the arrays without clearing the projection therefore resets nothing the
    // user can see: the previous take stayed painted through the whole next
    // one until its first projection landed. `formattedText` lived at the call
    // sites for the same reason and was missed by every caller that forgot it —
    // both now belong to the single reset so no start path can be half-clean.
    latestTranscriptProjection = nil
    formattedText = ""
    preview = ""
    committedUtterances = []
    contextMarkers = []
    highlights = []
    selectedHighlightId = nil
    lastTeachAcknowledgement = nil
    speechWasActive = false
    deliveredText = ""
    manualHumanEditPending = false
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
    manualHumanEditPending = false
    cancelWarmupWatchdog()
    warmingUp = false
    audioReady = true
    if recording {
      mode = .listening
    }
  }

  private func insertingContextMarkers(into text: String) -> String {
    guard !contextMarkers.isEmpty else { return text }
    var rendered = text
    let ordered = contextMarkers.sorted {
      if $0.position == $1.position { return $0.order > $1.order }
      return $0.position > $1.position
    }
    for item in ordered {
      let offset = min(max(item.position, 0), rendered.count)
      let index = rendered.index(rendered.startIndex, offsetBy: offset)
      let previous: Character? =
        index > rendered.startIndex ? rendered[rendered.index(before: index)] : nil
      let next: Character? = index < rendered.endIndex ? rendered[index] : nil
      // A marker landing INSIDE a word ("mn|ie") stays unpadded, so the
      // split is lossless downstream: title derivation strips the bare
      // marker and the word reads whole again ("mnie"). Space padding is
      // only for word-boundary insertions.
      let splitsWord =
        (previous?.isLetter == true || previous?.isNumber == true)
        && (next?.isLetter == true || next?.isNumber == true)
      let needsLeadingSpace = !splitsWord && previous != nil && previous?.isWhitespace != true
      let needsTrailingSpace = !splitsWord && next != nil && next?.isWhitespace != true
      let insertion =
        (needsLeadingSpace ? " " : "")
        + item.marker
        + (needsTrailingSpace ? " " : "")
      rendered.insert(contentsOf: insertion, at: index)
    }
    return rendered
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
      warmingUp || audioReady || vadActive,
      !finalized,
      !transcribing,
      !isFinalPass,
      mode == .listening
    else { return }
    levelMeter.push(rms: rms)
    if levelMeter.gain != nil { hasMeasuredAudioLevel = true }
  }

  func applyVad(_ active: Bool) {
    // Drop late VAD toggles after finalize: the waveform is gone in Idle and a
    // stray `vadActive` flip is just another needless @Published invalidation.
    guard !finalized else { return }
    vadActive = active
    if active {
      speechWasActive = true
      cancelWarmupWatchdog()
      warmingUp = false
      audioReady = true
    }
  }

  func selectHighlight(_ highlight: OverlayHighlight) {
    selectedHighlightId = highlight.id
  }

  /// One-click send-span-to-Teach. Production goes through `qualityTeachSpan`
  /// behind `QualityCaptureHost`; tests inject `teachSpan`.
  func sendHighlightToTeach(_ highlight: OverlayHighlight) {
    guard highlightsEnabled else { return }
    guard let index = highlights.firstIndex(where: { $0.id == highlight.id }) else { return }
    if let teachSpan {
      do {
        let acknowledgement = try teachSpan(highlight)
        highlights[index].taught = true
        lastTeachAcknowledgement = acknowledgement
        if !acknowledgement.isEmpty { showToast(acknowledgement) }
      } catch {
        showToast("Teach failed")
      }
      return
    }
    guard !QualityCaptureHost.isRunningTests else { return }
    let variant = highlight.teachVariant
    let canonical = highlight.teachCanonical
    let kind = highlight.teachKind
    Task.detached(priority: .utility) { [weak self] in
      let result = try? qualityTeachSpan(
        variant: variant,
        canonical: canonical,
        kind: kind
      )
      await MainActor.run {
        guard let self else { return }
        if let acknowledgement = result?.acknowledgement {
          if let idx = self.highlights.firstIndex(where: { $0.id == highlight.id }) {
            self.highlights[idx].taught = true
          }
          self.lastTeachAcknowledgement = acknowledgement
          if !acknowledgement.isEmpty { self.showToast(acknowledgement) }
        } else {
          self.showToast("Teach failed")
        }
      }
    }
  }

  private func noteSpeechGap(utteranceId: UInt64, speechPct: Float?) {
    let heard = speechWasActive || (speechPct ?? 0) > 0
    speechWasActive = false
    guard heard else { return }
    let gap = OverlayCanvas.speechGap(utteranceId: utteranceId)
    if !highlights.contains(where: { $0.id == gap.id }) {
      highlights.append(gap)
    }
  }

  private func sliceUtteranceText(_ text: String, start: UInt64, end: UInt64) -> String {
    guard let startOffset = Int(exactly: start),
      let endOffset = Int(exactly: end),
      startOffset <= endOffset,
      endOffset <= text.count
    else { return "" }
    let startIndex = text.index(text.startIndex, offsetBy: startOffset)
    let endIndex = text.index(text.startIndex, offsetBy: endOffset)
    return String(text[startIndex..<endIndex])
  }

  private func rebaseHighlights(
    utteranceId: UInt64,
    start: UInt64,
    end: UInt64,
    replacementCount: UInt64
  ) {
    let removed = end >= start ? end - start : 0
    let delta = Int64(replacementCount) - Int64(removed)
    highlights = highlights.compactMap { highlight in
      guard highlight.utteranceId == utteranceId, highlight.kind == .lexiconCorrected else {
        return highlight
      }
      if highlight.charEnd <= start { return highlight }
      if highlight.charStart >= end {
        var shifted = highlight
        let startShift = Int64(highlight.charStart) + delta
        let endShift = Int64(highlight.charEnd) + delta
        guard startShift >= 0, endShift >= startShift else { return nil }
        shifted.charStart = UInt64(startShift)
        shifted.charEnd = UInt64(endShift)
        return shifted
      }
      return nil
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
  /// that imitates progressive projection revisions (mock: 46ms).
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
    s.formattedText =
      "Add a rate limiter to the login route and write a test that covers the throttle window. Keep the existing error shape."
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
  func pasteText(text: String) async throws -> CsPasteResult {
    try await hotkeys.pasteText(text: text)
  }
  func deferText(text: String) async throws -> CsPasteResult {
    try await hotkeys.deferText(text: text)
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
    try await hotkeys.transcribeFile(path: path)
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

  func onTranscriptProjection(event: CsTranscriptProjectionEvent) {
    DispatchQueue.main.async {
      MainActor.assumeIsolated { self.state?.applyTranscriptProjection(event) }
    }
  }

  func onRecordingPreparing() {
    DispatchQueue.main.async { MainActor.assumeIsolated { self.state?.handleRecordingPreparing() } }
  }
  func onRecordingStarted() {
    DispatchQueue.main.async { MainActor.assumeIsolated { self.state?.handleRecordingStarted() } }
  }
  func onRecordingStopped() {
    DispatchQueue.main.async {
      MainActor.assumeIsolated { self.state?.finishControllerRecording() }
    }
  }
  func onRecordingFinalising() {
    DispatchQueue.main.async {
      MainActor.assumeIsolated { self.state?.handleRecordingFinalising() }
    }
  }
  /// Legacy wire callback retained by the UniFFI protocol. Transcript text is
  /// admitted only through `onTranscriptProjection`.
  func onPreview(text _: String) {}
  /// Legacy wire callback retained by the UniFFI protocol. Transcript text is
  /// admitted only through `onTranscriptProjection`.
  func onCorrection(text _: String, previousText _: String) {}
  func onFinal(
    utteranceId _: UInt64,
    text _: String,
    avgLogprob _: Float?,
    speechPct _: Float?,
    confidenceFlags _: [String]
  ) {}
  func onReplaceRange(
    utteranceId _: UInt64, start _: UInt64, end _: UInt64, text _: String,
    source _: CsLayerSource
  ) {}
  func onInsertAnnotation(
    utteranceId _: UInt64, position _: UInt64, text _: String, kind _: CsAnnotationKind
  ) {}
  func onContextMarker(position: UInt64, marker: String) {
    DispatchQueue.main.async {
      MainActor.assumeIsolated {
        self.state?.applyContextMarker(position: position, marker: marker)
      }
    }
  }
  func onSessionFinalised(sessionId: String, layerSummary: CsLayerSummary) {
    DispatchQueue.main.async { MainActor.assumeIsolated { self.state?.applySessionFinalised() } }
  }
  func onFinalTranscriptReady(text _: String) {}
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
    DispatchQueue.main.async {
      MainActor.assumeIsolated { self.state?.applyNoSpeech(reason: reason) }
    }
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
    func currentOverlayPolicy() -> OverlayPolicySnapshot? {
      OverlayPolicySnapshot(autoPasteEnabled: true, autoFormatLevel: .correction)
    }
    func setAutoPasteEnabled(_ enabled: Bool) {}
    func pasteText(text: String) async throws -> CsPasteResult {
      CsPasteResult(
        outcome: .pasted,
        targetAppName: nil,
        frontmostAppName: nil,
        deferredInsertShortcut: nil,
        deferredInsertFailure: nil
      )
    }
    func deferText(text: String) async throws -> CsPasteResult {
      CsPasteResult(
        outcome: .deferredInsertArmed,
        targetAppName: nil,
        frontmostAppName: "Codescribe",
        deferredInsertShortcut: "⌘⌥V",
        deferredInsertFailure: nil
      )
    }
    func copyTaggedTranscript(text: String) async throws {}
    func pasteTargetAppName() async -> String? { nil }
    func sendAssistiveTranscript(text: String) async throws -> Bool { true }
    func transcribeFile(path: String) async throws -> CsTranscription {
      CsTranscription(text: "", language: "en")
    }
  }
#endif
