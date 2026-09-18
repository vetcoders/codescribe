import Foundation
import XCTest

@testable import Codescribe

/// One composer gesture, one explicit take — proved against the production
/// adapter, not against a re-implementation of its rules.
///
/// `RealComposerDictation` is the owner under test. The only substituted
/// boundary is the shared controller itself (`ComposerCaptureControlling`),
/// which is the FFI object; every decision — which press starts, which press
/// stops, which press must refuse, and which entry point a start uses — stays in
/// production code.
///
/// The incident this pins (Founder take ca23c06b, 2026-09-09): the composer mic
/// called `startAssistiveRecording`, i.e. the hands-free lane, so the take
/// inherited silence epochs. It also read "the controller is recording" as "I am
/// recording" and would end a dictation another surface owned.
@MainActor
final class ComposerTurnOwnershipTests: XCTestCase {
  /// Every call the composer gesture makes on the shared controller, in order.
  private enum CaptureCall: Equatable {
    case isRecording
    case startComposerTurn
    /// Carries the identity the gesture asked to stop, so a test can assert the
    /// adapter named *its own* capture rather than "whatever is live".
    case stop(String)
  }

  private enum CaptureFailure: Error { case refused }

  // Configure only between joined gestures; the adapter serializes controller calls.
  private final class FakeCaptureSurface: ComposerCaptureControlling, @unchecked Sendable {
    /// Answers handed to successive `isRecording()` calls; the last one repeats.
    private var recordingAnswers: [Bool]
    private var answerCursor = 0
    var startFails = false
    var stopFails = false
    /// Identity the controller admits for the next start.
    var admittedCaptureId = "capture-1"
    /// Typed answer the conditional stop returns when it does not throw.
    var stopOutcome: CsConditionalStop = .stopped
    var onStart: (@MainActor @Sendable () async -> Void)?
    var onQuery: (@MainActor @Sendable () async -> Void)?
    var onStop: (@MainActor @Sendable () async -> Void)?
    private(set) var calls: [CaptureCall] = []

    init(recording: [Bool]) {
      self.recordingAnswers = recording
    }

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
      if startFails { throw CaptureFailure.refused }
      return CsCaptureHandle(captureId: admittedCaptureId)
    }

