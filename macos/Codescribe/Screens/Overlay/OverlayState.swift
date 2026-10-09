import AppKit
import Observation
import SwiftUI

/// Window presentation only; the reducer continues to own the transcript.
enum OverlayPresentationMode: CaseIterable, Hashable {
  case mini, midi, expanded
}

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
  func startRecording(assistive: Bool, language: CsLanguage?) async throws
  func startsInAssistiveMode() -> Bool
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
  func cloudRetranscribeConfigured() -> Bool
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
  /// Export one word's PCM from the retained take audio as a temp WAV clip
  /// with `padMs` of context on both sides. The sample window is pinned to the
  /// physical occurrence (session + capture epoch), never to word text.
  func wordAudioClip(
    sessionId: String, captureEpoch: UInt64, sampleStart: UInt64, sampleEnd: UInt64,
    padMs: UInt32
  ) throws -> String
  /// One-click Teach from a canvas span through the existing quality path.
  func teachSpan(variant: String, canonical: String, kind: String) throws
    -> CsQualityCommitResult
  func transcribeFile(path: String) async throws -> CsTranscription
  func transcribeTake(sessionId: String, path: String) async throws -> CsTranscription
  /// Production formatter over one revision of a transcript reopened from
  /// history. Rust reads the source from that archive's revision chain and
  /// commits an applied result there; the reducer and the Bus are untouched.
  func formatArchivedTranscript(
    archivePath: String, sourceRevision: UInt64, level: FormattingPolicyOption?
  ) async throws -> CsArchivedFormat
  /// Commit an explicit edit or retranscription of one archive against the
  /// revision the canvas shows. The archived text and audio stay untouched.
  func commitArchivedRevision(
    archivePath: String, sourceRevision: UInt64, renderedText: String,
    kind: CsArchiveRevisionKind
  ) async throws -> CsArchivedDocument
  /// Restore an earlier version of one archive as a new revision (Undo).
  func restoreArchivedRevision(
    archivePath: String, sourceRevision: UInt64, restoreRevision: UInt64
  ) async throws -> CsArchivedDocument
  func channelRosterSnapshot() async -> [CsChannelRosterState]
  func toggleAgentChannel(digit: UInt8) async throws
  func archiveAgent(request: CsAgentArchiveRequest) async throws -> String
}

extension DictationEngine {
  func archiveAgent(request _: CsAgentArchiveRequest) async throws -> String {
    throw CocoaError(.fileWriteUnknown)
  }
  func cloudRetranscribeConfigured() -> Bool { false }
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
  func wordAudioClip(
    sessionId _: String, captureEpoch _: UInt64, sampleStart _: UInt64, sampleEnd _: UInt64,
    padMs _: UInt32
  ) throws -> String {
    throw NSError(domain: "Word audio unavailable", code: 1)
  }
  func teachSpan(variant _: String, canonical _: String, kind _: String) throws
    -> CsQualityCommitResult
  {
    throw NSError(domain: "Teach unavailable", code: 1)
  }
  func transcribeTake(sessionId _: String, path: String) async throws -> CsTranscription {
    try await transcribeFile(path: path)
  }
  func formatArchivedTranscript(
    archivePath _: String, sourceRevision _: UInt64, level _: FormattingPolicyOption?
  ) async throws -> CsArchivedFormat {
    throw NSError(domain: "Archived transcript formatting unavailable", code: 1)
  }
  func commitArchivedRevision(
    archivePath _: String, sourceRevision _: UInt64, renderedText _: String,
    kind _: CsArchiveRevisionKind
  ) async throws -> CsArchivedDocument {
    throw NSError(domain: "Archived transcript revisions unavailable", code: 1)
  }
  func restoreArchivedRevision(
    archivePath _: String, sourceRevision _: UInt64, restoreRevision _: UInt64
  ) async throws -> CsArchivedDocument {
    throw NSError(domain: "Archived transcript revisions unavailable", code: 1)
  }
  func overlayExpandedByDefault() -> Bool { true }
  func setOverlayExpandedByDefault(_ enabled: Bool) -> Bool { false }
  func overlayKeepVisibleBetweenTakes() -> Bool { false }
  func setOverlayKeepVisibleBetweenTakes(_ enabled: Bool) -> Bool { false }
  func channelRosterSnapshot() async -> [CsChannelRosterState] { [] }
  func toggleAgentChannel(digit _: UInt8) async throws {
    throw NSError(domain: "Agent channel unavailable", code: 1)
  }
}

/// Paste mode is not part of it: Settings › Shortcuts and the tray cycle own
/// `paste_mode`, and the overlay neither shows nor writes it (Founder direction
/// as relayed in the Codex handoff, Annex A2, 2026-09-29).
struct OverlayPolicySnapshot: Equatable {
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
  /// Rust's session identity for a projected take; nil for an archive.
  let sessionId: String?
  /// The transcript reopened from history this work belongs to. Its path is
  /// the archive owner's identity and `reducerRevision` is then the archive
  /// revision the draft was written against.
  let archive: OverlaySupersededArchive?
  /// The last authoritative document Rust projected for this session.
  let renderedText: String
  let reducerRevision: UInt64
  /// The user's uncommitted edit at the capture boundary, when it differed from
  /// `renderedText`. `nil` means nothing was unsaved — the document itself is
  /// still ledger-authoritative and reproducible.
  let unsavedDraft: String?

  var id: String {
    if let archive { return "archive:\(archive.path)#\(reducerRevision)" }
    return "\(sessionId ?? "")#\(reducerRevision)"
  }
  var hasUnsavedEdits: Bool { unsavedDraft != nil }
  /// What an explicit recovery hands back: the user's own edit when one was
  /// unsaved, otherwise the projected document.
  var recoverableText: String { unsavedDraft ?? renderedText }

  init(sessionId: String, renderedText: String, reducerRevision: UInt64, unsavedDraft: String?) {
    self.sessionId = sessionId
    archive = nil
    self.renderedText = renderedText
    self.reducerRevision = reducerRevision
    self.unsavedDraft = unsavedDraft
  }

  init(archive: OverlaySupersededArchive, renderedText: String, revision: UInt64, unsavedDraft: String) {
    sessionId = nil
    self.archive = archive
    self.renderedText = renderedText
    reducerRevision = revision
    self.unsavedDraft = unsavedDraft
  }
}

/// Which archived transcript a retained edit was written for.
struct OverlaySupersededArchive: Equatable {
  let path: String
  let recordedAt: Date
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
    case .fullHq: String(localized: "Full HQ file pass")
    case .cloud: String(localized: "Cloud pass")
    }
  }

  var help: String {
    switch self {
    case .fullHq:
      String(localized: "Full local Whisper file pass over the last session audio")
    case .cloud: String(localized: "Cloud STT pass over the last session audio")
    }
  }
}

enum OverlayIntent: String, Equatable, Hashable, CaseIterable {
  case startRecording = "start-recording"
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
  /// Restore the version an archive's last format replaced, as a new revision
  /// of that archive. Projected only for a transcript reopened from history.
  case undoFormat = "undo-format"
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
  var formattedText: String {
    guard let projection = latestTranscriptProjection else { return "" }
    return projection.reducerAction == "derived_projection"
      ? projection.deliveryText ?? projection.renderedText : projection.renderedText
  }
  /// A6 uncertain-word spans from the reducer projection (UTF-16 ranges into
  /// `formattedText`). The overlay stays a pure projection: classification
  /// happened in Rust; `canvasUncertainWords` maps them onto the canvas.
  var uncertainSpans: [CsUncertainSpan] { latestTranscriptProjection?.uncertainSpans ?? [] }
  /// View-local editor payload. It is never delivery or transcript truth; only
  /// `formattedText`, repainted from the Rust projection, feeds downstream
  /// actions. The canvas paints it while a completed take is under review.
  var revisionDraft = ""
  /// True while the canvas is being edited. Native selection can also hold
  /// keyboard focus without entering an edit.
  private(set) var isEditingTranscript = false
  private(set) var revision: UInt64 = 0
  private(set) var revisionCommitPending = false
  private(set) var revisionCommitError: String?
  private(set) var formatterCommitPending = false
  private(set) var formatterError: String?
  private(set) var maxPreparationError: String?
  private(set) var transcriptStorageError: String?
  /// Read-only projection of this take's Bus journal revisions.
  private(set) var documentHistory: [CsDocumentHistoryEntry] = []
  /// A transcript reopened from history. While set it owns the canvas and
  /// every canvas action; the projected take behind it keeps its own state.
  private(set) var archivedTranscript: OverlayArchivedTranscript?
  /// Editor payload for the archive, the twin of `revisionDraft`.
  private var archiveDraft = ""
  private(set) var archiveActionPending = false
  private(set) var archiveActionError: String?
  /// Fence for archive presentation: bumped on every open, leave and action,
  /// so a late result can only repaint the archive request that started it.
  /// It fences painting only; Rust's revision chain owns what was accepted.
  private var archiveGeneration: UInt64 = 0
  /// Admission ticket for history opens. A read that finishes after a newer
  /// request, a dismissed list, a close or a new take holds a stale ticket.
  @ObservationIgnored private var historyOpenAdmission: UInt64 = 0
  private var chromeBehindArchive: OverlayProjectedChrome?
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
  /// Engine failure text is secondary detail, never the primary footer copy.
  private(set) var errorDiagnosticDetail: String?
  private(set) var errorFooterSummary = String(localized: "Transcription failed")
  /// History of the controller's started callback for this capture generation.
  /// Preparing is intent only; Stop and abort must not erase a confirmed start.
  private(set) var captureDidStart = false
  private(set) var currentTakeAudioAvailable = false
  private(set) var presentationStatus: OverlayPresentationStatus?
  private(set) var compactProjection: CsCompactProjection?
  private(set) var errorLifecycleDetail =
    String(localized: "No transcript was delivered.")
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
  private(set) var cloudRetranscribeConfigured = false
  private(set) var autoFormatLevel: FormattingPolicyOption = .correction
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

  /// The window already reached this point through a user drag. Save its origin
  /// for an explicit Free motion choice while preserving the selected anchor.
  func recordUserDrag(at origin: NSPoint) {
    OverlayPlacement.persistOrigin(origin)
    userDraggedOverlay()
  }

  // MARK: Injected collaborators (all optional so #Preview renders standalone)
  /// The recording core. Injected by the orchestrator. Do NOT instantiate here.
  var engine: DictationEngine?
  /// Handoff to the agent surface — wired by the orchestrator (routes the text
  /// into AgentChat, which streams it through `CodescribeAgent.streamReply`).
  var onSendToAgent: ((String) -> Void)?
  var onContinueMaxConsultation: ((String) -> Bool)?
  /// Dismiss the floating window when the overlay's own lifecycle ends
  /// (auto-hide, warmup watchdog, agent delivery) — wired by the orchestrator,
  /// which keeps an open agent channel on screen against it.
  var onClose: (() -> Void)?
  /// The human Close intent (the brand dot) — wired by the orchestrator to a
  /// dismissal that no automatic visibility rule can veto.
  var onCloseIntent: (() -> Void)?
  /// Window chrome only: folding never ends capture or creates text edits.
  /// Leaving an edited canvas uses its existing commit-on-blur path.
  private(set) var presentationMode: OverlayPresentationMode = .mini
  var isCollapsed: Bool { presentationMode != .expanded }
  var isMini: Bool { presentationMode == .mini }
  private(set) var expandedByDefault = true
  private(set) var keepVisibleBetweenTakes = false
  private(set) var expansionPreferenceError: String?
  @ObservationIgnored var onPresentationModeChanged: ((OverlayPresentationMode) -> Void)?
  /// Presentation changes come from explicit controls; pointer motion never morphs the window.
  func toggleCollapsed() {
    switch presentationMode {
    case .mini: setPresentationMode(.midi)
    case .midi: setPresentationMode(.expanded)
    case .expanded: setPresentationMode(.mini)
    }
  }

  func requestHeaderRecording(_ intent: OverlayIntent) {
    if intent == .startRecording { selectConversation(nil) }
    relayIntent(intent)
  }

  func setPresentationMode(_ mode: OverlayPresentationMode) {
    guard presentationMode != mode else { return }
    presentationMode = mode
    if mode != .expanded { showsAgentMonitor = false }
    onPresentationModeChanged?(mode)
  }

  func clearPointerHover() {
    isPointerHovering = false
  }

  /// The menu toggle alone persists the take-start preference.
  func setExpandedByDefault(_ expanded: Bool) {
    guard let engine, engine.setOverlayExpandedByDefault(expanded) else {
      expansionPreferenceError = String(localized: "Couldn't save overlay preference")
      return
    }
    expansionPreferenceError = nil
    applyPreferredExpansion()
  }

  private func applyPreferredExpansion(forTake: Bool = false) {
    guard let engine else { return }
    expandedByDefault = engine.overlayExpandedByDefault()
    setPresentationMode(expandedByDefault ? .expanded : (forTake ? .midi : .mini))
  }

