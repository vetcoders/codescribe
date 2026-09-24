import Foundation
import XCTest

@testable import Codescribe

/// The delivery half of one composer turn: capture identity on the way in,
/// receiver acknowledgement on the way out.
///
/// Everything here is written against production owners — `RealComposerDictation`
/// for the gesture, `AgentChatStore` for admission and composer ownership,
/// `OverlayState` for the projection boundary. The only substituted surfaces are
/// the two process boundaries: the FFI capture object, and the chat engine whose
/// real implementation is a Rust turn. Queueing, dispatch and every ownership
/// decision stay in the production store; no policy is re-implemented in a fake.
///
/// The claim under test is narrow and specific: **a route that was selected, an
/// event that was queued and a callback that was invoked are all statements
/// about the postman.** Only the receiver's typed receipt is delivery.
@MainActor
final class ComposerDeliveryJoinTests: XCTestCase {

  // MARK: Doubles

  private enum CaptureCall: Equatable {
    case isRecording
    case startComposerTurn
    case stop(String)
  }

  private enum CaptureFailure: Error { case transport }

  private final class FakeCaptureSurface: ComposerCaptureControlling, @unchecked Sendable {
    private var recordingAnswers: [Bool]
    private var answerCursor = 0
    var admittedCaptureId = "capture-A"
    var stopOutcome: CsConditionalStop = .stopped
    var stopThrows = false
    var onQuery: (@MainActor @Sendable () -> Void)?
    var onStart: (@MainActor @Sendable () async -> Void)?
    var onStop: (@MainActor @Sendable () async -> Void)?
    private(set) var calls: [CaptureCall] = []

    init(recording: [Bool]) { self.recordingAnswers = recording }

    func isRecording() async -> Bool {
      calls.append(.isRecording)
      await onQuery?()
      let answer = recordingAnswers[min(answerCursor, recordingAnswers.count - 1)]
      answerCursor += 1
      return answer
    }

    func startComposerTurnRecording() async throws -> CsCaptureHandle {
      calls.append(.startComposerTurn)
      await onStart?()
      return CsCaptureHandle(captureId: admittedCaptureId)
    }

    func stopComposerTurnRecording(handle: CsCaptureHandle) async throws -> CsConditionalStop {
      calls.append(.stop(handle.captureId))
      await onStop?()
      if stopThrows { throw CaptureFailure.transport }
      return stopOutcome
    }

    var stoppedIdentities: [String] {
      calls.compactMap { if case .stop(let id) = $0 { return id } else { return nil } }
    }
  }

  private final class StubThreadsProvider: ChatThreadsProviding {
    func listThreads() -> [ChatThread] {
      [("t_a", "Thread A"), ("t_b", "Thread B")].map { row in
        var thread = ChatThread(title: row.1, meta: "now")
        thread.backendId = row.0
        thread.messagesLoaded = true
        return thread
      }
    }
    func searchThreads(query: String) -> [ChatThread] { listThreads() }
    func loadMessages(backendId: String) -> [ChatMessage] { [] }
    func deleteThread(backendId: String) -> Bool { true }
    func setThreadFavorite(backendId: String, isFavorite: Bool) -> Bool { true }
    func renameThread(backendId: String, title: String) -> Bool { true }
    func setGeneratedTitle(backendId: String, title: String) -> Bool { true }
    func exportThreadMarkdown(backendId: String, assistantOnly: Bool) -> String? { nil }
    func generateThreadId() -> String { "t_generated" }
  }

  /// Readiness is published only after the continuation is registered. Tests
  /// await that handshake; cancellation resumes every outstanding continuation.
  @MainActor
  private final class HeldReplyState {
    var startedTexts: [String] = []
    var onStart: ((String) -> Void)?
    private var pending: [(UUID, CheckedContinuation<String, Error>)] = []
    private var cancelled: Set<UUID> = []
    private var isClosed = false

    func register(_ id: UUID, text: String, continuation: CheckedContinuation<String, Error>) {
      guard !isClosed, !cancelled.contains(id), !Task.isCancelled else {
        continuation.resume(throwing: CancellationError())
        return
      }
      pending.append((id, continuation))
      startedTexts.append(text)
      onStart?(text)
    }

    func finishNext(_ reply: String) {
      guard !pending.isEmpty else { return }
      pending.removeFirst().1.resume(returning: reply)
    }

    func cancel(_ id: UUID) {
      cancelled.insert(id)
      guard let index = pending.firstIndex(where: { $0.0 == id }) else { return }
      pending.remove(at: index).1.resume(throwing: CancellationError())
    }

    func cancelAll() {
      isClosed = true
      let held = pending
      pending.removeAll()
      for (_, continuation) in held { continuation.resume(throwing: CancellationError()) }
    }
  }

  private final class HeldReplyEngine: AgentChatEngine {
    let state = HeldReplyState()
    private(set) var sendOrigins: [(AgentSendOrigin, Int, String, Bool)] = []
    func isAvailable() -> Bool { true }
    func availabilityDetail() -> String? { nil }
    func generateThreadTitle(_ text: String) async throws -> String? { nil }
    func recordSendOrigin(
      _ origin: AgentSendOrigin, chars: Int, threadId: String, recordingActive: Bool
    ) {
      sendOrigins.append((origin, chars, threadId, recordingActive))
    }

    func streamReply(
      _ text: String,
      threadId: String,
      attachmentPaths: [String],
      onDelta: @escaping @MainActor (String) -> Void,
      onReasoning: @escaping @MainActor (String) -> Void,
      onToolExecuting: @escaping @MainActor (String, String) -> Void,
      onToolResult: @escaping @MainActor (String, String, Bool, String) -> Void
    ) async throws -> String {
      let id = UUID()
      let reply = try await withTaskCancellationHandler {
        try await withCheckedThrowingContinuation { continuation in
          state.register(id, text: text, continuation: continuation)
        }
      } onCancel: { [state] in
        Task { @MainActor in state.cancel(id) }
      }
      onDelta(reply)
      return reply
    }

    func cancelReply(threadId: String) -> Bool {
      state.cancelAll()
      return true
    }
  }

  // MARK: Isolation

  private func isolatedDefaults() -> UserDefaults {
    let name = "Codescribe.ComposerDeliveryJoinTests." + UUID().uuidString
    let defaults = UserDefaults(suiteName: name)!
    addTeardownBlock { UserDefaults(suiteName: name)?.removePersistentDomain(forName: name) }
    return defaults
  }

  private func admitCapture(_ store: AgentChatStore, threadID: UUID, id: String = "join-session") {
    let request = store.beginComposerCaptureRequest(threadID: threadID)
    store.completeComposerCaptureStart(request, live: true, handle: CsCaptureHandle(captureId: id))
  }

  private struct Fixture: Sendable {
    let store: AgentChatStore
    let dictation: RealComposerDictation
    let surface: FakeCaptureSurface
    let threadA: UUID
    let threadB: UUID
  }

  private func makeFixture(recording: [Bool]) -> Fixture {
    let store = AgentChatStore(threadsProvider: StubThreadsProvider(), persistenceDefaults: isolatedDefaults())
    let surface = FakeCaptureSurface(recording: recording)
    let dictation = RealComposerDictation(store: store, hotkeys: surface)
    store.dictation = dictation
    let threadA = store.threads.first { $0.backendId == "t_a" }!.id
    let threadB = store.threads.first { $0.backendId == "t_b" }!.id
    store.select(threadA)
    return Fixture(
      store: store, dictation: dictation, surface: surface, threadA: threadA, threadB: threadB)
  }

  private func settle(_ fixture: Fixture) async {
    await fixture.dictation.transitionTask?.value
  }

  private var nextSequence: UInt64 = 0

  /// Build one projection through the production boundary. `lifecycleTerminal`
  /// and `delivery` are explicit because they are exactly what this suite is
  /// about; nothing here infers them from phase, label or action text.
  private func project(
    _ text: String,
    to state: OverlayState,
    sessionId: String = "join-session",
    mode: String = "agent",
    phase: String,
    terminal: Bool,
    lifecycleTerminal: Bool,
    delivery: CsTranscriptDelivery,
    reducerAction: String,
    deliveryText: String? = nil,
    manualEditReceipt: String? = nil,
    presentationReceipt: CsProjectedPresentationReceipt? = nil
  ) {
    nextSequence += 1
    let sequence = nextSequence
    let sampleStart = presentationReceipt?.sampleStart ?? (sequence - 1) * 16_000
    let sampleEnd = presentationReceipt?.sampleEnd ?? sequence * 16_000
    let receipt = CsProjectedAcousticReceipt(
      acousticSerialVersion: 1,
      acousticSerial: "join-acoustic-\(sequence)",
      sessionId: sessionId,
      captureEpoch: 1,
      sampleStart: sampleStart,
      sampleEnd: sampleEnd,
      durationMs: 1_000,
      energyIntegral: 1,
      meanRmsDbfs: -20,
      peakDbfs: -6,
      vadOpenSample: sampleStart,
      vadCloseSample: sampleEnd,
      evidenceCalibrationVersion: "test-v1",
      wordEvidenceReceipts: ["join-word-\(sequence)"],
      layerDecisionReceipts: ["join-layer-\(sequence)"],
      sealReceipt: presentationReceipt?.sourceSealReceipt ?? (terminal ? "join-seal-\(sequence)" : nil),
      manualEditReceipt: manualEditReceipt,
      presentationReceipt: presentationReceipt
    )
    state.applyTranscriptProjection(
      CsTranscriptProjectionEvent(
        schema: "codescribe.transcript_projection.v1",
        sequence: sequence,
        emittedAt: "2026-09-09T20:00:00Z",
        sessionId: sessionId,
        mode: mode,
        reducerRevision: sequence,
        reducerAction: reducerAction,
        occurrenceSessionId: sessionId,
        captureEpoch: 1,
        sampleStart: sampleStart,
        sampleEnd: sampleEnd,
        documentIndex: sequence - 1,
        label: terminal ? "terminal" : "live",
        renderedText: text,
        deliveryText: deliveryText,
        phase: phase,
        canPaste: terminal,
        canInsert: terminal,
        canCopy: !text.isEmpty,
        canRetranscribe: terminal,
        canFormat: !terminal,
        canSendToAgent: false,
        terminal: terminal,
        lifecycleTerminal: lifecycleTerminal,
        delivery: delivery,
        acousticReceipts: [receipt],
        sealCoverage: nil,
        consultationPresentations: []
      )
    )
  }

