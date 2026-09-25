import AppKit
import Observation
import SwiftUI

// View model for the dictation overlay, backed by the redesign hotkey/controller
// bridge (`CodescribeHotkeys` / `CsTranscriptionListener`).
//
// The view talks only to the thin `DictationEngine` protocol below, so #Preview
// renders standalone against seeded view models (`OverlayState.previewListening()`).
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

/// Minimal slice of the controller-backed dictation surface the overlay needs.
/// Kept as a protocol so the view-model + preview compile without a live Rust core.
@MainActor
protocol DictationEngine: AnyObject {
  func setListener(_ listener: CsTranscriptionListener)
  func startRecording(language: CsLanguage?) async throws
  func stopRecording() async throws -> String
  func commitUserRevision(
    sessionId: String, sourceRevision: UInt64, renderedText: String
  ) async throws -> CsUserRevisionResult
  func commitRetranscribeRevision(
    sessionId: String, sourceRevision: UInt64, renderedText: String
  ) async throws -> CsUserRevisionResult
  func commitFormatterRevision(
    sessionId: String, sourceRevision: UInt64, level: FormattingPolicyOption?
  ) async throws -> CsUserRevisionResult
  func documentHistory(sessionId: String) async throws -> [CsDocumentHistoryEntry]
  func restoreDocumentRevision(
    sessionId: String, sourceRevision: UInt64, restoreRevision: UInt64
  ) async throws -> CsUserRevisionResult
  func isRecording() async -> Bool
  func initModel() async throws
  func isModelLoaded() -> Bool
  func currentOverlayPolicy() -> OverlayPolicySnapshot?
  func setAutoPasteEnabled(_ enabled: Bool)
  func setAutoFormatLevel(_ level: FormattingPolicyOption)
  func overlayExpandedByDefault() -> Bool
  func setOverlayExpandedByDefault(_ enabled: Bool) -> Bool
  func overlayKeepVisibleBetweenTakes() -> Bool
  func setOverlayKeepVisibleBetweenTakes(_ enabled: Bool) -> Bool
  func pasteText(text: String) async throws -> CsPasteResult
  func deferText(text: String) async throws -> CsPasteResult
  func copyTaggedTranscript(text: String) async throws
  func pasteTargetAppName() async -> String?
  func sendAssistiveTranscript(text: String) async throws -> Bool
  func lastSessionAudioPath() -> String?
  func sessionAudioPath(sessionId: String) -> String?
  func transcribeFile(path: String) async throws -> CsTranscription
  func transcribeTake(sessionId: String, path: String) async throws -> CsTranscription
}

extension DictationEngine {
  func documentHistory(sessionId _: String) async throws -> [CsDocumentHistoryEntry] { [] }
  func restoreDocumentRevision(
    sessionId _: String, sourceRevision _: UInt64, restoreRevision _: UInt64
  ) async throws -> CsUserRevisionResult {
    throw NSError(domain: "Transcript history unavailable", code: 1)
  }
  func commitRetranscribeRevision(
    sessionId: String, sourceRevision: UInt64, renderedText: String
  ) async throws -> CsUserRevisionResult {
    try await commitUserRevision(
      sessionId: sessionId, sourceRevision: sourceRevision, renderedText: renderedText)
  }
  func lastSessionAudioPath() -> String? { nil }
  func sessionAudioPath(sessionId: String) -> String? { nil }
  func transcribeTake(sessionId _: String, path: String) async throws -> CsTranscription {
    try await transcribeFile(path: path)
  }
  func overlayExpandedByDefault() -> Bool { true }
  func setOverlayExpandedByDefault(_ enabled: Bool) -> Bool { false }
  func overlayKeepVisibleBetweenTakes() -> Bool { false }
  func setOverlayKeepVisibleBetweenTakes(_ enabled: Bool) -> Bool { false }
}

struct OverlayPolicySnapshot: Equatable {
  let autoPasteEnabled: Bool
  let autoFormatLevel: FormattingPolicyOption
}

/// Value-only rendering model for Rust-owned product status. It deliberately
/// contains no command, Settings route, or transcript field.
struct OverlayPresentationStatus: Equatable {
  let schema: String
  let emittedAt: String
  let sessionId: String?
  let kind: String
  let code: String
  let statusLabel: String
  let headline: String
  let message: String
  let isError: Bool
  let terminal: Bool
  let calibrationVersion: String?
}

/// One superseded take, retained as a single identity-associated unit.
///
/// Retention used to be two disconnected slots — a projection and a draft
/// string — with no production reader and no association between them, so a
/// third capture could clear the draft while the projection still described the
/// take that authored it. Identity, document and unsent edit now move together
/// or not at all.
///
/// The identity is Rust's: a take with no observed projection has no session id
/// and is therefore not retained, rather than retained under a minted one.
struct OverlaySupersededTake: Equatable, Identifiable {
  let sessionId: String
  /// The last authoritative document Rust projected for this session.
  let renderedText: String
  let reducerRevision: UInt64
  /// The user's uncommitted edit at the capture boundary, when it differed from
  /// `renderedText`. `nil` means nothing was unsaved — the document itself is
  /// still ledger-authoritative and reproducible.
  let unsavedDraft: String?

  var id: String { "\(sessionId)#\(reducerRevision)" }
  var hasUnsavedEdits: Bool { unsavedDraft != nil }
  /// What an explicit recovery hands back: the user's own edit when one was
  /// unsaved, otherwise the projected document.
  var recoverableText: String { unsavedDraft ?? renderedText }
}

/// How a sibling presentation status addresses the receiver's current capture.
///
/// The producer states this, it is never guessed: `PresentationStatusProjection`
/// carries `session_id: Option<String>` and a typed `kind`
/// (`app/presentation/status_projection.rs`). `admission_refused` is a capture
/// lifecycle verdict — with the refused session when one exists, without it when
/// the refusal happened before a session did. The two calibration outcomes carry
/// no session by construction: they are Settings-owned microphone results, so
/// letting one release a live capture would be minting capture identity for an
/// event that has none.
enum OverlayStatusAddressing: Equatable {
  /// Addressed to the capture the receiver is showing (or to the one it just
  /// optimistically opened). Presents and releases exactly as before.
  case currentCapture
  /// A known retired session's delayed status. It may not touch the successor.
  case retiredSession
  /// A status with no capture identity, arriving while a capture is in flight.
  /// The card is still product truth; the capture is not its to end.
  case foreignToLiveCapture
}

/// Presentation phase supplied by the reducer-owned projection. Swift parses
/// the wire value but never derives a phase from text, callbacks, or seals.
enum OverlayMode: String, Equatable {
  case listening
  case finalizing
  case formatted
  /// The lifecycle settled with usable words whose acoustic coverage the
  /// ledger refused. It is its own terminal outcome, not a dialect of the
  /// other two: `formatted` would claim a seal that was never recorded, and
  /// `error` would claim the words are lost when they are on the canvas.
  /// Producer: `TranscriptProjectionPhase::CoverageRefused`.
  case coverageRefused = "coverage_refused"
  case noSpeech = "no_speech"
  case error
}

/// A user command emitted by the overlay rail. The rail decides only which
/// projected commands to paint; this value crosses the view/state seam without
/// carrying a second copy of reducer or delivery policy.
/// Retranscribe pass picked on the dock. Path prefixes are the bridge contract
/// (`bridge/src/recording.rs` `split_retranscribe_path`).
enum OverlayRetranscribePass: String, CaseIterable, Identifiable {
  case fullHq = "hq"
  case cloud = "cloud"

  var id: String { rawValue }

  var pathPrefix: String { "\(rawValue):" }

  var visibleName: String {
    switch self {
    case .fullHq: "Full HQ file pass"
    case .cloud: "Cloud pass"
    }
  }

  var help: String {
    switch self {
    case .fullHq: "Full local Whisper file pass over the last session audio"
    case .cloud: "Cloud STT pass over the last session audio"
    }
  }
}

enum OverlayIntent: String, Equatable, Hashable, CaseIterable {
  case finish
  case commitRevision = "commit-revision"
  case discardRevision = "discard-revision"
  case copy
  case insertPaste = "insert-paste"
  case retranscribe
  /// Restore the exact rendered text the last Retranscribe replaced, committed
  /// as a new user revision on the same session. Rail-projected only while the
  /// replaced text is still recoverable (same session, no newer capture).
  case undoRetranscribe = "undo-retranscribe"
  case format
  case sendToAgent = "send-to-agent"
  /// Hand one retained superseded take back to the user, or drop it on an
  /// explicit acknowledgement. These are the only two commands on the rail the
  /// reducer does not project: retained work is presentation-local by
  /// construction, because the bytes it protects are an edit Rust never saw.
  case recoverSuperseded = "recover-superseded"
  case discardSuperseded = "discard-superseded"
  case close
}

/// The sole cross-thread ingress into the overlay. UniFFI callbacks enqueue
/// values here; one main-actor consumer applies them in arrival order.
enum OverlayListenerEvent: Sendable {
  case transcriptProjection(CsTranscriptProjectionEvent)
  case presentationStatus(CsPresentationStatusEvent)
  case compactProjection(CsCompactProjection)
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

  func onPresentationStatus(event: CsPresentationStatusEvent) {
    continuation.yield(.presentationStatus(event))
  }

