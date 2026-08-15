import Foundation
import XCTest

@testable import Codescribe

/// Ownership proof for the Agent voice lane.
///
/// There is exactly ONE recorder behind Right Option, the tray, and the composer
/// mic. Before this cut every voice-lane fact was read live and globally, so the
/// surface could disagree with the recorder in three ways at once:
///
///   * a phase entered optimistically (`.preparing`) but only cleared when an
///     assistive latch happened to read true — both `.preparing` and `.recording`
///     are non-actionable, so one missed read left the mic dead until relaunch;
///   * a `dictationBlocked` flag with no terminal owner, which routed every later
///     press into a stop that no-ops against an idle controller;
///   * a routing target republished on every rail selection, so browsing threads
///     mid-sentence moved the in-flight transcript to a thread the user was not
///     dictating into — and painted a ripple mic there to match.
///
/// The invariant these tests hold: the capture is owned by the thread the rail was
/// on when the gesture fired, ownership is released by every terminal beat, and no
/// terminal beat is conditional.
@MainActor
final class AgentVoiceLaneOwnershipTests: XCTestCase {
  private final class SpyRoutingEngine: AgentChatEngine {
    private(set) var assistiveTargets: [String?] = []

    func isAvailable() -> Bool { true }
    func availabilityDetail() -> String? { nil }
    func generateThreadTitle(_ text: String) async throws -> String? { nil }
    func streamReply(
      _ text: String,
      threadId: String,
      attachmentPaths: [String],
      onDelta: @escaping @MainActor (String) -> Void,
      onReasoning: @escaping @MainActor (String) -> Void,
      onToolExecuting: @escaping @MainActor (String, String) -> Void,
      onToolResult: @escaping @MainActor (String, String, Bool, String) -> Void
    ) async throws -> String { "" }
    func cancelReply(threadId: String) -> Bool { false }
    func setAssistiveTargetThread(backendId: String?) {
      assistiveTargets.append(backendId)
    }
  }

  private final class StubThreadsProvider: ChatThreadsProviding {
    var rows: [(id: String, title: String)]

    init(_ rows: [(id: String, title: String)]) {
      self.rows = rows
    }

