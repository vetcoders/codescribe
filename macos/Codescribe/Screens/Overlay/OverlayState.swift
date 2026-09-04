import AppKit
import Observation
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
//   raw preview/correction/final/patch events remain IPC diagnostics and never
//   cross the product-facing listener.
//   on_vad_active → speech start/stop → drives the WaveformView pulse.
//   on_audio_level → capture RMS per block → real waveform amplitude (U22;
//                   closes the old AMPLITUDE GAP — ambient eq is now only the
//                   fallback when no live level arrives).
//   on_no_speech → user-facing reason sideband; projection owns the phase.
//   on_error     → recovery detail sideband; projection owns the phase.

// MARK: - Engine seam (orchestrator injects the real adapter in App.swift)

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
/// Input: `CsTranscriptProjectionEvent`. Output: visible overlay text and
/// evidence affordances.
private struct OverlayTranscriptProjection: Equatable {
  let schema: String
  let sequence: UInt64
  let emittedAt: String
  let sessionId: String
  let mode: String
  let phase: OverlayMode
  let reducerRevision: UInt64
  let reducerAction: String
  let occurrenceSessionId: String
  let captureEpoch: UInt64
  let sampleStart: UInt64
  let sampleEnd: UInt64
  let documentIndex: UInt64
  let label: String
  let renderedText: String
  let canPaste: Bool
  let canInsert: Bool
  let canCopy: Bool
  let canRetranscribe: Bool
  let canFormat: Bool
  let terminal: Bool
  let acousticReceipts: [OverlayProjectedAcousticReceipt]
}

/// Minimal slice of the controller-backed dictation surface the overlay needs.
/// Kept as a protocol so the view-model + preview compile without a live Rust core.
@MainActor
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

/// Presentation phase supplied by the reducer-owned projection. Swift parses
/// the wire value but never derives a phase from text, callbacks, or seals.
enum OverlayMode: String, Equatable {
  case listening
  case finalizing
  case formatted
  case noSpeech = "no_speech"
  case error
}

/// The sole cross-thread ingress into the overlay. UniFFI callbacks enqueue
/// values here; one main-actor consumer applies them in arrival order.
enum OverlayListenerEvent: Sendable {
  case transcriptProjection(CsTranscriptProjectionEvent)
  case recordingPreparing
  case recordingStarted
  case recordingStopped
  case recordingFinalising
  case sessionFinalised
  case vadActive(Bool)
  case audioLevel(Float)
  case noSpeech(String)
  case error(String)
}

@MainActor
@Observable
final class OverlayState {

  // MARK: Published state
  private(set) var transcriptMode = "dictation"
  private(set) var mode: OverlayMode = .listening
  private(set) var formattedText = ""
  private(set) var revision: UInt64 = 0
  private(set) var canPaste = false
  private(set) var canInsert = false
  private(set) var canCopy = false
  private(set) var canRetranscribe = false
  private(set) var canFormat = false
  private(set) var terminal = false
  var vadActive: Bool = false  // drives the WaveformView pulse
  /// Live capture level for the waveform. NOT on purpose — the
  /// waveform's TimelineView reads it every frame; see `AudioLevelMeter`.
  let levelMeter = AudioLevelMeter()
  /// Distinguishes a measured microphone feed from the explicit ambient
  /// fallback used by legacy/disconnected engines before any RMS arrives.
  private(set) var hasMeasuredAudioLevel = false
  var audioReady: Bool = false  // recorder confirmed; STT/VAD may still be warming
  var warmingUp: Bool = false  // true after user intent, before audio/VAD proves life
  /// Stop is in flight. This is a controller/lifecycle guard only; visible phase
  /// and waveform presentation come from the projection's `phase` field.
  var transcribing: Bool = false
  var toast: String?  // transient error notice
  var errorMessage: String?
  private(set) var errorLifecycleDetail =
    "Recording stopped before a transcript was available."
  /// Settings destination that can resolve the current terminal error. This is
  /// presentation routing only; the controller remains the admission authority.
  private(set) var recoverySettingsSection: SettingsSection?
  private(set) var recoverySettingsAnchor: SettingsAnchor?
  /// Prompt-free policy snapshot from C02's persisted settings owner. These
  /// values are replaced only by a fresh engine read, never by optimistic UI.
  private(set) var autoPasteEnabled = true
  private(set) var autoFormatLevel: FormattingPolicyOption = .correction
  /// Assistive sessions never expose delivery controls. The controller owns
  /// that authoritative session gate and updates this presentation fence.
  private(set) var autoPasteControlAvailable = true
  /// Serving-engine label latched once per session. Rendering never performs
  /// settings I/O or a UniFFI read.
  private(set) var engineChip = "local apple"
  /// Lifecycle evidence that the final pass is active. It never selects a
  /// presentation phase; the reducer projection owns that field.
  var isFinalPass: Bool = false
  /// Human-facing notice shown in the `.noSpeech` outcome body. Set when a
  /// session finalizes without usable text; refined by `on_no_speech`'s reason
  /// so VAD silence and quality-gate rejection read differently.
  var noSpeechNotice: String = OverlayState.defaultNoSpeechNotice
  private(set) var indicatorMode: CsIndicatorMode = .hold