  private func listening(_ text: String, to state: OverlayState, sessionId: String = "join-session")
  {
    project(
      text, to: state, sessionId: sessionId, phase: "listening", terminal: false,
      lifecycleTerminal: false, delivery: .unattempted, reducerAction: "apply_ledger_decision")
  }

  private func terminalRevision(
    _ text: String, to state: OverlayState, sessionId: String = "join-session",
    receipt: String = "formatter-join-1"
  ) {
    project(
      text, to: state, sessionId: sessionId, phase: "formatted", terminal: true,
      lifecycleTerminal: false, delivery: .unattempted, reducerAction: "apply_manual_edit",
      manualEditReceipt: receipt)
  }

  private func sessionEnded(
    _ text: String, to state: OverlayState, sessionId: String = "join-session",
    delivery: CsTranscriptDelivery = .composerPending,
    deliveryText: String? = nil
  ) {
    project(
      text, to: state, sessionId: sessionId, phase: "formatted", terminal: true,
      lifecycleTerminal: true, delivery: delivery, reducerAction: "session_ended",
      deliveryText: deliveryText)
  }

  func testComposerReceivesDeliveryEnvelopeWhileOverlayKeepsCleanDocument() {
    let f = makeFixture(recording: [false, true])
    let state = OverlayState()
    state.connectComposer(to: f.store)
    admitCapture(f.store, threadID: f.threadA, id: "join-session")

    sessionEnded(
      "clean working text", to: state,
      deliveryText: "<codescribe mode=\"agent\">clean working text</codescribe>")

    XCTAssertEqual(state.activeText, "clean working text")
    XCTAssertEqual(
      f.store.draft,
      "<codescribe mode=\"agent\">clean working text</codescribe>"
    )
  }

  /// Synthetic receiver fixture. Rust's nonempty ledger/Bus mapping test proves
  /// minting; this test proves that presentation provenance cannot settle capture.
  func testIncrementalLightProofStaysListeningUntilOneOwnedLifecycleDelivery() {
    let f = makeFixture(recording: [false, true])
    let state = OverlayState()
    state.connectComposer(to: f.store)
    admitCapture(f.store, threadID: f.threadA, id: "join-session")
    f.store.select(f.threadA)
    f.store.draft = "typed"
    var stopped = 0
    state.onRecordingStopped = { stopped += 1 }
    let proof = CsProjectedPresentationReceipt(
      receiptId: "fixture-light-plus-1", provenance: "light-plus", sessionId: "join-session",
      sourceRevision: 0, revision: 1, captureEpoch: 1, sampleStart: 0, sampleEnd: 16_000,
      sourceSealReceipt: "fixture-occurrence-seal-1", sentenceBreakBefore: false,
      sourceLabel: "pierwsze zdanie",
      leftContext: "", leftContextSha256: "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
      shapedText: "Pierwsze zdanie.")
    project("Pierwsze zdanie.", to: state, phase: "listening", terminal: false,
      lifecycleTerminal: false, delivery: .unattempted, reducerAction: "apply_incremental_shaping",
      presentationReceipt: proof)
    XCTAssertTrue(f.store.ownsLiveDictation)
    XCTAssertEqual(f.store.composerCaptureHandle?.captureId, "join-session")
    XCTAssertEqual(f.store.draft, "typed")
    XCTAssertEqual(stopped, 0)
    XCTAssertEqual(state.latestTranscriptProjection?.acousticReceipts.first?.presentationReceipt, proof)
    XCTAssertNil(state.latestTranscriptProjection?.acousticReceipts.first?.manualEditReceipt)
    project("Pierwsze zdanie. dalsze słowa", to: state, phase: "listening", terminal: false,
      lifecycleTerminal: false, delivery: .unattempted, reducerAction: "apply_ledger_decision",
      presentationReceipt: proof)
    XCTAssertTrue(f.store.ownsLiveDictation)
    XCTAssertEqual(f.store.draft, "typed")
    XCTAssertEqual(stopped, 0)
    sessionEnded("Pierwsze zdanie. dalsze słowa", to: state)
    let delivered = f.store.draft
    let stoppedOnce = stopped
    XCTAssertEqual(stoppedOnce, 1)
    sessionEnded("Pierwsze zdanie. dalsze słowa", to: state)
    XCTAssertEqual(delivered, "typed\nPierwsze zdanie. dalsze słowa")
    XCTAssertEqual(f.store.draft, delivered)
    XCTAssertEqual(stopped, stoppedOnce)
    XCTAssertFalse(f.store.ownsLiveDictation)
  }

  // MARK: Acceptance 1 — capture identity, atomically checked

  /// The controller's own identity for the take reaches the composer, and the
  /// stop names it. Without this the gesture can only say "stop whatever is
  /// live", which is the ask this cut exists to refuse.
  func testStopNamesTheCaptureTheControllerAdmittedForThisGesture() async {
    let f = makeFixture(recording: [false, true, true])
    f.surface.admittedCaptureId = "capture-mine"

    f.dictation.toggle()
    await settle(f)
    XCTAssertEqual(f.store.composerCaptureHandle?.captureId, "capture-mine")

    f.dictation.toggle()
    await settle(f)
    XCTAssertEqual(f.surface.stoppedIdentities, ["capture-mine"])
  }

  /// A take that replaced ours between the query and the stop is refused at the
  /// controller. The foreign take survives, and — the half that is easy to miss
  /// — no new take is started in its place.
  func testForeignReplacementBetweenQueryAndStopLeavesTheForeignTakeRunning() async {
    let f = makeFixture(recording: [false, true, true])
    f.dictation.toggle()
    await settle(f)
    f.surface.stopOutcome = .foreignCapture

    f.dictation.toggle()
    await settle(f)

    XCTAssertEqual(f.surface.stoppedIdentities, ["capture-A"], "exactly one named stop attempt")
    XCTAssertEqual(
      f.surface.calls.filter { $0 == .startComposerTurn }.count, 1,
      "a refused stop must not become a start")
    XCTAssertFalse(f.store.ownsLiveDictation)
    XCTAssertNil(f.store.composerCaptureHandle, "a lost capture keeps no stop permission")
  }

  /// Ownership without an admitted identity is not stop permission. The gesture
  /// reconciles instead of falling back to an unconditional stop.
  func testOwnershipWithoutAnAdmittedIdentityNeverIssuesAStop() async {
    let f = makeFixture(recording: [false, true, true])
    f.dictation.toggle()
    await settle(f)
    XCTAssertTrue(f.store.ownsLiveDictation)
    f.store.reconcileComposerCaptureLost()
    f.store.setDictationPhase(.recording)

    f.dictation.toggle()
    await settle(f)

    XCTAssertTrue(f.surface.stoppedIdentities.isEmpty)
  }

  // MARK: Acceptance 2 — lifecycle events cannot grant ownership or release another capture

  /// A shared lifecycle beat paints; it does not manufacture local ownership.
  func testForeignLifecyclePhaseDoesNotGrantLocalStopPermission() {
    let f = makeFixture(recording: [true])

    f.store.setDictationPhase(.recording)

    XCTAssertFalse(f.store.ownsLiveDictation)
    XCTAssertNil(f.store.composerCaptureHandle)
  }

  /// A terminal projection from a session that is already over must not release
  /// the capture the user has since started. Sessions are keyed by id, so the
  /// late event lands on its own (retired) session and nothing else.
  func testDelayedPriorSessionTerminalDoesNotReleaseTheCurrentCapture() {
    let f = makeFixture(recording: [false, true])
    let state = OverlayState()
    state.connectComposer(to: f.store)
    var stopped = 0
    state.onRecordingStopped = { stopped += 1 }
    admitCapture(f.store, threadID: f.threadA, id: "session-1")
    listening("first take", to: state, sessionId: "session-1")
    sessionEnded("first take", to: state, sessionId: "session-1")
    let afterFirst = stopped

    f.store.select(f.threadB)
    f.store.draft = "B typed"
    admitCapture(f.store, threadID: f.threadB, id: "session-2")
    f.store.dictationBlocked = true
    listening("second take", to: state, sessionId: "session-2")
    sessionEnded("first take", to: state, sessionId: "session-1")

    XCTAssertEqual(stopped, afterFirst, "retired A cannot stop B")
    XCTAssertEqual(f.store.composerCaptureHandle?.captureId, "session-2")
    XCTAssertEqual(f.store.dictationThreadID, f.threadB)
    XCTAssertEqual(f.store.dictationPhase, .recording)
    XCTAssertTrue(f.store.ownsLiveDictation)
    XCTAssertTrue(f.store.dictationBlocked)
    XCTAssertEqual(f.store.draft, "B typed")
    XCTAssertEqual(state.latestTranscriptProjection?.sessionId, "session-2")
    XCTAssertFalse(state.terminal)
    XCTAssertEqual(state.activeText, "second take")
  }

  func testUnseenOldTerminalCannotUseNewCapturePermissionBeforeItsFirstProjection() {
    let f = makeFixture(recording: [false, true])
    let state = OverlayState()
    state.connectComposer(to: f.store)
    admitCapture(f.store, threadID: f.threadB, id: "B")
    f.store.select(f.threadB)
    f.store.draft = "B draft"
    var stopped = 0
    state.onRecordingStopped = { stopped += 1 }
    sessionEnded("unseen A", to: state, sessionId: "A")
    sessionEnded("unseen A", to: state, sessionId: "A")
    XCTAssertEqual(stopped, 0)
    XCTAssertEqual(f.store.composerCaptureHandle?.captureId, "B")
    XCTAssertTrue(f.store.ownsLiveDictation)
    XCTAssertEqual(f.store.draft, "B draft")
    XCTAssertEqual(f.store.composerRecoveryDocuments.map(\.text), ["unseen A"])
  }