  func onCompactProjection(event: CsCompactProjection) {
    continuation.yield(.compactProjection(event))
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

@MainActor
@Observable
final class OverlayState {

  // MARK: Published state
  private(set) var transcriptMode = "dictation"
  private(set) var mode: OverlayMode = .listening
  var formattedText: String { latestTranscriptProjection?.renderedText ?? "" }
  /// View-local editor payload. It is never delivery or transcript truth; only
  /// `formattedText`, repainted from the Rust projection, feeds downstream
  /// actions. The canvas paints it while the formatted take is under review.
  var revisionDraft = ""
  /// True while the transcript canvas holds keyboard focus on the panel. The
  /// panel is key only inside this window; see `FloatingOverlayPanel`.
  private(set) var isEditingTranscript = false
  private(set) var revision: UInt64 = 0
  private(set) var revisionCommitPending = false
  private(set) var revisionCommitError: String?
  private(set) var formatterCommitPending = false
  private(set) var formatterError: String?
  /// Read-only projection of this take's Bus journal revisions.
  private(set) var documentHistory: [CsDocumentHistoryEntry] = []
  private var historyReadSessionId: String?
  private(set) var userRevisionProvenance: String?
  private(set) var canPaste = false
  private(set) var canInsert = false
  private(set) var canCopy = false
  private(set) var canRetranscribe = false
  private(set) var canFormat = false
  private(set) var canSendToAgent = false
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
  private(set) var presentationStatus: OverlayPresentationStatus?
  private(set) var compactProjection: CsCompactProjection?
  private(set) var errorLifecycleDetail =
    "No transcript was delivered."
  /// Standing explanation for a take whose terminal seal the ledger refused,
  /// independently of measured coverage. Non-nil while the terminal projection carried
  /// `coverage_refused`, so the surface cannot outlive the fact it describes.
  ///
  /// It is a notice, never a document: it holds no transcript bytes, cannot
  /// be delivered, and does not touch `formattedText`. The words themselves
  /// stay exactly where the reducer put them — on the canvas.
  private(set) var coverageRefusalNotice: String?
  /// Prompt-free policy snapshot from C02's persisted settings owner. These
  /// values are replaced only by a fresh engine read, never by optimistic UI.
  private(set) var autoPasteEnabled = true
  private(set) var autoFormatLevel: FormattingPolicyOption = .correction
  /// Assistive sessions never expose delivery controls. The controller owns
  /// that authoritative session gate and updates this presentation fence.
  private(set) var autoPasteControlAvailable = true
  /// Serving-engine label latched once per session. Rendering never performs
  /// settings I/O or a UniFFI read.
  private(set) var engineChip = "not yet served"
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
      freeMotion = false
      onPlacementChanged?()
    }
  }
  /// Free motion: the panel keeps (and restores) wherever the user dragged it.
  private(set) var freeMotion: Bool = OverlayPlacement.freeMotion {
    didSet {
      guard freeMotion != oldValue else { return }
      OverlayPlacement.freeMotion = freeMotion
    }
  }
  /// Wired by the orchestrator: re-derive the visible panel's origin now.
  var onPlacementChanged: (() -> Void)?

  /// A menu selection is an immediate positioning command, including when the
  /// user chooses the already-selected anchor to leave Free motion.
  func selectPlacementAnchor(_ anchor: OverlayAnchor) {
    if placementAnchor != anchor {
      placementAnchor = anchor
    } else {
      freeMotion = false
      onPlacementChanged?()
    }
  }

  /// Free motion starts from the panel's current/restored origin; subsequent
  /// windowDidMove callbacks persist every user drag.
  func selectFreeMotion() {
    freeMotion = true
    onPlacementChanged?()
  }

  /// The window already reached this point through a user drag. Persist it
  /// before changing modes, without asking the orchestrator to place it again.
  func recordUserDrag(at origin: NSPoint) {
    OverlayPlacement.persistOrigin(origin)
    freeMotion = true
    userDraggedOverlay()
  }

  // MARK: Injected collaborators (all optional so #Preview renders standalone)
  /// The recording core. Injected by the orchestrator. Do NOT instantiate here.
  var engine: DictationEngine?
  /// Handoff to the agent surface — wired by the orchestrator (routes the text
  /// into AgentChat, which streams it through `CodescribeAgent.streamReply`).
  var onSendToAgent: ((String) -> Void)?
  /// Dismiss the floating window — wired by the orchestrator.
  var onClose: (() -> Void)?
  /// Window chrome only: folding never ends capture or creates text edits.
  /// Leaving an edited canvas uses its existing commit-on-blur path.
  private(set) var isCollapsed = true
  private(set) var expandedByDefault = true
  private(set) var keepVisibleBetweenTakes = false
  private(set) var expansionPreferenceError: String?
  @ObservationIgnored var onCollapseChanged: ((Bool) -> Void)?

  func toggleCollapsed() {
    isCollapsed.toggle()
    onCollapseChanged?(isCollapsed)
  }

  /// The menu toggle alone persists the take-start preference.
  func setExpandedByDefault(_ expanded: Bool) {
    guard let engine, engine.setOverlayExpandedByDefault(expanded) else {
      expansionPreferenceError = "Couldn't save overlay preference"
      return
    }
    expansionPreferenceError = nil
    applyPreferredExpansion()
  }

  private func applyPreferredExpansion() {
    guard let engine else { return }
    expandedByDefault = engine.overlayExpandedByDefault()
    let collapsed = !expandedByDefault
    guard isCollapsed != collapsed else { return }
    isCollapsed = collapsed
    onCollapseChanged?(collapsed)
  }

  func setKeepVisibleBetweenTakes(_ enabled: Bool) {
    guard let engine, engine.setOverlayKeepVisibleBetweenTakes(enabled) else {
      expansionPreferenceError = "Couldn't save overlay preference"
      return
    }
    expansionPreferenceError = nil
    keepVisibleBetweenTakes = engine.overlayKeepVisibleBetweenTakes()
    if keepVisibleBetweenTakes {
      cancelAutoHide()
    } else if terminal {
      restartAutoHideCountdown()
    }
  }
  var onRecordingPreparing: (() -> Void)?
  var onRecordingStarted: (() -> Void)?
  var onRecordingStopped: (() -> Void)?
  @ObservationIgnored var onPresentationStatus: (() -> Void)?
  /// Presentation-only invalidation seam. The window controller may use the
  /// already-admitted projection to grow its canvas; no transcript bytes leave
  /// this passive overlay boundary.
  @ObservationIgnored var onTranscriptPresentationChanged: (() -> Void)?
  /// Hand the terminal document to the Agent composer and read back what the
  /// receiver did with it.
  ///
  /// The return value is the whole point: the overlay is the postman, and a
  /// postman does not sign for the parcel. Only an `.admitted` or `.parked`
  /// receipt lets this state release the capture destination or record the
  /// delivery as done. A `.retained` receipt keeps the text on screen and
  /// recoverable instead of painting a success the composer never saw.
  @ObservationIgnored var onComposerTranscript: ((String, String) -> ComposerDeliveryReceipt)?
  /// Content-free success seam. No transcript crosses this callback.
  var onSuccessfulDictation: (() -> Void)?

  /// Strong refs for the one ordered Rust-callback ingress.
  @ObservationIgnored private let listener: CsTranscriptionListener
  @ObservationIgnored private var receiverOwnsRecovery: (() -> Bool)?
  @ObservationIgnored private var acceptsCaptureTerminal: ((String) -> Bool)?
  @ObservationIgnored var onCaptureEnded: ((String) -> Void)?
  @ObservationIgnored private var retiredProjectionSessions: Set<String> = []
  @ObservationIgnored private var admittedComposerSessions: Set<String> = []

  func connectComposer(to store: AgentChatStore) {
    receiverOwnsRecovery = { [weak store] in store != nil }
    onComposerTranscript = { [weak store] text, sessionID in
      store?.receiveDictationTranscript(text, captureID: sessionID) ?? .retained(text)
    }
    acceptsCaptureTerminal = { [weak store] sessionID in
      guard let store else { return true }
      return !store.hasComposerCaptureRequest || store.composerCaptureHandle?.captureId == sessionID
    }
    onCaptureEnded = { [weak store] sessionID in
      store?.finishDictationCapture(sessionID: sessionID)
    }
  }

  @ObservationIgnored private let eventStream: AsyncStream<OverlayListenerEvent>
  @ObservationIgnored private var eventTask: Task<Void, Never>?

  static let defaultNoSpeechNotice = "No speech detected"
  /// Missing coverage is unverified, never proof of missing words.
  static let defaultCoverageRefusalNotice =
    "Coverage could not be verified — these words were kept, not sealed"

  /// Display copy distinguishes measured coverage from refused finality.
  /// Absent coverage is unknown; complete coverage does not mint a terminal seal.
  private var coverageRefusalCopy: (status: String, notice: String) {
    guard let coverage = latestTranscriptProjection?.sealCoverage else {
      return ("unverified coverage", Self.defaultCoverageRefusalNotice)
    }
    switch coverage.status {
    case .incomplete:
      return ("incomplete coverage", "Incomplete coverage — these words were kept, not sealed")
    case .unavailable:
      let explanation: String
      switch coverage.unavailableReason {
      case .notObserved: explanation = "No acoustic measurement was available for this take"
      case .identityMismatch: explanation = "The acoustic measurement did not match this take"
      case .invalidMeasurement: explanation = "The acoustic measurement could not be used"
      case .partialObservation:
        explanation = "The acoustic measurement covered only part of this take"
      case .unknown, nil: explanation = "The acoustic measurement was unavailable"
      }
      return ("measurement unavailable", "\(explanation) — these words were kept, not sealed")
    case .complete:
      return ("unsealed transcript", "These words were kept, but the transcript was not sealed")
    case .unknown:
      return ("unverified coverage", Self.defaultCoverageRefusalNotice)
    }
  }

  /// Admission metadata only; no transcript or acoustic adjudication is stored.
  /// Keep each session's fence when its paint retires so delayed delivery still
  /// addresses that session without comparing its sequence to a successor's.
  private var projectionOrder: [String: (sequence: UInt64, revision: UInt64, epoch: UInt64)] = [:]
  private var endedProjectionSessions: Set<String> = []

  private(set) var recording = false
  /// Reason from `on_no_speech`, captured before the terminal stop.
  private var pendingNoSpeechMessage: String?
  /// The exact rendered text at the terminal projection.
  private var deliveredText: String = ""
  private var deliveredTextSessionId: String?
  /// Missing/refusing callbacks retain documents by identity across sessions.
  /// This is fallback presentation; the connected store owns recovery actions.
  private var retainedComposerDocuments: [(id: String, text: String)] = []
  var retainedComposerDelivery: String? {
    retainedComposerDocuments.isEmpty
      ? nil
      : retainedComposerDocuments.map(\.text).joined(separator: "\n")
  }
  private var qualityCapturedProvenance: String?
  private var pendingRevisionSessionId: String?
  private var pendingRevisionSource: UInt64?
  private var revisionFocusCommitTask: Task<Void, Never>?
  /// Last reducer-owned projection painted by Swift. The reducer owns ordering
  /// and finality within a session; retired sessions cannot repaint the current one.
  private var finalized = false
  /// Latest immutable projection event only; Rust `TranscriptRevision` remains
  /// the document owner and Rust `AcousticSerial` remains evidence authority.
  private(set) var latestTranscriptProjection: CsTranscriptProjectionEvent?
  /// Monotonic, presentation-local capture generation. Every asynchronous UI
  /// job (auto-hide wake, warmup watchdog, panel handoff completion) captures
  /// the generation it was armed under and refuses to act once it moved.
  ///
  /// This is a fence, never an authority: it cannot mint a session, an
  /// occurrence, a seal, a delivery acknowledgement or acoustic evidence. Rust
  /// owns all five, and a UI counter that started naming them would be exactly
  /// the forged transcript authority this overlay is forbidden to invent.
  private(set) var captureGeneration: UInt64 = 0
  /// The ONE owner of superseded work, oldest first.
  ///
  /// Every entry is an identity-associated `OverlaySupersededTake` moved OUT of
  /// the paint path when a new capture is admitted, so a pending capture can
  /// never show a previous take as new speech. This is not a second transcript
  /// history store: delivery recovery still belongs to the composer store and
  /// `retainedComposerDocuments`, and nothing here can be replayed into the
  /// reducer.
  ///
  /// Capacity is asymmetric, and the asymmetry is stated plainly rather than
  /// dressed up as a bound. Clean documents ARE capped at one, because each
  /// stays authoritative in the ledger and is reproducible from it. Unsaved
  /// edits have NO count limit: from a UI-only fence the only two ways to
  /// impose one would be dropping a user's edit (silent loss — forbidden) or
  /// refusing microphone admission (not this layer's call). So this array is
  /// bounded by the user's own unresolved decisions, not by a constant, and
  /// `unacknowledgedSupersededEditCount` reports that count — it does not
  /// enforce anything. That residual capacity boundary is disclosed, not fixed
  /// here; closing it needs an owner outside this fence.
  private(set) var supersededTakes: [OverlaySupersededTake] = []
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
  private(set) var autoHideDeadline: TimeInterval?
  private var isPointerHovering = false
  private let nowProvider: () -> TimeInterval
  private let autoSendEnabled: () -> Bool
  /// Terminal countdown for non-Agent outcomes and opted-in Agent delivery.
  static let autoHideDelaySeconds: TimeInterval = 5