  // MARK: Session capture clock (UI_DIVERGENCE_AUDIT pkt 5 — overlay timer)
  /// Monotonic uptime stamp of the moment capture began for the open session.
  /// The overlay's live `00:00` counter derives from this: the user's absolute
  /// reference for audio sync, transcription lag, and stream drift.
  private(set) var captureStartedAtUptime: TimeInterval?
  /// Freeze stamp — set when capture stops (Finish / native release / abort) so
  /// the counter halts at the session's true duration instead of ticking
  /// through the final pass.
  private(set) var captureEndedAtUptime: TimeInterval?

  // MARK: Panel placement (persisted; the window orchestrator repositions live)
  /// Anchored placement: one of six screen anchors, applied on every show().
  /// Picking an anchor exits free motion — the pick's intent is "go there".
  var placementAnchor: OverlayAnchor = OverlayPlacement.anchor {
    didSet {
      guard placementAnchor != oldValue else { return }
      OverlayPlacement.anchor = placementAnchor
      if freeMotion { freeMotion = false } else { onPlacementChanged?() }
    }
  }
  /// Free motion: the panel keeps (and restores) wherever the user dragged it.
  var freeMotion: Bool = OverlayPlacement.freeMotion {
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

  /// Strong refs for the one ordered Rust-callback ingress.
  @ObservationIgnored private let listener: CsTranscriptionListener
  @ObservationIgnored private let eventStream: AsyncStream<OverlayListenerEvent>
  @ObservationIgnored private var eventTask: Task<Void, Never>?

  static let defaultNoSpeechNotice = "No speech detected"

  private var recording = false
  /// Reason from `on_no_speech`, captured before the terminal stop.
  private var pendingNoSpeechMessage: String?
  /// The exact rendered text at the terminal projection.
  private var deliveredText: String = ""
  /// Last reducer-owned projection painted by Swift. The reducer owns ordering
  /// and finality; the overlay does not second-guess an event that reached it.
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
  /// Belt-and-suspenders guard against an orphaned optimistic "starting" overlay.
  /// The Rust bridge now guarantees a terminal event for every preparing it shows
  /// (`compensate_orphaned_preparing`); this watchdog is the second layer: if no
  /// started/activity/stopped/finish arrives within `warmupWatchdogNanos`, the
  /// overlay dismisses itself instead of hanging on "starting" forever.
  private var warmupWatchdogTask: Task<Void, Never>?
  private static let warmupWatchdogNanos: UInt64 = 4_000_000_000

  // MARK: Activity-anchored auto-hide for terminal outcomes
  private var autoHideTask: Task<Void, Never>?
  private var autoHideDeadline: TimeInterval?
  private var isPointerHovering = false
  private let nowProvider: () -> TimeInterval
  /// Single source of truth for the Founder-dictated terminal lifetime.
  /// Five seconds is the comfortable end of the requested 3–5 second range.
  static let autoHideDelaySeconds: TimeInterval = 5

  init(nowProvider: @escaping () -> TimeInterval = { ProcessInfo.processInfo.systemUptime }) {
    let channel = AsyncStream<OverlayListenerEvent>.makeStream()
    eventStream = channel.stream
    listener = DictationListener(continuation: channel.continuation)
    self.nowProvider = nowProvider
    eventTask = Task { @MainActor [weak self, eventStream] in
      for await event in eventStream {
        guard let self else { return }
        apply(event)
      }
    }
  }

  func attach() {
    engine?.setListener(listener)
  }

  private func apply(_ event: OverlayListenerEvent) {
    switch event {
    case .transcriptProjection(let projection): applyTranscriptProjection(projection)
    case .recordingPreparing: handleRecordingPreparing()
    case .recordingStarted: handleRecordingStarted()
    case .recordingStopped: finishControllerRecording()
    case .recordingFinalising: handleRecordingFinalising()
    case .sessionFinalised: applySessionFinalised()
    case .vadActive(let active): applyVad(active)
    case .audioLevel(let rms): applyAudioLevel(rms)
    case .noSpeech(let reason): applyNoSpeech(reason: reason)
    case .error(let message): handleError(message: message)
    }
  }

  // MARK: Derived display (one source of truth for the view)

  var statusText: String {
    switch mode {
    case .listening: return "listening"
    case .finalizing: return "finalizing"
    case .formatted: return "formatted"
    case .noSpeech: return "no speech"
    case .error: return "error"
    }
  }

  /// Narrow-window projection of the same single phase truth. The full status
  /// keeps level honesty at normal widths; the live waveform carries that
  /// evidence at the 320 pt floor without forcing the pill into a vertical
  /// capsule.
  var compactStatusText: String {
    statusText
  }
  var statusColor: Color {
    switch mode {
    case .listening: return CSColor.terracotta
    case .finalizing: return CSColor.amber
    case .formatted: return CSColor.oliveLight
    case .noSpeech: return CSColor.textMuted
    case .error: return CSColor.terracotta
    }
  }

  /// Only a reducer-projected listening phase may ripple.
  var statusRippling: Bool {
    mode == .listening
      && (audioReady || vadActive)
  }

  /// Footer left engine chip — last stop serving label when available, else
  /// configured preference. Never a hardcoded "local whisper" (STT_CONTRACT).
  var footerEngineLabel: String {
    engineChip
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
  private var rawLiveText: String {
    formattedText
  }

  var liveText: String {
    rawLiveText
  }

  /// Text shown in the listening/finalizing body.
  ///
  /// CAPTURED WORDS ALWAYS WIN OVER PHASE. The previous shape let the
  /// transcribing phase replace the live canvas with "transcribing…", so
  /// stopping a recording made the user's own words vanish behind a spinner
  /// until the final text swapped in — the Founder dictated the bug report
  /// into the very canvas that then ate it (2026-08-09 20:13): "wyłączenie
  /// nagrywania zastępuje tekst … i podmienia dopiero ostateczny tekst a tego
  /// ma nie być". The overlay doctrine forbids exactly this class: never drop
  /// visible transcript. Phase placeholders render only on an EMPTY canvas;
  /// the header pill carries the phase otherwise.
  var listeningDisplay: String {
    if !liveText.isEmpty { return liveText }
    return mode == .finalizing ? "finalizing…" : "listening…"
  }

  /// Timer is mandatory for any session that has started, including the
  /// frozen value after stop.
  var showsSessionTimer: Bool {
    captureStartedAtUptime != nil
  }

  /// Exact rendered bytes for non-canvas consumers such as copy/delivery.
  var activeText: String {
    formattedText
  }

  /// Post-take review owns the floating panel. The formatted / no-speech
  /// surface must not yield to an Assistive tray tick — that path calls
  /// `hide()` and arms Agent auto-send.
  var blocksAssistiveOverlayHide: Bool {
    mode == .formatted || mode == .noSpeech
  }

  var autoPasteAccessibilityValue: String {
    autoPasteEnabled ? "On" : "Off"
  }

  var audioLevelAccessibilityValue: String {
    guard let gain = levelMeter.gain else { return "Waiting for measured level" }
    switch gain {
    case ..<0.12: return "Very quiet"
    case ..<0.35: return "Quiet"
    case ..<0.68: return "Good level"
    default: return "Strong level"
    }
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
      presentTerminalError(
        message:
          "Microphone access is off for Codescribe. Enable it in System Settings › Privacy & Security › Microphone.",
        toast: "Microphone access denied"
      )
      return
    }
    engine.setListener(listener)
    warmingUp = true
    resetTranscript()
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
      let state = await SpeechRecognitionPermission.request()
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
    // Prevent duplicate stops while Rust emits authoritative finalizing and
    // terminal projections. This flag never paints a phase.
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
    toastTask?.cancel()
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

  private func refreshOverlayPolicyTruth() {
    guard let truth = engine?.currentOverlayPolicy() else { return }
    autoPasteEnabled = truth.autoPasteEnabled
    autoFormatLevel = truth.autoFormatLevel
  }

  private var engineChipLatched = false

  private func refreshEngineChip(reset: Bool) {
    if reset { engineChipLatched = false }
    guard !engineChipLatched else { return }
    engineChipLatched = true
    if let serving = currentServingVerdict() {
      let engine = serving.engine.trimmingCharacters(in: .whitespacesAndNewlines)
      if !engine.isEmpty {
        engineChip = Self.displayEngineChip(engine)
        return
      }
    }
    let preference = CodescribeConfig().loadSettings().sttEngine?
      .trimmingCharacters(in: .whitespacesAndNewlines)
    switch preference?.lowercased() {
    case "whisper", "candle": engineChip = "local whisper"
    case "auto": engineChip = "auto · apple-first"
    case let preference? where !preference.isEmpty: engineChip = preference
    default: engineChip = "local apple"
    }
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

  // MARK: P0-D quality loop

  private func captureQualityIfEdited(action: String) {
    guard mode == .formatted else { return }
    // `commitOverlayQualityRecord` is a free FFI function, not a call on the
    // injected `engine` — so a mocked engine does NOT stop it, and the XCTest
    // suite was appending two synthetic corrections ("original delivered
    // transcript here with user fix") to the FOUNDER'S live
    // ~/.codescribe/quality/corrections.jsonl on every run. 276 of 501 rows
    // in the real store came from test runs, and they surfaced in Settings ›
    // Dictionary as if the user had made them (Founder screenshot
    // 2026-08-09 14:21, three seconds after a suite finished). The keychain
    // test-host gate landed earlier did not cover this path.
    guard !QualityCaptureHost.isRunningTests else { return }
    let delivered = deliveredText.trimmingCharacters(in: .whitespacesAndNewlines)
    let edited = formattedText.trimmingCharacters(in: .whitespacesAndNewlines)
    guard !edited.isEmpty else { return }
    let isEdited = delivered != edited
    // Unedited transcripts used to never reach the review queue — but "not
    // corrected on the overlay" means "no time right now", not "perfect"
    // (Founder, 2026-08-09). Capture them once per session, on close, so
    // Settings › Dictionary can serve as the deferred correction desk. The
    // identical delivered/edited pair teaches the lexicon nothing (word-pair
    // extraction over a zero delta yields zero rules), so this fills the
    // queue without poisoning learning.
    guard isEdited || action == "close" else { return }
    let recordedAction = isEdited ? action : "close-unreviewed"
    let editProvenance: String? = nil
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
    warmingUp = true
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
    refreshEngineChip(reset: true)
    onRecordingPreparing?()
    armWarmupWatchdog()
  }

  func handleRecordingStarted() {
    cancelWarmupWatchdog()
    finalized = false
    isFinalPass = false
    warmingUp = false
    audioReady = true
    if !recording {
      hasMeasuredAudioLevel = false
      levelMeter.reset()
      resetTranscript()
      errorMessage = nil
      beginCaptureClock()
    }
    if captureStartedAtUptime == nil {
      beginCaptureClock()
    }
    recording = true
    refreshOverlayPolicyTruth()
    refreshEngineChip(reset: false)
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

    if shouldNotifyStopped {
      finalized = true
      onRecordingStopped?()
    }
  }

  /// Native hold-release / toggle-stop lifecycle evidence. It freezes capture
  /// resources and guards duplicate transitions, but never selects a visible
  /// phase; only a projection can do that.
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
    onClose?()
  }

  private var isTerminalMode: Bool {
    terminal
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
      abortRecordingSession()
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

  /// Route a repairable terminal error to the closest product surface. The
  /// destination is presentation-only: it never changes controller admission.
  static func recoverySettingsSection(from message: String) -> SettingsSection? {
    if speechAuthNotice(from: message) != nil { return .creator }
    let lowered = message.lowercased()
    if lowered.contains("microphone access") || lowered.contains("microphone permission") {
      return .audio
    }
    guard let range = message.range(of: "admission_") else {
      if lowered.contains("transcription_failed") || lowered.contains("stt model") {
        return .engine
      }
      return nil
    }
    let tail = String(message[range.lowerBound...])
    let code = tail.prefix { $0 == "_" || $0.isLetter }
    if code == "admission_refused" {
      let detail = tail.dropFirst(code.count).drop { $0 == ":" || $0 == " " }
      return recoverySettingsSection(from: String(detail))
    }
    switch code {
    case "admission_seal_vad_unavailable": return .engine
    case "admission_calibration_missing", "admission_calibration_no_profile",
      "admission_calibration_refused", "admission_calibration_unusable",
      "admission_seal_lane_disarmed", "admission_capture_device_unavailable":
      return .audio
    default: return nil
    }
  }

  static func recoverySettingsAnchor(from message: String) -> SettingsAnchor? {
    let lowered = message.lowercased()
    if lowered.contains("microphone access") || lowered.contains("microphone permission") {
      return .audioReadiness
    }
    guard let range = message.range(of: "admission_") else { return nil }
    let tail = String(message[range.lowerBound...])
    let code = tail.prefix { $0 == "_" || $0.isLetter }
    if code == "admission_refused" {
      let detail = tail.dropFirst(code.count).drop { $0 == ":" || $0 == " " }
      return recoverySettingsAnchor(from: String(detail))
    }
    switch code {
    case "admission_capture_device_unavailable": return .audioInput
    case "admission_calibration_missing", "admission_calibration_no_profile",
      "admission_calibration_refused", "admission_calibration_unusable",
      "admission_seal_lane_disarmed":
      return .audioReadiness
    default: return nil
    }
  }

  private func presentTerminalError(message: String, toast: String) {
    let speechNotice = OverlayState.speechAuthNotice(from: message)
    let admissionNotice = OverlayState.admissionNotice(from: message)
    let recoverySection = OverlayState.recoverySettingsSection(from: message)
    let recoveryAnchor = OverlayState.recoverySettingsAnchor(from: message)
    let captureHadStarted = recording
    let message = speechNotice ?? admissionNotice?.detail ?? message
    let toast = speechNotice ?? admissionNotice?.headline ?? toast
    abortRecordingSession()
    pendingNoSpeechMessage = nil
    noSpeechNotice = OverlayState.defaultNoSpeechNotice
    isFinalPass = false
    errorMessage = message
    recoverySettingsSection = recoverySection
    recoverySettingsAnchor = recoveryAnchor
    errorLifecycleDetail =
      captureHadStarted
      ? "Recording stopped before a transcript was available."
      : "Recording did not start."
    finalized = true
    showToast(toast)
  }

  // MARK: Listener-driven mutations (called on the main actor by DictationListener)

  /// Parse one reducer-owned projection, then paint every contract field 1:1.
  /// Ordering, admission, availability and terminal decisions have already
  /// happened in Rust; this method never reconstructs them from text or receipts.
  func applyTranscriptProjection(_ event: CsTranscriptProjectionEvent) {
    guard let phase = OverlayMode(rawValue: event.phase) else {
      assertionFailure("Unknown transcript projection phase: \(event.phase)")
      return
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
    let projection = OverlayTranscriptProjection(
      schema: event.schema,
      sequence: event.sequence,
      emittedAt: event.emittedAt,
      sessionId: event.sessionId,
      mode: event.mode,
      phase: phase,
      reducerRevision: event.reducerRevision,
      reducerAction: event.reducerAction,
      occurrenceSessionId: event.occurrenceSessionId,
      captureEpoch: event.captureEpoch,
      sampleStart: event.sampleStart,
      sampleEnd: event.sampleEnd,
      documentIndex: event.documentIndex,
      label: event.label,
      renderedText: event.renderedText,
      canPaste: event.canPaste,
      canInsert: event.canInsert,
      canCopy: event.canCopy,
      canRetranscribe: event.canRetranscribe,
      canFormat: event.canFormat,
      terminal: event.terminal,
      acousticReceipts: acousticReceipts
    )
    applyProjection(projection)
  }

  private func applyProjection(_ projection: OverlayTranscriptProjection) {
    let signalsFirstSuccessfulTerminal =
      !terminal && projection.terminal && projection.phase == .formatted
    if projection.terminal {
      // Release capture before flipping `finalized`; abort uses the previous
      // value to decide whether the app-level stopped callback is still owed.
      abortRecordingSession()
    }
    latestTranscriptProjection = projection
    if !projection.terminal {
      markTranscriptActivity()
    }
    transcriptMode = projection.mode
    mode = projection.phase
    revision = projection.reducerRevision
    formattedText = projection.renderedText
    canPaste = projection.canPaste
    canInsert = projection.canInsert
    canCopy = projection.canCopy
    canRetranscribe = projection.canRetranscribe
    canFormat = projection.canFormat
    terminal = projection.terminal
    finalized = projection.terminal

    if projection.terminal {
      deliveredText = projection.renderedText
      agentFinalTranscriptAppeared = projection.phase == .formatted
      if signalsFirstSuccessfulTerminal {
        onSuccessfulDictation?()
      }
      if projection.phase == .noSpeech {
        noSpeechNotice = pendingNoSpeechMessage ?? OverlayState.defaultNoSpeechNotice
      }
      restartAutoHideCountdown()
    }
  }

  func applySessionFinalised() {
    guard !finalized else { return }
    markTranscriptActivity()
    // Lifecycle-only evidence that formatting is running. Presentation remains
    // whatever the latest reducer projection says.
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
    noSpeechNotice = message
  }

  private func resetTranscript() {
    deliveredText = ""
    pendingNoSpeechMessage = nil
    noSpeechNotice = OverlayState.defaultNoSpeechNotice
    recoverySettingsSection = nil
    recoverySettingsAnchor = nil
    errorLifecycleDetail = "Recording stopped before a transcript was available."
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
    // stray `vadActive` flip is just another needless invalidation.
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

  /// Seeded view model for #Preview in the listening state.
  static func previewListening() -> OverlayState {
    let s = OverlayState()
    s.applyProjection(
      previewProjection(
        "add a rate limiter to the login route and write a test for it",
        phase: .listening,
        terminal: false
      )
    )
    s.vadActive = true
    return s
  }

  /// Seeded view model for #Preview in the post-capture transcribing phase.
  static func previewTranscribing() -> OverlayState {
    let s = OverlayState()
    s.applyProjection(
      previewProjection(
        "add a rate limiter to the login route and write a test for it",
        phase: .finalizing,
        terminal: false
      )
    )
    s.audioReady = true
    return s
  }

  /// Seeded view model for #Preview in the no-speech outcome (session ended
  /// without any usable text).
  static func previewNoSpeech() -> OverlayState {
    let s = OverlayState()
    s.applyProjection(previewProjection("", phase: .noSpeech, terminal: true))
    s.noSpeechNotice = OverlayState.defaultNoSpeechNotice
    return s
  }

  /// Seeded view model for #Preview in the finalized state.
  static func previewFormatted() -> OverlayState {
    let s = OverlayState()
    s.applyProjection(
      previewProjection(
        "Add a rate limiter to the login route and write a test that covers the throttle window. Keep the existing error shape.",
        phase: .formatted,
        terminal: true
      )
    )
    return s
  }

  private static func previewProjection(
    _ renderedText: String,
    phase: OverlayMode,
    terminal: Bool
  ) -> OverlayTranscriptProjection {
    OverlayTranscriptProjection(
      schema: "preview", sequence: 1, emittedAt: "preview", sessionId: "preview", mode: "dictation",
      phase: phase, reducerRevision: 1, reducerAction: "preview_fixture",
      occurrenceSessionId: "preview",
      captureEpoch: 0, sampleStart: 0, sampleEnd: 0, documentIndex: 0, label: renderedText,
      renderedText: renderedText, canPaste: false, canInsert: false,
      canCopy: !renderedText.isEmpty, canRetranscribe: terminal, canFormat: !terminal,
      terminal: terminal, acousticReceipts: [])
  }
}

/// Adapter for the redesign hotkey/controller path. This is the product path:
/// one `RecordingController`, one event stream, one Swift overlay surface.
@MainActor
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

// MARK: - Listener bridge (Rust callbacks → ordered stream → main actor)

/// A value-only callback adapter. Its immutable continuation is Sendable, so
/// the class needs no unchecked promise about actor isolation.
final class DictationListener: CsTranscriptionListener {
  private let continuation: AsyncStream<OverlayListenerEvent>.Continuation

  init(continuation: AsyncStream<OverlayListenerEvent>.Continuation) {
    self.continuation = continuation
  }

  func onTranscriptProjection(event: CsTranscriptProjectionEvent) {
    continuation.yield(.transcriptProjection(event))
  }

  func onRecordingPreparing() {
    continuation.yield(.recordingPreparing)
  }
  func onRecordingStarted() {
    continuation.yield(.recordingStarted)
  }
  func onRecordingStopped() {
    continuation.yield(.recordingStopped)
  }
  func onRecordingFinalising() {
    continuation.yield(.recordingFinalising)
  }
  func onSessionFinalised(sessionId: String, layerSummary: CsLayerSummary) {
    continuation.yield(.sessionFinalised)
  }
  func onVadActive(active: Bool) {
    continuation.yield(.vadActive(active))
  }
  func onAudioLevel(rms: Float) {
    continuation.yield(.audioLevel(rms))
  }
  func onNoSpeech(reason: String) {
    // Route the reason into the dedicated no-speech OUTCOME (a persistent
    // body + Close), not a transient toast that fades and leaves an empty
    // editable FINAL behind. `applyNoSpeech` maps the reason to a user-facing
    // notice (genuine silence vs. quality-gate rejection).
    continuation.yield(.noSpeech(reason))
  }
  func onError(message: String) {
    continuation.yield(.error(message))
  }
}

// MARK: - Mock engine for #Preview

#if DEBUG
  @MainActor
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