  func testDuplicateCaptureStartCannotReplaceAdmittedIdentity() async throws {
    let f = makeFixture(recording: [false])
    f.dictation.toggle()
    await settle(f)
    let request = try XCTUnwrap(f.store.currentComposerCaptureRequestID)
    let handle = try XCTUnwrap(f.store.composerCaptureHandle)
    let state = OverlayState()
    state.connectComposer(to: f.store)
    f.store.select(f.threadB)
    f.store.draft = "B typed"

    // Replayed success remains idempotent even when telemetry says idle.
    f.store.completeComposerCaptureStart(request, live: false, handle: handle)
    XCTAssertEqual(f.store.composerCaptureHandle?.captureId, "capture-A")
    XCTAssertEqual(f.store.dictationThreadID, f.threadA)
    XCTAssertTrue(f.store.ownsLiveDictation)
    XCTAssertEqual(f.store.dictationPhase, .recording)
    XCTAssertEqual(f.store.draft, "B typed")

    // B cannot obtain a new request while A owns admission. A duplicate reply
    // cannot use that existing request to register a foreign delivery owner.
    XCTAssertEqual(f.store.beginComposerCaptureRequest(threadID: f.threadB), request)
    f.store.completeComposerCaptureStart(
      request, live: true, handle: CsCaptureHandle(captureId: "foreign"))
    f.store.completeComposerCaptureStart(request, live: false, handle: nil)
    XCTAssertEqual(f.store.composerCaptureHandle?.captureId, "capture-A")
    XCTAssertEqual(f.store.currentComposerCaptureRequestID, request)
    XCTAssertEqual(f.store.dictationThreadID, f.threadA)
    XCTAssertTrue(f.store.ownsLiveDictation)
    XCTAssertEqual(f.store.dictationPhase, .recording)

    sessionEnded("foreign words", to: state, sessionId: "foreign")
    XCTAssertEqual(f.store.composerRecoveryDocuments.map(\.text), ["foreign words"])
    XCTAssertEqual(f.store.currentComposerCaptureRequestID, request)
    XCTAssertEqual(f.store.composerCaptureHandle?.captureId, "capture-A")
    XCTAssertEqual(f.store.draft, "B typed")
    f.dictation.toggle()
    await settle(f)
    XCTAssertEqual(f.surface.calls, [.isRecording, .startComposerTurn, .stop("capture-A")])

    sessionEnded("A words", to: state, sessionId: "capture-A")
    sessionEnded("A words", to: state, sessionId: "capture-A")
    XCTAssertFalse(f.store.hasComposerCaptureRequest)
    XCTAssertEqual(f.store.draft, "B typed")
    f.store.select(f.threadA)
    XCTAssertEqual(f.store.draft, "A words")
    XCTAssertEqual(f.store.composerRecoveryDocuments.map(\.text), ["foreign words"])
    XCTAssertTrue(f.store.threads.allSatisfy { $0.messages.isEmpty })
  }

  func testTerminalBeforeStartReplyRetainsTextAndCannotResurrectStopPermission() {
    let f = makeFixture(recording: [false, true])
    let state = OverlayState()
    state.connectComposer(to: f.store)
    let request = f.store.beginComposerCaptureRequest(threadID: f.threadA)
    sessionEnded("early text", to: state, sessionId: "early")
    f.store.completeComposerCaptureStart(request, live: true,
      handle: CsCaptureHandle(captureId: "early"))
    XCTAssertFalse(f.store.ownsLiveDictation)
    XCTAssertFalse(f.store.hasComposerCaptureRequest)
    XCTAssertNil(f.store.composerCaptureHandle)
    XCTAssertTrue(f.store.composerRecoveryDocuments.isEmpty)
    XCTAssertEqual(f.store.draft, "early text")
  }

  func testRecoveryRetriesAndIdenticalDistinctTakesKeepExactDocumentsUntilExplicitAction() throws {
    let f = makeFixture(recording: [false, true])
    let state = OverlayState()
    state.connectComposer(to: f.store)
    let text = "  Zażółć\nrepeat repeat\t🙂  "
    sessionEnded(text, to: state, sessionId: "A")
    sessionEnded(text, to: state, sessionId: "A")
    sessionEnded(text, to: state, sessionId: "B")
    sessionEnded(text, to: state, sessionId: "A")
    XCTAssertEqual(f.store.composerRecoveryDocuments.count, 2)
    for document in f.store.composerRecoveryDocuments {
      XCTAssertEqual(Array(document.text.utf8), Array(text.utf8))
    }
    let first = try XCTUnwrap(f.store.composerRecoveryDocuments.first)
    let second = try XCTUnwrap(f.store.composerRecoveryDocuments.last)
    f.store.select(f.threadB)
    f.store.draft = "typed B"
    XCTAssertFalse(f.store.insertComposerRecovery(first.id, into: f.threadA))
    XCTAssertFalse(f.store.copyComposerRecovery(first.id) { _ in false })
    XCTAssertEqual(f.store.composerRecoveryDocuments.count, 2)
    XCTAssertTrue(f.store.insertComposerRecovery(first.id, into: f.threadB))
    XCTAssertEqual(f.store.draft, "typed B\n" + text)
    XCTAssertFalse(f.store.insertComposerRecovery(first.id, into: f.threadB))
    var copied = ""
    XCTAssertTrue(f.store.copyComposerRecovery(second.id) { copied = $0; return true })
    XCTAssertEqual(Array(copied.utf8), Array(text.utf8))
    sessionEnded(text, to: state, sessionId: "A")
    sessionEnded(text, to: state, sessionId: "B")
    XCTAssertTrue(f.store.composerRecoveryDocuments.isEmpty, "consumed documents do not resurrect")
    XCTAssertEqual(f.store.selectedThreadID, f.threadB)
    XCTAssertTrue(f.store.threads.allSatisfy { $0.messages.isEmpty })
  }

  func testDeletedOwnerRecoveryCanBeDismissedWithoutSendingOrChangingSelection() throws {
    let f = makeFixture(recording: [false, true])
    admitCapture(f.store, threadID: f.threadA, id: "deleted")
    f.store.select(f.threadB)
    f.store.delete(try XCTUnwrap(f.store.threads.first { $0.id == f.threadA }))
    let state = OverlayState()
    state.connectComposer(to: f.store)
    sessionEnded("deleted owner words", to: state, sessionId: "deleted")
    let document = try XCTUnwrap(f.store.composerRecoveryDocuments.first)
    XCTAssertFalse(f.store.insertComposerRecovery(document.id, into: f.threadA))
    XCTAssertEqual(f.store.composerRecoveryDocuments.first?.text, "deleted owner words")
    f.store.dismissComposerRecovery(document.id)
    sessionEnded("deleted owner words", to: state, sessionId: "deleted")
    XCTAssertTrue(f.store.composerRecoveryDocuments.isEmpty)
    XCTAssertEqual(f.store.selectedThreadID, f.threadB)
    XCTAssertEqual(f.store.draft, "")
    XCTAssertTrue(f.store.threads.allSatisfy { $0.messages.isEmpty })
  }

  func testFreshMatchingTerminalThroughRealWiringDeliversOnceAndReleasesHandle() {
    let f = makeFixture(recording: [false, true])
    let state = OverlayState()
    state.connectComposer(to: f.store)
    admitCapture(f.store, threadID: f.threadA, id: "fresh")
    sessionEnded("  fresh bytes  ", to: state, sessionId: "fresh")
    sessionEnded("  fresh bytes  ", to: state, sessionId: "fresh")
    XCTAssertEqual(f.store.draft, "  fresh bytes  ")
    XCTAssertNil(f.store.composerCaptureHandle)
    XCTAssertFalse(f.store.hasComposerCaptureRequest)
    XCTAssertTrue(f.store.composerRecoveryDocuments.isEmpty)
  }

  func testRealAdapterJoinsTerminalThatArrivesBeforeStartAcknowledgment() async {
    let f = makeFixture(recording: [false, true])
    let state = OverlayState()
    state.connectComposer(to: f.store)
    f.surface.onStart = { [self] in
      sessionEnded("before acknowledgment", to: state, sessionId: "capture-A")
    }
    f.dictation.toggle()
    await settle(f)
    f.surface.onStart = nil
    XCTAssertEqual(f.store.draft, "before acknowledgment")
    XCTAssertFalse(f.store.ownsLiveDictation)
    XCTAssertNil(f.store.composerCaptureHandle)
    XCTAssertTrue(f.store.composerRecoveryDocuments.isEmpty)
  }

  func testWiredRevisionDeliversBeforeMatchingCaptureReleaseAndOnlyOnce() {
    let f = makeFixture(recording: [false, true])
    let state = OverlayState()
    state.connectComposer(to: f.store)
    admitCapture(f.store, threadID: f.threadA)
    listening("raw", to: state)
    terminalRevision("revised", to: state)
    XCTAssertTrue(f.store.ownsLiveDictation)
    XCTAssertEqual(f.store.draft, "")
    var draftAtStop: String?
    state.onRecordingStopped = { draftAtStop = f.store.draft }
    sessionEnded("revised", to: state)
    sessionEnded("revised", to: state)
    XCTAssertEqual(draftAtStop, "revised")
    XCTAssertEqual(f.store.draft, "revised")
    XCTAssertFalse(f.store.hasComposerCaptureRequest)
    XCTAssertNil(f.store.composerCaptureHandle)
    XCTAssertFalse(f.store.dictationBlocked)
  }

  // MARK: Acceptance 3 — recovery without a blind timer