  init(
    nowProvider: @escaping () -> TimeInterval = { ProcessInfo.processInfo.systemUptime },
    autoSendEnabled: @escaping () -> Bool = { CodescribeConfig().loadSettings().agentAutoSend }
  ) {
    let channel = AsyncStream<OverlayListenerEvent>.makeStream()
    eventStream = channel.stream
    listener = DictationListener(continuation: channel.continuation)
    self.nowProvider = nowProvider
    self.autoSendEnabled = autoSendEnabled
    eventTask = Task { @MainActor [weak self, eventStream] in
      for await event in eventStream {
        guard let self else { return }
        apply(event)
      }
    }
  }

  func attach() {
    applyPreferredExpansion()
    keepVisibleBetweenTakes = engine?.overlayKeepVisibleBetweenTakes() ?? false
    engine?.setListener(listener)
  }

  private func apply(_ event: OverlayListenerEvent) {
    switch event {
    case .transcriptProjection(let projection): applyTranscriptProjection(projection)
    case .presentationStatus(let status): applyPresentationStatus(status)
    case .compactProjection(let projection): applyCompactProjection(projection)
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
    if let presentationStatus { return presentationStatus.statusLabel }
    switch mode {
    case .listening: return "listening"
    case .finalizing: return "finalizing"
    case .formatted: return "formatted"
    // Never "formatted": the pill is the first thing a user reads, and the
    // one word it must not say about a refused take is the word that means
    // sealed.
    case .coverageRefused: return coverageRefusalCopy.status
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

  /// Timer is mandatory for any session that has started, including the
  /// frozen value after stop.
  var showsSessionTimer: Bool {
    captureStartedAtUptime != nil
  }

  /// Exact engine text shared by the canvas, sizing, copy, and delivery.
  var activeText: String {
    formattedText
  }

  /// The one transcript canvas is an editor only for a formatted, sealed take
  /// that is not mid-commit. Listening / finalizing stay read-only and the
  /// panel never takes the keyboard for them.
  var isTranscriptEditable: Bool {
    (mode == .formatted || mode == .coverageRefused) && terminal && presentationStatus == nil
      && !revisionCommitPending && !formatterCommitPending
  }

  var isRevisionDraftDirty: Bool {
    (mode == .formatted || mode == .coverageRefused) && terminal
      && revisionDraft != formattedText
  }

  /// Bytes painted on the canvas: the local draft while a formatted take is
  /// under review or awaiting its ledger projection, the Rust projection
  /// otherwise. Delivery never reads this; it reads `activeText`.
  var canvasText: String {
    isRevisionDraftDirty ? revisionDraft : formattedText
  }

  /// Read-only words Rust keeps visible without mutation authority — for
  /// example a Whisper alternative the ledger refused as a whole-span
  /// replacement — in PCM order. They ride the capture-bound compact paint,
  /// never `formattedText`, so no canvas, copy, or delivery path reads them.
  /// Live only: a terminal take shows its sealed document alone.
  var liveEvidence: [CsUnanchoredEvidence] {
    guard !terminal, mode == .listening || mode == .finalizing else { return [] }
    return compactProjection?.evidence ?? []
  }

  /// Post-take review owns the floating panel. The formatted / no-speech
  /// surface must not yield to an Assistive tray tick — that path calls
  /// `hide()` and arms Agent auto-send.
  ///
  /// A refused take keeps its recovery controls on this panel. Agent delivery
  /// has its own untouched-final countdown; a tray tick cannot dismiss it.
  var blocksAssistiveOverlayHide: Bool {
    presentationStatus != nil || mode == .formatted || mode == .coverageRefused
      || mode == .noSpeech
  }

  /// The refusal card reports retained delivery, measured coverage and the
  /// missing seal separately. None of these facts grants terminal acceptance.
  var coverageRefusalDetail: String {
    if retainedComposerDelivery != nil {
      return
        "The handover came back. These words are retained here — recover them before the next take."
    }
    if latestTranscriptProjection?.sealCoverage?.status == .complete {
      return
        "Acoustic coverage was measured as complete, but this transcript has no current terminal seal."
    }
    return "No seal was recorded for this take, so nothing here is certified complete."
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
      // Stop acknowledges lifecycle; transcript projections own the text.
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

  /// Thin relay from the projection-driven rail into existing controller
  /// routes. No branch here changes phase, text, or availability optimistically;
  /// those fields move only when the next Rust projection arrives.
  func relayIntent(_ intent: OverlayIntent) {
    switch intent {
    case .finish:
      stop()
    case .commitRevision:
      commitRevisionDraft()
    case .discardRevision:
      discardRevisionDraft()
    case .copy:
      relayCopyIntent()
    case .insertPaste:
      relayInsertPasteIntent()
    case .retranscribe:
      // Keyboard / AX path without a menu pick: the local paradigm.
      relayRetranscribeIntent(pass: .fullHq)
    case .undoRetranscribe:
      undoRetranscribeIntent()
    case .format:
      relayFormatIntent()
    case .sendToAgent:
      sendToAgent()
    case .recoverSuperseded:
      recoverSupersededTake()
    case .discardSuperseded:
      discardSupersededTake()
    case .close:
      close()
    }
  }

  private func relayCopyIntent() {
    guard let engine else {
      presentActionFailure("Copy needs the recording engine", notice: "copy unavailable")
      return
    }
    let text = activeText
    Task { @MainActor in
      do {
        try await engine.copyTaggedTranscript(text: text)
        self.showFooterNotice("copied")
      } catch {
        self.presentActionFailure("Couldn't copy transcript: \(error)", notice: "copy failed")
      }
    }
  }

  private func relayInsertPasteIntent() {
    if engine == nil {
      presentActionFailure("Insert needs the recording engine", notice: "insert unavailable")
    }
    guard let engine else { return }
    captureQualityIfEdited(action: "paste")
    cancelAutoHide()
    showFooterNotice("inserting…", persists: true)
    let text = activeText
    let shouldDefer = insertCaretInCodescribeProbe()
    Task { @MainActor in
      defer { self.restartAutoHideCountdown() }
      do {
        let result: CsPasteResult
        if shouldDefer {
          result = try await engine.deferText(text: text)
        } else {
          result = try await engine.pasteText(text: text)
        }
        switch result.outcome {
        case .deferredInsertArmed:
          let shortcut = result.deferredInsertShortcut ?? "⌘⌥V"
          self.showFooterNotice(shortcut, persists: true)
        case .copiedToClipboard:
          self.showFooterNotice("copied")
        case .accessibilityPermissionNeeded:
          self.showFooterNotice("no ax")
        case .pasted:
          self.showFooterNotice("inserted")
        case .noop:
          self.showFooterNotice("no insert")
        }
      } catch {
        self.errorMessage = "Couldn't paste transcript: \(error)"
        self.showFooterNotice("no paste")
      }
    }
  }

  /// Explicit overlay Retranscribe. The pass is the operator's pick from the
  /// dock menu (grok `0a5e75fa3`, 2026-08-15): Full HQ local Whisper file pass
  /// or Cloud pass over the retained session audio. Never derived from a
  /// settings mode behind the operator's back.
  func retranscribe(pass: OverlayRetranscribePass) {
    relayRetranscribeIntent(pass: pass)
  }

  private func relayRetranscribeIntent(pass: OverlayRetranscribePass) {
    guard let engine else {
      presentActionFailure(
        "Retranscription needs the recording engine", notice: "retranscribe unavailable")
      return
    }
    guard let projection = latestTranscriptProjection else {
      presentActionFailure(
        "The visible take has no session identity", notice: "retranscribe unavailable")
      return
    }
    guard let path = engine.sessionAudioPath(sessionId: projection.sessionId) else {
      presentActionFailure(
        "The visible take's audio is unavailable", notice: "take audio unavailable")
      return
    }
    let prefixedPath = "\(pass.pathPrefix)\(path)"

    cancelAutoHide()
    let passEngine = pass == .cloud ? "cloud" : "local Whisper HQ"
    let previousChip = engineChip
    engineChip = "retranscribing · \(passEngine)"
    engineChipLatched = true
    showFooterNotice("retranscribing · \(passEngine)", persists: true)
    Task { @MainActor [weak self] in
      guard let self else { return }
      do {
        let result = try await engine.transcribeTake(
          sessionId: projection.sessionId, path: prefixedPath)
        let text = result.text.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !text.isEmpty else {
          self.engineChip = previousChip
          self.presentActionFailure(
            "The \(passEngine) pass returned no text", notice: "retranscribe returned no text")
          return
        }
        if !text.isEmpty {
          if self.latestTranscriptProjection?.sessionId == projection.sessionId {
            // The commit replaces the rendered text irreversibly on the
            // reducer's current tip. Retain the exact text it replaces, so the
            // rail can offer a real Back — a worse retranscription must never
            // be a one-way door.
            let replaced = projection.renderedText
            _ = try await engine.commitRetranscribeRevision(
              sessionId: projection.sessionId,
              sourceRevision: projection.reducerRevision,
              renderedText: text
            )
            if replaced != text,
              !replaced.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
            {
              self.retranscribeRollback = OverlayRetranscribeRollback(
                sessionId: projection.sessionId, renderedText: replaced)
            }
          } else {
            self.engineChip = previousChip
            self.presentActionFailure("A newer take replaced this overlay", notice: "take changed")
            return
          }
          if self.mode == .noSpeech {
            self.mode = .formatted
          }
        }
        self.engineChip = passEngine
        self.showFooterNotice(
          self.retranscribeRollback == nil
            ? "retranscribed" : "retranscribed — Back keeps the old text")
        self.restartAutoHideCountdown()
      } catch {
        self.engineChip = previousChip
        self.presentActionFailure(
          "Couldn't retranscribe recording: \(error)", notice: "retranscribe refused · \(error)")
        self.restartAutoHideCountdown()
      }
    }
  }

  /// The text a Retranscribe replaced, recoverable while its take is current.
  /// A `nil` session is the draft path (no live projection at commit time).
  struct OverlayRetranscribeRollback: Equatable {
    let sessionId: String?
    let renderedText: String
  }

  private(set) var retranscribeRollback: OverlayRetranscribeRollback?

  /// Back is honest only while the replaced text still belongs to the current
  /// document: same session for the committed path, any time for the draft path.
  var canUndoRetranscribe: Bool {
    guard let rollback = retranscribeRollback else { return false }
    guard let sessionId = rollback.sessionId else { return true }
    return latestTranscriptProjection?.sessionId == sessionId
  }

  /// Restore the pre-retranscribe text as a NEW user revision on the same
  /// session — no history rewrite, no forged seal; the reducer keeps both
  /// texts in its revision chain. The rollback slot is consumed only when the
  /// restore actually landed.
  func undoRetranscribeIntent() {
    guard let rollback = retranscribeRollback else { return }
    guard let sessionId = rollback.sessionId else {
      revisionDraft = rollback.renderedText
      retranscribeRollback = nil
      showFooterNotice("retranscribe undone")
      return
    }
    guard let engine else {
      presentActionFailure("Undo needs the recording engine", notice: "undo unavailable")
      return
    }
    guard let projection = latestTranscriptProjection, projection.sessionId == sessionId else {
      retranscribeRollback = nil
      presentActionFailure(
        "The retranscribed take is no longer current", notice: "nothing to undo")
      return
    }
    Task { @MainActor [weak self] in
      guard let self else { return }
      do {
        _ = try await engine.commitUserRevision(
          sessionId: sessionId,
          sourceRevision: projection.reducerRevision,
          renderedText: rollback.renderedText
        )
        self.retranscribeRollback = nil
        self.showFooterNotice("retranscribe undone")
      } catch {
        self.presentActionFailure(
          "Couldn't undo retranscribe: \(error)", notice: "undo failed — kept")
      }
    }
  }

  private func presentActionFailure(_ message: String, notice: String) {
    errorMessage = message
    showFooterNotice(notice)
  }

  func copyToPasteboard(_ pasteboard: NSPasteboard = .general) {
    // P0-D: capture user correction on FINAL for quality loop + lexicon learning.
    captureQualityIfEdited(action: "copy")
    pasteboard.clearContents()
    pasteboard.setString(activeText, forType: .string)
    restartAutoHideCountdown()
  }

  @discardableResult
  func sendToAgent() -> Task<Void, Never>? {
    guard terminal, canSendToAgent, !isRevisionDraftDirty,
      !revisionCommitPending, !formatterCommitPending
    else { return nil }
    // P0-D: capture user correction on FINAL for quality loop + lexicon learning.
    captureQualityIfEdited(action: "send")
    return deliverAgentTranscript()
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
    relayInsertPasteIntent()
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

  /// Same seam as auto-paste: write through the engine's config owner, then
  /// re-read durable truth. The picker never paints an optimistic level.
  func setAutoFormatLevel(_ level: FormattingPolicyOption) {
    guard let engine else { return }
    engine.setAutoFormatLevel(level)
    refreshOverlayPolicyTruth()
    restartAutoHideCountdown()
  }

  func close() {
    discardRevisionDraft()
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
    engineChip = "not yet served"
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
    let editProvenance = userRevisionProvenance
    if isEdited {
      // A string difference is not sufficient evidence of a user edit. Only a
      // reducer-returned user-edit receipt may enter the learning loop.
      guard let editProvenance else { return }
      guard qualityCapturedProvenance != editProvenance else { return }
      qualityCapturedProvenance = editProvenance
    }
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

  // MARK: Edit as revision (Swift side; Rust ledger mints the revision)

  /// The canvas took keyboard focus. Review stays open while the user types.
  func beginTranscriptEdit() {
    guard isTranscriptEditable, !isEditingTranscript else { return }
    isEditingTranscript = true
    // Taking the final canvas for review requires an explicit Agent send,
    // even after focus leaves or the reducer accepts the revision.
    agentAutoSendCancelled = true
    revisionCommitError = nil
    cancelAutoHide()
  }

  /// The canvas gave keyboard focus back. A dirty draft commits after a short
  /// grace so an explicit Discard / Close click can still cancel it.
  func endTranscriptEdit() {
    guard isEditingTranscript else { return }
    isEditingTranscript = false
    if isRevisionDraftDirty {
      scheduleRevisionCommitAfterFocusExit()
    } else if terminal {
      restartAutoHideCountdown()
    }
  }

  /// Canvas bytes changed under the user's caret.
  func updateRevisionDraft(_ text: String) {
    guard isTranscriptEditable else { return }
    revisionDraft = text
    noteRevisionDraftActivity()
  }

  /// User typing is local draft activity: keep the review panel alive without
  /// mutating projected text or any delivery source.
  func noteRevisionDraftActivity() {
    revisionCommitError = nil
    if isRevisionDraftDirty || isEditingTranscript {
      cancelAutoHide()
    } else if terminal {
      restartAutoHideCountdown()
    }
  }

  /// Commit on a genuine focus exit, but wait one click's worth so an explicit
  /// Discard or Close can cancel the scheduled commit before it crosses FFI.
  /// (T15 yielded once; a Discard click resigns the canvas on mouse-down and
  /// fires on mouse-up, and a bare yield ran the commit in between.)
  static let focusExitCommitGraceNanoseconds: UInt64 = 300_000_000

  func scheduleRevisionCommitAfterFocusExit() {
    revisionFocusCommitTask?.cancel()
    revisionFocusCommitTask = Task { @MainActor [weak self] in
      try? await Task.sleep(nanoseconds: OverlayState.focusExitCommitGraceNanoseconds)
      guard !Task.isCancelled else { return }
      self?.commitRevisionDraft()
    }
  }

  func discardRevisionDraft() {
    revisionFocusCommitTask?.cancel()
    revisionFocusCommitTask = nil
    guard !revisionCommitPending, !formatterCommitPending else { return }
    revisionDraft = formattedText
    revisionCommitError = nil
    if terminal, !isEditingTranscript { restartAutoHideCountdown() }
  }

  /// Send an immutable compare-and-swap request to Rust. This method never
  /// changes `formattedText`; the draft remains pending until the matching
  /// reducer projection returns through `applyTranscriptProjection`.
  func commitRevisionDraft() {
    revisionFocusCommitTask?.cancel()
    revisionFocusCommitTask = nil
    guard mode == .formatted || mode == .coverageRefused, terminal,
      isRevisionDraftDirty, !revisionCommitPending,
      !formatterCommitPending
    else {
      return
    }
    let proposed = revisionDraft
    guard !proposed.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else {
      revisionCommitError = "A transcript revision cannot be empty"
      return
    }
    guard let projection = latestTranscriptProjection, let engine else {
      revisionCommitError = "Transcript revision authority is unavailable"
      return
    }
    revisionCommitPending = true
    revisionCommitError = nil
    pendingRevisionSessionId = projection.sessionId
    pendingRevisionSource = projection.reducerRevision
    cancelAutoHide()
    Task { @MainActor [weak self] in
      guard let self else { return }
      do {
        let receipt = try await engine.commitUserRevision(
          sessionId: projection.sessionId,
          sourceRevision: projection.reducerRevision,
          renderedText: proposed
        )
        guard receipt.sessionId == projection.sessionId,
          receipt.sourceRevision == projection.reducerRevision,
          receipt.revision > receipt.sourceRevision,
          receipt.renderedText == proposed,
          receipt.provenanceReceipt.hasPrefix("user-edit-")
        else {
          revisionCommitPending = false
          pendingRevisionSessionId = nil
          pendingRevisionSource = nil
          revisionCommitError = "Transcript revision receipt was inconsistent"
          return
        }
        // The callback can arrive before this acknowledgement. Either way,
        // projection — never this receipt — owns the visible state transition.
      } catch {
        revisionCommitPending = false
        pendingRevisionSessionId = nil
        pendingRevisionSource = nil
        revisionCommitError = "Couldn't commit transcript revision: \(error)"
      }
    }
  }

  func prepareForExternalStart() {
    handleRecordingPreparing()
  }

  /// True once THIS capture proved it is alive: the recorder confirmed, audio
  /// or speech was measured, the final pass began, or Rust admitted text for it.
  /// Warmup is the only state the watchdog is allowed to dismiss, so this is
  /// exactly the predicate that must never regress under a duplicate beat.
  private var captureProvedLife: Bool {
    audioReady || vadActive || hasMeasuredAudioLevel || transcribing
      || latestTranscriptProjection != nil
  }

  func handleRecordingPreparing() {
    // A `preparing` that repeats for a capture which already PROVED it is alive
    // must not un-prove it. The unconditional `warmingUp = true` /
    // `audioReady = false` below, followed by `armWarmupWatchdog()`, re-armed
    // the orphan watchdog against a live take: four seconds later it saw
    // `warmingUp && !finalized` for an UNCHANGED generation, so the generation
    // fence passed, and it aborted the capture the user was still speaking into.
    // The generation guard cannot help here — same capture, same generation.
    //
    // Only the warmup fields and the watchdog are off limits. The route is
    // still re-derived, because the documented mid-hold Fn -> Fn+Shift upgrade
    // is delivered on exactly such a repeated beat (docs/TRANSCRIPT_BUS.md,
    // tray identity boundary), and suppressing it here would trade one silent
    // failure for another.
    if recording, captureProvedLife {
      agentSessionArmed = indicatorMode == .assistive
      autoPasteControlAvailable = !agentSessionArmed
      refreshOverlayPolicyTruth()
      onRecordingPreparing?()
      return
    }
    agentSessionArmed = indicatorMode == .assistive
    autoPasteControlAvailable = !agentSessionArmed
    finalized = false
    isFinalPass = false
    warmingUp = true
    audioReady = false
    hasMeasuredAudioLevel = false
    levelMeter.reset()
    if !recording {
      // Duplicate preparing/started for an OPEN capture must never reach this
      // branch: it would re-fence a take whose text was already admitted.
      admitNewCapture()
      resetTranscript()
      errorMessage = nil
      beginCaptureClock()
      applyPreferredExpansion()
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
      admitNewCapture()
      resetTranscript()
      errorMessage = nil
      beginCaptureClock()
      applyPreferredExpansion()
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
    let generation = captureGeneration
    warmupWatchdogTask = Task { @MainActor [weak self] in
      try? await Task.sleep(nanoseconds: OverlayState.warmupWatchdogNanos)
      guard !Task.isCancelled else { return }
      self?.fireWarmupWatchdog(generation: generation)
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
  /// `generation` is the capture this watchdog was armed for. A successor take
  /// that re-armed (or a capture that already ended and was replaced) leaves a
  /// resumed older task here; dismissing on it would close the wrong overlay.
  private func fireWarmupWatchdog(generation: UInt64) {
    guard generation == captureGeneration else { return }
    warmupWatchdogTask = nil
    guard warmingUp, !finalized else { return }
    abortRecordingSession(resetTranscript: true)
    onClose?()
  }

  private var isTerminalMode: Bool {
    terminal
  }

  private var mayAutoSendRefusedAgentTake: Bool {
    agentSessionArmed && agentFinalTranscriptAppeared && !agentAutoSendCancelled
      && canSendToAgent
      && !formattedText.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
  }

  private func restartAutoHideCountdown() {
    if keepVisibleBetweenTakes && !agentSessionArmed {
      cancelAutoHide()
      return
    }
    // A presentation revision may arrive before the controller's lifecycle
    // terminal. The Agent timer starts only after that terminal is observed.
    if agentSessionArmed && !agentFinalTranscriptAppeared {
      cancelAutoHide()
      return
    }
    if agentSessionArmed && !autoSendEnabled() {
      cancelAutoHide()
      return
    }
    // Refused coverage keeps recovery visible. The one exception is an armed
    // Agent take with untouched final words: its deadline sends through the
    // existing permission gate, without claiming an acoustic seal.
    guard mode != .coverageRefused || mayAutoSendRefusedAgentTake else {
      cancelAutoHide()
      return
    }
    // A take under review (caret in the canvas, or an uncommitted draft) is
    // never auto-hidden out from under the user.
    guard isTerminalMode, !isPointerHovering, !isEditingTranscript, !isRevisionDraftDirty
    else {
      cancelAutoHide()
      return
    }
    cancelAutoHide()
    autoHideDeadline = nowProvider() + OverlayState.autoHideDelaySeconds
    scheduleAutoHideWake(after: OverlayState.autoHideDelaySeconds, generation: captureGeneration)
  }

  private func scheduleAutoHideWake(after delay: TimeInterval, generation: UInt64) {
    let nanoseconds = UInt64(max(0, delay) * 1_000_000_000)
    autoHideTask = Task { @MainActor [weak self] in
      try? await Task.sleep(nanoseconds: nanoseconds)
      guard !Task.isCancelled else { return }
      self?.evaluateAutoHideDeadline(rescheduleIfEarly: true, generation: generation)
    }
  }

  /// The terminal outcome of the take named by `generation` may close its own
  /// overlay. A wake that survives into a successor capture is refused here
  /// rather than at the flags: `terminal`/`autoHideDeadline` describe whatever
  /// take is current, so they cannot answer "was this deadline mine?".
  private func evaluateAutoHideDeadline(rescheduleIfEarly: Bool, generation: UInt64) {
    guard generation == captureGeneration else { return }
    autoHideTask = nil
    if keepVisibleBetweenTakes && !agentSessionArmed {
      cancelAutoHide()
      return
    }
    if agentSessionArmed && !agentFinalTranscriptAppeared {
      cancelAutoHide()
      return
    }
    if agentSessionArmed && !autoSendEnabled() {
      cancelAutoHide()
      return
    }
    // Recheck the current verdict: an older wake cannot dismiss refused
    // recovery. An armed, untouched Agent take may reach its send gate.
    guard mode != .coverageRefused || mayAutoSendRefusedAgentTake else {
      cancelAutoHide()
      return
    }
    guard isTerminalMode, !isPointerHovering, let deadline = autoHideDeadline else { return }
    let remaining = deadline - nowProvider()
    if remaining > 0 {
      if rescheduleIfEarly {
        scheduleAutoHideWake(after: remaining, generation: generation)
      }
      return
    }
    autoHideDeadline = nil
    if agentSessionArmed, agentFinalTranscriptAppeared {
      if !agentAutoSendCancelled {
        sendToAgent()
      }
      return
    }
    onClose?()
  }

  /// Deterministic XCTest seam: tests inject a monotonic clock, advance it,
  /// and evaluate the same deadline logic without wall-clock sleeps.
  /// Supplying a previously armed deadline models a pending wake independently
  /// of scheduling cancellation, so the deadline's refusal guard is falsifiable.
  func fireAutoHideNowForTests(armedDeadline: TimeInterval? = nil) {
    if let armedDeadline { autoHideDeadline = armedDeadline }
    fireAutoHideForTests(generation: captureGeneration)
  }

  /// Deterministic negative seam: replay a wake that was armed by an EARLIER
  /// capture (pass its generation) and prove it cannot close the successor.
  func fireAutoHideForTests(generation: UInt64) {
    guard generation == captureGeneration else { return }
    autoHideTask?.cancel()
    autoHideTask = nil
    evaluateAutoHideDeadline(rescheduleIfEarly: false, generation: generation)
  }

  /// Deterministic negative seam for the orphaned-"starting" watchdog. It does
  /// NOT cancel the live task: cancelling the successor's own watchdog would
  /// hide the very refusal the negative test exists to observe.
  func fireWarmupWatchdogForTests(generation: UInt64) {
    fireWarmupWatchdog(generation: generation)
  }

  private func cancelAutoHide() {
    autoHideTask?.cancel()
    autoHideTask = nil
    autoHideDeadline = nil
  }

  @discardableResult
  private func deliverAgentTranscript() -> Task<Void, Never>? {
    let text = activeText.trimmingCharacters(in: .whitespacesAndNewlines)
    // No `agentSessionArmed` here: the explicit Send button is live for
    // every terminal overlay (dictation and formatting included), and the
    // controller falls back to the session trigger context when no
    // assistive context was armed (review P0-03). Auto-send remains gated
    // on the armed latch by its caller.
    guard !agentDeliveryStarted, !text.isEmpty, let engine else { return nil }
    agentDeliveryStarted = true
    let generation = captureGeneration
    cancelAutoHide()
    return Task { @MainActor [weak self] in
      guard let self else { return }
      do {
        let delivered = try await engine.sendAssistiveTranscript(text: text)
        // Rust owns the completed send. These callbacks and flags only own
        // this capture's presentation, never a successor's panel or latch.
        guard generation == captureGeneration else { return }
        if delivered {
          onSendToAgent?(text)
          onClose?()
        } else {
          agentDeliveryStarted = false
          showToast("Agent delivery is no longer available")
        }
      } catch {
        guard generation == captureGeneration else { return }
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
      // Assistive hides the overlay. "Transcript kept" on a canvas the user
      // cannot see is a drop. Hand the live projection to the composer join
      // before abort wipes capture identity — same throne as a clean stop.
      admitComposerDelivery(projection)
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

  private func presentTerminalError(message: String, toast: String) {
    let speechNotice = OverlayState.speechAuthNotice(from: message)
    let captureHadStarted = recording
    let message = speechNotice ?? message
    let toast = speechNotice ?? toast
    abortRecordingSession()
    pendingNoSpeechMessage = nil
    noSpeechNotice = OverlayState.defaultNoSpeechNotice
    isFinalPass = false
    errorMessage = message
    errorLifecycleDetail =
      captureHadStarted
      ? "No transcript was delivered."
      : "Recording did not start."
    finalized = true
    showToast(toast)
  }

  // MARK: Listener-driven mutations (called on the main actor by DictationListener)

  /// Classify a status against the current capture using only what the producer
  /// stated. See `OverlayStatusAddressing` for the protocol evidence.
  func statusAddressing(for event: CsPresentationStatusEvent) -> OverlayStatusAddressing {
    // An empty string is not an identity. Normalise it to "no identity" here
    // rather than letting it match nothing and slip through as an unobserved
    // session — that is the difference between an explicit disposition and an
    // accident of set membership.
    let sessionId = event.sessionId.flatMap { $0.isEmpty ? nil : $0 }
    if let sessionId, retiredProjectionSessions.contains(sessionId) {
      return .retiredSession
    }
    // Present and not retired: either the live take or a session this receiver
    // has never observed. The lifecycle callbacks carry no session id, so an
    // unobserved identity cannot be classified as a predecessor — and the sole
    // producer of `admission_refused` emits it from the start path of the take
    // the user just asked for. It is addressed here, and the paired control for
    // that is a current addressed failure still presenting and releasing.
    guard sessionId == nil else { return .currentCapture }
    let carriesNoCaptureIdentity =
      event.kind == "calibration_succeeded" || event.kind == "calibration_failed"
    if carriesNoCaptureIdentity, recording || warmingUp || transcribing {
      return .foreignToLiveCapture
    }
    return .currentCapture
  }

  /// Paint one Rust-owned status card. This sibling projection may close a
  /// failed/preparing capture lifecycle, but it never creates transcript text,
  /// receipts, or product actions in Swift — and it may only close the capture
  /// its own identity addresses.
  func applyPresentationStatus(_ event: CsPresentationStatusEvent) {
    let addressing = statusAddressing(for: event)
    switch addressing {
    case .retiredSession:
      // A known retired session's delayed terminal. `applyTranscriptProjection`
      // has fenced this since the donor cut, but this sibling path released the
      // capture BEFORE it ever looked at `event.sessionId`, so a predecessor's
      // late refusal aborted its successor and reset the successor's transcript.
      // The status carries the identity needed to refuse it; nothing else about
      // the current capture, text, mode or countdown may move.
      return
    case .foreignToLiveCapture:
      // A status with no capture identity, arriving mid-capture. It is still
      // product truth, so the card and its notice are painted, but it may not
      // end a take it cannot name.
      presentationStatus = OverlayPresentationStatus(
        schema: event.schema,
        emittedAt: event.emittedAt,
        sessionId: event.sessionId,
        kind: event.kind,
        code: event.code,
        statusLabel: event.statusLabel,
        headline: event.headline,
        message: event.message,
        isError: event.isError,
        terminal: event.terminal,
        calibrationVersion: event.calibrationVersion
      )
      onPresentationStatus?()
      showToast(event.headline)
      return
    case .currentCapture:
      break
    }
    abortRecordingSession(resetTranscript: true)
    let status = OverlayPresentationStatus(
      schema: event.schema,
      emittedAt: event.emittedAt,
      sessionId: event.sessionId,
      kind: event.kind,
      code: event.code,
      statusLabel: event.statusLabel,
      headline: event.headline,
      message: event.message,
      isError: event.isError,
      terminal: event.terminal,
      calibrationVersion: event.calibrationVersion
    )
    presentationStatus = status
    // Status is a sibling message, not a replacement transcript document.
    // Only a subsequent transcript projection may change the displayed bytes.
    canPaste = false
    canInsert = false
    canCopy = false
    canRetranscribe = false
    canFormat = false
    canSendToAgent = false
    mode = event.isError ? .error : .formatted
    terminal = event.terminal
    finalized = event.terminal
    errorMessage = event.isError ? event.message : nil
    onPresentationStatus?()
    showToast(event.headline)
    if event.terminal { restartAutoHideCountdown() }
  }

  /// Paint the engine document directly. An unfamiliar chrome phase must not
  /// prevent text delivery; retain the current chrome until a known phase arrives.
  /// Passive paint only. Neither capture admission nor document state is
  /// inferred from compact text. Sequence 1 comes from the opened recorder.
  func applyCompactProjection(_ projection: CsCompactProjection) {
    guard recording || transcribing, !terminal,
      !projection.sessionId.isEmpty, projection.captureEpoch > 0,
      !retiredProjectionSessions.contains(projection.sessionId),
      !endedProjectionSessions.contains(projection.sessionId)
    else { return }
    if let current = compactProjection {
      guard current.sessionId == projection.sessionId,
        current.captureEpoch == projection.captureEpoch,
        projection.sequence > current.sequence
      else { return }
    } else {
      guard projection.sequence == 1 else { return }
      if let current = latestTranscriptProjection {
        guard current.sessionId == projection.sessionId else { return }
      }
    }
    compactProjection = projection
  }

  func applyTranscriptProjection(_ projection: CsTranscriptProjectionEvent) {
    // Retired projections may still carry undelivered words, but cannot paint
    // or finalize the newer capture. Retry the identity-addressed receiver only.
    let foreignComposerTerminal =
      projection.terminal && projection.lifecycleTerminal
      && acceptsCaptureTerminal?(projection.sessionId) == false
    if retiredProjectionSessions.contains(projection.sessionId)
      || foreignComposerTerminal
    {
      defer { onTranscriptPresentationChanged?() }
      if projection.terminal && projection.lifecycleTerminal {
        if projection.delivery == .composerPending {
          admitComposerDelivery(projection, affectsCurrentCapture: false)
        }
        if endedProjectionSessions.insert(projection.sessionId).inserted {
          onCaptureEnded?(projection.sessionId)
        }
      }
      return
    }
    if let accepted = projectionOrder[projection.sessionId] {
      guard projection.sequence > accepted.sequence,
        projection.reducerRevision >= accepted.revision,
        projection.captureEpoch >= accepted.epoch
      else {
        // Re-observing the same authenticated terminal may retry a handover
        // the receiver refused. Only its existing admission receipt consumes
        // delivery; this must not repaint or release capture again.
        if projection.sequence == accepted.sequence,
          projection.reducerRevision == accepted.revision,
          projection.captureEpoch == accepted.epoch,
          projection.terminal, projection.lifecycleTerminal,
          projection.delivery == .composerPending,
          endedProjectionSessions.contains(projection.sessionId)
        {
          admitComposerDelivery(projection, affectsCurrentCapture: false)
        }
        return
      }
    }
    projectionOrder[projection.sessionId] = (
      projection.sequence, projection.reducerRevision, projection.captureEpoch
    )
    // Replayed lifecycle cannot repaint or release capture twice. A later
    // addressed offer can still retry an unacknowledged composer handover.
    let lifecycleTerminal = projection.terminal && projection.lifecycleTerminal
    if lifecycleTerminal && endedProjectionSessions.contains(projection.sessionId) {
      if projection.delivery == .composerPending {
        admitComposerDelivery(projection, affectsCurrentCapture: false)
      }
      return
    }
    if lifecycleTerminal { endedProjectionSessions.insert(projection.sessionId) }
    defer { onTranscriptPresentationChanged?() }
    let priorProjection = latestTranscriptProjection
    let isNewSession = priorProjection?.sessionId != projection.sessionId
    let draftWasDirty = isRevisionDraftDirty
    let revisionReceipt = projection.acousticReceipts
      .compactMap(\.manualEditReceipt)
      .first(where: { $0.hasPrefix("user-edit-") })
    let formatterReceipt = projection.acousticReceipts
      .compactMap(\.manualEditReceipt)
      .first(where: { $0.hasPrefix("formatter-") })
    let completesPendingRevision =
      revisionCommitPending
      && projection.reducerAction == "apply_manual_edit"
      && projection.terminal
      && projection.sessionId == pendingRevisionSessionId
      && projection.reducerRevision > (pendingRevisionSource ?? UInt64.max)
      && revisionReceipt != nil
    let completesPendingFormatter =
      formatterCommitPending
      && projection.reducerAction == "apply_manual_edit"
      && projection.terminal
      && projection.sessionId == pendingRevisionSessionId
      && projection.reducerRevision > (pendingRevisionSource ?? UInt64.max)
      && formatterReceipt != nil
    // A successful acoustic terminal has its own callback. Agent auto-send
    // below uses lifecycle completion and nonempty text, not this seal signal.
    let signalsFirstSuccessfulTerminal =
      !terminal && projection.terminal && projection.phase == OverlayMode.formatted.rawValue
    // Initialize before the first event can obtain a receiver receipt or retain
    // refused bytes. The first observed projection may already end this session.
    if isNewSession {
      documentHistory = []
      historyReadSessionId = nil
      if let priorProjection { retiredProjectionSessions.insert(priorProjection.sessionId) }
      deliveredText = ""
      deliveredTextSessionId = nil
      qualityCapturedProvenance = nil
      userRevisionProvenance = nil
      revisionCommitPending = false
      formatterCommitPending = false
      pendingRevisionSessionId = nil
      pendingRevisionSource = nil
      revisionCommitError = nil
      formatterError = nil
      revisionFocusCommitTask?.cancel()
      revisionFocusCommitTask = nil
    }

    // The reducer emits two different terminals. `apply_manual_edit` is a
    // presentation revision — a Light+ mint or a formatter receipt — and it can
    // legitimately arrive BEFORE the controller publishes `session_ended`.
    // Treating it as the lifecycle terminal released the capture and consumed
    // the delivery slot early, so the real delivery event then failed its own
    // dedup check and the take was never handed to the composer. Only the
    // lifecycle line ends the capture.
    // Typed, not parsed: the producer states whether this is the line that ends
    // the session or a revision of its final document.
    let isLifecycleTerminal = projection.terminal && projection.lifecycleTerminal
    if isLifecycleTerminal {
      // Deliver BEFORE releasing capture. `abortRecordingSession` fires the
      // app-level stopped callback, which clears the composer's thread latch —
      // the receiver needs that latch to know which conversation owns the take.
      // Typed destination from the producer; the human-facing `label` is never
      // consulted, because a delivery protocol that reads a display string is
      // one copy-edit away from silently routing a take nowhere.
      if projection.delivery == .composerPending {
        admitComposerDelivery(projection)
      }
      onCaptureEnded?(projection.sessionId)
      // Release capture before flipping `finalized`; abort uses the previous
      // value to decide whether the app-level stopped callback is still owed.
      abortRecordingSession()
    }
    latestTranscriptProjection = projection
    // Mirror this accepted projection; a successor or a later document verdict
    // cannot inherit a notice belonging to its predecessor.
    coverageRefusalNotice =
      projection.terminal && projection.phase == OverlayMode.coverageRefused.rawValue
      ? coverageRefusalCopy.notice : nil
    if !projection.terminal {
      markTranscriptActivity()
    }
    transcriptMode = projection.mode
    mode = OverlayMode(rawValue: projection.phase) ?? mode
    revision = projection.reducerRevision
    canPaste = projection.canPaste
    canInsert = projection.canInsert
    canCopy = projection.canCopy
    canRetranscribe = projection.canRetranscribe
    canFormat = projection.canFormat
    canSendToAgent = projection.canSendToAgent
    terminal = projection.terminal
    // A document revision cannot consume the capture's pending stopped callback.
    // Preserve an already-finalized lifecycle when revising its document later.
    if !projection.terminal || isLifecycleTerminal {
      finalized = projection.terminal
    }

    userRevisionProvenance = revisionReceipt
    if completesPendingRevision {
      revisionCommitPending = false
      pendingRevisionSessionId = nil
      pendingRevisionSource = nil
      revisionCommitError = nil
      revisionDraft = projection.renderedText
    } else if completesPendingFormatter {
      formatterCommitPending = false
      pendingRevisionSessionId = nil
      pendingRevisionSource = nil
      formatterError = nil
      revisionDraft = projection.renderedText
      showFooterNotice("formatted")
    } else if !draftWasDirty || isNewSession {
      revisionDraft = projection.renderedText
    }

    if projection.terminal {
      if isLifecycleTerminal && historyReadSessionId != projection.sessionId {
        historyReadSessionId = projection.sessionId
        refreshDocumentHistory(
          sessionId: projection.sessionId, sourceRevision: projection.reducerRevision)
      }
      if deliveredTextSessionId != projection.sessionId {
        deliveredText = projection.renderedText
        deliveredTextSessionId = projection.sessionId
      }
      if isLifecycleTerminal {
        agentFinalTranscriptAppeared =
          !projection.renderedText.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
        if projection.delivery == .copiedToClipboard {
          showFooterNotice("copied")
        }
      }
      if signalsFirstSuccessfulTerminal {
        onSuccessfulDictation?()
      }
      if projection.phase == OverlayMode.noSpeech.rawValue {
        noSpeechNotice = pendingNoSpeechMessage ?? OverlayState.defaultNoSpeechNotice
      }
      restartAutoHideCountdown()
      if revisionReceipt != nil {
        captureQualityIfEdited(action: "revision")
      }
    }
  }

  /// Offer one terminal document to the composer and record only what the
  /// receiver actually accepted.
  ///
  /// Every early return is a case where no delivery may be claimed:
  ///
  /// - an empty document is nothing to deliver, so it claims neither success
  ///   nor failure;
  /// - a duplicate lifecycle terminal for a session already handed over must
  ///   not insert a second copy;
  /// - a missing callback means no receiver is wired at all — the text stays
  ///   retained and visible rather than being reported as delivered.
  private func admitComposerDelivery(
    _ projection: CsTranscriptProjectionEvent, affectsCurrentCapture: Bool = true
  ) {
    guard !admittedComposerSessions.contains(projection.sessionId) else { return }
    // This take was routed to the composer, so the overlay's own deadline must
    // never submit it — not on the happy path, and least of all on the path
    // where the handover failed. Auto-sending words the composer never received
    // would be the loudest possible version of the bug this cut closes.
    if affectsCurrentCapture { agentAutoSendCancelled = true }
    // The terminal projection keeps reducer truth clean and carries the
    // controller-rendered delivery envelope separately for this one sink.
    let text = projection.deliveryText ?? projection.renderedText
    guard !text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else { return }
    guard let onComposerTranscript else {
      retainComposerDelivery(
        text, sessionID: projection.sessionId, notice: "no composer receiver",
        showsNotice: affectsCurrentCapture)
      return
    }
    switch onComposerTranscript(text, projection.sessionId) {
    case .admitted, .parked:
      // The receiver holds the bytes. Only now is this session's delivery slot
      // consumed: the text is in a draft the user can edit and send explicitly.
      admittedComposerSessions.insert(projection.sessionId)
      retainedComposerDocuments.removeAll { $0.id == projection.sessionId }
    case .retained(let refused):
      if receiverOwnsRecovery?() == true {
        retainedComposerDocuments.removeAll { $0.id == projection.sessionId }
        if affectsCurrentCapture { showFooterNotice("kept in composer recovery") }
        return
      }
      retainComposerDelivery(
        refused, sessionID: projection.sessionId, notice: "kept for recovery",
        showsNotice: affectsCurrentCapture)
    case .empty:
      break
    }
  }

  /// Keep a refused delivery accessible. This is not an error state for the
  /// transcript — the words are exactly the bytes the reducer committed, and
  /// what failed is the handover. It says nothing about a seal: a document
  /// reaches this path under `coverage_refused` too, where the ledger
  /// explicitly refused coverage, so claiming these bytes are sealed would
  /// mint the one receipt this whole route exists to avoid faking.
  ///
  /// The notice persists. A handover that came back is standing state, not an
  /// event: a chip that fades after 2.6 s leaves a user with words on screen,
  /// no destination, and nothing saying so. `resetTranscript` clears it when
  /// the next capture starts.
  private func retainComposerDelivery(
    _ text: String, sessionID: String, notice: String, showsNotice: Bool = true
  ) {
    if !retainedComposerDocuments.contains(where: { $0.id == sessionID }) {
      retainedComposerDocuments.append((id: sessionID, text: text))
    }
    if showsNotice {
      errorMessage = nil
      showFooterNotice(notice, persists: true)
    }
  }

  func formatTranscript(at level: FormattingPolicyOption) {
    relayFormatIntent(level: level)
  }

  private func relayFormatIntent(level: FormattingPolicyOption? = nil) {
    guard mode == .formatted || mode == .coverageRefused, terminal, canFormat,
      !isRevisionDraftDirty,
      !revisionCommitPending, !formatterCommitPending
    else { return }
    guard let projection = latestTranscriptProjection, let engine else {
      formatterError = "Transcript formatter authority is unavailable"
      showFooterNotice("format unavailable")
      return
    }
    formatterCommitPending = true
    formatterError = nil
    pendingRevisionSessionId = projection.sessionId
    pendingRevisionSource = projection.reducerRevision
    cancelAutoHide()
    showFooterNotice("formatting…", persists: true)
    Task { @MainActor [weak self] in
      guard let self else { return }
      do {
        let receipt = try await engine.commitFormatterRevision(
          sessionId: projection.sessionId,
          sourceRevision: projection.reducerRevision,
          level: level
        )
        guard receipt.sessionId == projection.sessionId,
          receipt.sourceRevision == projection.reducerRevision,
          receipt.revision > receipt.sourceRevision,
          !receipt.renderedText.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty,
          receipt.provenanceReceipt.hasPrefix("formatter-")
        else {
          formatterCommitPending = false
          pendingRevisionSessionId = nil
          pendingRevisionSource = nil
          formatterError = "Formatter revision receipt was inconsistent"
          showFooterNotice("format failed")
          restartAutoHideCountdown()
          return
        }
        // Projection can arrive before acknowledgement. It is still the only
        // path that may repaint `formattedText` or the local editor draft.
      } catch {
        formatterCommitPending = false
        pendingRevisionSessionId = nil
        pendingRevisionSource = nil
        formatterError = "Couldn't format transcript: \(error)"
        showFooterNotice("format failed")
        restartAutoHideCountdown()
      }
    }
  }

  func restoreDocumentRevision(_ selectedRevision: UInt64) {
    guard terminal, !isRevisionDraftDirty, !revisionCommitPending,
      !formatterCommitPending, let projection = latestTranscriptProjection,
      documentHistory.contains(where: { $0.revision == selectedRevision }),
      selectedRevision != projection.reducerRevision, let engine
    else { return }
    revisionCommitPending = true
    revisionCommitError = nil
    pendingRevisionSessionId = projection.sessionId
    pendingRevisionSource = projection.reducerRevision
    cancelAutoHide()
    Task { @MainActor [weak self] in
      guard let self else { return }
      do {
        let receipt = try await engine.restoreDocumentRevision(
          sessionId: projection.sessionId,
          sourceRevision: projection.reducerRevision,
          restoreRevision: selectedRevision)
        guard receipt.sessionId == projection.sessionId,
          receipt.sourceRevision == projection.reducerRevision,
          receipt.revision > receipt.sourceRevision,
          receipt.provenanceReceipt.hasPrefix("user-edit-")
        else {
          revisionCommitPending = false
          pendingRevisionSessionId = nil
          pendingRevisionSource = nil
          revisionCommitError = "Transcript restore receipt was inconsistent"
          return
        }
        // Only the matching reducer callback repaints the canvas.
      } catch {
        revisionCommitPending = false
        pendingRevisionSessionId = nil
        pendingRevisionSource = nil
        revisionCommitError = "Couldn't restore transcript version: \(error)"
      }
    }
  }

  func loadDocumentHistory() {
    guard terminal, let projection = latestTranscriptProjection else { return }
    refreshDocumentHistory(
      sessionId: projection.sessionId, sourceRevision: projection.reducerRevision)
  }

  private func refreshDocumentHistory(sessionId: String, sourceRevision: UInt64) {
    guard let engine else { return }
    Task { @MainActor [weak self] in
      do {
        let entries = try await engine.documentHistory(sessionId: sessionId)
        guard let self, self.latestTranscriptProjection?.sessionId == sessionId,
          self.latestTranscriptProjection?.reducerRevision == sourceRevision
        else { return }
        self.documentHistory = entries
      } catch {
        guard let self, self.latestTranscriptProjection?.sessionId == sessionId else { return }
        self.revisionCommitError = "Couldn't read transcript history: \(error)"
      }
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

  /// Move the outgoing take into the single retention owner.
  ///
  /// The counterexample this exists to refuse: edited take A -> capture B
  /// retains A -> B ends clean or empty -> capture C. The old one-slot store
  /// replaced A's draft with `nil` at C's boundary, so an edit no user decision
  /// had ever consumed was gone. Nothing here evicts unsaved work; a capture
  /// boundary is not a user decision.
  ///
  /// `prior` is the outgoing take's own projection. A capture with no observed
  /// projection never reaches here, so nothing is retained under a minted
  /// identity — and, just as importantly, nothing is dropped either: an earlier
  /// unacknowledged take is not this boundary's to discard.
  private func retainSupersededTake(
    _ prior: CsTranscriptProjectionEvent, draftWasDirty: Bool
  ) {
    let unsavedDraft = draftWasDirty ? revisionDraft : nil
    guard unsavedDraft != nil || !prior.renderedText.isEmpty else { return }
    let take = OverlaySupersededTake(
      sessionId: prior.sessionId,
      renderedText: prior.renderedText,
      reducerRevision: prior.reducerRevision,
      unsavedDraft: unsavedDraft
    )
    if !take.hasUnsavedEdits {
      // Clean documents are genuinely capped at one. Unsaved edits are never
      // evicted to make room for this, and are never capped here at all.
      supersededTakes.removeAll { !$0.hasUnsavedEdits }
    }
    supersededTakes.append(take)
    if take.hasUnsavedEdits {
      // Tell the user at the moment the edit is set aside, and how many are now
      // waiting. This is disclosure, not a capacity mechanism.
      showFooterNotice(supersededRecoveryNotice)
    }
  }

  /// The retained take an explicit recovery or discard acts on. Oldest first,
  /// so a queue of unacknowledged edits is walked in the order it was created.
  var pendingSupersededTake: OverlaySupersededTake? { supersededTakes.first }
  var hasRecoverableSupersededWork: Bool { !supersededTakes.isEmpty }
  var unacknowledgedSupersededEditCount: Int {
    supersededTakes.filter(\.hasUnsavedEdits).count
  }
  /// Human-readable retention state. It reports how many decisions are waiting;
  /// it does not cap them. See `supersededTakes` for the disclosed boundary.
  var supersededRecoveryNotice: String {
    let edits = unacknowledgedSupersededEditCount
    switch edits {
    case 0: return "previous take retained"
    case 1: return "1 unsaved edit to recover"
    default: return "\(edits) unsaved edits to recover"
    }
  }

  /// The single write the recovery action performs, and its honest outcome.
  ///
  /// This is a WRITER seam, not merely an injectable `NSPasteboard`, because no
  /// real pasteboard can be made to fail on demand — so the failure branch of
  /// the production action would otherwise be untestable, which is exactly how
  /// an ignored `setString` result survives review. Tests inject both outcomes
  /// and neither one touches the user's real clipboard.
  @ObservationIgnored var recoveryClipboardWriter: (String) -> Bool = { text in
    let pasteboard = NSPasteboard.general
    pasteboard.clearContents()
    return pasteboard.setString(text, forType: .string)
  }

  /// Set when a recovery write failed. The retained item is still present, so
  /// this is a "try again", never a loss report. Cleared by the next successful
  /// recovery or by an explicit discard.
  private(set) var recoveryFailure: String?

  /// Hand one retained take back to the user.
  ///
  /// Recovery is deliberately NOT a reducer path. It commits nothing, mints no
  /// session, forges no seal, submits nothing, and takes no focus: the bytes
  /// leave through `recoveryClipboardWriter`. The current canvas is untouched,
  /// so a pending capture keeps its empty screen while the previous words reach
  /// the clipboard — the retained take is never painted as current speech.
  ///
  /// The item is consumed ONLY on a confirmed write. Dropping it on an
  /// unchecked return value would destroy the only copy of an unsaved edit at
  /// the exact moment the copy did not happen — the silent loss this owner
  /// exists to prevent, reintroduced one line lower.
  func recoverSupersededTake() {
    guard let take = supersededTakes.first else { return }
    guard recoveryClipboardWriter(take.recoverableText) else {
      recoveryFailure =
        take.hasUnsavedEdits
        ? "Couldn't copy the unsaved edit — it is still retained, try again"
        : "Couldn't copy the previous take — it is still retained, try again"
      // Persisting: a failure the user must be able to read after the 2.6 s
      // fade, and the retained work already keeps this rail revealed. The
      // `.formatted` body shows the full sentence; the footer covers every
      // other phase, including a live capture.
      showFooterNotice("recover failed — kept", persists: true)
      if terminal, !isEditingTranscript { restartAutoHideCountdown() }
      return
    }
    recoveryFailure = nil
    supersededTakes.removeFirst()
    showFooterNotice(take.hasUnsavedEdits ? "unsaved edit copied" : "previous take copied")
    // Mirrors `discardRevisionDraft`: interacting with the panel must not be
    // the reason a terminal take closes under the user's hand.
    if terminal, !isEditingTranscript { restartAutoHideCountdown() }
  }

  /// The only path that drops retained work. No capture boundary, late event or
  /// watchdog may reach it; the user acknowledges the loss explicitly.
  func discardSupersededTake() {
    guard !supersededTakes.isEmpty else { return }
    let take = supersededTakes.removeFirst()
    recoveryFailure = nil
    showFooterNotice(take.hasUnsavedEdits ? "unsaved edit discarded" : "previous take discarded")
    if terminal, !isEditingTranscript { restartAutoHideCountdown() }
  }

  /// One capture boundary.
  ///
  /// Called only from the lifecycle admission the controller already owns
  /// (`preparing` / `started` while nothing is recording). It is deliberately
  /// NOT driven by a Bus row: `docs/TRANSCRIPT_BUS.md` forbids treating
  /// `session_started` as permission to mutate product state, so the overlay
  /// fences on the controller's own callback instead.
  ///
  /// The pending capture gets an empty canvas and no inherited action
  /// capabilities. The superseded take keeps its document and its draft
  /// readable, and its session is retired: a late projection for it can still
  /// finish its addressed delivery, but it can no longer repaint, finalize or
  /// auto-hide the successor.
  private func admitNewCapture() {
    if let compactProjection {
      retiredProjectionSessions.insert(compactProjection.sessionId)
    }
    compactProjection = nil
    // Read the draft's dirtiness against the OUTGOING document, before any
    // field below moves. Computed after the reset it would compare against an
    // empty projection and misclassify a clean draft as unsaved work.
    let draftWasDirty = isRevisionDraftDirty
    captureGeneration &+= 1
    // A scheduled focus-exit commit belongs to the take being superseded.
    // Letting it fire across the boundary would send an FFI revision for a
    // closed session while a new capture is live, so the bytes are preserved
    // for recovery rather than committed behind the user's back.
    revisionFocusCommitTask?.cancel()
    revisionFocusCommitTask = nil
    // Retire and retain in one step, while the outgoing projection is still
    // readable: the identity is passed in rather than re-read, so no later
    // reordering of this method can silently turn retention into a no-op.
    if let prior = latestTranscriptProjection {
      retiredProjectionSessions.insert(prior.sessionId)
      retainSupersededTake(prior, draftWasDirty: draftWasDirty)
    }
    latestTranscriptProjection = nil
    revisionDraft = ""
    documentHistory = []
    historyReadSessionId = nil
    // Retained chrome is evidence about the previous take, not this one.
    transcriptMode = "dictation"
    mode = .listening
    terminal = false
    revision = 0
    canPaste = false
    canInsert = false
    canCopy = false
    canRetranscribe = false
    canFormat = false
    canSendToAgent = false
    // The canvas is no longer editable, so AppKit will resign it; recording the
    // presentation truth here keeps the caret and the auto-hide hold honest.
    // `endTranscriptEdit` is idempotent, so the later resign cannot double-fire
    // a commit for a draft this boundary already moved to recovery.
    isEditingTranscript = false
    revisionCommitPending = false
    formatterCommitPending = false
    revisionCommitError = nil
    formatterError = nil
    pendingRevisionSessionId = nil
    pendingRevisionSource = nil
    userRevisionProvenance = nil
    onTranscriptPresentationChanged?()
  }

  private func resetTranscript() {
    deliveredText = ""
    pendingNoSpeechMessage = nil
    // A rollback belongs to the take whose Retranscribe created it. Unlike
    // `supersededTakes` it repaints the canvas, so it must never survive into
    // a new capture and restore words over a different take.
    retranscribeRollback = nil
    noSpeechNotice = OverlayState.defaultNoSpeechNotice
    coverageRefusalNotice = nil
    // A persisting chip belongs to the take that raised it. Nothing else
    // cleared one, so a retained-delivery or deferred-insert notice used to
    // ride into the next capture and describe the wrong take. Clearing the
    // NOTICE is not clearing the WORK: `retainedComposerDocuments` and
    // `supersededTakes` are keyed by their own session identities and are
    // deliberately untouched here.
    toastTask?.cancel()
    toastTask = nil
    toast = nil
    presentationStatus = nil
    errorLifecycleDetail = "No transcript was delivered."
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
    s.isCollapsed = false
    s.applyTranscriptProjection(
      previewProjection(
        "add a rate limiter to the login route and write a test for it",
        phase: .listening,
        terminal: false
      )
    )
    s.vadActive = true
    return s
  }

  /// Seeded view model for #Preview: a live take with a refused Whisper
  /// alternative painted beside the canvas as read-only evidence.
  static func previewListeningWithEvidence() -> OverlayState {
    let s = previewListening()
    s.compactProjection = CsCompactProjection(
      sessionId: "preview", captureEpoch: 1, sequence: 1, text: "write a test for it",
      degraded: false,
      evidence: [
        CsUnanchoredEvidence(
          sampleStart: 16_000, sampleEnd: 48_000, text: "add a rate limit to the log-in route",
          reason: "exclusive_tail_awaiting_whole_span")
      ])
    return s
  }

  /// Seeded view model for #Preview in the post-capture transcribing phase.
  static func previewTranscribing() -> OverlayState {
    let s = OverlayState()
    s.isCollapsed = false
    s.applyTranscriptProjection(
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
    s.isCollapsed = false
    s.applyTranscriptProjection(previewProjection("", phase: .noSpeech, terminal: true))
    s.noSpeechNotice = OverlayState.defaultNoSpeechNotice
    return s
  }

  /// Seeded view model for #Preview in the finalized state.
  static func previewFormatted() -> OverlayState {
    let s = OverlayState()
    s.isCollapsed = false
    s.applyTranscriptProjection(
      previewProjection(
        "Add a rate limiter to the login route and write a test that covers the throttle window. Keep the existing error shape.",
        phase: .formatted,
        terminal: true
      )
    )
    return s
  }

  /// Seeded view model for the terminal error phase.
  static func previewError() -> OverlayState {
    let s = OverlayState()
    s.isCollapsed = false
    s.applyTranscriptProjection(
      previewProjection("", phase: .error, terminal: true)
    )
    s.errorMessage = "The transcription engine could not finish this take."
    return s
  }

  private static func previewProjection(
    _ renderedText: String,
    phase: OverlayMode,
    terminal: Bool
  ) -> CsTranscriptProjectionEvent {
    let isFormatted = phase == .formatted
    return CsTranscriptProjectionEvent(
      schema: "preview", sequence: 1, emittedAt: "preview", sessionId: "preview", mode: "dictation",
      reducerRevision: 1, reducerAction: "preview_fixture",
      occurrenceSessionId: "preview",
      captureEpoch: 0, sampleStart: 0, sampleEnd: 0, documentIndex: 0, label: renderedText,
      renderedText: renderedText, deliveryText: nil, phase: phase.rawValue, canPaste: isFormatted,
      canInsert: isFormatted,
      canCopy: !renderedText.isEmpty, canRetranscribe: phase == .noSpeech || isFormatted,
      canFormat: isFormatted,
      canSendToAgent: isFormatted,
      terminal: terminal, lifecycleTerminal: terminal, delivery: .unattempted, acousticReceipts: [],
      sealCoverage: nil, consultationPresentations: [])
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
  func commitUserRevision(
    sessionId: String, sourceRevision: UInt64, renderedText: String
  ) async throws -> CsUserRevisionResult {
    try await hotkeys.commitUserRevision(
      sessionId: sessionId,
      sourceRevision: sourceRevision,
      renderedText: renderedText
    )
  }
  func commitFormatterRevision(
    sessionId: String, sourceRevision: UInt64, level: FormattingPolicyOption?
  ) async throws -> CsUserRevisionResult {
    try await hotkeys.commitFormatterRevisionAtLevel(
      sessionId: sessionId,
      sourceRevision: sourceRevision,
      level: level?.rawValue
    )
  }
  func documentHistory(sessionId: String) async throws -> [CsDocumentHistoryEntry] {
    try await hotkeys.documentHistory(sessionId: sessionId)
  }
  func restoreDocumentRevision(
    sessionId: String, sourceRevision: UInt64, restoreRevision: UInt64
  ) async throws -> CsUserRevisionResult {
    try await hotkeys.restoreDocumentRevision(
      sessionId: sessionId,
      sourceRevision: sourceRevision,
      restoreRevision: restoreRevision)
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
  func overlayExpandedByDefault() -> Bool {
    config.overlayExpandedByDefault()
  }
  func setOverlayExpandedByDefault(_ enabled: Bool) -> Bool {
    config.setOverlayExpandedByDefault(enabled: enabled)
  }
  func overlayKeepVisibleBetweenTakes() -> Bool {
    config.overlayKeepVisibleBetweenTakes()
  }
  func setOverlayKeepVisibleBetweenTakes(_ enabled: Bool) -> Bool {
    config.setOverlayKeepVisibleBetweenTakes(enabled: enabled)
  }
  func setAutoFormatLevel(_ level: FormattingPolicyOption) {
    _ = try? config.setAutoFormatLevel(level: level.rawValue)
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
  func lastSessionAudioPath() -> String? {
    hotkeys.lastSessionAudioPath()
  }
  func sessionAudioPath(sessionId: String) -> String? {
    hotkeys.sessionAudioPath(sessionId: sessionId)
  }
  func commitRetranscribeRevision(
    sessionId: String, sourceRevision: UInt64, renderedText: String
  ) async throws -> CsUserRevisionResult {
    try await hotkeys.commitRetranscribeRevision(
      sessionId: sessionId, sourceRevision: sourceRevision, renderedText: renderedText)
  }
  func transcribeFile(path: String) async throws -> CsTranscription {
    try await hotkeys.transcribeFile(path: path)
  }
  func transcribeTake(sessionId: String, path: String) async throws -> CsTranscription {
    try await hotkeys.transcribeTake(sessionId: sessionId, path: path)
  }
}