  func setKeepVisibleBetweenTakes(_ enabled: Bool) {
    guard let engine, engine.setOverlayKeepVisibleBetweenTakes(enabled) else {
      expansionPreferenceError = String(localized: "Couldn't save overlay preference")
      return
    }
    expansionPreferenceError = nil
    keepVisibleBetweenTakes = engine.overlayKeepVisibleBetweenTakes()
    if pinKeepsOverlayVisible {
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

  static let defaultNoSpeechNotice = String(localized: "No speech detected")

  /// Display copy distinguishes measured coverage from refused finality.
  /// Absent coverage is unknown; complete coverage does not mint a terminal seal.
  /// The notice is the observation sentence the footer chip also shows.
  private var coverageRefusalCopy: (status: String, notice: String) {
    let coverage = latestTranscriptProjection?.sealCoverage
    let notice = OverlayWarningCopy.sealRefused(
      coverage, sampleRateHz: coverage?.sampleRateHz
    ).sentence
    switch coverage?.status {
    case .incomplete: return (String(localized: "incomplete coverage"), notice)
    case .unavailable: return (String(localized: "measurement unavailable"), notice)
    case .complete: return (String(localized: "unsealed transcript"), notice)
    case .unknown, nil: return (String(localized: "unverified coverage"), notice)
    }
  }

  /// The footer reports seal coverage or measured quiet microphone input.
  /// Nil when neither applies.
  var footerWarning: OverlayWarningCopy? {
    if mode == .coverageRefused {
      let coverage = latestTranscriptProjection?.sealCoverage
      return .sealRefused(coverage, sampleRateHz: coverage?.sampleRateHz)
    }
    if mode == .listening && levelMeter.hasLowInputSignal { return .quietInput }
    return nil
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
  /// Request bytes and generation fence acknowledgements, never acoustic identity.
  private var pendingRevisionDraft: String?
  private var revisionRequestGeneration: UInt64 = 0
  private var revisionFocusCommitTask: Task<Void, Never>?
  /// Completion of the admitted capture, independent of document finality.
  /// Only a new capture/session clears it; later document observations cannot.
  private(set) var finalized = false
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
  private let micAccessProvider: () -> Bool
  /// Terminal countdown for non-Agent outcomes and opted-in Agent delivery.
  static let autoHideDelaySeconds: TimeInterval = 5

  init(
    nowProvider: @escaping () -> TimeInterval = { ProcessInfo.processInfo.systemUptime },
    autoSendEnabled: @escaping () -> Bool = { CodescribeConfig().loadSettings().agentAutoSend },
    micAccessProvider: @escaping () -> Bool = { micPermissionGranted() || requestMicPermission() }
  ) {
    let channel = AsyncStream<OverlayListenerEvent>.makeStream()
    eventStream = channel.stream
    listener = DictationListener(continuation: channel.continuation)
    self.nowProvider = nowProvider
    self.autoSendEnabled = autoSendEnabled
    self.micAccessProvider = micAccessProvider
    eventTask = Task { @MainActor [weak self, eventStream] in
      for await event in eventStream {
        guard let self else { return }
        apply(event)
      }
    }
  }

  func attach() {
    // Attaching loads policy; a new take or an explicit control opens the canvas.
    expandedByDefault = engine?.overlayExpandedByDefault() ?? true
    keepVisibleBetweenTakes = engine?.overlayKeepVisibleBetweenTakes() ?? false
    engine?.setListener(listener)
    if engine is ControllerDictationEngine {
      observeChannelDelivery(
        using: OverlayChannelDeliveryReader(
          root: OverlayChannelDeliveryReader.productionRoot(),
          sharedBus: URL(fileURLWithPath: agentConversationBusPath())))
    }
  }

  private(set) var channelDelivery: [OverlayChannelDelivery] = []
  private(set) var channelStatusUnavailable = false
  private(set) var channelHudStates: [String: OverlayChannelHudProjection] = [:]
  private(set) var channelRosterNames: [String: String] = [:]
  private(set) var channelToggleError: String?
  private(set) var pendingChannelToggles: Set<String> = []
  private(set) var conversations: [OverlayConversation] = []
  private(set) var archivedAgentOwners: Set<OverlayConversationOwner> = []
  private(set) var pendingAgentArchives: Set<OverlayConversationOwner> = []
  private(set) var agentArchiveError: String?
  @ObservationIgnored private var agentArchiveRevision: UInt64 = 0
  @ObservationIgnored var archiveAgentCommand: ((OverlayConversationOwner) async throws -> Void)?
  private(set) var selectedConversationID: String?
  private(set) var showsAgentMonitor = false
  @ObservationIgnored var onAgentSidebarPresented: (() -> Void)?
  private var playbackMutes: [AgentPlaybackIdentity: Bool] = [:]
  private var channelPlaybackAgents: [String: AgentPlaybackBinding] = [:]
  private var pendingPlaybackIdentities: Set<AgentPlaybackIdentity> = []
  @ObservationIgnored private var playbackPreferenceRevision: UInt64 = 0
  private(set) var playbackPreferenceError: String?
  private var channelRoster: [CsChannelRosterState] = []
  private var pendingChannelConversation: CsChannelRosterState?
  private(set) var replyControlErrors: [String: String] = [:]
  private(set) var pendingReplyControls: Set<String> = []
  private var viewedReplyIDs: Set<String> = []
  /// Canonical reply message ids this surface has already loaded or admitted.
  /// The first inventory is history. Later ids are admissions. Membership
  /// survives pruning of the retained snapshot: replacing the set with whatever
  /// is still retained makes a replay of the same id look new. This is
  /// presentation knowledge for the life of this state, not message storage,
  /// and it is not evicted — eviction would admit that replay again.
  private var observedReplyIDs: Set<String> = []
  private var replyInventoryBaselined = false
  /// True only while the presentation callback for one qualified fresh reply
  /// is still in flight. History, an identical snapshot, ACK, playback, and
  /// manual navigation leave it false, so those repaints do not reopen a panel.
  @ObservationIgnored private(set) var freshReplyPresentationRequested = false
  /// The mounted composer is the panel's editor, including a blank field.
  /// A nonempty draft is not this fact.
  @ObservationIgnored private var composerEditorActive = false
  /// Monotonic instant when typing protection ends. Checked when a reply is
  /// admitted. Nothing is scheduled, and expiry does not revisit an id already
  /// stored in `observedReplyIDs`.
  @ObservationIgnored private var composerTypingDeadline: TimeInterval?
  static let composerTypingHorizon: TimeInterval = 15
  private var conversationIsVisible = false

  var selectedConversation: OverlayConversation? {
    conversations.first { $0.id == selectedConversationID }
  }
  var showsMyDictation: Bool { selectedConversationID == nil }

  func archiveCandidate(for channel: String) -> OverlayConversationOwner? {
    guard !channelStatusUnavailable, channel.count == 1, "123456789".contains(channel),
      let hud = channelHudStates[channel], hud.followerAlive == false, !hud.open,
      let provider = hud.provider, !provider.isEmpty,
      let session = hud.providerSessionID, !session.isEmpty,
      let name = channelRosterNames[channel]
    else { return nil }
    let owner = OverlayConversationOwner(row: [
      "provider": provider, "provider_session_id": session, "channel": channel, "name": name,
      "lease_id": AgentPlaybackIdentity.leaseIdentifier(provider: provider, session: session),
    ])
    guard let owner, !archivedAgentOwners.contains(owner) else { return nil }
    return owner
  }

  func archiveAgent(_ owner: OverlayConversationOwner) async {
    guard archiveCandidate(for: owner.channel) == owner,
      !pendingAgentArchives.contains(owner)
    else { return }
    pendingAgentArchives.insert(owner)
    agentArchiveError = nil
    defer { pendingAgentArchives.remove(owner) }
    do {
      if let archiveAgentCommand {
        try await archiveAgentCommand(owner)
      } else {
        guard let engine else { throw CocoaError(.fileWriteUnknown) }
        try await RealAgentBridgeInstaller.archiveBusAgent(owner: owner) { request in
          try await engine.archiveAgent(request: request)
        }
      }
      agentArchiveRevision &+= 1
      archivedAgentOwners.insert(owner)
      onChannelPresentationChanged?()
    } catch {
      agentArchiveError = String(
        localized: "Couldn't archive agent. The channel and conversation are retained.")
    }
  }

  private func playbackIdentity(for conversation: OverlayConversation) -> AgentPlaybackIdentity? {
    guard let owner = conversation.owner, let bus = conversation.messages.first?.busPath else {
      return nil
    }
    return AgentPlaybackIdentity(
      provider: owner.provider, session: owner.providerSessionID, bus: bus)
  }

  private func playbackIdentity(for channel: String) -> AgentPlaybackIdentity? {
    guard let hud = channelHudStates[channel],
      let identity = channelPlaybackAgents[channel]?.identity,
      hud.provider == identity.provider, hud.providerSessionID == identity.session
    else { return nil }
    return identity
  }

  var channelAgentDescriptors: [String: String] {
    var descriptors: [String: String] = [:]
    for (channel, binding) in channelPlaybackAgents where playbackIdentity(for: channel) != nil {
      descriptors[channel] = binding.descriptor
    }
    return descriptors
  }

  func conversationAgentDescriptor(_ conversation: OverlayConversation) -> String? {
    guard let identity = playbackIdentity(for: conversation) else { return nil }
    return channelPlaybackAgents.values.first { $0.identity == identity }?.descriptor
  }

  var channelPlaybackMuted: [String: Bool] {
    var result: [String: Bool] = [:]
    for channel in channelHudStates.keys {
      if let identity = playbackIdentity(for: channel), let muted = playbackMutes[identity] {
        result[channel] = muted
      }
    }
    return result
  }

  var pendingPlaybackChannels: Set<String> {
    Set(
      channelHudStates.keys.filter {
        playbackIdentity(for: $0).map { pendingPlaybackIdentities.contains($0) } ?? false
      })
  }

  var pendingPlaybackOwners: Set<String> {
    Set(
      conversations.compactMap { conversation in
        guard let identity = playbackIdentity(for: conversation),
          pendingPlaybackIdentities.contains(identity)
        else { return nil }
        return conversation.owner?.id
      })
  }

  func conversationPlaybackMuted(_ conversation: OverlayConversation) -> Bool? {
    playbackIdentity(for: conversation).flatMap { playbackMutes[$0] }
  }

  func canToggleConversationMicrophone(_ conversation: OverlayConversation) -> Bool {
    guard !channelStatusUnavailable, let owner = conversation.owner,
      !archivedAgentOwners.contains(where: { $0.id == owner.id && $0.channel == owner.channel }),
      OverlayChannelStatusView.toggleDigit(for: conversation.channel) != nil,
      let hud = channelHudStates[conversation.channel], hud.provider == owner.provider,
      hud.providerSessionID == owner.providerSessionID
    else { return false }
    return conversations.filter {
      $0.channel == conversation.channel && $0.owner?.provider == owner.provider
        && $0.owner?.providerSessionID == owner.providerSessionID
    }.count == 1
  }

  func conversationMicrophoneOpen(_ conversation: OverlayConversation) -> Bool {
    canToggleConversationMicrophone(conversation)
      && channelHudStates[conversation.channel]?.open == true
  }

  func toggleConversationMicrophone(_ conversation: OverlayConversation) async {
    guard canToggleConversationMicrophone(conversation),
      let digit = OverlayChannelStatusView.toggleDigit(for: conversation.channel)
    else { return }
    await toggleAgentChannel(digit)
  }

  func toggleConversationPlayback(_ conversation: OverlayConversation) async {
    guard let identity = playbackIdentity(for: conversation) else { return }
    await togglePlayback(identity)
  }

  func toggleChannelPlayback(_ channel: String) async {
    guard let identity = playbackIdentity(for: channel) else { return }
    await togglePlayback(identity)
  }

  private func togglePlayback(_ identity: AgentPlaybackIdentity) async {
    guard let muted = playbackMutes[identity],
      !pendingPlaybackIdentities.contains(identity)
    else { return }
    pendingPlaybackIdentities.insert(identity)
    playbackPreferenceRevision &+= 1
    defer {
      pendingPlaybackIdentities.remove(identity)
      playbackPreferenceRevision &+= 1
    }
    do {
      try await RealAgentBridgeInstaller.setPlaybackMuted(!muted, for: identity)
      // Show the owner's durable receipt, never an optimistic button state.
      let snapshot = await RealAgentBridgeInstaller.playbackMuteSnapshot(for: [identity])
      playbackMutes[identity] = snapshot[identity]
      playbackPreferenceError =
        snapshot[identity] == nil
        ? String(localized: "Playback status unavailable") : nil
    } catch {
      playbackPreferenceError = error.userFacingMessage
    }
  }

  private func refreshPlaybackMutes() async {
    let revision = playbackPreferenceRevision
    let roster = channelRoster
    var candidates: [String: AgentPlaybackIdentity] = [:]
    for (channel, hud) in channelHudStates {
      guard let provider = hud.provider, let session = hud.providerSessionID,
        !provider.isEmpty, !session.isEmpty
      else { continue }
      // The candidate locates the lease; its bus is replaced by that lease's bus.
      candidates[channel] = AgentPlaybackIdentity(
        provider: provider, session: session, bus: agentConversationBusPath())
    }
    let bound = await RealAgentBridgeInstaller.boundPlaybackAgents(for: candidates)
    let identities = Set(bound.values.map(\.identity))
      .union(conversations.compactMap { playbackIdentity(for: $0) })
    let snapshot = await RealAgentBridgeInstaller.playbackMuteSnapshot(for: identities)
    guard !Task.isCancelled, revision == playbackPreferenceRevision, roster == channelRoster else {
      return
    }
    channelPlaybackAgents = bound
    // A poll started before a click may carry the old receipt for that identity.
    let retained = playbackMutes.filter { pendingPlaybackIdentities.contains($0.key) }
    playbackMutes = snapshot.filter { !pendingPlaybackIdentities.contains($0.key) }
      .merging(retained, uniquingKeysWith: { _, current in current })
  }

  func unreadReplies(in conversation: OverlayConversation) -> Int {
    conversation.replyIDs.filter { !viewedReplyIDs.contains($0) }.count
  }

  /// Viewing only changes presentation metadata. Capture stays controller-owned.
  func selectConversation(_ id: String?, expand: Bool = true) {
    guard id == nil || conversations.contains(where: { $0.id == id }) else { return }
    revisionFocusCommitTask?.cancel()
    revisionFocusCommitTask = nil
    selectedConversationID = id
    conversationFocusRevision &+= 1
    showsAgentMonitor = false
    pendingChannelConversation = nil
    if id != nil && expand {
      expandAgentSurface()
    } else {
      onChannelPresentationChanged?()
    }
    markVisibleConversationRead()
  }

  func showTranscription() {
    selectConversation(nil)
    expandAgentSurface()
  }

  func showAgentMonitor(expand: Bool = true) {
    onAgentSidebarPresented?()
    pendingChannelConversation = nil
    showsAgentMonitor = true
    if expand { expandAgentSurface() }
  }

  func toggleAgentSidebar() {
    if showsAgentMonitor && !isCollapsed { hideAgentSidebar() } else { showAgentMonitor() }
  }

  func hideAgentSidebar() {
    showsAgentMonitor = false
    markVisibleConversationRead()
    onChannelPresentationChanged?()
  }

  private func expandAgentSurface() {
    cancelAutoHide()
    setPresentationMode(.expanded)
    onChannelPresentationChanged?()
  }

  /// Follow controller intent, never a name guessed from transcript text.
  /// A late observer snapshot may supply the matching conversation afterward.
  private func followChannelConversation(_ row: CsChannelRosterState) {
    pendingChannelConversation = row
    resolveChannelConversation()
    if expandedByDefault { expandAgentSurface() }
  }

  private func resolveChannelConversation() {
    guard let row = pendingChannelConversation else { return }
    let matches = conversations.filter { conversation in
      guard conversation.channel == row.channel else { return false }
      if row.channel == "0" { return conversation.owner == nil }
      guard let provider = row.provider, let session = row.providerSessionId,
        let owner = conversation.owner
      else { return false }
      return owner.provider == provider && owner.providerSessionID == session
    }
    guard matches.count == 1 else {
      selectedConversationID = nil
      showsAgentMonitor = true
      return
    }
    selectedConversationID = matches[0].id
    conversationFocusRevision &+= 1
    showsAgentMonitor = false
    pendingChannelConversation = nil
    markVisibleConversationRead()
  }

  func setConversationVisible(_ visible: Bool) {
    conversationIsVisible = visible
    markVisibleConversationRead()
  }

  private func markVisibleConversationRead() {
    guard conversationIsVisible, !isCollapsed, !showsAgentMonitor, let selectedConversation else {
      return
    }
    viewedReplyIDs.formUnion(selectedConversation.replyIDs)
  }

  func applyConversationSnapshot(_ snapshot: OverlayChannelDeliverySnapshot) {
    if archivedAgentOwners != snapshot.archivedOwners {
      archivedAgentOwners = snapshot.archivedOwners
      onChannelPresentationChanged?()
    }
    applyChannelDelivery(snapshot.deliveries)
    guard conversations != snapshot.conversations else {
      // The first snapshot can already match the stored inventory, including
      // an empty one. That snapshot is still the baseline, so the next genuine
      // reply is an admission. A later identical snapshot is already baselined
      // and stays passive, including a first load that already holds history.
      if !replyInventoryBaselined {
        observedReplyIDs.formUnion(Set(conversations.flatMap(\.replyIDs)))
        replyInventoryBaselined = true
      }
      return
    }
    let controllerPending = pendingChannelConversation != nil
    conversations = snapshot.conversations
    // Loading saved history is an inventory change, not a fresh reply admission.
    // Its mirrored broadcast ids must stay passive as well.
    observedReplyIDs.formUnion(snapshot.historyReplyIDs)
    observedReplyIDs.formUnion(
      conversations.filter { conversation in
        guard let owner = conversation.owner else { return false }
        return archivedAgentOwners.contains { $0.id == owner.id && $0.channel == owner.channel }
      }.flatMap(\.replyIDs))
    if let selectedConversationID,
      !conversations.contains(where: { $0.id == selectedConversationID })
    {
      self.selectedConversationID = nil
    }
    resolveChannelConversation()
    let retained = Set(conversations.flatMap(\.replyIDs))
    viewedReplyIDs.formIntersection(retained)
    replyControlErrors = replyControlErrors.filter { retained.contains($0.key) }
    followNewlyReceivedReply(preservingControllerFocus: controllerPending)
    markVisibleConversationRead()
    onChannelPresentationChanged?()
    freshReplyPresentationRequested = false
  }

  /// Live controller capture keeps the canvas, including the moment before a
  /// stale open-channel paint is corrected. A reply that arrives here stays
  /// in the existing unread inventory and must not bump the capture's focus
  /// revision out from under `followDictationCapturePresentation`.
  private var activeCaptureOwnsPresentation: Bool {
    recording || warmingUp || transcribing
  }

  /// The mounted composer reports focus and typing into presentation metadata.
  /// Neither fact is a conversation, a draft, or a document.
  func noteComposerEditorActive(_ active: Bool) {
    composerEditorActive = active
  }

  func noteComposerTypingActivity() {
    composerTypingDeadline = nowProvider() + Self.composerTypingHorizon
  }

  /// Active editor holds without a clock, blank draft included. Typing holds
  /// until the monotonic deadline, using the same `remaining > 0` cut as
  /// auto-hide: the instant at `now == deadline` has elapsed.
  private var composerRetainsAutomaticReplyFocus: Bool {
    if composerEditorActive { return true }
    guard let composerTypingDeadline else { return false }
    return composerTypingDeadline - nowProvider() > 0
  }

  /// Bring one newly admitted reply's owner into the existing surface.
  /// History, a repeated id, playback or ACK of a known id, and an owner that
  /// is not unique stay where the user already is. Channel number, display
  /// name and reply text are not identity.
  private func followNewlyReceivedReply(preservingControllerFocus: Bool) {
    let current = Set(conversations.flatMap(\.replyIDs))
    let fresh = replyInventoryBaselined ? current.subtracting(observedReplyIDs) : []
    observedReplyIDs.formUnion(current)
    let establishingInventory = !replyInventoryBaselined
    replyInventoryBaselined = true
    // Inventory is already updated. A retained composer consumes the admission
    // here, so the horizon ending later cannot select that owner.
    guard !establishingInventory, !fresh.isEmpty, !preservingControllerFocus,
      !activeCaptureOwnsPresentation,
      !composerRetainsAutomaticReplyFocus,
      let conversation = ownedConversation(forNewReplies: fresh)
    else { return }
    if selectedConversationID == conversation.id {
      if isCollapsed && expandedByDefault { expandAgentSurface() }
    } else {
      selectConversation(conversation.id, expand: expandedByDefault)
    }
    // The selection callback above is an ordinary repaint. The snapshot's
    // own callback, still ahead, is the one that may ask the panel to show.
    freshReplyPresentationRequested = true
  }

  /// The reader places a causal broadcast reply on channel 0 and on its
  /// private owner. Focus follows the private owner id (provider, session,
  /// lease). Channel 0 is not a second owner, and two owners in one snapshot
  /// are not a guess.
  private func ownedConversation(forNewReplies fresh: Set<String>) -> OverlayConversation? {
    var match: OverlayConversation?
    for conversation in conversations {
      guard let conversationOwner = conversation.owner else { continue }
      let claimed = Set(
        conversation.messages.compactMap { message -> String? in
          guard message.kind == .reply, fresh.contains(message.id),
            let owner = message.owner,
            owner.id == conversationOwner.id,
            owner.provider == conversationOwner.provider,
            owner.providerSessionID == conversationOwner.providerSessionID
          else { return nil }
          return message.id
        })
      guard !claimed.isEmpty else { continue }
      guard claimed == fresh, match == nil else { return nil }
      match = conversation
    }
    return match
  }

  var conversationDrafts: [String: String] = [:]
  private(set) var pendingTextMessages: Set<String> = []
  private(set) var textMessageErrors: [String: String] = [:]
  private(set) var conversationFocusRevision: UInt64 = 0
  @ObservationIgnored var publishConversationText:
    @MainActor (OverlayConversationOwner, String) async throws -> Void = {
      owner, text in try await RealAgentBridgeInstaller.sendBusText(owner: owner, text: text)
    }

  func sendConversationText(_ conversation: OverlayConversation) async {
    guard let owner = conversation.owner, let draft = conversationDrafts[conversation.id],
      !draft.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty,
      !pendingTextMessages.contains(conversation.id)
    else { return }
    pendingTextMessages.insert(conversation.id)
    textMessageErrors.removeValue(forKey: conversation.id)
    defer { pendingTextMessages.remove(conversation.id) }
    do {
      try await publishConversationText(owner, draft)
      // A newer draft belongs to the Founder, even if publication finishes late.
      if conversationDrafts[conversation.id] == draft { conversationDrafts[conversation.id] = "" }
    } catch {
      textMessageErrors[conversation.id] = error.userFacingMessage
    }
  }

  func controlReply(_ message: OverlayConversationMessage, stop: Bool) async {
    guard message.kind == .reply, message.supportsSpeechPlayback,
      let owner = message.owner, let replyID = message.replyID
    else { return }
    let active = message.playback.map { ["waiting", "playing"].contains($0.state) } ?? false
    guard stop ? active : !active && !pendingReplyControls.contains(message.id) else { return }
    let ticket: String
    if stop, let playback = message.playback {
      ticket = playback.ticket
    } else {
      ticket = String(
        UUID().uuidString.replacingOccurrences(of: "-", with: "").lowercased().prefix(24))
    }
    pendingReplyControls.insert(message.id)
    replyControlErrors.removeValue(forKey: message.id)
    defer { pendingReplyControls.remove(message.id) }
    do {
      try await RealAgentBridgeInstaller.controlBusReply(
        replyID: replyID, ticket: ticket, provider: owner.provider,
        session: owner.providerSessionID, busPath: message.busPath, stop: stop)
    } catch {
      replyControlErrors[message.id] = error.userFacingMessage
    }
  }
  var visibleChannelRows: [OverlayChannelDelivery] {
    let deliveredDigits = Set(channelDelivery.map(\.channel))
    let boundWithoutDelivery = channelRosterNames.keys.sorted().compactMap {
      digit -> OverlayChannelDelivery? in
      guard !deliveredDigits.contains(digit), let audience = channelRosterNames[digit] else {
        return nil
      }
      return OverlayChannelDelivery(
        channel: digit, agent: audience, deliveryID: nil, stage: nil,
        isOpen: channelHudStates[digit]?.open ?? false)
    }
    return (channelDelivery + boundWithoutDelivery).filter { row in
      !archivedAgentOwners.contains { owner in
        let hud = channelHudStates[row.channel]
        return owner.channel == row.channel && owner.provider == hud?.provider
          && owner.providerSessionID == hud?.providerSessionID
      }
    }.sorted { $0.channel < $1.channel }
  }
  var hasOpenChannel: Bool {
    visibleChannelRows.contains { channel in
      channelHudStates[channel.channel]?.open ?? channel.isOpen
    }
  }
  /// The channel roster projects controller-owned capture without changing
  /// the dictation document's lifecycle or phase.
  var channelAudioCaptureActive: Bool {
    channelHudStates.values.contains(where: \.open)
  }

  /// Both agent destinations share one semantic accent. Viewing saved
  /// conversations alone never turns ordinary dictation into an agent mode.
  var usesAgentAccent: Bool {
    channelAudioCaptureActive || indicatorMode == .assistive
  }

  var audioCaptureActive: Bool {
    channelAudioCaptureActive
      || (recording && !finalized && !transcribing && !isFinalPass && mode == .listening)
  }
  @ObservationIgnored var onChannelPresentationChanged: (() -> Void)?
  @ObservationIgnored private var channelObservationTask: Task<Void, Never>?

  func observeChannelDelivery(using reader: OverlayChannelDeliveryReader) {
    channelObservationTask?.cancel()
    channelObservationTask = Task { @MainActor [weak self] in
      while !Task.isCancelled {
        guard self != nil else { return }
        if let snapshot = await self?.engine?.channelRosterSnapshot() {
          guard !Task.isCancelled, let self else { return }
          self.applyChannelRoster(snapshot)
        }
        do {
          let archiveRevision = self?.agentArchiveRevision
          let focusRevision = self?.conversationFocusRevision
          let selectedOwner = self?.selectedConversation?.owner
          let archivedOwner = selectedOwner.flatMap { owner in
            self?.archivedAgentOwners.first { $0.id == owner.id && $0.channel == owner.channel }
          }
          let snapshot = try await reader.readSnapshot(selectedOwner: archivedOwner)
          guard !Task.isCancelled, self != nil else { return }
          // A focus change can discard this paint, but not turn that archive's
          // first-read history into a fresh admission on the next poll.
          self?.observedReplyIDs.formUnion(snapshot.historyReplyIDs)
          if archiveRevision == self?.agentArchiveRevision,
            focusRevision == self?.conversationFocusRevision
          {
            self?.applyConversationSnapshot(snapshot)
          }
        } catch {
          guard !Task.isCancelled, let self else { return }
          if !channelStatusUnavailable {
            channelStatusUnavailable = true
            onChannelPresentationChanged?()
          }
        }
        await self?.refreshPlaybackMutes()
        do { try await Task.sleep(for: .milliseconds(500)) } catch { return }
      }
    }
  }

  func applyChannelRoster(_ snapshot: [CsChannelRosterState]) {
    let names = Dictionary(
      snapshot.map { ($0.channel, $0.audience) }, uniquingKeysWith: { _, latest in latest })
    let projected = Dictionary(
      snapshot.map { row in
        (
          row.channel,
          OverlayChannelHudProjection(
            open: row.open, loud: row.loud,
            autosealDeadline: row.autosealDeadlineUnixMs.map {
              Date(timeIntervalSince1970: Double($0) / 1_000)
            },
            followerAlive: row.followerAlive,
            provider: row.provider, providerSessionID: row.providerSessionId)
        )
      }, uniquingKeysWith: { _, latest in latest })
    guard snapshot != channelRoster else { return }
    let newlyOpened = snapshot.filter { row in
      row.open
        && !channelRoster.contains {
          $0.open && $0.channel == row.channel && $0.provider == row.provider
            && $0.providerSessionId == row.providerSessionId
        }
    }
    let wasOpen = hasOpenChannel
    let wasCapturingAudio = audioCaptureActive
    channelRoster = snapshot
    channelHudStates = projected
    channelRosterNames = names
    if wasCapturingAudio != audioCaptureActive {
      levelMeter.reset()
      hasMeasuredAudioLevel = false
    }
    if let pending = pendingChannelConversation,
      !snapshot.contains(where: {
        $0.open && $0.channel == pending.channel && $0.provider == pending.provider
          && $0.providerSessionId == pending.providerSessionId
      })
    {
      pendingChannelConversation = nil
    }
    if newlyOpened.count == 1, let row = newlyOpened.first {
      followChannelConversation(row)
    } else if newlyOpened.count > 1 {
      // Simultaneous recipients belong to the existing aggregate conversation.
      selectConversation(
        conversations.first { $0.channel == "0" }?.id, expand: expandedByDefault)
      if selectedConversationID == nil { showAgentMonitor(expand: expandedByDefault) }
    }
    if hasOpenChannel {
      cancelAutoHide()
    } else if wasOpen && terminal {
      restartAutoHideCountdown()
    }
    onChannelPresentationChanged?()
  }

  func toggleAgentChannel(_ digit: UInt8) async {
    let channel = String(digit)
    guard (0...9).contains(digit), let engine,
      !pendingChannelToggles.contains(channel)
    else { return }
    pendingChannelToggles.insert(channel)
    defer { pendingChannelToggles.remove(channel) }
    do {
      try await engine.toggleAgentChannel(digit: digit)
      channelToggleError = nil
      applyChannelRoster(await engine.channelRosterSnapshot())
    } catch {
      channelToggleError = error.userFacingMessage
    }
  }

  func applyChannelDelivery(_ snapshot: [OverlayChannelDelivery]) {
    guard snapshot != channelDelivery || channelStatusUnavailable else { return }
    let wasOpen = hasOpenChannel
    channelDelivery = snapshot
    channelStatusUnavailable = false
    if hasOpenChannel {
      cancelAutoHide()
    } else if wasOpen && terminal {
      restartAutoHideCountdown()
    }
    onChannelPresentationChanged?()
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
    case .listening: return String(localized: "listening")
    case .finalizing: return String(localized: "finalizing")
    case .formatted: return String(localized: "formatted")
    // Never "formatted": the pill is the first thing a user reads, and the
    // one word it must not say about a refused take is the word that means
    // sealed.
    case .coverageRefused: return coverageRefusalCopy.status
    case .noSpeech: return String(localized: "no speech")
    case .error: return String(localized: "error", comment: "Overlay status pill: the take failed")
    }
  }

  /// Only a reducer-projected listening phase may ripple.
  var statusRippling: Bool {
    mode == .listening
      && (audioReady || vadActive)
  }

  /// The header status light, or nil when no take is live. Projection only:
  /// phase from the reducer, mode from the tray feed, silence from measured
  /// capture level.
  var recordingLight: OverlayRecordingLight? {
    // A channel owns live capture independently of the ordinary take's phase.
    if channelAudioCaptureActive { return .agent }
    return OverlayRecordingLight.resolve(
      mode: mode, terminal: terminal, recording: recording, transcribing: transcribing,
      indicatorMode: indicatorMode, silent: levelMeter.isSilent)
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

  /// Presentation activity only; capture and reducer receipts retain ownership.
  /// A stopped capture can still carry a listening projection until its seal.
  var animatesTranscriptCaret: Bool {
    !isCollapsed && !isEditingTranscript && !terminal && presentationStatus == nil
      && recording && (statusRippling || (mode == .finalizing && transcribing))
  }

  /// Frozen durations stay readable without scheduling another render tick.
  var sessionTimerPaused: Bool {
    !showsSessionTimer || captureEndedAtUptime != nil
  }

  /// Exact engine text shared by the canvas, sizing, copy, and delivery.
  var activeText: String {
    archivedTranscript?.text ?? formattedText
  }

  /// Human review depends on capture completion, not the engine's seal,
  /// warning, phase label or a revision request in flight. Typing stays local.
  var isTranscriptEditable: Bool {
    if archivedTranscript != nil {
      return !archiveActionPending && !recording && !warmingUp && !transcribing
    }
    return finalized && latestTranscriptProjection != nil
      && !recording && !warmingUp && !transcribing
  }

  /// Engine state changes cannot hide or discard an unsaved human draft.
  var isRevisionDraftDirty: Bool {
    if let archivedTranscript {
      return !archiveDraft.utf8.elementsEqual(archivedTranscript.text.utf8)
    }
    return isProjectedDraftDirty
  }

  /// The projected take's own unsaved edit, independent of a reopened archive.
  private var isProjectedDraftDirty: Bool {
    latestTranscriptProjection != nil && !revisionDraft.utf8.elementsEqual(formattedText.utf8)
  }

  /// Bytes painted on the canvas: the local draft while a completed take is
  /// under review or awaiting its ledger projection, the Rust projection
  /// otherwise. Delivery never reads this; it reads `activeText`.
  var canvasText: String {
    if let archivedTranscript {
      return isRevisionDraftDirty ? archiveDraft : archivedTranscript.text
    }
    return isRevisionDraftDirty ? revisionDraft : formattedText
  }

  /// Uncertain words painted on the canvas. The spans index into
  /// `formattedText`, so a dirty local draft (or any canvas text that is not
  /// the projection) invalidates them all — the renderer drops the paint
  /// rather than color the wrong bytes. `coverage_refused` / `degraded` never
  /// create spans upstream, so they can never create color here.
  var canvasUncertainWords: [OverlayUncertainWord] {
    guard archivedTranscript == nil, !isRevisionDraftDirty else { return [] }
    return OverlayUncertainWordProjection.project(spans: uncertainSpans, text: formattedText)
  }

  // MARK: Uncertain word actions (A6-3)

  /// The word clip currently playing; a new Play stops the previous one.
  private var uncertainWordSound: NSSound?

  /// Play the word's own PCM: the span pins the occurrence, the bridge cuts
  /// the clip with ±150 ms of context. Absent retained audio is an honest
  /// footer notice, never silence.
  func playUncertainWord(_ word: OverlayUncertainWord) {
    guard let engine else { return }
    uncertainWordSound?.stop()
    uncertainWordSound = nil
    do {
      let clip = try engine.wordAudioClip(
        sessionId: word.occurrenceSessionId,
        captureEpoch: word.occurrenceCaptureEpoch,
        sampleStart: word.slotSampleStart,
        sampleEnd: word.slotSampleEnd,
        padMs: 150
      )
      guard let sound = NSSound(contentsOfFile: clip, byReference: true) else {
        showFooterNotice(String(localized: "Word audio unavailable"))
        return
      }
      uncertainWordSound = sound
      sound.play()
    } catch {
      showFooterNotice(String(localized: "Word audio unavailable"))
    }
  }

  /// Teach the dictionary through the existing `quality_teach_span` path —
  /// no parallel mechanism. The acknowledgement comes from the quality core.
  func teachUncertainWord(_ word: OverlayUncertainWord, canonical: String) {
    guard let engine else { return }
    let trimmed = canonical.trimmingCharacters(in: .whitespacesAndNewlines)
    guard !trimmed.isEmpty else { return }
    do {
      let result = try engine.teachSpan(
        variant: word.word, canonical: trimmed, kind: "lexicon_corrected")
      showFooterNotice(result.acknowledgement)
    } catch {
      showFooterNotice(String(localized: "Could not save the correction"))
    }
  }

  /// Read-only words Rust keeps visible without mutation authority — for
  /// example a Whisper alternative the ledger refused as a whole-span
  /// replacement — in PCM order. They ride the capture-bound compact paint,
  /// never `formattedText`, so no canvas, copy, or delivery path reads them.
  /// Capture completion does not settle alternatives. Keep the same capture's
  /// evidence available for review until Rust removes it or a new capture resets paint.
  var liveEvidence: [CsUnanchoredEvidence] {
    guard let paint = compactProjection,
      !retiredProjectionSessions.contains(paint.sessionId)
    else { return [] }
    if let document = latestTranscriptProjection {
      guard document.sessionId == paint.sessionId,
        document.captureEpoch == 0 || document.captureEpoch == paint.captureEpoch
      else { return [] }
    }
    return paint.evidence
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
      return String(
        localized:
          "The handover came back. These words are retained here — recover them before the next take."
      )
    }
    if latestTranscriptProjection?.sealCoverage?.status == .complete {
      return String(
        localized:
          "Acoustic coverage was measured as complete, but this transcript has no current terminal seal."
      )
    }
    return String(
      localized: "No seal was recorded for this take, so nothing here is certified complete.")
  }

  /// Availability is refreshed at events and action opening, never during paint.
  /// It grants no command: the reducer's action bits still own permission.
  var currentTakeRecoveryDetail: String {
    if currentTakeAudioAvailable {
      if terminal && canRetranscribe && !isRevisionDraftDirty {
        return String(
          localized:
            "Audio for this take is available. Use Transcribe this take again in More actions.")
      }
      return String(
        localized: "Audio for this take is available, but retranscription is not available here.")
    }
    return String(
      localized:
        "Audio availability for this take could not be confirmed here. You can start a new take.")
  }

  var audioLevelAccessibilityValue: String {
    guard let gain = levelMeter.gain else {
      return String(localized: "Waiting for measured level")
    }
    switch gain {
    case ..<0.12: return String(localized: "Very quiet", comment: "Measured microphone level")
    case ..<0.35: return String(localized: "Quiet", comment: "Measured microphone level")
    case ..<0.68: return String(localized: "Good level", comment: "Measured microphone level")
    default: return String(localized: "Strong level", comment: "Measured microphone level")
    }
  }

  // MARK: Recording lifecycle (engine-backed; no-op when engine is absent)

  /// Start through the same controller and current mode snapshot as the tray.
  /// Preparing synchronously reserves this take against a second click.
  func start(language: CsLanguage? = nil) {
    guard let engine, !recording,
      terminal || (!transcribing && mode != .finalizing && captureStartedAtUptime == nil)
    else {
      return
    }
    let assistive = engine.startsInAssistiveMode()
    prepareForExternalStart()
    Task { @MainActor in await self.runStart(assistive: assistive, language: language) }
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

  private func runStart(assistive: Bool, language: CsLanguage?) async {
    guard let engine else { return }
    guard micAccessProvider() else {
      presentTerminalError(
        message: String(
          localized:
            "Microphone access is off for Codescribe. Enable it in System Settings › Privacy & Security › Microphone."
        ),
        toast: String(localized: "Microphone access denied")
      )
      return
    }
    engine.setListener(listener)
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
      try await engine.startRecording(assistive: assistive, language: language)
    } catch {
      await handleStartFailure(error, assistive: assistive, language: language)
    }
  }

  /// A start failure caused by an undetermined Speech Recognition grant is
  /// recoverable in place: fire the TCC dialog from the main app process (so
  /// the grant lands on the app's identity, which the bridge child inherits)
  /// and retry the start once when authorized. Every other failure — and a
  /// declined dialog — funnels into the terminal error path, where
  /// `speechAuthNotice` rewrites raw `speech_auth_*` markers.
  private func handleStartFailure(_ error: Error, assistive: Bool, language: CsLanguage?) async {
    let described = "\(error)"
    if described.contains("speech_auth_not_determined"), !speechAuthRequestAttempted {
      speechAuthRequestAttempted = true
      abortRecordingSession()
      let state = await SpeechRecognitionPermission.request()
      if state == .granted {
        prepareForExternalStart()
        await runStart(assistive: assistive, language: language)
        return
      }
    }
    presentTerminalError(
      message: String(
        localized: "Couldn't start recording: \(described)",
        comment: "The placeholder is the engine's own failure text"),
      toast: String(localized: "Couldn't start recording")
    )
  }

  private func runStop() async {
    guard let engine else { return }
    let generation = captureGeneration
    // Prevent duplicate stops while Rust emits authoritative finalizing and
    // terminal projections. This flag never paints a phase.
    transcribing = true
    warmingUp = false
    freezeCaptureClock()
    levelMeter.reset()
    do {
      // Stop acknowledges lifecycle; transcript projections own the text.
      _ = try await engine.stopRecording()
      guard generation == captureGeneration else { return }
      finishControllerRecording()
    } catch {
      guard generation == captureGeneration else { return }
      presentTerminalError(
        message: String(
          localized: "Couldn't finalize transcript: \(String(describing: error))",
          comment: "The placeholder is the engine's own failure text"),
        toast: String(localized: "Couldn't finalize transcript")
      )
    }
  }

  // MARK: Action row

  /// Thin relay from overlay controls into the controller routes. Start reserves
  /// capture locally; transcript phase and text still come from Rust projections.
  func relayIntent(_ intent: OverlayIntent) {
    switch intent {
    case .startRecording:
      start()
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
    case .undoFormat:
      undoArchivedRevision()
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
      presentActionFailure(
        String(localized: "Copy needs the recording engine"),
        notice: String(localized: "copy unavailable"))
      return
    }
    let text = activeText
    Task { @MainActor in
      do {
        try await engine.copyTaggedTranscript(text: text)
        self.showFooterNotice(String(localized: "copied"))
      } catch {
        self.presentActionFailure(
          String(
            localized: "Couldn't copy transcript: \(String(describing: error))",
            comment: "The placeholder is the engine's own failure text"),
          notice: String(localized: "copy failed"))
      }
    }
  }

  private func relayInsertPasteIntent() {
    if engine == nil {
      presentActionFailure(
        String(localized: "Insert needs the recording engine"),
        notice: String(localized: "insert unavailable"))
    }
    guard let engine else { return }
    captureQualityIfEdited(action: "paste")
    cancelAutoHide()
    showFooterNotice(String(localized: "inserting…"), persists: true)
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
          self.showFooterNotice(String(localized: "copied"))
        case .accessibilityPermissionNeeded:
          self.showFooterNotice(
            String(localized: "no ax", comment: "Footer notice: no Accessibility permission"))
        case .pasteRequested:
          self.showFooterNotice(String(localized: "Paste requested"))
        case .noop:
          self.showFooterNotice(String(localized: "no insert"))
        }
      } catch {
        self.errorMessage = String(
          localized: "Couldn't paste transcript: \(String(describing: error))",
          comment: "The placeholder is the engine's own failure text")
        self.showFooterNotice(String(localized: "no paste"))
      }
    }
  }

  /// Startup diagnostics are projections of the Rust owners, independent of
  /// the current take. Starting a new recording must not hide storage failure.
  func setMaxPreparationError(_ message: String?) {
    maxPreparationError = message == nil ? nil
      : String(localized: "Max could not prepare its conversation.")
  }

  func setTranscriptStorageError(_ message: String?) {
    transcriptStorageError = message == nil ? nil
      : String(localized: "Transcript history is unavailable. Your text remains visible.")
  }

  var completedMaxConsultationID: String? {
    guard terminal, let receipt = latestTranscriptProjection?.consultationPresentations.last,
      !receipt.consultationId.isEmpty
    else { return nil }
    return receipt.consultationId
  }

  func continueMaxConsultationInChat() {
    guard let backendID = completedMaxConsultationID else { return }
    if onContinueMaxConsultation?(backendID) != true {
      showFooterNotice(String(localized: "This consultation is unavailable in chat."))
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
    if archivedTranscript != nil {
      retranscribeArchivedTranscript(pass: pass)
      return
    }
    guard let engine else {
      presentActionFailure(
        String(localized: "Retranscription needs the recording engine"),
        notice: String(localized: "retranscribe unavailable"))
      return
    }
    guard let projection = latestTranscriptProjection else {
      presentActionFailure(
        String(localized: "The visible take has no session identity"),
        notice: String(localized: "retranscribe unavailable"))
      return
    }
    guard let path = engine.sessionAudioPath(sessionId: projection.sessionId) else {
      presentActionFailure(
        String(localized: "The visible take's audio is unavailable"),
        notice: String(localized: "take audio unavailable"))
      return
    }
    let prefixedPath = "\(pass.pathPrefix)\(path)"
    let generation = captureGeneration

    cancelAutoHide()
    let passEngine =
      pass == .cloud
      ? String(localized: "cloud", comment: "Retranscribe pass: the cloud engine")
      : String(localized: "local Whisper HQ", comment: "Retranscribe pass: the local engine")
    let previousChip = engineChip
    let running = String(
      localized: "retranscribing · \(passEngine)",
      comment: "The placeholder is the name of the engine running the pass")
    engineChip = running
    engineChipLatched = true
    showFooterNotice(running, persists: true)
    Task { @MainActor [weak self] in
      guard let self, generation == self.captureGeneration else { return }
      do {
        let result = try await engine.transcribeTake(
          sessionId: projection.sessionId, path: prefixedPath)
        guard generation == self.captureGeneration,
          self.latestTranscriptProjection?.sessionId == projection.sessionId
        else { return }
        let text = result.text.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !text.isEmpty else {
          self.engineChip = previousChip
          self.presentActionFailure(
            String(
              localized: "The \(passEngine) pass returned no text",
              comment: "The placeholder is the name of the engine that ran the pass"),
            notice: String(localized: "retranscribe returned no text"))
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
            guard generation == self.captureGeneration,
              self.latestTranscriptProjection?.sessionId == projection.sessionId
            else { return }
            if replaced != text,
              !replaced.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
            {
              self.retranscribeRollback = OverlayRetranscribeRollback(
                sessionId: projection.sessionId, renderedText: replaced)
            }
          } else {
            self.engineChip = previousChip
            self.presentActionFailure(
              String(localized: "A newer take replaced this overlay"),
              notice: String(localized: "take changed"))
            return
          }
          if self.mode == .noSpeech {
            self.mode = .formatted
          }
        }
        self.engineChip = passEngine
        self.errorMessage = nil
        self.errorDiagnosticDetail = nil
        self.showFooterNotice(
          self.retranscribeRollback == nil
            ? String(localized: "retranscribed")
            : String(localized: "retranscribed — Back keeps the old text"))
        self.restartAutoHideCountdown()
      } catch {
        guard generation == self.captureGeneration,
          self.latestTranscriptProjection?.sessionId == projection.sessionId
        else { return }
        self.engineChip = previousChip
        let described = String(describing: error)
        self.presentActionFailure(
          String(localized: "Couldn't transcribe this take again"),
          notice: String(localized: "Retranscription failed"))
        self.errorDiagnosticDetail = described
        self.errorFooterSummary = String(localized: "Retranscription failed")
        self.errorLifecycleDetail =
          self.activeText.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
          ? String(localized: "No new transcript was produced for this take.")
          : String(localized: "The existing transcript is still here.")
        self.refreshRetranscriptionAvailability()
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
    if archivedTranscript != nil { return archivedUndoIntent == .undoRetranscribe }
    guard let rollback = retranscribeRollback else { return false }
    guard let sessionId = rollback.sessionId else { return true }
    return latestTranscriptProjection?.sessionId == sessionId
  }

  /// Restore the pre-retranscribe text as a NEW user revision on the same
  /// session — no history rewrite, no forged seal; the reducer keeps both
  /// texts in its revision chain. The rollback slot is consumed only when the
  /// restore actually landed.
  func undoRetranscribeIntent() {
    if archivedTranscript != nil {
      undoArchivedRevision()
      return
    }
    guard let rollback = retranscribeRollback else { return }
    guard let sessionId = rollback.sessionId else {
      revisionDraft = rollback.renderedText
      retranscribeRollback = nil
      showFooterNotice(String(localized: "retranscribe undone"))
      return
    }
    guard let engine else {
      presentActionFailure(
        String(localized: "Undo needs the recording engine"),
        notice: String(localized: "undo unavailable"))
      return
    }
    guard let projection = latestTranscriptProjection, projection.sessionId == sessionId else {
      retranscribeRollback = nil
      presentActionFailure(
        String(localized: "The retranscribed take is no longer current"),
        notice: String(localized: "nothing to undo"))
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
        self.showFooterNotice(String(localized: "retranscribe undone"))
      } catch {
        self.presentActionFailure(
          String(
            localized: "Couldn't undo retranscribe: \(String(describing: error))",
            comment: "The placeholder is the engine's own failure text"),
          notice: String(localized: "undo failed — kept"))
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
    if archivedTranscript != nil { return sendArchivedTranscriptToAgent() }
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

  func close() {
    invalidateHistoryOpens()
    // An unsaved archive edit is retained for explicit recovery before the
    // canvas lets go of it; closing is not a decision to drop it.
    leaveArchivedTranscript()
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
    onCloseIntent?()
  }

  func refreshRetranscriptionAvailability() {
    cloudRetranscribeConfigured = engine?.cloudRetranscribeConfigured() ?? false
    currentTakeAudioAvailable =
      latestTranscriptProjection.map {
        engine?.sessionAudioPath(sessionId: $0.sessionId) != nil
      } ?? false
  }

  private func refreshOverlayPolicyTruth() {
    refreshRetranscriptionAvailability()
    guard let truth = engine?.currentOverlayPolicy() else { return }
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
    // An archive has no delivered take to compare against; its text must
    // never be recorded as a correction of the take behind it.
    guard archivedTranscript == nil, mode == .formatted else { return }
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
        input: CsOverlayCorrectionInput(
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
    if isRevisionDraftDirty && showsMyDictation {
      scheduleRevisionCommitAfterFocusExit()
    } else if terminal {
      restartAutoHideCountdown()
    }
  }

  /// Canvas bytes changed under the user's caret.
  func updateRevisionDraft(_ text: String) {
    guard isTranscriptEditable else { return }
    if archivedTranscript != nil {
      archiveDraft = text
      noteRevisionDraftActivity()
      return
    }
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
      guard !Task.isCancelled, self?.showsMyDictation == true else { return }
      self?.commitRevisionDraft()
    }
  }

  func discardRevisionDraft() {
    revisionFocusCommitTask?.cancel()
    revisionFocusCommitTask = nil
    guard !revisionCommitPending, !formatterCommitPending else { return }
    if let archivedTranscript {
      archiveDraft = archivedTranscript.text
      revisionCommitError = nil
      if !isEditingTranscript { restartAutoHideCountdown() }
      return
    }
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
    guard isTranscriptEditable, isRevisionDraftDirty, !revisionCommitPending,
      !formatterCommitPending
    else {
      return
    }
    if archivedTranscript != nil {
      commitArchivedDraft()
      return
    }
    guard terminal, latestTranscriptProjection?.terminal == true else {
      revisionCommitError = "The take has no terminal document revision — your draft is kept"
      return
    }
    let proposed = revisionDraft
    guard !proposed.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else {
      revisionCommitError = String(localized: "A transcript revision cannot be empty")
      return
    }
    guard let projection = latestTranscriptProjection, let engine else {
      revisionCommitError = String(localized: "Transcript revision authority is unavailable")
      return
    }
    revisionCommitPending = true
    revisionCommitError = nil
    pendingRevisionSessionId = projection.sessionId
    pendingRevisionSource = projection.reducerRevision
    pendingRevisionDraft = proposed
    revisionRequestGeneration &+= 1
    let requestGeneration = revisionRequestGeneration
    cancelAutoHide()
    Task { @MainActor [weak self] in
      guard let self else { return }
      do {
        let receipt = try await engine.commitUserRevision(
          sessionId: projection.sessionId,
          sourceRevision: projection.reducerRevision,
          renderedText: proposed
        )
        guard revisionRequestGeneration == requestGeneration, revisionCommitPending,
          pendingRevisionSessionId == projection.sessionId,
          pendingRevisionSource == projection.reducerRevision,
          latestTranscriptProjection?.sessionId == projection.sessionId
        else { return }
        guard receipt.sessionId == projection.sessionId,
          receipt.sourceRevision == projection.reducerRevision,
          receipt.revision > receipt.sourceRevision,
          receipt.renderedText == proposed,
          receipt.provenanceReceipt.hasPrefix("user-edit-")
        else {
          revisionCommitPending = false
          pendingRevisionSessionId = nil
          pendingRevisionSource = nil
          pendingRevisionDraft = nil
          revisionCommitError = String(localized: "Transcript revision receipt was inconsistent")
          return
        }
        // The callback can arrive before this acknowledgement. Either way,
        // projection — never this receipt — owns the visible state transition.
      } catch {
        guard revisionRequestGeneration == requestGeneration, revisionCommitPending,
          pendingRevisionSessionId == projection.sessionId,
          pendingRevisionSource == projection.reducerRevision,
          latestTranscriptProjection?.sessionId == projection.sessionId
        else { return }
        revisionCommitPending = false
        pendingRevisionSessionId = nil
        pendingRevisionSource = nil
        pendingRevisionDraft = nil
        revisionCommitError = String(
          localized: "Couldn't commit transcript revision: \(String(describing: error))",
          comment: "The placeholder is the engine's own failure text")
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
      refreshOverlayPolicyTruth()
      onRecordingPreparing?()
      return
    }
    agentSessionArmed = indicatorMode == .assistive
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
      applyPreferredExpansion(forTake: true)
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
      applyPreferredExpansion(forTake: true)
    }
    if captureStartedAtUptime == nil {
      beginCaptureClock()
    }
    recording = true
    if !captureDidStart {
      followDictationCapturePresentation()
    }
    captureDidStart = true
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
    if !channelAudioCaptureActive {
      levelMeter.reset()
      hasMeasuredAudioLevel = false
    }

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
    if !channelAudioCaptureActive {
      levelMeter.reset()
      hasMeasuredAudioLevel = false
    }
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

  /// The "Transcription Overlay" preference as the panel's owner last read it
  /// (take start, status card, preference write). A plain value, never a
  /// settings read: the countdown runs on hover-out and every terminal paint.
  /// The pin keeps the transcript overlay between takes; with the preference
  /// off there is no such overlay, so a status card shown despite it leaves on
  /// the ordinary countdown instead of staying until closed by hand.
  @ObservationIgnored var transcriptOverlayEnabled = true

  private var pinKeepsOverlayVisible: Bool {
    keepVisibleBetweenTakes && transcriptOverlayEnabled
  }

  private var mayAutoSendRefusedAgentTake: Bool {
    agentSessionArmed && agentFinalTranscriptAppeared && !agentAutoSendCancelled
      && canSendToAgent
      && !formattedText.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
  }

  private func restartAutoHideCountdown() {
    if hasOpenChannel {
      cancelAutoHide()
      return
    }
    if pinKeepsOverlayVisible && !agentSessionArmed {
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
    if pinKeepsOverlayVisible && !agentSessionArmed {
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
    let text = latestTranscriptProjection?.renderedText ?? activeText
    // No `agentSessionArmed` here: the explicit Send button is live for
    // every terminal overlay (dictation and formatting included), and the
    // controller falls back to the session trigger context when no
    // assistive context was armed (review P0-03). Auto-send remains gated
    // on the armed latch by its caller.
    guard !agentDeliveryStarted,
      !text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty, let engine
    else { return nil }
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
          showToast(String(localized: "Agent delivery is no longer available"))
        }
      } catch {
        guard generation == captureGeneration else { return }
        agentDeliveryStarted = false
        showToast(String(localized: "Couldn't send to Agent"))
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
      errorMessage = String(localized: "Transcription interrupted")
      errorDiagnosticDetail = message
      errorFooterSummary = String(localized: "Transcription incomplete")
      errorLifecycleDetail = String(
        localized: "The text received so far is still here. It may be incomplete.")
      refreshRetranscriptionAvailability()
      showToast(errorFooterSummary)
      return
    }
    presentTerminalError(
      message: message, toast: String(localized: "Couldn't start recording"))
  }

  /// User-facing rewrite for Speech Recognition TCC failures. The engine
  /// reports raw bridge markers (`speech_auth_not_determined` / `_denied` /
  /// `_restricted`); surfacing those verbatim reads as a crash, when the fix
  /// is one System Settings toggle. Returns nil for every other error.
  static func speechAuthNotice(from message: String) -> String? {
    guard message.contains("speech_auth_") else { return nil }
    // One sentence per branch: a translation must be free to reorder it.
    if message.contains("speech_auth_not_determined") {
      return String(
        localized:
          "Apple dictation needs Speech Recognition access — grant it in Settings › Dictation or System Settings › Privacy & Security › Speech Recognition"
      )
    }
    if message.contains("speech_auth_denied") || message.contains("speech_auth_restricted") {
      return String(
        localized:
          "Speech Recognition is off for Codescribe — enable it in System Settings › Privacy & Security › Speech Recognition"
      )
    }
    return String(
      localized:
        "Speech Recognition access is unavailable — check System Settings › Privacy & Security › Speech Recognition"
    )
  }

  private func presentTerminalError(message: String, toast: String) {
    let captureHadStarted = captureDidStart
    let speechNotice = OverlayState.speechAuthNotice(from: message)
    let headline =
      speechNotice
      ?? (captureHadStarted
        ? String(localized: "Couldn't finish transcription") : toast)
    abortRecordingSession()
    pendingNoSpeechMessage = nil
    noSpeechNotice = OverlayState.defaultNoSpeechNotice
    isFinalPass = false
    errorMessage = headline
    errorDiagnosticDetail = message
    errorFooterSummary =
      captureHadStarted
      ? String(localized: "Transcription failed") : String(localized: "Recording failed")
    errorLifecycleDetail =
      captureHadStarted
      ? String(localized: "Recording started, but transcription did not finish.")
      : String(localized: "Recording did not start.")
    finalized = true
    refreshRetranscriptionAvailability()
    showToast(speechNotice ?? errorFooterSummary)
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
    // A sibling status must keep the retained document's draft and capture
    // completion. Reset transient presentation only when no document is held.
    abortRecordingSession(resetTranscript: latestTranscriptProjection == nil)
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
    if event.terminal { finalized = true }
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
    guard !projection.sessionId.isEmpty, projection.captureEpoch > 0,
      !retiredProjectionSessions.contains(projection.sessionId)
    else { return }
    if let document = latestTranscriptProjection {
      guard document.sessionId == projection.sessionId,
        document.captureEpoch == 0 || document.captureEpoch == projection.captureEpoch
      else { return }
    }
    if let current = compactProjection {
      guard current.sessionId == projection.sessionId,
        current.captureEpoch == projection.captureEpoch,
        projection.sequence > current.sequence
      else { return }
    } else {
      guard projection.sequence == 1 else { return }
    }
    if terminal || endedProjectionSessions.contains(projection.sessionId) {
      // Only the current terminal's capture can update review evidence. An
      // epoch-free lifecycle needs an already bound compact capture; it cannot
      // admit a new one. These snapshots never reopen recording or delivery.
      guard let document = latestTranscriptProjection, document.terminal,
        document.sessionId == projection.sessionId,
        compactProjection != nil || document.captureEpoch == projection.captureEpoch
      else { return }
    } else {
      guard recording || transcribing else { return }
    }
    compactProjection = projection
    onTranscriptPresentationChanged?()
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
    // A late revision of the take behind a reopened archive updates that take
    // only; any other projection takes the canvas back.
    let keepsArchive =
      archivedTranscript != nil && !lifecycleTerminal
      && latestTranscriptProjection?.sessionId == projection.sessionId
    if !keepsArchive { leaveArchivedTranscript() }
    defer {
      if keepsArchive {
        chromeBehindArchive = projectedChrome
        paintArchivedChrome()
      }
    }
    let priorProjection = latestTranscriptProjection
    let isNewSession = priorProjection?.sessionId != projection.sessionId
    let draftWasDirty = isProjectedDraftDirty
    // A text snapshot can arrive before its compact snapshot. Keep same-capture
    // evidence through seals and terminals, but never carry foreign capture paint.
    if let paint = compactProjection,
      paint.sessionId != projection.sessionId
        || (projection.captureEpoch > 0 && paint.captureEpoch != projection.captureEpoch)
    {
      if paint.sessionId != projection.sessionId {
        retiredProjectionSessions.insert(paint.sessionId)
      }
      compactProjection = nil
    }
    let revisionReceipt: String?
    if let receipt = projection.documentRevisionReceipt {
      let matchesCapture: Bool
      if let epoch = receipt.captureEpoch {
        matchesCapture =
          epoch > 0 && epoch == projection.captureEpoch
          && receipt.captureReceiptId?.isEmpty == false
      } else {
        matchesCapture = !projection.acousticReceipts.isEmpty
      }
      revisionReceipt =
        receipt.provenance == .userEdit
          && receipt.sessionId == projection.sessionId
          && receipt.sourceRevision < UInt64.max
          && receipt.sourceRevision + 1 == receipt.revision
          && receipt.revision == projection.reducerRevision
          && receipt.renderedText.utf8.elementsEqual(projection.renderedText.utf8)
          && !receipt.receiptId.isEmpty && matchesCapture
        ? receipt.receiptId : nil
    } else {
      revisionReceipt = projection.acousticReceipts
        .compactMap(\.manualEditReceipt)
        .first(where: { $0.hasPrefix("user-edit-") })
    }
    let formatterReceipt =
      projection.reducerAction == "derived_projection"
        && projection.label.hasPrefix("formatter-") ? projection.label : nil
    let completesPendingRevision =
      revisionCommitPending
      && projection.reducerAction == "apply_manual_edit"
      && projection.terminal
      && projection.sessionId == pendingRevisionSessionId
      && projection.reducerRevision > (pendingRevisionSource ?? UInt64.max)
      && (projection.documentRevisionReceipt == nil
        || projection.documentRevisionReceipt?.sourceRevision == pendingRevisionSource)
      && revisionReceipt != nil
    let completesPendingFormatter =
      formatterCommitPending
      && projection.reducerAction == "derived_projection"
      && projection.terminal
      && projection.sessionId == pendingRevisionSessionId
      && projection.reducerRevision == pendingRevisionSource
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
      if let priorProjection {
        // An unannounced different session cannot inherit the outgoing
        // capture's completion. Its own lifecycle must prove it ended.
        finalized = false
        retiredProjectionSessions.insert(priorProjection.sessionId)
        retainSupersededTake(priorProjection, draftWasDirty: draftWasDirty)
      }
      deliveredText = ""
      deliveredTextSessionId = nil
      qualityCapturedProvenance = nil
      userRevisionProvenance = nil
      revisionCommitPending = false
      formatterCommitPending = false
      pendingRevisionSessionId = nil
      pendingRevisionSource = nil
      pendingRevisionDraft = nil
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
    if projection.reducerAction == "light_plus_tick_deadline" {
      showFooterNotice(projection.label)
    }
    // Mirror this accepted projection; a successor or a later document verdict
    // cannot inherit a notice belonging to its predecessor.
    coverageRefusalNotice =
      projection.terminal && projection.phase == OverlayMode.coverageRefused.rawValue
      ? coverageRefusalCopy.notice : nil
    if !projection.terminal, !finalized {
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
    if projection.terminal { refreshRetranscriptionAvailability() }
    // A document observation cannot consume a stopped callback or reopen capture.
    if isLifecycleTerminal { finalized = true }

    userRevisionProvenance = revisionReceipt
    if completesPendingRevision {
      revisionCommitPending = false
      pendingRevisionSessionId = nil
      pendingRevisionSource = nil
      revisionCommitError = nil
      if let pendingRevisionDraft,
        revisionDraft.utf8.elementsEqual(pendingRevisionDraft.utf8)
      {
        revisionDraft = formattedText
      }
      pendingRevisionDraft = nil
    } else if completesPendingFormatter {
      formatterCommitPending = false
      pendingRevisionSessionId = nil
      pendingRevisionSource = nil
      pendingRevisionDraft = nil
      formatterError = nil
      if !draftWasDirty { revisionDraft = formattedText }
      showFooterNotice(String(localized: "formatted"))
      loadDocumentHistory()
    } else if !draftWasDirty || isNewSession {
      revisionDraft = formattedText
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
          showFooterNotice(String(localized: "copied"))
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
        text, sessionID: projection.sessionId,
        notice: String(localized: "no composer receiver"),
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
        if affectsCurrentCapture {
          showFooterNotice(String(localized: "kept in composer recovery"))
        }
        return
      }
      retainComposerDelivery(
        refused, sessionID: projection.sessionId,
        notice: String(localized: "kept for recovery"),
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
    if archivedTranscript != nil {
      formatArchivedTranscript(level: level)
      return
    }
    guard mode == .formatted || mode == .coverageRefused, terminal, canFormat,
      !isRevisionDraftDirty,
      !revisionCommitPending, !formatterCommitPending
    else { return }
    guard let projection = latestTranscriptProjection, let engine else {
      formatterError = String(localized: "Transcript formatter authority is unavailable")
      showFooterNotice(String(localized: "format unavailable"))
      return
    }
    formatterCommitPending = true
    formatterError = nil
    pendingRevisionSessionId = projection.sessionId
    pendingRevisionSource = projection.reducerRevision
    pendingRevisionDraft = revisionDraft
    revisionRequestGeneration &+= 1
    let requestGeneration = revisionRequestGeneration
    cancelAutoHide()
    showFooterNotice(String(localized: "formatting…"), persists: true)
    Task { @MainActor [weak self] in
      guard let self else { return }
      do {
        let receipt = try await engine.commitFormatterRevision(
          sessionId: projection.sessionId,
          sourceRevision: projection.reducerRevision,
          level: level
        )
        guard revisionRequestGeneration == requestGeneration, formatterCommitPending,
          pendingRevisionSessionId == projection.sessionId,
          pendingRevisionSource == projection.reducerRevision,
          latestTranscriptProjection?.sessionId == projection.sessionId
        else { return }
        guard receipt.sessionId == projection.sessionId,
          receipt.sourceRevision == projection.reducerRevision,
          receipt.revision > receipt.sourceRevision,
          !receipt.renderedText.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty,
          receipt.provenanceReceipt.hasPrefix("formatter-")
        else {
          formatterCommitPending = false
          pendingRevisionSessionId = nil
          pendingRevisionSource = nil
          pendingRevisionDraft = nil
          formatterError = String(localized: "Formatter revision receipt was inconsistent")
          showFooterNotice(String(localized: "format failed"))
          restartAutoHideCountdown()
          return
        }
        // Projection can arrive before acknowledgement. It is still the only
        // path that may repaint `formattedText` or the local editor draft.
      } catch {
        guard revisionRequestGeneration == requestGeneration, formatterCommitPending,
          pendingRevisionSessionId == projection.sessionId,
          pendingRevisionSource == projection.reducerRevision,
          latestTranscriptProjection?.sessionId == projection.sessionId
        else { return }
        formatterCommitPending = false
        pendingRevisionSessionId = nil
        pendingRevisionSource = nil
        pendingRevisionDraft = nil
        formatterError = String(
          localized: "Couldn't format transcript: \(String(describing: error))",
          comment: "The placeholder is the engine's own failure text")
        showFooterNotice(String(localized: "format failed"))
        restartAutoHideCountdown()
      }
    }
  }

  func restoreDocumentRevision(_ selectedRevision: UInt64) {
    guard archivedTranscript == nil, terminal, !isRevisionDraftDirty, !revisionCommitPending,
      !formatterCommitPending, let projection = latestTranscriptProjection,
      documentHistory.contains(where: { $0.revision == selectedRevision }),
      selectedRevision != projection.reducerRevision, let engine
    else { return }
    revisionCommitPending = true
    revisionCommitError = nil
    pendingRevisionSessionId = projection.sessionId
    pendingRevisionSource = projection.reducerRevision
    pendingRevisionDraft = revisionDraft
    revisionRequestGeneration &+= 1
    let requestGeneration = revisionRequestGeneration
    cancelAutoHide()
    Task { @MainActor [weak self] in
      guard let self else { return }
      do {
        let receipt = try await engine.restoreDocumentRevision(
          sessionId: projection.sessionId,
          sourceRevision: projection.reducerRevision,
          restoreRevision: selectedRevision)
        guard revisionRequestGeneration == requestGeneration, revisionCommitPending,
          pendingRevisionSessionId == projection.sessionId,
          pendingRevisionSource == projection.reducerRevision,
          latestTranscriptProjection?.sessionId == projection.sessionId
        else { return }
        guard receipt.sessionId == projection.sessionId,
          receipt.sourceRevision == projection.reducerRevision,
          receipt.revision > receipt.sourceRevision,
          receipt.provenanceReceipt.hasPrefix("user-edit-")
        else {
          revisionCommitPending = false
          pendingRevisionSessionId = nil
          pendingRevisionSource = nil
          pendingRevisionDraft = nil
          revisionCommitError = String(localized: "Transcript restore receipt was inconsistent")
          return
        }
        // Only the matching reducer callback repaints the canvas.
      } catch {
        guard revisionRequestGeneration == requestGeneration, revisionCommitPending,
          pendingRevisionSessionId == projection.sessionId,
          pendingRevisionSource == projection.reducerRevision,
          latestTranscriptProjection?.sessionId == projection.sessionId
        else { return }
        revisionCommitPending = false
        pendingRevisionSessionId = nil
        pendingRevisionSource = nil
        pendingRevisionDraft = nil
        revisionCommitError = String(
          localized: "Couldn't restore transcript version: \(String(describing: error))",
          comment: "The placeholder is the engine's own failure text")
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
        self.revisionCommitError = String(
          localized: "Couldn't read transcript history: \(String(describing: error))",
          comment: "The placeholder is the engine's own failure text")
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
      message = String(localized: "Speech too quiet or short — adjust the mic and try again")
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
  /// One plural key, so the count agreement is the catalog's business and no
  /// branch here can disagree with the translation.
  var supersededRecoveryNotice: String {
    let edits = unacknowledgedSupersededEditCount
    guard edits > 0 else { return String(localized: "previous take retained") }
    return String(
      localized: "\(edits) unsaved edits to recover",
      comment: "The placeholder is how many retained edits are waiting")
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
        ? String(localized: "Couldn't copy the unsaved edit — it is still retained, try again")
        : String(localized: "Couldn't copy the previous take — it is still retained, try again")
      // Persisting: a failure the user must be able to read after the 2.6 s
      // fade, and the retained work already keeps this rail revealed. The
      // `.formatted` body shows the full sentence; the footer covers every
      // other phase, including a live capture.
      showFooterNotice(String(localized: "recover failed — kept"), persists: true)
      if terminal, !isEditingTranscript { restartAutoHideCountdown() }
      return
    }
    recoveryFailure = nil
    supersededTakes.removeFirst()
    showFooterNotice(
      take.hasUnsavedEdits
        ? String(localized: "unsaved edit copied")
        : String(localized: "previous take copied"))
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
    showFooterNotice(
      take.hasUnsavedEdits
        ? String(localized: "unsaved edit discarded")
        : String(localized: "previous take discarded"))
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
    // A new take owns the canvas. Accepted archive revisions are already
    // durable in that archive's chain; an unsaved archive edit moves into the
    // superseded-work owner under the archive's own identity, and any history
    // read still in flight can no longer land.
    invalidateHistoryOpens()
    leaveArchivedTranscript()
    if let compactProjection {
      retiredProjectionSessions.insert(compactProjection.sessionId)
    }
    compactProjection = nil
    // Read the draft's dirtiness against the OUTGOING document, before any
    // field below moves. Computed after the reset it would compare against an
    // empty projection and misclassify a clean draft as unsaved work.
    let draftWasDirty = isRevisionDraftDirty
    captureGeneration &+= 1
    captureDidStart = false
    currentTakeAudioAvailable = false
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
    pendingRevisionDraft = nil
    userRevisionProvenance = nil
    followDictationCapturePresentation()
    onTranscriptPresentationChanged?()
  }

  /// A new ordinary capture shows its own canvas. The controller's roster
  /// resolves a channel snapshot that still describes the preceding capture.
  private func followDictationCapturePresentation() {
    if !hasOpenChannel, indicatorMode != .assistive, !showsMyDictation {
      selectConversation(nil)
    }
    guard let engine else { return }
    let generation = captureGeneration
    let focusRevision = conversationFocusRevision
    Task { @MainActor [weak self] in
      let roster = await engine.channelRosterSnapshot()
      guard let self, captureGeneration == generation,
        conversationFocusRevision == focusRevision, recording, !finalized
      else { return }
      applyChannelRoster(roster)
      guard !roster.contains(where: \.open), indicatorMode != .assistive, !showsMyDictation
      else { return }
      selectConversation(nil)
    }
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
    errorDiagnosticDetail = nil
    errorLifecycleDetail = String(localized: "No transcript was delivered.")
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
  /// during controller-owned live dictation or channel capture. Dictation
  /// finalisation cannot suppress another subscriber's still-open microphone.
  func applyAudioLevel(
    _ rms: Float, now: TimeInterval = ProcessInfo.processInfo.systemUptime
  ) {
    guard audioCaptureActive else { return }
    levelMeter.push(rms: rms, speechActive: vadActive, now: now)
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
    s.presentationMode = .expanded
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
    s.presentationMode = .expanded
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
    s.presentationMode = .expanded
    s.applyTranscriptProjection(previewProjection("", phase: .noSpeech, terminal: true))
    s.noSpeechNotice = OverlayState.defaultNoSpeechNotice
    return s
  }

  /// Seeded view model for #Preview in the finalized state.
  static func previewFormatted() -> OverlayState {
    let s = OverlayState()
    s.presentationMode = .expanded
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
    s.presentationMode = .expanded
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
      sealCoverage: nil, consultationPresentations: [], uncertainSpans: [])
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
  func startRecording(assistive: Bool, language: CsLanguage?) async throws {
    if assistive {
      try await hotkeys.startAssistiveRecording()
    } else {
      try await hotkeys.startRecording()
    }
  }
  func startsInAssistiveMode() -> Bool { config.trayToggles().startAssistive }
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
  func cloudRetranscribeConfigured() -> Bool { config.cloudFileRetranscriptionAvailable() }
  func currentOverlayPolicy() -> OverlayPolicySnapshot? {
    let toggles = config.trayToggles()
    guard let formatLevel = FormattingPolicyOption(rawValue: toggles.formattingLevel) else {
      return nil
    }
    return OverlayPolicySnapshot(autoFormatLevel: formatLevel)
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
  func wordAudioClip(
    sessionId: String, captureEpoch: UInt64, sampleStart: UInt64, sampleEnd: UInt64,
    padMs: UInt32
  ) throws -> String {
    try hotkeys.wordAudioClip(
      sessionId: sessionId, captureEpoch: captureEpoch, sampleStart: sampleStart,
      sampleEnd: sampleEnd, padMs: padMs)
  }
  func teachSpan(variant: String, canonical: String, kind: String) throws
    -> CsQualityCommitResult
  {
    try qualityTeachSpan(variant: variant, canonical: canonical, kind: kind)
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
  func formatArchivedTranscript(
    archivePath: String, sourceRevision: UInt64, level: FormattingPolicyOption?
  ) async throws -> CsArchivedFormat {
    try await hotkeys.formatArchivedTranscript(
      archivePath: archivePath, sourceRevision: sourceRevision, level: level?.rawValue)
  }
  func commitArchivedRevision(
    archivePath: String, sourceRevision: UInt64, renderedText: String,
    kind: CsArchiveRevisionKind
  ) async throws -> CsArchivedDocument {
    try await Task.detached {
      try CodescribeThreads().commitHistoryRevision(
        path: archivePath, sourceRevision: sourceRevision, renderedText: renderedText,
        kind: kind)
    }.value
  }
  func restoreArchivedRevision(
    archivePath: String, sourceRevision: UInt64, restoreRevision: UInt64
  ) async throws -> CsArchivedDocument {
    try await Task.detached {
      try CodescribeThreads().restoreHistoryRevision(
        path: archivePath, sourceRevision: sourceRevision, restoreRevision: restoreRevision)
    }.value
  }
  func transcribeTake(sessionId: String, path: String) async throws -> CsTranscription {
    try await hotkeys.transcribeTake(sessionId: sessionId, path: path)
  }
  func channelRosterSnapshot() async -> [CsChannelRosterState] {
    await hotkeys.channelRosterSnapshot()
  }
  func toggleAgentChannel(digit: UInt8) async throws {
    try await hotkeys.toggleAgentChannel(digit: digit)
  }
  func archiveAgent(request: CsAgentArchiveRequest) async throws -> String {
    try await hotkeys.archiveAgentChannel(request: request)
  }
}

/// Presentation chrome a projection paints. Kept aside while an archive owns
/// the canvas, so leaving it returns the projected take exactly as it was.
struct OverlayProjectedChrome: Equatable {
  let mode: OverlayMode
  let terminal: Bool
  let canPaste: Bool
  let canInsert: Bool
  let canCopy: Bool
  let canRetranscribe: Bool
  let canFormat: Bool
  let canSendToAgent: Bool
  let coverageRefusalNotice: String?
}

// MARK: - Transcript reopened from history

/// How the canvas answered one history open request.
enum OverlayArchiveOpenOutcome: Equatable {
  case opened
  case refused(String)
  /// A newer request, a dismissed list, a close or a new take took over.
  case superseded
}

extension OverlayState {
  /// Why history cannot take the canvas now. Opening never cancels, stops or
  /// replaces a live capture, and never strands an unsaved edit.
  var archiveOpenRefusal: String? {
    let captureInFlight =
      recording || warmingUp || transcribing
      || (archivedTranscript == nil && !terminal
        && (mode == .listening || mode == .finalizing)
        && (captureStartedAtUptime != nil || latestTranscriptProjection != nil))
    if captureInFlight {
      return String(
        localized: "Finish the current take before opening a saved transcript.",
        comment: "History is disabled while a take is being recorded or finished")
    }
    if revisionCommitPending || formatterCommitPending || archiveActionPending {
      return String(
        localized: "Wait for the current change to finish before opening a saved transcript.",
        comment: "History is disabled while a revision, format or retranscription runs")
    }
    if isRevisionDraftDirty {
      return String(
        localized: "Commit or discard your edit before opening a saved transcript.",
        comment: "History is disabled while the canvas holds an unsaved edit")
    }
    return nil
  }

  /// Footer line naming where the canvas text came from and, once the
  /// archive has accepted revisions, which version is shown.
  var archivedTranscriptOrigin: String? {
    guard let archivedTranscript else { return nil }
    let date = archivedTranscript.recordedAt.formatted(date: .abbreviated, time: .shortened)
    guard archivedTranscript.revision > 0 else {
      return String(
        localized: "From history · \(date)",
        comment: "Footer under a transcript reopened from history; the placeholder is its date")
    }
    let version = Int(clamping: archivedTranscript.revision)
    return String(
      localized: "From history · \(date) · version \(version)",
      comment:
        "Footer under a revised transcript reopened from history; the placeholders are its date and the version shown"
    )
  }

  /// Retranscription is bound to the archive's own audio. Without it, say so.
  var retranscribeUnavailableReason: String? {
    guard let archivedTranscript, archivedTranscript.audioPath == nil else { return nil }
    return String(
      localized: "The audio for this saved transcript is no longer available.",
      comment: "Transcribe again is impossible: the archived take has no audio file")
  }

  /// The Undo the archive's own revision chain offers: what its latest format
  /// or retranscription replaced. Edits and restores offer none.
  var archivedUndoIntent: OverlayIntent? {
    guard let archivedTranscript, archivedTranscript.undoRevision != nil, !archiveActionPending
    else { return nil }
    switch archivedTranscript.provenance {
    case "formatter": return .undoFormat
    case "retranscribe": return .undoRetranscribe
    default: return nil
    }
  }

  /// Issue the ticket one history open request must present to land.
  func admitHistoryOpen() -> UInt64 {
    historyOpenAdmission &+= 1
    return historyOpenAdmission
  }

  /// Every history open still in flight becomes stale: the list was
  /// dismissed, the overlay closed or a new take took the canvas.
  func invalidateHistoryOpens() {
    historyOpenAdmission &+= 1
  }

  /// Put an archived take on the canvas under its admission ticket. A stale
  /// ticket is superseded silently; a refusal names its reason in the footer.
  func openArchivedTranscript(
    _ archived: OverlayArchivedTranscript, admission: UInt64
  ) -> OverlayArchiveOpenOutcome {
    guard admission == historyOpenAdmission else { return .superseded }
    if let reason = archiveOpenRefusal {
      showFooterNotice(reason)
      return .refused(reason)
    }
    revisionFocusCommitTask?.cancel()
    revisionFocusCommitTask = nil
    if archivedTranscript == nil { chromeBehindArchive = projectedChrome }
    archiveGeneration &+= 1
    archivedTranscript = archived
    archiveDraft = archived.text
    archiveActionPending = false
    archiveActionError = nil
    isEditingTranscript = false
    // Taking the canvas for history is review: the take behind it is no
    // longer auto-sent to Agent, exactly as when the user starts an edit.
    agentAutoSendCancelled = true
    toastTask?.cancel()
    toast = nil
    paintArchivedChrome()
    cloudRetranscribeConfigured = engine?.cloudRetranscribeConfigured() ?? false
    selectConversation(nil)
    setPresentationMode(.expanded)
    restartAutoHideCountdown()
    onTranscriptPresentationChanged?()
    return .opened
  }

  /// Open with a fresh ticket (keyboard and programmatic paths).
  @discardableResult
  func openArchivedTranscript(_ archived: OverlayArchivedTranscript) -> Bool {
    openArchivedTranscript(archived, admission: admitHistoryOpen()) == .opened
  }

  /// Hand the canvas back to the projected take. Accepted revisions are
  /// already durable in the archive's chain; an unsaved archive edit moves to
  /// the superseded-work owner under the archive's identity, never into the
  /// projected take. Pending archive presentation is fenced off.
  func leaveArchivedTranscript() {
    guard let archived = archivedTranscript else { return }
    if isRevisionDraftDirty {
      retainArchivedWork(archived, unsavedDraft: archiveDraft)
    }
    archiveGeneration &+= 1
    archivedTranscript = nil
    archiveDraft = ""
    archiveActionPending = false
    archiveActionError = nil
    isEditingTranscript = false
    if let chrome = chromeBehindArchive {
      mode = chrome.mode
      terminal = chrome.terminal
      canPaste = chrome.canPaste
      canInsert = chrome.canInsert
      canCopy = chrome.canCopy
      canRetranscribe = chrome.canRetranscribe
      canFormat = chrome.canFormat
      canSendToAgent = chrome.canSendToAgent
      coverageRefusalNotice = chrome.coverageRefusalNotice
    }
    chromeBehindArchive = nil
  }

  /// Retain archive text the chain did not accept, for explicit recovery or
  /// discard through the one superseded-work owner.
  fileprivate func retainArchivedWork(_ archived: OverlayArchivedTranscript, unsavedDraft: String) {
    guard !unsavedDraft.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else { return }
    let take = OverlaySupersededTake(
      archive: OverlaySupersededArchive(path: archived.path, recordedAt: archived.recordedAt),
      renderedText: archived.text,
      revision: archived.revision,
      unsavedDraft: unsavedDraft)
    guard !supersededTakes.contains(take) else { return }
    supersededTakes.append(take)
    showFooterNotice(supersededRecoveryNotice)
  }

  fileprivate var projectedChrome: OverlayProjectedChrome {
    OverlayProjectedChrome(
      mode: mode, terminal: terminal, canPaste: canPaste, canInsert: canInsert,
      canCopy: canCopy, canRetranscribe: canRetranscribe, canFormat: canFormat,
      canSendToAgent: canSendToAgent, coverageRefusalNotice: coverageRefusalNotice)
  }

  /// A finished document with the archive's own capabilities. Insert is
  /// offered because the Rust paste route still verifies the target; Send to
  /// Agent because the Rust send resolves its own route and destination.
  fileprivate func paintArchivedChrome() {
    guard let archivedTranscript else { return }
    mode = .formatted
    terminal = true
    canPaste = true
    canInsert = true
    canCopy = true
    canRetranscribe = archivedTranscript.audioPath != nil
    canFormat = engine != nil
    canSendToAgent = engine != nil
    coverageRefusalNotice = nil
  }

  /// Paint a document Rust returned for an archive. Only the canvas still
  /// showing that same archive moves, only forward, and never over an edit
  /// the user is typing.
  @discardableResult
  fileprivate func acceptArchivedDocument(_ document: CsArchivedDocument) -> Bool {
    guard var archived = archivedTranscript, !isRevisionDraftDirty, archived.accept(document)
    else { return false }
    archivedTranscript = archived
    archiveDraft = archived.text
    return true
  }

  /// Commit the archive edit as a new revision of that archive. The draft
  /// stays on the canvas, dirty and recoverable, until Rust accepts it.
  fileprivate func commitArchivedDraft() {
    guard let source = archivedTranscript, !archiveActionPending else { return }
    let proposed = archiveDraft
    guard !proposed.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else {
      revisionCommitError = String(localized: "A transcript revision cannot be empty")
      return
    }
    guard let engine else {
      revisionCommitError = String(localized: "Transcript revision authority is unavailable")
      return
    }
    revisionCommitError = nil
    archiveActionPending = true
    archiveActionError = nil
    archiveGeneration &+= 1
    let generation = archiveGeneration
    cancelAutoHide()
    Task { @MainActor [weak self] in
      let outcome: Result<CsArchivedDocument, Error>
      do {
        outcome = .success(
          try await engine.commitArchivedRevision(
            archivePath: source.path, sourceRevision: source.revision, renderedText: proposed,
            kind: .userEdit))
      } catch {
        outcome = .failure(error)
      }
      guard let self else { return }
      if case .success(let document) = outcome {
        // Accepted: a copy of this edit retained at a capture boundary is no
        // longer unsaved work.
        self.supersededTakes.removeAll {
          $0.archive?.path == source.path && $0.unsavedDraft == document.renderedText
        }
      }
      guard self.archiveGeneration == generation, self.archivedTranscript?.path == source.path
      else { return }
      self.archiveActionPending = false
      switch outcome {
      case .success(let document):
        // The draft is the accepted text, so it is clean before painting.
        self.archiveDraft = self.archivedTranscript?.text ?? proposed
        self.acceptArchivedDocument(document)
        self.showFooterNotice(
          String(
            localized: "Edit saved to this transcript. The original text is kept.",
            comment: "Footer after an edit of a transcript reopened from history was saved"))
      case .failure(let error):
        // The draft stays dirty on the canvas: commit again or discard.
        self.archiveActionError = String(
          localized: "Couldn't save the edit to this transcript: \(String(describing: error))",
          comment: "The placeholder is the engine's own failure text")
      }
      if !self.isEditingTranscript { self.restartAutoHideCountdown() }
    }
  }

  fileprivate func formatArchivedTranscript(level: FormattingPolicyOption?) {
    guard let source = archivedTranscript, !archiveActionPending, !isRevisionDraftDirty else {
      return
    }
    guard let engine else {
      archiveActionError = String(localized: "Transcript formatter authority is unavailable")
      showFooterNotice(String(localized: "format unavailable"))
      return
    }
    archiveActionPending = true
    archiveActionError = nil
    archiveGeneration &+= 1
    let generation = archiveGeneration
    cancelAutoHide()
    showFooterNotice(String(localized: "formatting…"), persists: true)
    Task { @MainActor [weak self] in
      let outcome: Result<CsArchivedFormat, Error>
      do {
        // Rust reads the source from this archive's chain at this revision
        // and commits an applied result there, whatever the canvas shows by
        // the time it returns.
        outcome = .success(
          try await engine.formatArchivedTranscript(
            archivePath: source.path, sourceRevision: source.revision, level: level))
      } catch {
        outcome = .failure(error)
      }
      guard let self else { return }
      guard self.archiveGeneration == generation, self.archivedTranscript?.path == source.path
      else {
        // Durable in A's chain already; repaint only if A is shown again.
        if case .success(let result) = outcome, let document = result.document {
          self.acceptArchivedDocument(document)
        }
        return
      }
      self.archiveActionPending = false
      self.restartAutoHideCountdown()
      switch outcome {
      case .success(let result) where result.outcome == .applied:
        guard let document = result.document, self.acceptArchivedDocument(document) else {
          self.archiveActionError = String(
            localized: "Formatting failed. The transcript is unchanged.",
            comment: "The formatting provider failed for a transcript reopened from history")
          self.showFooterNotice(String(localized: "format failed"))
          return
        }
        self.showFooterNotice(String(localized: "formatted"))
      case .success(let result) where result.outcome == .unchanged:
        self.showFooterNotice(
          String(
            localized: "Formatting made no changes.",
            comment: "The formatter returned the transcript unchanged"))
      case .success(let result) where result.outcome == .unavailable:
        self.archiveActionError = String(
          localized: "Formatting is not available for this level. The transcript is unchanged.",
          comment: "The formatter skipped the request, for example formatting is off")
        self.showFooterNotice(String(localized: "format failed"))
      case .success:
        self.archiveActionError = String(
          localized: "Formatting failed. The transcript is unchanged.",
          comment: "The formatting provider failed for a transcript reopened from history")
        self.showFooterNotice(String(localized: "format failed"))
      case .failure(let error):
        self.archiveActionError = String(
          localized: "Couldn't format transcript: \(String(describing: error))",
          comment: "The placeholder is the engine's own failure text")
        self.showFooterNotice(String(localized: "format failed"))
      }
    }
  }

  /// Transcribe the archive's own paired audio again and commit the result
  /// as a revision of that archive. Never the last session, never the
  /// projected take: no audio means a stated refusal.
  fileprivate func retranscribeArchivedTranscript(pass: OverlayRetranscribePass) {
    guard let source = archivedTranscript, !archiveActionPending, !isRevisionDraftDirty else {
      return
    }
    guard let engine else {
      presentActionFailure(
        String(localized: "Retranscription needs the recording engine"),
        notice: String(localized: "retranscribe unavailable"))
      return
    }
    guard let audioPath = source.audioPath else {
      archiveActionError = retranscribeUnavailableReason
      showFooterNotice(String(localized: "take audio unavailable"))
      return
    }
    guard pass != .cloud || cloudRetranscribeConfigured else {
      archiveActionError = String(
        localized: "Cloud transcription is not set up. Use the local pass or configure it in Settings.",
        comment: "Cloud retranscription requested without a configured cloud provider")
      showFooterNotice(String(localized: "retranscribe unavailable"))
      return
    }
    let passEngine =
      pass == .cloud
      ? String(localized: "cloud", comment: "Retranscribe pass: the cloud engine")
      : String(localized: "local Whisper HQ", comment: "Retranscribe pass: the local engine")
    archiveActionPending = true
    archiveActionError = nil
    archiveGeneration &+= 1
    let generation = archiveGeneration
    cancelAutoHide()
    showFooterNotice(
      String(
        localized: "retranscribing · \(passEngine)",
        comment: "The placeholder is the name of the engine running the pass"),
      persists: true)
    Task { @MainActor [weak self] in
      let transcribed: Result<String, Error>
      do {
        let result = try await engine.transcribeFile(path: "\(pass.pathPrefix)\(audioPath)")
        transcribed = .success(result.text.trimmingCharacters(in: .whitespacesAndNewlines))
      } catch {
        transcribed = .failure(error)
      }
      // The new words belong to archive A whatever the canvas shows now, so
      // they are committed to A's chain against the revision they replace.
      var committed: Result<CsArchivedDocument, Error>?
      if case .success(let text) = transcribed, !text.isEmpty, text != source.text {
        do {
          committed = .success(
            try await engine.commitArchivedRevision(
              archivePath: source.path, sourceRevision: source.revision, renderedText: text,
              kind: .retranscribe))
        } catch {
          committed = .failure(error)
        }
      }
      guard let self else { return }
      let current =
        self.archiveGeneration == generation && self.archivedTranscript?.path == source.path
      guard current else {
        switch (transcribed, committed) {
        case (_, .success(let document)?):
          self.acceptArchivedDocument(document)
        case (.success(let text), .failure?):
          // Not accepted and A is no longer on the canvas: keep the words.
          self.retainArchivedWork(source, unsavedDraft: text)
        default:
          break
        }
        return
      }
      self.archiveActionPending = false
      self.restartAutoHideCountdown()
      switch (transcribed, committed) {
      case (.failure(let error), _):
        self.archiveActionError = String(localized: "Couldn't transcribe this take again")
        self.errorDiagnosticDetail = String(describing: error)
        self.showFooterNotice(String(localized: "Retranscription failed"))
      case (.success(let text), nil) where text.isEmpty:
        self.archiveActionError = String(
          localized: "The \(passEngine) pass returned no text",
          comment: "The placeholder is the name of the engine that ran the pass")
        self.showFooterNotice(String(localized: "retranscribe returned no text"))
      case (.success, nil):
        self.showFooterNotice(String(localized: "retranscribed"))
      case (_, .success(let document)?):
        self.acceptArchivedDocument(document)
        self.showFooterNotice(
          self.canUndoRetranscribe
            ? String(localized: "retranscribed — Back keeps the old text")
            : String(localized: "retranscribed"))
      case (.success(let text), .failure(let error)?):
        // Not accepted: the new words stay on the canvas as an unsaved draft
        // the user can commit again or discard.
        self.archiveDraft = text
        self.archiveActionError = String(
          localized: "Couldn't save the new transcription: \(String(describing: error))",
          comment: "The placeholder is the engine's own failure text")
        self.showFooterNotice(String(localized: "Retranscription failed"))
      }
    }
  }

  /// Undo the archive's latest format or retranscription: Rust restores the
  /// version it replaced as a new revision of the same archive.
  func undoArchivedRevision() {
    guard let source = archivedTranscript, let restore = source.undoRevision,
      !archiveActionPending, !isRevisionDraftDirty
    else {
      showFooterNotice(String(localized: "nothing to undo"))
      return
    }
    guard let engine else {
      presentActionFailure(
        String(localized: "Undo needs the recording engine"),
        notice: String(localized: "undo unavailable"))
      return
    }
    let undoesFormat = source.provenance == "formatter"
    archiveActionPending = true
    archiveActionError = nil
    archiveGeneration &+= 1
    let generation = archiveGeneration
    cancelAutoHide()
    Task { @MainActor [weak self] in
      let outcome: Result<CsArchivedDocument, Error>
      do {
        outcome = .success(
          try await engine.restoreArchivedRevision(
            archivePath: source.path, sourceRevision: source.revision, restoreRevision: restore))
      } catch {
        outcome = .failure(error)
      }
      guard let self else { return }
      guard self.archiveGeneration == generation, self.archivedTranscript?.path == source.path
      else {
        if case .success(let document) = outcome { self.acceptArchivedDocument(document) }
        return
      }
      self.archiveActionPending = false
      self.restartAutoHideCountdown()
      switch outcome {
      case .success(let document):
        self.acceptArchivedDocument(document)
        self.showFooterNotice(
          undoesFormat
            ? String(localized: "format undone")
            : String(localized: "retranscribe undone"))
      case .failure(let error):
        self.archiveActionError = String(
          localized: "Couldn't undo: \(String(describing: error))",
          comment: "The placeholder is the engine's own failure text")
        self.showFooterNotice(String(localized: "undo failed — kept"))
      }
    }
  }

  /// The same explicit Send to Agent as a fresh take, carrying the archive's
  /// accepted text to the chat the user currently has selected. It never
  /// fires on open, never replays the archive's original delivery and never
  /// touches the projected take's delivery latch.
  fileprivate func sendArchivedTranscriptToAgent() -> Task<Void, Never>? {
    guard let source = archivedTranscript, canSendToAgent, !isRevisionDraftDirty,
      !archiveActionPending, let engine,
      !source.text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
    else { return nil }
    archiveActionPending = true
    archiveActionError = nil
    archiveGeneration &+= 1
    let generation = archiveGeneration
    let text = source.text
    cancelAutoHide()
    return Task { @MainActor [weak self] in
      let outcome: Result<Bool, Error>
      do {
        outcome = .success(try await engine.sendAssistiveTranscript(text: text))
      } catch {
        outcome = .failure(error)
      }
      guard let self, self.archiveGeneration == generation,
        self.archivedTranscript?.path == source.path
      else { return }
      self.archiveActionPending = false
      switch outcome {
      case .success(true):
        self.onSendToAgent?(text)
        self.onClose?()
      case .success(false):
        self.showToast(String(localized: "Agent delivery is no longer available"))
        self.restartAutoHideCountdown()
      case .failure:
        self.showToast(String(localized: "Couldn't send to Agent"))
        self.restartAutoHideCountdown()
      }
    }
  }
}