  /// A false query is not a terminal receipt, including after transport failure.
  func testFailedStopCannotBeReconciledByFalseRecordingTelemetry() async {
    let f = makeFixture(recording: [false, true, true, false, false])
    f.dictation.toggle()
    await settle(f)
    f.surface.stopThrows = true

    f.dictation.toggle()
    await settle(f)
    let request = f.store.currentComposerCaptureRequestID
    XCTAssertEqual(
      f.store.dictationThreadID, f.threadA, "a failed stop keeps the pending destination")
    XCTAssertTrue(f.store.composerCaptureAwaitingTerminal)
    XCTAssertTrue(f.store.composerStopRetryAvailable)

    let calls = f.surface.calls
    f.surface.stopThrows = false
    f.dictation.toggle()
    await settle(f)

    XCTAssertEqual(
      f.surface.calls, calls + [.stop("capture-A")],
      "explicit retry addresses the same handle without querying or starting again")
    XCTAssertEqual(f.store.currentComposerCaptureRequestID, request)
    XCTAssertEqual(f.store.composerCaptureHandle?.captureId, "capture-A")
    XCTAssertTrue(f.store.composerCaptureAwaitingTerminal)
    XCTAssertFalse(f.store.composerStopRetryAvailable)
    XCTAssertFalse(f.store.finishDictationCapture(sessionID: "foreign"))
    XCTAssertEqual(f.store.currentComposerCaptureRequestID, request)
    f.dictation.toggle()
    await settle(f)
    XCTAssertEqual(f.surface.calls, calls + [.stop("capture-A")], "pending success is not retry permission")
    let state = OverlayState()
    state.connectComposer(to: f.store)
    sessionEnded("eventual words", to: state, sessionId: "capture-A")
    XCTAssertEqual(f.store.draft, "eventual words")
    XCTAssertFalse(f.store.hasComposerCaptureRequest)
  }

  /// A start still in flight is never replaced by a second press. Reconciliation
  /// applies only once stop permission has been spent.
  func testAPendingStartIsNotReplacedByASecondPress() async {
    let f = makeFixture(recording: [false, false, false])
    let gate = Gate(expectation(description: "start awaiting admission"))
    f.surface.onStart = { await gate.wait() }
    f.dictation.toggle()
    await fulfillment(of: [gate.entered], timeout: 1)
    XCTAssertTrue(f.store.hasComposerCaptureRequest)
    XCTAssertFalse(f.store.ownsLiveDictation, "no admitted handle yet")

    f.dictation.toggle()
    gate.release()
    await settle(f)

    XCTAssertEqual(
      f.surface.calls.filter { $0 == .startComposerTurn }.count, 1,
      "a request that never spent stop permission still blocks a replacement")
  }

  @MainActor
  private final class Gate {
    let entered: XCTestExpectation
    private var continuations: [CheckedContinuation<Void, Never>] = []
    private var released = false
    init(_ entered: XCTestExpectation) { self.entered = entered }
    func wait() async {
      guard !released else { return }
      await withCheckedContinuation { continuation in
        continuations.append(continuation)
        if continuations.count == 1 { entered.fulfill() }
      }
    }
    func release() {
      released = true
      let pending = continuations
      continuations.removeAll()
      for continuation in pending { continuation.resume() }
    }
  }

  func testFailureBannerExpiryKeepsPendingPresentationAndOriginalDelivery() async {
    let f = makeFixture(recording: [false])
    let expiry = Gate(expectation(description: "banner clock registered"))
    f.store.waitForDictationFailureExpiry = { await expiry.wait() }
    f.dictation.toggle()
    await settle(f)
    f.surface.stopThrows = true
    f.dictation.toggle()
    await settle(f)
    await fulfillment(of: [expiry.entered], timeout: 1)
    let request = f.store.currentComposerCaptureRequestID
    let bannerTask = f.store.dictationFailureTask
    f.store.select(f.threadB)
    f.store.draft = "B typed"
    expiry.release()
    await bannerTask?.value
    guard case .failed = f.store.dictationPhase else {
      return XCTFail("expiry must preserve the actionable Stop retry")
    }
    XCTAssertTrue(f.store.composerStopRetryAvailable)
    XCTAssertTrue(f.store.composerCaptureAwaitingTerminal)
    XCTAssertEqual(f.store.dictationThreadID, f.threadA)
    let calls = f.surface.calls
    f.surface.stopThrows = false
    f.dictation.toggle()
    await settle(f)
    XCTAssertEqual(f.surface.calls, calls + [.stop("capture-A")])
    XCTAssertEqual(f.store.currentComposerCaptureRequestID, request)
    XCTAssertEqual(f.store.composerCaptureHandle?.captureId, "capture-A")
    XCTAssertTrue(f.store.composerCaptureAwaitingTerminal)
    XCTAssertFalse(f.store.composerStopRetryAvailable)
    XCTAssertFalse(f.store.finishDictationCapture(sessionID: "foreign"))
    XCTAssertEqual(f.store.currentComposerCaptureRequestID, request)
    XCTAssertEqual(f.store.draft, "B typed")
    let state = OverlayState()
    state.connectComposer(to: f.store)
    sessionEnded("  A exact words  ", to: state, sessionId: "capture-A")
    sessionEnded("  A exact words  ", to: state, sessionId: "capture-A")
    XCTAssertEqual(f.store.draft, "B typed")
    XCTAssertFalse(f.store.hasComposerCaptureRequest)
    f.store.select(f.threadA)
    XCTAssertEqual(f.store.draft, "  A exact words  ")
  }

  func testStaleStopRepliesAndFailuresCannotRepaintOrReleaseSuccessor() async {
    let outcomes: [CsConditionalStop?] = [.stopped, .foreignCapture, .noLiveCapture, .alreadyStopping, .pending, .admissionUnavailable, nil]
    for outcome in outcomes {
      let f = makeFixture(recording: [false])
      f.dictation.toggle()
      await settle(f)
      let stop = Gate(expectation(description: "old Stop held"))
      f.surface.onStop = { await stop.wait() }
      f.surface.stopThrows = outcome == nil
      f.surface.stopOutcome = outcome ?? .stopped
      f.dictation.toggle()
      await fulfillment(of: [stop.entered], timeout: 1)
      let state = OverlayState()
      state.connectComposer(to: f.store)
      sessionEnded("old words", to: state, sessionId: "capture-A")
      f.store.select(f.threadB)
      f.store.draft = "new draft"
      admitCapture(f.store, threadID: f.threadB, id: "successor")
      stop.release()
      await settle(f)
      XCTAssertEqual(f.store.composerCaptureHandle?.captureId, "successor")
      XCTAssertEqual(f.store.dictationThreadID, f.threadB)
      XCTAssertEqual(f.store.dictationPhase, .recording)
      XCTAssertEqual(f.store.draft, "new draft")
      XCTAssertTrue(f.store.ownsLiveDictation)
    }
  }

  func testStaleFailureBannerCannotPaintIdleOverSuccessor() async {
    let f = makeFixture(recording: [false])
    let expiry = Gate(expectation(description: "old failure expiry registered"))
    f.store.waitForDictationFailureExpiry = { await expiry.wait() }
    f.dictation.toggle()
    await settle(f)
    f.surface.stopThrows = true
    f.dictation.toggle()
    await settle(f)
    await fulfillment(of: [expiry.entered], timeout: 1)
    let oldBanner = f.store.dictationFailureTask
    let state = OverlayState()
    state.connectComposer(to: f.store)
    sessionEnded("A", to: state, sessionId: "capture-A")
    f.surface.admittedCaptureId = "successor"
    f.store.select(f.threadB)
    f.dictation.toggle()
    await settle(f)
    expiry.release()
    await oldBanner?.value
    XCTAssertEqual(f.store.composerCaptureHandle?.captureId, "successor")
    XCTAssertEqual(f.store.dictationPhase, .recording)
    XCTAssertEqual(f.store.dictationThreadID, f.threadB)
  }

  func testEarlyTerminalJoinsInitiatingThreadAfterAcknowledgedStartBarrier() async {
    let f = makeFixture(recording: [false])
    let start = Gate(expectation(description: "start reply held"))
    f.surface.onStart = { await start.wait() }
    f.dictation.toggle()
    await fulfillment(of: [start.entered], timeout: 1)
    let state = OverlayState()
    state.connectComposer(to: f.store)
    f.store.select(f.threadB)
    f.store.draft = "B draft"
    sessionEnded("early A", to: state, sessionId: "capture-A")
    XCTAssertEqual(f.store.composerRecoveryDocuments.map(\.text), ["early A"])
    f.dictation.toggle()
    start.release()
    await settle(f)
    XCTAssertFalse(f.store.hasComposerCaptureRequest)
    XCTAssertFalse(f.store.ownsLiveDictation)
    XCTAssertEqual(f.store.draft, "B draft")
    XCTAssertTrue(f.store.composerRecoveryDocuments.isEmpty)
    sessionEnded("early A", to: state, sessionId: "capture-A")
    f.store.select(f.threadA)
    XCTAssertEqual(f.store.draft, "early A")
    XCTAssertEqual(f.surface.calls.filter { $0 == .startComposerTurn }.count, 1)
  }

  // MARK: Acceptance 5 — presentation terminal is not delivery terminal

  /// The production sequence that broke delivery: a Light+/formatter revision is
  /// terminal and arrives BEFORE `session_ended`. It must not consume the
  /// delivery slot, and the later lifecycle line must still insert — exactly
  /// once, even when the end is duplicated.
  func testTerminalRevisionBeforeSessionEndedStillDeliversExactlyOnce() {
    let state = OverlayState()
    var admitted: [String] = []
    state.onComposerTranscript = { text, _ in
      admitted.append(text)
      return .admitted(threadID: UUID())
    }

    listening("raw words", to: state)
    terminalRevision("formatted words", to: state)
    sessionEnded("formatted words", to: state)
    sessionEnded("formatted words", to: state)

    XCTAssertEqual(admitted, ["formatted words"])
  }

  /// A formatter that changed nothing produces the same ordering with identical
  /// bytes. Sameness of text is not a reason to skip a delivery.
  func testNoOpFormatterRevisionStillLeavesTheDeliveryToTheLifecycleLine() {
    let state = OverlayState()
    var admitted: [String] = []
    state.onComposerTranscript = { text, _ in
      admitted.append(text)
      return .admitted(threadID: UUID())
    }

    listening("unchanged", to: state)
    terminalRevision("unchanged", to: state)
    sessionEnded("unchanged", to: state)

    XCTAssertEqual(admitted, ["unchanged"])
  }

  /// A refused seal still has committed words, and they are still deliverable.
  /// The refusal degrades the claim about coverage, never the route.
  func testRefusedSealStillDeliversItsCommittedWords() {
    let state = OverlayState()
    var admitted: [String] = []
    state.onComposerTranscript = { text, _ in
      admitted.append(text)
      return .admitted(threadID: UUID())
    }

    listening("partial words", to: state)
    sessionEnded("partial words", to: state)

    XCTAssertEqual(admitted, ["partial words"])
  }