    func stopComposerTurnRecording(handle: CsCaptureHandle) async throws -> CsConditionalStop {
      calls.append(.stop(handle.captureId))
      await onStop?()
      if stopFails { throw CaptureFailure.refused }
      return stopOutcome
    }
  }

  private final class SpyRoutingEngine: AgentChatEngine {
    private(set) var lastTarget: String?
    func isAvailable() -> Bool { true }
    func availabilityDetail() -> String? { nil }
    func generateThreadTitle(_ text: String) async throws -> String? { nil }
    func streamReply(
      _ text: String, threadId: String, attachmentPaths: [String],
      onDelta: @escaping @MainActor (String) -> Void,
      onReasoning: @escaping @MainActor (String) -> Void,
      onToolExecuting: @escaping @MainActor (String, String) -> Void,
      onToolResult: @escaping @MainActor (String, String, Bool, String) -> Void
    ) async throws -> String { "" }
    func cancelReply(threadId: String) -> Bool { false }
    func setAssistiveTargetThread(backendId: String?) { lastTarget = backendId }
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

  /// "A stop was requested", independent of which capture it named. Existing
  /// cases assert the gesture's stop policy; the identity assertions are their
  /// own tests below.
  private func stopRequested(_ calls: [CaptureCall]) -> Bool {
    calls.contains { if case .stop = $0 { return true } else { return false } }
  }

  private func stopCount(_ calls: [CaptureCall]) -> Int {
    calls.filter { if case .stop = $0 { return true } else { return false } }.count
  }

  private struct Fixture: Sendable {
    let store: AgentChatStore
    let dictation: RealComposerDictation
    let surface: FakeCaptureSurface
    let threadA: UUID
    let threadB: UUID
  }

  private func makeFixture(recording: [Bool]) -> Fixture {
    let name = "Codescribe.ComposerTurnOwnershipTests." + UUID().uuidString
    let defaults = UserDefaults(suiteName: name)!
    addTeardownBlock { UserDefaults(suiteName: name)?.removePersistentDomain(forName: name) }
    let store = AgentChatStore(threadsProvider: StubThreadsProvider(), persistenceDefaults: defaults)
    let surface = FakeCaptureSurface(recording: recording)
    let dictation = RealComposerDictation(store: store, hotkeys: surface)
    store.dictation = dictation
    let threadA = store.threads.first { $0.backendId == "t_a" }!.id
    let threadB = store.threads.first { $0.backendId == "t_b" }!.id
    store.select(threadA)
    return Fixture(
      store: store, dictation: dictation, surface: surface, threadA: threadA, threadB: threadB)
  }

  /// Join the actual gesture task; scheduler yields do not prove completion.
  private func settle(_ fixture: Fixture) async {
    await fixture.dictation.transitionTask?.value
  }

  // MARK: The entry point itself

  func testIdleComposerPressUsesTheOneTurnEntryPointNotTheHandsFreeLane() async {
    let f = makeFixture(recording: [false, false])

    f.dictation.toggle()
    await settle(f)

    XCTAssertTrue(
      f.surface.calls.contains(.startComposerTurn),
      "the composer must open its own one-turn take"
    )
  }

  // MARK: Ownership

  func testAForeignLiveCaptureIsReportedBusyAndNeverStopped() async {
    let f = makeFixture(recording: [true])
    // Nothing latched: the live take was started by a hotkey/tray/overlay.
    XCTAssertFalse(f.store.ownsLiveDictation)

    f.dictation.toggle()
    await settle(f)

    XCTAssertFalse(
      stopRequested(f.surface.calls),
      "a composer press must never end a capture another surface owns"
    )
    XCTAssertFalse(
      f.surface.calls.contains(.startComposerTurn),
      "and it must not open a second take on top of it either"
    )
    XCTAssertTrue(f.store.dictationBlocked, "the mic reads busy while a foreign take is live")
    XCTAssertEqual(
      f.store.dictationPhase, .idle,
      "the optimistic press must not strand the mic in a non-actionable state"
    )
    XCTAssertNil(f.store.dictationThreadID, "a refused gesture owns nothing")
  }

  func testOwnLiveCaptureIsStoppedByTheSecondPress() async {
    let f = makeFixture(recording: [false, true, true])
    f.dictation.toggle()
    await settle(f)
    XCTAssertTrue(f.store.ownsLiveDictation)

    f.dictation.toggle()
    await settle(f)

    XCTAssertTrue(stopRequested(f.surface.calls), "the owning surface ends its own take")
    XCTAssertFalse(f.store.ownsLiveDictation)
    XCTAssertEqual(f.store.dictationThreadID, f.threadA, "terminal text still has a destination")
  }

  func testThreadAKeepsTheCaptureAfterSelectingThreadB() async {
    let f = makeFixture(recording: [false, true])

    f.dictation.toggle()
    await settle(f)
    XCTAssertEqual(f.store.dictationThreadID, f.threadA)

    f.store.select(f.threadB)

    XCTAssertEqual(
      f.store.dictationThreadID, f.threadA,
      "ownership is decided at the gesture and not re-decided by browsing"
    )
    XCTAssertFalse(
      f.store.dictationOwnsSelectedThread,
      "thread B must see the mic as busy, not as its own live capture"
    )
    f.dictation.toggle()
    await settle(f)
    XCTAssertTrue(stopRequested(f.surface.calls))
    XCTAssertEqual(f.store.dictationThreadID, f.threadA)
  }

  // MARK: Recoverable lifecycle

  func testDoubleClickWhileInFlightIssuesOneStartOnly() async {
    let f = makeFixture(recording: [false, false])

    f.dictation.toggle()
    f.dictation.toggle()
    await settle(f)

    XCTAssertEqual(
      f.surface.calls.filter { $0 == .startComposerTurn }.count, 1,
      "the in-flight guard swallows the second press"
    )
  }

  func testStartFailureLeavesTheMicPressableAgain() async {
    let f = makeFixture(recording: [false, true])
    f.surface.startFails = true

    f.dictation.toggle()
    await settle(f)

    guard case .failed = f.store.dictationPhase else {
      return XCTFail("a refused start must surface as a recoverable failure")
    }
    XCTAssertNil(f.store.dictationThreadID, "a take that never began owns nothing")
    XCTAssertFalse(f.store.ownsLiveDictation)
    f.store.setDictationPhase(.recording) // A foreign lifecycle cannot revive the failed request.
    f.dictation.toggle()
    await settle(f)
    XCTAssertFalse(stopRequested(f.surface.calls))
  }

  func testStopFailureRevokesLivePermissionButAllowsAddressedRetry() async {
    let f = makeFixture(recording: [false, true, true])
    f.dictation.toggle()
    await settle(f)
    f.surface.stopFails = true

    f.dictation.toggle()
    await settle(f)

    guard case .failed = f.store.dictationPhase else {
      return XCTFail("a refused stop must surface as a recoverable failure")
    }
    XCTAssertFalse(f.store.ownsLiveDictation)
    XCTAssertEqual(f.store.dictationThreadID, f.threadA)
    f.store.setDictationPhase(.recording) // A later foreign phase cannot grant another stop.
    f.dictation.toggle()
    await settle(f)
    XCTAssertEqual(stopCount(f.surface.calls), 2)
    XCTAssertTrue(f.store.composerStopRetryAvailable)
    XCTAssertEqual(f.surface.calls.filter { $0 == .startComposerTurn }.count, 1)
    f.store.endDictationSession()
    XCTAssertNil(f.store.dictationThreadID)
    XCTAssertFalse(f.store.hasComposerCaptureRequest)
  }

  func testForeignLifecycleDuringTheControllerQueryCannotAuthorizeStop() async {
    let f = makeFixture(recording: [true])
    f.surface.onQuery = { [store = f.store] in store.setDictationPhase(.recording) }

    f.dictation.toggle()
    XCTAssertNil(f.store.dictationThreadID, "optimistic display has no destination")
    XCTAssertFalse(f.store.ownsLiveDictation)
    await settle(f)

    XCTAssertEqual(f.surface.calls, [.isRecording])
    XCTAssertFalse(f.store.ownsLiveDictation)
    XCTAssertNil(f.store.dictationThreadID)
    XCTAssertTrue(f.store.dictationBlocked)
  }

  func testTerminalThenForeignPhaseBeforeStartReplyCannotReviveLocalOwnership() async {
    let f = makeFixture(recording: [false, true])
    f.surface.onStart = { [store = f.store, threadB = f.threadB] in
      store.endDictationSession()
      store.select(threadB)
      store.setDictationPhase(.recording) // Another surface started after our terminal.
    }

    f.dictation.toggle()
    await settle(f)
    XCTAssertFalse(f.store.ownsLiveDictation)
    f.dictation.toggle()
    await settle(f)

    XCTAssertEqual(f.surface.calls.filter { $0 == .startComposerTurn }.count, 1)
    XCTAssertFalse(stopRequested(f.surface.calls))
    XCTAssertNil(f.store.dictationThreadID)
  }

  func testTerminalBeforeStopReplyRevokesPreviouslyCompletedStart() async {
    let f = makeFixture(recording: [false, true, true])
    f.dictation.toggle()
    await settle(f)
    XCTAssertTrue(f.store.ownsLiveDictation)
    f.surface.onStop = { [store = f.store] in
      store.endDictationSession()
      store.setDictationPhase(.recording)
    }

    f.dictation.toggle()
    await settle(f)

    XCTAssertTrue(stopRequested(f.surface.calls))
    XCTAssertFalse(f.store.ownsLiveDictation)
  }

  func testSelectionDuringIdleQueryKeepsGestureDestination() async {
    let f = makeFixture(recording: [false, true])
    let engine = SpyRoutingEngine()
    f.store.engine = engine
    f.surface.onQuery = { [store = f.store, threadB = f.threadB] in store.select(threadB) }
    f.surface.onStart = { XCTAssertEqual(engine.lastTarget, "t_a") }

    f.dictation.toggle()
    await settle(f)

    XCTAssertEqual(f.store.selectedThreadID, f.threadB)
    XCTAssertEqual(f.store.dictationThreadID, f.threadA)
    XCTAssertTrue(f.store.ownsLiveDictation)
  }

  func testPendingTerminalDestinationCannotAuthorizeStopOrBeReplacedByStart() async {
    let f = makeFixture(recording: [false, true, true])
    f.dictation.toggle()
    await settle(f)
    f.dictation.toggle()
    await settle(f)
    let completedCalls = f.surface.calls
    f.store.select(f.threadB)
    f.store.setDictationPhase(.recording)

    f.dictation.toggle()
    await settle(f)

    XCTAssertEqual(f.surface.calls, completedCalls)
    XCTAssertFalse(f.store.ownsLiveDictation)
    XCTAssertEqual(f.store.dictationThreadID, f.threadA)
    f.store.endDictationSession()
    XCTAssertNil(f.store.dictationThreadID)
    XCTAssertFalse(f.store.hasComposerCaptureRequest)
  }

  func testTerminalDuringStopCannotTurnThatGestureIntoANewStart() async {
    let f = makeFixture(recording: [false, true, false])
    f.dictation.toggle()
    await settle(f)
    f.surface.onStop = { [store = f.store] in store.endDictationSession() }

    f.dictation.toggle()
    await settle(f)

    XCTAssertEqual(f.surface.calls.filter { $0 == .startComposerTurn }.count, 1)
    XCTAssertTrue(stopRequested(f.surface.calls))
    XCTAssertFalse(f.store.ownsLiveDictation)
    XCTAssertNil(f.store.dictationThreadID)
  }

  /// Acknowledgment is emitted only after the continuation is registered.
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

  func testOwnedStopReachesHandleWhileAnAcknowledgedRecordingQueryNeverAnswers() async {
    let f = makeFixture(recording: [false])
    f.dictation.toggle()
    await settle(f)
    XCTAssertTrue(f.store.ownsLiveDictation, "handle admission needs no telemetry round trip")
    let query = Gate(expectation(description: "recording query held"))
    f.surface.onQuery = { await query.wait() }
    let blockedQuery = Task { await f.surface.isRecording() }
    await fulfillment(of: [query.entered], timeout: 1)
    let reached = expectation(description: "named Stop reached while query held")
    f.surface.onStop = { reached.fulfill() }
    f.surface.stopOutcome = .pending
    f.dictation.toggle()
    await fulfillment(of: [reached], timeout: 1)
    // Release even if the assertion timed out, so a regression cannot strand
    // the suite's continuation. The endpoint assertion happened before release.
    query.release()
    _ = await blockedQuery.value
    await settle(f)
    XCTAssertEqual(stopCount(f.surface.calls), 1)
    XCTAssertTrue(f.store.composerCaptureAwaitingTerminal)
    XCTAssertEqual(f.store.dictationPhase, .preparing)
  }

  func testPendingAndFalseRecordingCannotReplaceThreadOrStartAgain() async {
    let f = makeFixture(recording: [false])
    f.dictation.toggle()
    await settle(f)
    f.surface.stopOutcome = .pending
    f.dictation.toggle()
    await settle(f)
    f.store.select(f.threadB)
    let calls = f.surface.calls
    f.dictation.toggle()
    await settle(f)
    XCTAssertEqual(f.surface.calls, calls)
    XCTAssertEqual(f.store.dictationThreadID, f.threadA)
    XCTAssertEqual(f.store.dictationPhase, .preparing)
    XCTAssertEqual(f.store.receiveDictationTranscript("A's exact words", captureID: "capture-1"), .parked(threadID: f.threadA))
    XCTAssertTrue(f.store.finishDictationCapture(sessionID: "capture-1"))
    XCTAssertEqual(f.store.draft, "")
    f.store.select(f.threadA)
    XCTAssertEqual(f.store.draft, "A's exact words")
  }

  func testAdmissionUnavailableKeepsSameHandleForExplicitRetry() async {
    let f = makeFixture(recording: [false])
    f.dictation.toggle()
    await settle(f)
    f.surface.stopOutcome = .admissionUnavailable
    f.dictation.toggle()
    await settle(f)
    XCTAssertTrue(f.store.ownsLiveDictation)
    XCTAssertEqual(f.store.dictationThreadID, f.threadA)
    f.surface.stopOutcome = .pending
    f.dictation.toggle()
    await settle(f)
    XCTAssertEqual(f.surface.calls.filter { if case .stop("capture-1") = $0 { return true }; return false }.count, 2)
    XCTAssertFalse(f.store.ownsLiveDictation)
  }

  func testCancelledAdapterStopCannotErasePendingOwner() async {
    let f = makeFixture(recording: [false])
    f.dictation.toggle()
    await settle(f)
    let stop = Gate(expectation(description: "Stop registered"))
    f.surface.onStop = { await stop.wait() }
    f.surface.stopOutcome = .pending
    f.dictation.toggle()
    await fulfillment(of: [stop.entered], timeout: 1)
    f.dictation.transitionTask?.cancel()
    stop.release()
    await settle(f)
    XCTAssertEqual(f.store.dictationThreadID, f.threadA)
    XCTAssertTrue(f.store.composerCaptureAwaitingTerminal)
    f.dictation.toggle()
    await settle(f)
    XCTAssertEqual(stopCount(f.surface.calls), 1)
  }

  // W2 contracts, UNRUN: transport recovery addresses the original request.
  func testTransportRetryKeepsCaptureRequestThreadAndDraftUntilAddressedDelivery() async {
    let f = makeFixture(recording: [false])
    f.dictation.toggle()
    await settle(f)
    let request = f.store.currentComposerCaptureRequestID
    f.surface.stopFails = true
    f.dictation.toggle()
    await settle(f)
    XCTAssertTrue(f.store.composerStopRetryAvailable)
    XCTAssertFalse(f.store.ownsLiveDictation)
    f.store.select(f.threadB)
    f.store.draft = "B's unsent draft"
    f.surface.stopFails = false
    f.dictation.toggle()
    await settle(f)
    XCTAssertEqual(f.surface.calls, [.isRecording, .startComposerTurn, .stop("capture-1"), .stop("capture-1")])
    XCTAssertEqual(f.store.currentComposerCaptureRequestID, request)
    XCTAssertEqual(f.store.dictationThreadID, f.threadA)
    XCTAssertEqual(f.store.draft, "B's unsent draft")
    XCTAssertFalse(f.store.composerStopRetryAvailable)
    XCTAssertTrue(f.store.composerCaptureAwaitingTerminal, "Stopped is not delivery acknowledgement")
    XCTAssertEqual(f.store.receiveDictationTranscript("A's recovered words", captureID: "capture-1"), .parked(threadID: f.threadA))
    XCTAssertTrue(f.store.finishDictationCapture(sessionID: "capture-1"))
    XCTAssertEqual(f.store.draft, "B's unsent draft")
    f.store.select(f.threadA)
    XCTAssertEqual(f.store.draft, "A's recovered words")
  }

  func testRetryFailureStaysActionableAfterBannerExpiryWithoutInferringIdle() async {
    let f = makeFixture(recording: [false])
    f.dictation.toggle()
    await settle(f)
    let expiry = Gate(expectation(description: "failure banner clock entered"))
    f.store.waitForDictationFailureExpiry = { await expiry.wait() }
    f.surface.stopFails = true
    f.dictation.toggle()
    await settle(f)
    await fulfillment(of: [expiry.entered], timeout: 1)
    expiry.release()
    await f.store.dictationFailureTask?.value
    guard case .failed = f.store.dictationPhase else {
      return XCTFail("preparing would disable the real composer mic retry")
    }
    XCTAssertTrue(f.store.composerStopRetryAvailable)
    XCTAssertFalse(f.store.ownsLiveDictation)
    XCTAssertEqual(f.store.dictationThreadID, f.threadA)
    f.surface.stopFails = false
    f.dictation.toggle()
    await settle(f)
    XCTAssertEqual(f.surface.calls, [.isRecording, .startComposerTurn, .stop("capture-1"), .stop("capture-1")])
    XCTAssertTrue(f.store.composerCaptureAwaitingTerminal)
  }

  func testDelayedRetryReplyCannotChangeAReplacementRequest() async {
    let f = makeFixture(recording: [false])
    f.dictation.toggle()
    await settle(f)
    f.surface.stopFails = true
    f.dictation.toggle()
    await settle(f)
    guard let oldRequest = f.store.currentComposerCaptureRequestID else {
      return XCTFail("the failed transport must retain its request")
    }
    let gate = Gate(expectation(description: "retry is suspended"))
    f.surface.stopFails = false
    f.surface.stopOutcome = .foreignCapture
    f.surface.onStop = { await gate.wait() }
    f.dictation.toggle()
    await fulfillment(of: [gate.entered], timeout: 1)
    f.store.endDictationSession()
    f.store.select(f.threadB)
    let replacement = f.store.beginComposerCaptureRequest(threadID: f.threadB)
    f.store.completeComposerCaptureStart(replacement, live: true, handle: CsCaptureHandle(captureId: "capture-2"))
    gate.release()
    await settle(f)
    XCTAssertNotEqual(oldRequest, replacement)
    XCTAssertEqual(f.store.currentComposerCaptureRequestID, replacement)
    XCTAssertEqual(f.store.composerCaptureHandle?.captureId, "capture-2")
    XCTAssertTrue(f.store.ownsLiveDictation)
    XCTAssertEqual(f.store.dictationThreadID, f.threadB)
    f.store.reportComposerStopFailure("late failure", requestID: oldRequest, handle: CsCaptureHandle(captureId: "capture-1"))
    XCTAssertFalse(f.store.composerStopRetryAvailable)
    XCTAssertTrue(f.store.ownsLiveDictation)
  }

}