    func listThreads() -> [ChatThread] {
      rows.map { row in
        var thread = ChatThread(title: row.title, meta: "now")
        thread.backendId = row.id
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

  private struct Fixture {
    let store: AgentChatStore
    let engine: SpyRoutingEngine
    let capturing: UUID
    let other: UUID
  }

  /// Two persisted threads, rail sitting on the first one.
  private func makeFixture() -> Fixture {
    let engine = SpyRoutingEngine()
    let store = AgentChatStore(
      engine: engine,
      threadsProvider: StubThreadsProvider([
        ("t_capture", "Capture owner"),
        ("t_other", "Other"),
      ])
    )
    let capturing = store.threads.first { $0.backendId == "t_capture" }!.id
    let other = store.threads.first { $0.backendId == "t_other" }!.id
    store.select(capturing)
    return Fixture(store: store, engine: engine, capturing: capturing, other: other)
  }

  // MARK: Terminal beats are unconditional

  func testTerminalBeatReleasesTheMicWhateverTheLaneTurnedOutToBe() {
    let f = makeFixture()
    f.store.setDictationPhase(.preparing)
    f.store.dictationBlocked = true

    f.store.endDictationSession()

    XCTAssertEqual(f.store.dictationPhase, .idle, "a terminal beat must always reach rest")
    XCTAssertFalse(f.store.dictationBlocked, "the microphone is free once the session ends")
    XCTAssertNil(f.store.dictationThreadID, "ownership must not outlive the capture")
  }

  func testAStoppedSessionCannotLeaveTheMicInANonActionableState() {
    let f = makeFixture()
    // The exact shape of the historical dead mic: the gesture set `.preparing`,
    // the session ended, and nothing ever cleared it.
    f.store.setDictationPhase(.preparing)
    f.store.endDictationSession()

    // `.preparing` and `.recording` are the two states the composer disables.
    XCTAssertFalse(
      f.store.dictationPhase == .preparing || f.store.dictationPhase == .recording,
      "the mic must be pressable again after a session ends"
    )
  }

  func testFailedPhaseKeepsItsBannerButStillReleasesOwnership() {
    let f = makeFixture()
    f.store.setDictationPhase(.recording)
    f.store.reportDictationFailure("boom")

    f.store.endDictationSession()

    guard case .failed(let message) = f.store.dictationPhase else {
      return XCTFail("the inline failure must survive its own self-clearing timer")
    }
    XCTAssertEqual(message, "boom")
    XCTAssertFalse(f.store.dictationBlocked)
    XCTAssertNil(f.store.dictationThreadID, "a failed session still owns nothing")
  }

  // MARK: Thread-switch ghost

  func testBrowsingAnotherThreadMidCaptureNeverStealsTheRoutingTarget() {
    let f = makeFixture()
    f.store.setDictationPhase(.recording)
    let targetAtGesture = f.engine.assistiveTargets.last ?? nil
    XCTAssertEqual(targetAtGesture, "t_capture")

    f.store.select(f.other)

    XCTAssertEqual(
      f.engine.assistiveTargets.last ?? nil, "t_capture",
      "the transcript belongs to the thread the mic was pressed in"
    )
    XCTAssertEqual(f.store.dictationThreadID, f.capturing)
  }

  func testBrowsingAnotherThreadMidCaptureShowsBusyRatherThanAGhostRipple() {
    let f = makeFixture()
    f.store.setDictationPhase(.recording)
    XCTAssertTrue(f.store.dictationOwnsSelectedThread, "the capturing thread owns the affordance")

    f.store.select(f.other)

    XCTAssertFalse(
      f.store.dictationOwnsSelectedThread,
      "a thread that is not receiving the dictation must not render a recording mic"
    )
  }

  func testSessionEndResyncsTheRoutingTargetToWhereTheRailActuallyIs() {
    let f = makeFixture()
    f.store.setDictationPhase(.recording)
    f.store.select(f.other)

    f.store.endDictationSession()

    XCTAssertEqual(
      f.engine.assistiveTargets.last ?? nil, "t_other",
      "once the mic is free the next capture follows the rail again"
    )
    XCTAssertTrue(f.store.dictationOwnsSelectedThread)
  }

  func testIdleSelectionChangesStillPublishNormally() {
    let f = makeFixture()
    f.store.select(f.other)
    XCTAssertEqual(f.engine.assistiveTargets.last ?? nil, "t_other")
    f.store.select(f.capturing)
    XCTAssertEqual(f.engine.assistiveTargets.last ?? nil, "t_capture")
  }

  // MARK: Ownership latch

  func testOwnershipIsLatchedAtTheGestureNotReDecidedPerPhase() {
    let f = makeFixture()
    f.store.setDictationPhase(.preparing)
    f.store.select(f.other)
    // A late `.recording` (the controller's started beat) must not re-latch onto
    // whatever the rail drifted to while the recorder was warming up.
    f.store.setDictationPhase(.recording)

    XCTAssertEqual(f.store.dictationThreadID, f.capturing)
    XCTAssertEqual(f.engine.assistiveTargets.last ?? nil, "t_capture")
  }

  func testVoiceTurnBindingRepublishesEvenWhileACaptureIsLatched() {
    let engine = SpyRoutingEngine()
    let store = AgentChatStore(
      engine: engine,
      threadsProvider: StubThreadsProvider([("t_history", "History")])
    )
    store.newThread()
    store.setDictationPhase(.recording)

    // Binding a local draft to a freshly minted backend id is an identity
    // transition, not the user moving away — it must cross the freeze.
    store.ingestVoiceTurn(threadId: "t_voice_owner", userText: "keep this thread")

    XCTAssertEqual(engine.assistiveTargets.last ?? nil, "t_voice_owner")
    XCTAssertEqual(store.currentThread?.backendId, "t_voice_owner")
  }
}