  /// An empty capture claims nothing in either direction: no delivery, and no
  /// failure either.
  func testEmptyCaptureMakesNoDeliveryClaimAtAll() {
    let state = OverlayState()
    var calls = 0
    state.onComposerTranscript = { _, _ in
      calls += 1
      return .empty
    }

    sessionEnded("   ", to: state)

    XCTAssertEqual(calls, 0)
    XCTAssertNil(state.retainedComposerDelivery)
  }

  // MARK: Acceptance 6 — no success claim without a receipt

  /// No receiver is wired. The text must stay accessible rather than be reported
  /// as delivered to a composer that never saw it.
  func testMissingReceiverRetainsTheTextInsteadOfClaimingDelivery() {
    let state = OverlayState()
    state.onComposerTranscript = nil

    listening("orphan words", to: state)
    sessionEnded("orphan words", to: state)

    XCTAssertEqual(state.retainedComposerDelivery, "orphan words")
  }

  /// The receiver refuses. The overlay keeps the exact bytes and does not mark
  /// the session delivered, so a later retry is still possible.
  func testReceiverRejectionKeepsTheDeliveryRecoverableAndRetryable() {
    let state = OverlayState()
    var offers = 0
    state.onComposerTranscript = { text, _ in
      offers += 1
      return .retained(text)
    }

    listening("refused words", to: state)
    sessionEnded("refused words", to: state)
    XCTAssertEqual(state.retainedComposerDelivery, "refused words")

    sessionEnded("refused words", to: state)
    XCTAssertEqual(offers, 2, "a refused delivery does not consume the session's delivery slot")
  }

  /// A refused handover must not fall back to submitting the words. The take was
  /// routed to a draft; nobody asked for it to be sent.
  func testARefusedHandoverStillCancelsTheOverlayAutoSend() {
    let state = OverlayState()
    var sentToAgent: [String] = []
    state.onSendToAgent = { sentToAgent.append($0) }
    state.onComposerTranscript = { text, _ in .retained(text) }

    listening("refused words", to: state)
    sessionEnded("refused words", to: state)
    state.fireAutoHideNowForTests()

    XCTAssertEqual(state.retainedComposerDelivery, "refused words")
    XCTAssertTrue(sentToAgent.isEmpty, "a failed handover never becomes an automatic send")
  }

  /// A take the controller did not route to the composer is never offered to it.
  /// The typed disposition is the whole gate; the display label is not read.
  func testATakeWithoutAComposerDispositionIsNeverOfferedToTheComposer() {
    let state = OverlayState()
    var offers = 0
    state.onComposerTranscript = { _, _ in
      offers += 1
      return .admitted(threadID: UUID())
    }

    listening("pasted words", to: state)
    sessionEnded("pasted words", to: state, delivery: .sinkAccepted)

    XCTAssertEqual(offers, 0)
  }

  // MARK: Acceptance 7 — the result belongs to the capturing thread

  /// Record in A, switch to B and type there, then finish A. B keeps its own
  /// draft and attachments, the selection does not move, and A's words wait for
  /// A rather than being appended to whatever is on screen.
  func testResultTargetsTheCapturingThreadWithoutStealingTheCurrentSelection() {
    let f = makeFixture(recording: [false, true])
    admitCapture(f.store, threadID: f.threadA)
    f.store.select(f.threadB)
    f.store.draft = "typed in B"
    f.store.pendingAttachments = [PendingAttachment(url: URL(fileURLWithPath: "/tmp/b.png"))]
    let bAttachmentIDs = f.store.pendingAttachments.map(\.id)

    let receipt = f.store.receiveDictationTranscript("words from A", captureID: "join-session")

    XCTAssertEqual(receipt, .parked(threadID: f.threadA))
    XCTAssertEqual(f.store.selectedThreadID, f.threadB, "delivery never moves the rail")
    XCTAssertEqual(f.store.draft, "typed in B", "B's draft is B's")
    XCTAssertNil(f.store.unsentDictationNotice, "A's unsent marker must not paint B")
    XCTAssertEqual(f.store.pendingAttachments.count, 1, "B keeps its staged attachments")

    f.store.select(f.threadA)
    XCTAssertEqual(f.store.draft, "words from A", "A's words surface in A")
    XCTAssertEqual(f.store.unsentDictationNotice, "Not sent — dictated text is in the draft")
    XCTAssertTrue(f.store.pendingAttachments.isEmpty, "B's image did not travel to A")

    // The return trip is the other half of the same claim: B's composition was
    // parked, not consumed to make A's assertion true.
    f.store.select(f.threadB)
    XCTAssertEqual(f.store.draft, "typed in B", "B is exactly as the user left it")
    XCTAssertNil(f.store.unsentDictationNotice)
    XCTAssertEqual(f.store.pendingAttachments.map(\.id), bAttachmentIDs, "the same staged files")
  }

  /// When the capturing thread is the one on screen, the words land in the live
  /// draft and join an existing one instead of gluing onto its last word.
  func testDeliveryToTheSelectedOwnerAppendsToTheLiveDraft() {
    let f = makeFixture(recording: [false, true])
    admitCapture(f.store, threadID: f.threadA)
    f.store.draft = "already here"

    let receipt = f.store.receiveDictationTranscript("spoken words", captureID: "join-session")

    XCTAssertEqual(receipt, .admitted(threadID: f.threadA))
    XCTAssertEqual(f.store.draft, "already here\nspoken words")
  }

  func testAgentVoiceDeliveryStaysUnsentAndNamesTheDraft() {
    let f = makeFixture(recording: [false, true])
    let state = OverlayState()
    state.connectComposer(to: f.store)
    admitCapture(f.store, threadID: f.threadA)
    let messagesBefore = f.store.currentThread?.messages.count

    listening("the complete", to: state)
    XCTAssertEqual(f.store.draft, "", "live preview stays outside the draft")
    sessionEnded("the complete dictated message", to: state)

    XCTAssertEqual(f.store.draft, "the complete dictated message")
    XCTAssertEqual(f.store.currentThread?.messages.count, messagesBefore)
    XCTAssertEqual(f.store.unsentDictationNotice, "Not sent — dictated text is in the draft")
  }

  func testMidCaptureSendDoesNotConsumeTheTerminalDictation() {
    let f = makeFixture(recording: [false, true])
    let state = OverlayState()
    state.connectComposer(to: f.store)
    admitCapture(f.store, threadID: f.threadA)
    f.store.setDictationPhase(.recording)
    f.store.draft = "mi"
    f.store.send(origin: .enter)
    XCTAssertEqual(f.store.draft, "")

    sessionEnded("mi and every word that followed", to: state)
    XCTAssertEqual(f.store.draft, "mi and every word that followed")
    XCTAssertEqual(f.store.unsentDictationNotice, "Not sent — dictated text is in the draft")
  }

  func testAcceptedSendCarriesOriginSizeThreadAndRecordingStateToRustSeam() {
    let engine = HeldReplyEngine()
    let store = AgentChatStore(
      engine: engine, threadsProvider: StubThreadsProvider(),
      persistenceDefaults: isolatedDefaults())
    let thread = store.threads.first { $0.backendId == "t_a" }!.id
    store.select(thread)
    store.setDictationPhase(.recording)
    store.draft = "mi"
    store.send(origin: .enter)

    XCTAssertEqual(engine.sendOrigins.count, 1)
    XCTAssertEqual(engine.sendOrigins.first?.0, .enter)
    XCTAssertEqual(engine.sendOrigins.first?.1, 2)
    XCTAssertEqual(engine.sendOrigins.first?.2, "t_a")
    XCTAssertEqual(engine.sendOrigins.first?.3, true)
    engine.state.cancelAll()
  }

  /// The capturing thread was deleted while the take was in flight. There is no
  /// destination left, so the words become explicit recovery — never a silent
  /// drop and never a dangling selection.
  func testDeletedCapturingThreadYieldsExplicitRetainedRecovery() {
    let f = makeFixture(recording: [false, true])
    let threadA = f.store.threads.first { $0.id == f.threadA }!
    admitCapture(f.store, threadID: f.threadA)
    f.store.select(f.threadB)
    f.store.delete(threadA)

    let receipt = f.store.receiveDictationTranscript("words with nowhere to go", captureID: "join-session")

    XCTAssertEqual(receipt, .retained("words with nowhere to go"))
    XCTAssertEqual(f.store.retainedComposerDelivery, "words with nowhere to go")
    XCTAssertEqual(f.store.selectedThreadID, f.threadB)
  }

  /// A delivery already parked for a thread that is then deleted is surfaced for
  /// recovery rather than removed with the thread.
  func testDeletingAThreadSurfacesItsParkedDeliveryForRecovery() {
    let f = makeFixture(recording: [false, true])
    admitCapture(f.store, threadID: f.threadA)
    f.store.select(f.threadB)
    XCTAssertEqual(f.store.receiveDictationTranscript("parked words", captureID: "join-session"), .parked(threadID: f.threadA))

    f.store.delete(f.store.threads.first { $0.id == f.threadA }!)

    XCTAssertEqual(f.store.retainedComposerDelivery, "parked words")
  }

  /// A capture with no owner at all cannot be guessed into one.
  func testDeliveryWithoutAnOwningThreadIsRetainedNotRoutedToTheSelection() {
    let f = makeFixture(recording: [false, true])
    f.store.select(f.threadB)
    f.store.draft = "typed in B"

    let receipt = f.store.receiveDictationTranscript("ownerless words", captureID: "join-session")

    XCTAssertEqual(receipt, .retained("ownerless words"))
    XCTAssertEqual(f.store.draft, "typed in B")
  }

  // MARK: Acceptance 8 — parked text is offered, never sent

  /// Delivery parks the words in a draft the user can still edit. Nothing here
  /// submits them, and the overlay's own deadline is told to stop trying.
  func testAdmittedDeliveryCancelsTheOverlayAutoSendInsteadOfSubmitting() {
    let f = makeFixture(recording: [false, true])
    admitCapture(f.store, threadID: f.threadA)
    let state = OverlayState()
    state.connectComposer(to: f.store)

    listening("draft words", to: state)
    sessionEnded("draft words", to: state)

    XCTAssertEqual(f.store.draft, "draft words", "the words are in the composer, unsent")
    XCTAssertTrue(f.store.threads.allSatisfy { $0.messages.isEmpty }, "nothing was submitted")
  }

  // MARK: Thread-owned composer — every thread keeps its own unsent work

  /// The full round trip with work on both sides: A is mid-sentence, B is
  /// mid-sentence with an image staged, and A's take lands while B is on screen.
  /// Both compositions must survive both directions of the switch, and A's
  /// delivered words must join *A's* sentence rather than B's.
  func testEveryThreadKeepsItsOwnComposerAcrossAFullRoundTrip() {
    let f = makeFixture(recording: [false, true])
    f.store.draft = "half a thought in A"
    admitCapture(f.store, threadID: f.threadA)

    f.store.select(f.threadB)
    f.store.draft = "half a thought in B"
    f.store.addAttachments([URL(fileURLWithPath: "/tmp/round-trip-b.png")])
    let bAttachmentIDs = f.store.pendingAttachments.map(\.id)

    XCTAssertEqual(f.store.receiveDictationTranscript("spoken for A", captureID: "join-session"), .parked(threadID: f.threadA))

    f.store.select(f.threadA)
    XCTAssertEqual(
      f.store.draft, "half a thought in A\nspoken for A",
      "the delivery joins A's own sentence, not the one typed in B")
    XCTAssertTrue(f.store.pendingAttachments.isEmpty, "A never staged anything")

    f.store.select(f.threadB)
    XCTAssertEqual(f.store.draft, "half a thought in B")
    XCTAssertEqual(f.store.pendingAttachments.map(\.id), bAttachmentIDs)

    f.store.select(f.threadA)
    XCTAssertEqual(
      f.store.draft, "half a thought in A\nspoken for A",
      "returning a second time is not a second delivery")
  }

  /// Two takes finish for A while B is on screen. Selecting A shows each
  /// document exactly once, and selecting A again does not repeat them.
  func testRepeatedSelectionSurfacesEachDeliveredDocumentExactlyOnce() throws {
    let f = makeFixture(recording: [false, true])
    admitCapture(f.store, threadID: f.threadA)
    let predecessor = try XCTUnwrap(f.store.currentComposerCaptureRequestID)
    f.store.select(f.threadB)

    XCTAssertEqual(f.store.receiveDictationTranscript("first take", captureID: "join-session"), .parked(threadID: f.threadA))
    // Receiving the document does not release the admitted capture request.
    XCTAssertEqual(f.store.currentComposerCaptureRequestID, predecessor)
    XCTAssertTrue(f.store.finishDictationCapture(sessionID: "join-session"))
    XCTAssertFalse(f.store.hasComposerCaptureRequest)
    XCTAssertNil(f.store.composerCaptureHandle)
    XCTAssertFalse(f.store.ownsLiveDictation)
    admitCapture(f.store, threadID: f.threadA, id: "second-session")
    let successor = try XCTUnwrap(f.store.currentComposerCaptureRequestID)
    XCTAssertNotEqual(successor, predecessor)
    XCTAssertEqual(f.store.composerCaptureHandle?.captureId, "second-session")
    XCTAssertEqual(f.store.receiveDictationTranscript("second take", captureID: "second-session"), .parked(threadID: f.threadA))

    XCTAssertTrue(f.store.finishDictationCapture(sessionID: "second-session"))
    XCTAssertFalse(f.store.hasComposerCaptureRequest)
    XCTAssertNil(f.store.composerCaptureHandle)
    XCTAssertFalse(f.store.ownsLiveDictation)
    XCTAssertEqual(f.store.selectedThreadID, f.threadB)

    f.store.select(f.threadA)
    XCTAssertEqual(f.store.draft, "first take\nsecond take")
    let focusAfterFirstArrival = f.store.composerFocusRequest

    f.store.select(f.threadB)
    f.store.select(f.threadA)
    XCTAssertEqual(f.store.draft, "first take\nsecond take", "no document is delivered twice")
    XCTAssertEqual(
      f.store.composerFocusRequest, focusAfterFirstArrival,
      "an already-surfaced document does not grab the caret again")
  }

  /// A delivery for a thread the user is not reading changes nothing they can
  /// see: not the selection, not the composer, not the caret.
  func testADeliveryForAnUnselectedThreadChangesNothingOnScreen() {
    let f = makeFixture(recording: [false, true])
    admitCapture(f.store, threadID: f.threadA)
    f.store.select(f.threadB)
    f.store.draft = "mid-sentence in B"
    f.store.addAttachments([URL(fileURLWithPath: "/tmp/untouched-b.png")])
    let bAttachmentIDs = f.store.pendingAttachments.map(\.id)
    let focusBefore = f.store.composerFocusRequest

    XCTAssertEqual(f.store.receiveDictationTranscript("for A only", captureID: "join-session"), .parked(threadID: f.threadA))

    XCTAssertEqual(f.store.selectedThreadID, f.threadB)
    XCTAssertEqual(f.store.draft, "mid-sentence in B")
    XCTAssertEqual(f.store.pendingAttachments.map(\.id), bAttachmentIDs)
    XCTAssertEqual(f.store.composerFocusRequest, focusBefore, "no caret jump for a parked take")
  }

  // MARK: Every selection entrypoint hands the composer over

  /// The rail assigns the published property directly rather than calling
  /// `select`. Ownership lives in the property observer precisely so that this
  /// path cannot be the one that forgets.
  func testDirectSelectionAssignmentHandsTheComposerOverToo() {
    let f = makeFixture(recording: [false, true])
    f.store.draft = "typed in A"
    f.store.addAttachments([URL(fileURLWithPath: "/tmp/direct-a.png")])
    let aAttachmentIDs = f.store.pendingAttachments.map(\.id)

    f.store.selectedThreadID = f.threadB

    XCTAssertEqual(f.store.draft, "", "B never had a composition")
    XCTAssertTrue(f.store.pendingAttachments.isEmpty, "A's image stays with A")

    f.store.selectedThreadID = f.threadA
    XCTAssertEqual(f.store.draft, "typed in A")
    XCTAssertEqual(f.store.pendingAttachments.map(\.id), aAttachmentIDs)
  }

  /// Starting a new conversation opens an empty composer. It must not do that by
  /// destroying the sentence in progress, and the images staged for the previous
  /// thread must not follow the user into the new one.
  func testANewThreadOpensAnEmptyComposerAndParksTheUnfinishedOne() {
    let f = makeFixture(recording: [false, true])
    f.store.draft = "unfinished in A"
    f.store.addAttachments([URL(fileURLWithPath: "/tmp/new-thread-a.png")])
    let aAttachmentIDs = f.store.pendingAttachments.map(\.id)

    f.store.newThread()

    XCTAssertNotEqual(f.store.selectedThreadID, f.threadA)
    XCTAssertEqual(f.store.draft, "", "a fresh thread starts with an empty box")
    XCTAssertTrue(f.store.pendingAttachments.isEmpty, "staged images do not follow the user")

    f.store.select(f.threadA)
    XCTAssertEqual(f.store.draft, "unfinished in A", "the sentence was parked, not deleted")
    XCTAssertEqual(f.store.pendingAttachments.map(\.id), aAttachmentIDs)
  }

  /// Negative control. A thread that has never held a composition contributes
  /// nothing on selection — the composer is empty because that thread's box is
  /// empty, not because a previous owner's text leaked in and was cleared.
  func testSelectingAThreadWithNoStoredCompositionLeavesAnEmptyComposer() {
    let f = makeFixture(recording: [false, true])

    f.store.select(f.threadB)

    XCTAssertEqual(f.store.draft, "")
    XCTAssertTrue(f.store.pendingAttachments.isEmpty)

    f.store.select(f.threadA)
    XCTAssertEqual(f.store.draft, "", "an empty thread stores nothing to hand back")
    XCTAssertTrue(f.store.pendingAttachments.isEmpty)
  }

  /// Filtering and reloading the rail rebuilds rows. Compositions are keyed by
  /// the thread's stable id, which those paths preserve, so unsent work survives
  /// a search round trip instead of following whichever row ends up selected.
  func testSearchAndRefreshRestoreReturnEachThreadToItsOwnComposition() {
    let f = makeFixture(recording: [false, true])
    f.store.draft = "written in A"
    f.store.select(f.threadB)
    f.store.draft = "written in B"
    f.store.addAttachments([URL(fileURLWithPath: "/tmp/search-b.png")])
    let bAttachmentIDs = f.store.pendingAttachments.map(\.id)

    f.store.searchThreads("Thread")
    f.store.searchThreads("")
    f.store.refreshThreads()

    XCTAssertEqual(f.store.selectedThreadID, f.threadB, "filtering is not a selection gesture")
    XCTAssertEqual(f.store.draft, "written in B")
    XCTAssertEqual(f.store.pendingAttachments.map(\.id), bAttachmentIDs)

    f.store.select(f.threadA)
    XCTAssertEqual(f.store.draft, "written in A", "A's words came back to A across the rebuild")
  }

  // MARK: Sending consumes one owner's composition

  /// Send empties the composer it was sent from and nothing else.
  func testSendConsumesOnlyTheSelectedThreadsComposition() {
    let f = makeFixture(recording: [false, true])
    f.store.select(f.threadB)
    f.store.draft = "still unsent in B"
    f.store.addAttachments([URL(fileURLWithPath: "/tmp/send-b.png")])
    let bAttachmentIDs = f.store.pendingAttachments.map(\.id)

    f.store.select(f.threadA)
    f.store.draft = "ask A"
    f.store.addAttachments([URL(fileURLWithPath: "/tmp/send-a.png")])
    f.store.send()

    XCTAssertEqual(f.store.draft, "", "A's own composer was consumed")
    XCTAssertTrue(f.store.pendingAttachments.isEmpty)
    let sent = f.store.threads.first { $0.id == f.threadA }?.messages.first { $0.role == .you }
    XCTAssertEqual(sent?.text, "ask A")
    XCTAssertEqual(sent?.attachments.count, 1, "the send carried A's staged file, not B's")

    f.store.select(f.threadB)
    XCTAssertEqual(f.store.draft, "still unsent in B", "B's message was never sent for it")
    XCTAssertEqual(f.store.pendingAttachments.map(\.id), bAttachmentIDs)
  }

  /// A turn accepted, queued behind an active one, and finally streamed in A
  /// touches no part of B's unsent composition at any point in that lifecycle.
  func testAQueuedAndStreamingTurnNeverEmptiesAnotherThreadsComposer() async {
    let engine = HeldReplyEngine()
    defer { engine.state.cancelAll() }
    let firstStarted = expectation(description: "first continuation registered")
    let secondStarted = expectation(description: "second continuation registered")
    engine.state.onStart = { text in
      if text == "first ask" { firstStarted.fulfill() }
      if text == "second ask" { secondStarted.fulfill() }
    }
    let store = AgentChatStore(engine: engine, threadsProvider: StubThreadsProvider(),
      persistenceDefaults: isolatedDefaults())
    let threadA = store.threads.first { $0.backendId == "t_a" }!.id
    let threadB = store.threads.first { $0.backendId == "t_b" }!.id

    store.select(threadB)
    store.draft = "waiting in B"
    store.addAttachments([URL(fileURLWithPath: "/tmp/queued-b.png")])
    let bAttachmentIDs = store.pendingAttachments.map(\.id)

    store.select(threadA)
    store.draft = "first ask"
    store.send()
    await fulfillment(of: [firstStarted], timeout: 2)
    guard engine.state.startedTexts.count == 1 else { return }

    store.draft = "second ask"
    store.send()
    XCTAssertEqual(store.queuedTurns.map(\.text), ["second ask"], "the second ask is queued")

    engine.state.finishNext("first reply")
    await fulfillment(of: [secondStarted], timeout: 2)
    guard engine.state.startedTexts.count == 2 else { return }
    engine.state.finishNext("second reply")
    await store.waitForComposerTurns(in: threadA)
    XCTAssertTrue(store.queuedTurns.isEmpty)
    XCTAssertNil(store.activeComposerTurn)
    XCTAssertEqual(engine.state.startedTexts, ["first ask", "second ask"])
    let replies = store.threads.first { $0.id == threadA }?.messages.filter { $0.role == .assistant }
    XCTAssertEqual(replies?.map(\.text), ["first reply", "second reply"])

    store.select(threadB)
    XCTAssertEqual(store.draft, "waiting in B", "B's sentence outlived A's whole turn lifecycle")
    XCTAssertEqual(store.pendingAttachments.map(\.id), bAttachmentIDs)
  }

  func testIndependentStorePersistenceCannotCorruptSentinelPendingWork() async throws {
    let sentinelDefaults = isolatedDefaults()
    let otherDefaults = isolatedDefaults()
    let engine = HeldReplyEngine()
    defer { engine.state.cancelAll() }
    let started = expectation(description: "sentinel continuation registered")
    engine.state.onStart = { _ in started.fulfill() }
    let sentinel = AgentChatStore(engine: engine, threadsProvider: StubThreadsProvider(),
      persistenceDefaults: sentinelDefaults)
    let sentinelThread = try XCTUnwrap(sentinel.selectedThreadID)
    sentinel.draft = "sentinel pending work"
    sentinel.addAttachments([URL(fileURLWithPath: "/tmp/sentinel.png")])
    sentinel.send()
    await fulfillment(of: [started], timeout: 2)
    guard engine.state.startedTexts.count == 1 else { return }
    let accepted = try XCTUnwrap(sentinelDefaults.data(forKey: AgentChatStore.acceptedTurnsDefaultsKey))
    let attachments = try XCTUnwrap(sentinelDefaults.data(forKey: AgentChatStore.attachmentMetadataDefaultsKey))

    let other = AgentChatStore(threadsProvider: StubThreadsProvider(), persistenceDefaults: otherDefaults)
    let otherThread = try XCTUnwrap(other.selectedThreadID)
    other.draft = "independent send"
    other.addAttachments([URL(fileURLWithPath: "/tmp/other.png")])
    other.send()
    await other.waitForComposerTurns(in: otherThread)
    XCTAssertEqual(sentinelDefaults.data(forKey: AgentChatStore.acceptedTurnsDefaultsKey), accepted)
    XCTAssertEqual(sentinelDefaults.data(forKey: AgentChatStore.attachmentMetadataDefaultsKey), attachments)
    engine.state.finishNext("sentinel reply")
    await sentinel.waitForComposerTurns(in: sentinelThread)
  }

  func testHeldEngineCancellationReleasesRegisteredContinuation() async {
    let engine = HeldReplyEngine()
    let started = expectation(description: "continuation ready for cancellation")
    engine.state.onStart = { _ in started.fulfill() }
    let store = AgentChatStore(engine: engine, threadsProvider: StubThreadsProvider(),
      persistenceDefaults: isolatedDefaults())
    guard let thread = store.selectedThreadID else { return XCTFail("missing thread") }
    defer { engine.state.cancelAll() }
    store.draft = "cancel me"
    store.send()
    await fulfillment(of: [started], timeout: 2)
    engine.state.cancelAll()
    await store.waitForComposerTurns(in: thread)
    XCTAssertNil(store.activeComposerTurn)
    XCTAssertTrue(store.queuedTurns.isEmpty)
  }

  // MARK: Deletion recovers words instead of migrating them

  /// Deleting a thread the user is not looking at takes its staged files with it
  /// and leaves its words recoverable — never silently poured into whichever
  /// thread happens to be selected.
  func testDeletingAnUnselectedThreadRecoversItsWordsAndStrandsNoAttachments() {
    let f = makeFixture(recording: [false, true])
    f.store.draft = "left behind in A"
    f.store.addAttachments([URL(fileURLWithPath: "/tmp/deleted-a.png")])

    f.store.select(f.threadB)
    f.store.draft = "still typing in B"

    f.store.delete(f.store.threads.first { $0.id == f.threadA }!)

    XCTAssertEqual(f.store.retainedComposerDelivery, "left behind in A")
    XCTAssertEqual(f.store.selectedThreadID, f.threadB, "deleting elsewhere does not move the rail")
    XCTAssertEqual(f.store.draft, "still typing in B")
    XCTAssertTrue(f.store.pendingAttachments.isEmpty, "the deleted thread's image is not re-staged")
  }

  /// Deleting the thread that is on screen moves the selection. Its composition
  /// must not travel with the cursor to the next thread.
  func testDeletingTheSelectedThreadNeverMigratesItsCompositionToTheNextOne() {
    let f = makeFixture(recording: [false, true])
    f.store.draft = "about to be deleted"
    f.store.addAttachments([URL(fileURLWithPath: "/tmp/deleted-selected.png")])

    f.store.delete(f.store.threads.first { $0.id == f.threadA }!)

    XCTAssertEqual(f.store.selectedThreadID, f.threadB)
    XCTAssertEqual(f.store.draft, "", "the next thread's composer is its own, and it is empty")
    XCTAssertTrue(f.store.pendingAttachments.isEmpty)
    XCTAssertEqual(
      f.store.retainedComposerDelivery, "about to be deleted",
      "the words are recoverable, not dropped and not auto-sent")
    XCTAssertTrue(
      f.store.threads.allSatisfy { $0.messages.isEmpty }, "recovery never submits anything")
  }

  /// The overlay's new presentation fence retires the previous take at the
  /// controller's capture admission (`preparing`/`started`). This is the owned
  /// delivery-side proof that the fence changed presentation only: the retired
  /// take's late lifecycle terminal still completes its addressed release, and
  /// it still cannot touch the successor's capture, composer or canvas.
  ///
  /// The sibling test `testDelayedPriorSessionTerminalDoesNotReleaseTheCurrentCapture`
  /// covers the same seam WITHOUT lifecycle beats; the beats are what this cut
  /// added, so they get their own witness rather than a loosened existing one.
  func testCaptureAdmissionBetweenTakesKeepsRetiredDeliveryAndSuccessorCanvasApart() {
    let f = makeFixture(recording: [false, true])
    let state = OverlayState()
    state.connectComposer(to: f.store)
    var stopped = 0
    state.onRecordingStopped = { stopped += 1 }
    var endedSessions: [String] = []
    let storeRelease = state.onCaptureEnded
    state.onCaptureEnded = { sessionID in
      endedSessions.append(sessionID)
      storeRelease?(sessionID)
    }

    admitCapture(f.store, threadID: f.threadA, id: "session-1")
    state.handleRecordingPreparing()
    state.handleRecordingStarted()
    listening("first take", to: state, sessionId: "session-1")
    XCTAssertEqual(state.activeText, "first take")
    state.finishControllerRecording()
    let afterFirst = stopped

    // An explicit addressed stop response releases the request, but preserves
    // its delivery receipt. Presentation completion alone cannot admit B.
    let predecessor = f.store.currentComposerCaptureRequestID
    XCTAssertEqual(f.store.beginComposerCaptureRequest(threadID: f.threadB), predecessor)
    if let request = predecessor, let handle = f.store.composerCaptureHandle {
      f.store.applyComposerStopOutcome(.noLiveCapture, requestID: request, handle: handle)
    } else {
      return XCTFail("predecessor must still own an admitted request")
    }
    XCTAssertFalse(f.store.hasComposerCaptureRequest)
    // A second take is legally admitted before the first terminal arrives.
    f.store.select(f.threadB)
    f.store.draft = "B typed"
    admitCapture(f.store, threadID: f.threadB, id: "session-2")
    let successor = f.store.currentComposerCaptureRequestID
    XCTAssertNotEqual(successor, predecessor)
    f.store.dictationBlocked = true
    state.handleRecordingPreparing()
    XCTAssertEqual(
      state.activeText, "", "the successor's canvas opens empty at capture admission")
    XCTAssertEqual(state.pendingSupersededTake?.sessionId, "session-1")
    state.handleRecordingStarted()
    listening("second take", to: state, sessionId: "session-2")

    // Now the predecessor's lifecycle terminal lands.
    sessionEnded("first take", to: state, sessionId: "session-1")

    XCTAssertEqual(
      endedSessions, ["session-1"],
      "the retired take still releases exactly its own capture")
    XCTAssertEqual(stopped, afterFirst, "and never issues a stop for the successor")
    XCTAssertEqual(f.store.composerCaptureHandle?.captureId, "session-2")
    XCTAssertEqual(f.store.dictationThreadID, f.threadB)
    XCTAssertEqual(f.store.currentComposerCaptureRequestID, successor)
    XCTAssertEqual(f.store.dictationPhase, .recording)
    XCTAssertTrue(f.store.ownsLiveDictation)
    XCTAssertTrue(f.store.dictationBlocked)
    XCTAssertEqual(f.store.draft, "B typed", "the successor's composer is untouched")
    XCTAssertTrue(
      f.store.threads.allSatisfy { $0.messages.isEmpty },
      "a retired terminal is never auto-submitted")

    XCTAssertEqual(state.latestTranscriptProjection?.sessionId, "session-2")
    XCTAssertEqual(state.activeText, "second take")
    XCTAssertEqual(state.mode, .listening)
    XCTAssertFalse(state.terminal)
    f.store.select(f.threadA)
    XCTAssertEqual(f.store.draft, "first take", "late delivery still belongs to the predecessor")
    f.store.select(f.threadB)
    XCTAssertEqual(f.store.draft, "B typed")
  }

  // MARK: Refusal recovery (rc-w2-refusal-ui) — UNRUN under W2

  /// One lifecycle terminal carrying `coverage_refused`, one production
  /// receiver, one admission.
  ///
  /// The whole point of routing refusal through the SAME `ComposerPending`
  /// path is that the receiver never learns the take was refused — a
  /// destination is not a quality judgement. What the receiver must see is a
  /// document for the thread that opened the capture, exactly once.
  private func refusedSessionEnded(
    _ text: String, to state: OverlayState, sessionId: String = "join-session",
    delivery: CsTranscriptDelivery = .composerPending
  ) {
    project(
      text, to: state, sessionId: sessionId, phase: "coverage_refused", terminal: true,
      lifecycleTerminal: true, delivery: delivery, reducerAction: "session_ended")
  }

  func testRefusedCoverageIsAdmittedOnceByTheThreadThatOwnsTheCapture() {
    let f = makeFixture(recording: [false, true])
    let state = OverlayState()
    state.connectComposer(to: f.store)
    admitCapture(f.store, threadID: f.threadA, id: "join-session")
    f.store.select(f.threadA)
    f.store.draft = "typed"

    refusedSessionEnded("unsealed words", to: state)

    XCTAssertEqual(f.store.draft, "typed\nunsealed words")
    XCTAssertEqual(state.mode, .coverageRefused, "the receiver's acceptance is not a seal")
    XCTAssertNotNil(state.coverageRefusalNotice)
    XCTAssertNil(state.retainedComposerDelivery, "an admitted document is not retained twice")
    XCTAssertTrue(
      f.store.threads.allSatisfy { $0.messages.isEmpty },
      "a refused take must not acquire auto-send it never had")

    // A duplicate lifecycle terminal for the same identity is one delivery.
    refusedSessionEnded("unsealed words", to: state)
    XCTAssertEqual(f.store.draft, "typed\nunsealed words")
  }

  /// The receiver refuses and owns the recovery. The overlay must record that
  /// the words are somewhere reachable WITHOUT keeping a second copy of its
  /// own — two owners for one document is how one of them goes stale.
  func testRefusedCoverageWithNoOwningThreadLandsInReceiverRecovery() {
    let f = makeFixture(recording: [false, true])
    let state = OverlayState()
    state.connectComposer(to: f.store)
    // No `admitCapture`: the store has no owner for this capture identity.

    refusedSessionEnded("orphaned but real", to: state, sessionId: "unowned-capture")

    XCTAssertEqual(
      f.store.composerRecoveryDocuments.map(\.text), ["orphaned but real"],
      "the receiver did not take ownership of the refused document")
    XCTAssertNil(
      state.retainedComposerDelivery,
      "the overlay kept a second copy of a document the receiver already owns")
    XCTAssertEqual(state.toast, "kept in composer recovery")
    XCTAssertEqual(state.mode, .coverageRefused)
  }

  /// No receiver wired at all. This is the fallback the overlay owns, and it
  /// is the one case where it may hold the bytes itself.
  func testRefusedCoverageWithNoReceiverStaysVisibleOnTheOverlay() {
    let state = OverlayState()
    refusedSessionEnded("nobody is listening", to: state)

    XCTAssertEqual(state.retainedComposerDelivery, "nobody is listening")
    XCTAssertEqual(state.toast, "no composer receiver")
    XCTAssertEqual(
      state.coverageRefusalDetail,
      "The handover came back. These words are retained here — recover them before the next take.")
    XCTAssertEqual(state.activeText, "nobody is listening", "the canvas still holds the words")
  }

  /// A predecessor's refused terminal arriving after its successor opened.
  /// It may recover to its OWN receiver and it may not touch the successor's
  /// canvas, phase or notice — a late refusal cannot retroactively mark a
  /// live take as incomplete.
  func testLateRefusedPredecessorRecoversWithoutRepaintingTheSuccessor() {
    let f = makeFixture(recording: [false, true])
    let state = OverlayState()
    state.connectComposer(to: f.store)
    admitCapture(f.store, threadID: f.threadA, id: "session-1")
    f.store.select(f.threadA)
    state.handleRecordingPreparing()
    state.handleRecordingStarted()
    listening("first take", to: state, sessionId: "session-1")
    state.finishControllerRecording()

    f.store.select(f.threadB)
    f.store.draft = "B typed"
    admitCapture(f.store, threadID: f.threadB, id: "session-2")
    state.handleRecordingPreparing()
    state.handleRecordingStarted()
    listening("second take", to: state, sessionId: "session-2")

    refusedSessionEnded("first take refused", to: state, sessionId: "session-1")

    XCTAssertEqual(f.store.draft, "B typed", "a late refusal edited the successor's composer")
    XCTAssertEqual(state.activeText, "second take")
    XCTAssertEqual(state.mode, .listening, "a late refusal repainted the live take")
    XCTAssertNil(state.coverageRefusalNotice, "the successor inherited a refusal it never had")
    XCTAssertFalse(state.terminal)

    // Parked, not lost. Asserted the way the user meets it — by going back to
    // the conversation that opened the take — because the store's composition
    // map is private and a test that reached into it would be checking an
    // implementation detail instead of the delivery.
    f.store.select(f.threadA)
    XCTAssertEqual(
      f.store.draft, "first take refused",
      "the predecessor's refused words never reached their own thread")
    XCTAssertTrue(
      f.store.threads.allSatisfy { $0.messages.isEmpty },
      "a parked refusal must never be submitted on the user's behalf")
  }
  func testEqualRevisionLaterSequenceDeliversRefusedWordsToOriginalComposer() throws {
    let f = makeFixture(recording: [false, true])
    let state = OverlayState()
    state.connectComposer(to: f.store)
    admitCapture(f.store, threadID: f.threadA, id: "join-session")
    listening("tak tak", to: state)
    var terminal = try XCTUnwrap(state.latestTranscriptProjection)
    terminal.sequence += 1
    terminal.terminal = true
    terminal.lifecycleTerminal = true
    terminal.delivery = .composerPending
    terminal.phase = "coverage_refused"
    terminal.reducerAction = "session_ended"
    terminal.sealCoverage = CsProjectedSealCoverageReceipt(
      status: .unavailable, unavailableReason: .partialObservation, speechSamples: 0,
      coveredSamples: 0, uncoveredSpeechRanges: [], maxUncoveredSamples: 0,
      incompleteThresholdSamples: 4_000, speechProducer: "capture_energy",
      availability: "discontinuous", observedSamples: nil, coverageRatio: nil)
    f.store.select(f.threadB)
    f.store.draft = "B typed"
    state.applyTranscriptProjection(terminal)
    state.applyTranscriptProjection(terminal)
    XCTAssertEqual(f.store.draft, "B typed")
    XCTAssertEqual(state.statusText, "measurement unavailable")
    f.store.select(f.threadA)
    XCTAssertEqual(f.store.draft, "tak tak")
    XCTAssertTrue(f.store.threads.allSatisfy { $0.messages.isEmpty })
  }

  func testRetiredEqualRevisionPendingDeliveryUsesSessionLocalOrder() throws {
    try assertRetiredPendingDelivery(newerOldDocument: false)
  }

  func testRetiredPendingDeliverySurvivesANewerDocumentBeforeItsLateLifecycle() throws {
    try assertRetiredPendingDelivery(newerOldDocument: true)
  }

  private func assertRetiredPendingDelivery(newerOldDocument: Bool) throws {
    let f = makeFixture(recording: [false, true])
    let state = OverlayState()
    state.connectComposer(to: f.store)
    admitCapture(f.store, threadID: f.threadA, id: "old-session")
    listening("old words", to: state, sessionId: "old-session")
    var old = try XCTUnwrap(state.latestTranscriptProjection)
    old.sequence = 100
    old.reducerRevision = 100
    state.applyTranscriptProjection(old)
    if newerOldDocument {
      var revision = old
      revision.sequence += 2
      revision.reducerRevision += 1
      state.applyTranscriptProjection(revision)
    }
    state.finishControllerRecording()
    if let request = f.store.currentComposerCaptureRequestID, let handle = f.store.composerCaptureHandle {
      f.store.applyComposerStopOutcome(.noLiveCapture, requestID: request, handle: handle)
    }
    f.store.select(f.threadB)
    f.store.draft = "B typed"
    admitCapture(f.store, threadID: f.threadB, id: "new-session")
    state.handleRecordingPreparing()
    state.handleRecordingStarted()
    listening("new words", to: state, sessionId: "new-session")
    let current = state.latestTranscriptProjection
    old.sequence += 1
    old.terminal = true
    old.lifecycleTerminal = true
    old.phase = "coverage_refused"
    old.delivery = .composerPending
    state.applyTranscriptProjection(old)
    state.applyTranscriptProjection(old)
    XCTAssertEqual(state.latestTranscriptProjection, current)
    XCTAssertEqual(state.activeText, "new words")
    XCTAssertNil(state.coverageRefusalNotice)
    XCTAssertEqual(f.store.draft, "B typed")
    XCTAssertEqual(f.store.composerCaptureHandle?.captureId, "new-session")
    f.store.select(f.threadA)
    XCTAssertEqual(f.store.draft, "old words")
  }

}
