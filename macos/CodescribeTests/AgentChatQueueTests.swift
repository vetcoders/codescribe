import Foundation
import XCTest

@testable import Codescribe

/// Contract tests for the composer turn queue: messages accepted while a turn
/// is in flight are durable, FIFO per thread, cancellable while queued, and
/// survive an app death (restart replays what never reached a terminal).
@MainActor
final class AgentChatQueueTests: XCTestCase {
  private final class GatedState: @unchecked Sendable {
    private let lock = NSLock()
    private var continuations: [CheckedContinuation<String, Error>] = []
    private var storedStarts: [(text: String, threadId: String)] = []

    var starts: [(text: String, threadId: String)] {
      lock.withLock { storedStarts }
    }

    func recordStart(text: String, threadId: String) {
      lock.withLock { storedStarts.append((text, threadId)) }
    }

    func suspend(_ continuation: CheckedContinuation<String, Error>) {
      lock.withLock { continuations.append(continuation) }
    }

    func finishNext(_ result: String) {
      let continuation = lock.withLock {
        continuations.isEmpty ? nil : continuations.removeFirst()
      }
      continuation?.resume(returning: result)
    }

    func failNext(_ error: Error) {
      let continuation = lock.withLock {
        continuations.isEmpty ? nil : continuations.removeFirst()
      }
      continuation?.resume(throwing: error)
    }
  }

  /// Engine whose streams stay open until the test releases them, so a turn
  /// can be held "active" while more messages are accepted.
  private final class GatedEngine: AgentChatEngine {
    let state = GatedState()

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
    ) async throws -> String {
      state.recordStart(text: text, threadId: threadId)
      return try await withCheckedThrowingContinuation { state.suspend($0) }
    }

    func cancelReply(threadId: String) -> Bool {
      state.failNext(CancellationError())
      return true
    }

    func installToolApprovalHandler(
      _ handler: @escaping @MainActor (PendingToolApproval) -> Void
    ) {}

    func resolveToolApproval(
      _ request: PendingToolApproval, approved: Bool, remember: Bool
    ) -> Bool { true }
  }

  private final class StubProvider: ChatThreadsProviding {
    func listThreads() -> [ChatThread] { [] }
    func searchThreads(query: String) -> [ChatThread] { [] }
    func loadMessages(backendId: String) -> [ChatMessage] { [] }
    func deleteThread(backendId: String) -> Bool { true }
    func setThreadFavorite(backendId: String, isFavorite: Bool) -> Bool { true }
    func renameThread(backendId: String, title: String) -> Bool { true }
    func setGeneratedTitle(backendId: String, title: String) -> Bool { true }
    func exportThreadMarkdown(backendId: String, assistantOnly: Bool) -> String? { nil }
    func generateThreadId() -> String { "t_\(UUID().uuidString)" }
  }

  override func setUp() {
    super.setUp()
    UserDefaults.standard.removeObject(forKey: AgentChatStore.acceptedTurnsDefaultsKey)
  }

  override func tearDown() {
    UserDefaults.standard.removeObject(forKey: AgentChatStore.acceptedTurnsDefaultsKey)
    super.tearDown()
  }

  private func waitUntil(
    timeout: TimeInterval = 2,
    _ message: String = "condition not met in time",
    _ condition: () -> Bool
  ) async {
    let deadline = Date().addingTimeInterval(timeout)
    while !condition(), Date() < deadline {
      try? await Task.sleep(nanoseconds: 10_000_000)
    }
    XCTAssertTrue(condition(), message)
  }

  private func sendMessage(_ text: String, in store: AgentChatStore) {
    store.draft = text
    store.send()
  }

  func testMessagesAcceptedDuringActiveTurnExecuteFIFO() async {
    let engine = GatedEngine()
    let store = AgentChatStore(engine: engine, threadsProvider: StubProvider())

    sendMessage("first", in: store)
    await waitUntil("first turn should start") { engine.state.starts.count == 1 }

    sendMessage("second", in: store)
    sendMessage("third", in: store)
    sendMessage("fourth", in: store)
    XCTAssertEqual(
      store.queuedTurns.map(\.text), ["second", "third", "fourth"],
      "messages accepted mid-turn must queue FIFO, not vanish"
    )

    engine.state.finishNext("r1")
    await waitUntil("second turn should start after first terminal") {
      engine.state.starts.count == 2
    }
    XCTAssertEqual(engine.state.starts[1].text, "second")

    engine.state.finishNext("r2")
    await waitUntil("third turn should start") { engine.state.starts.count == 3 }
    XCTAssertEqual(engine.state.starts[2].text, "third")

    engine.state.finishNext("r3")
    await waitUntil("fourth turn should start") { engine.state.starts.count == 4 }
    XCTAssertEqual(engine.state.starts[3].text, "fourth")

    engine.state.finishNext("r4")
    await waitUntil("queue should drain") { store.queuedTurns.isEmpty }
    // All four ran on the same backend thread — the queue is thread-bound.
    XCTAssertEqual(Set(engine.state.starts.map(\.threadId)).count, 1)
  }

  func testCancellingActiveTurnKeepsAndContinuesQueue() async {
    let engine = GatedEngine()
    let store = AgentChatStore(engine: engine, threadsProvider: StubProvider())

    sendMessage("first", in: store)
    await waitUntil("first turn should start") { engine.state.starts.count == 1 }
    sendMessage("second", in: store)
    XCTAssertEqual(store.queuedTurns.map(\.text), ["second"])

    store.stopActiveTurn()

    await waitUntil("queue must continue after a user Stop") {
      engine.state.starts.count == 2
    }
    XCTAssertEqual(engine.state.starts[1].text, "second")
    engine.state.finishNext("r2")
    await waitUntil("queue should drain") { store.queuedTurns.isEmpty }
  }

  func testProviderErrorDoesNotStrandQueuedMessages() async {
    let engine = GatedEngine()
    let store = AgentChatStore(engine: engine, threadsProvider: StubProvider())

    sendMessage("first", in: store)
    await waitUntil("first turn should start") { engine.state.starts.count == 1 }
    sendMessage("second", in: store)

    engine.state.failNext(NSError(domain: "test", code: 1))

    await waitUntil("queue must continue after a provider error") {
      engine.state.starts.count == 2
    }
    XCTAssertEqual(engine.state.starts[1].text, "second")
    engine.state.finishNext("r2")
  }

  func testCancelQueuedMessageBeforeDispatch() async {
    let engine = GatedEngine()
    let store = AgentChatStore(engine: engine, threadsProvider: StubProvider())

    sendMessage("first", in: store)
    await waitUntil("first turn should start") { engine.state.starts.count == 1 }
    sendMessage("doomed", in: store)
    guard let queuedID = store.queuedTurns.first?.id else {
      return XCTFail("expected a queued turn")
    }

    store.cancelQueuedTurn(queuedID)
    XCTAssertTrue(store.queuedTurns.isEmpty)

    engine.state.finishNext("r1")
    try? await Task.sleep(nanoseconds: 100_000_000)
    XCTAssertEqual(
      engine.state.starts.count, 1,
      "a cancelled queued message must never dispatch"
    )
    // Its durable record is gone too — a restart cannot resurrect it.
    let sidecarCount =
      UserDefaults.standard
      .data(forKey: AgentChatStore.acceptedTurnsDefaultsKey)
      .flatMap { try? JSONDecoder().decode([SidecarProbe].self, from: $0) }?
      .count ?? 0
    XCTAssertEqual(sidecarCount, 0, "no accepted-turn records may survive the terminals")
  }

  func testEditingQueuedMessageUpdatesDispatchAndDurableCopy() async {
    let engine = GatedEngine()
    let store = AgentChatStore(engine: engine, threadsProvider: StubProvider())

    sendMessage("first", in: store)
    await waitUntil("first turn should start") { engine.state.starts.count == 1 }
    sendMessage("old queued wording", in: store)
    let queuedID = try! XCTUnwrap(store.queuedTurns.first?.id)

    XCTAssertTrue(store.editQueuedTurn(queuedID, text: "  corrected queued wording  "))
    XCTAssertEqual(store.queuedTurns.first?.text, "corrected queued wording")
    let durableTexts =
      UserDefaults.standard
      .data(forKey: AgentChatStore.acceptedTurnsDefaultsKey)
      .flatMap { try? JSONDecoder().decode([SidecarTextProbe].self, from: $0) }?
      .map(\.text) ?? []
    XCTAssertTrue(durableTexts.contains("corrected queued wording"))
    XCTAssertFalse(durableTexts.contains("old queued wording"))

    engine.state.finishNext("r1")
    await waitUntil("edited turn should dispatch") { engine.state.starts.count == 2 }
    XCTAssertEqual(engine.state.starts[1].text, "corrected queued wording")
    engine.state.finishNext("r2")
  }

  func testComposerHistoryIncludesQueuedMessagesInFIFOOrder() async {
    let engine = GatedEngine()
    let store = AgentChatStore(engine: engine, threadsProvider: StubProvider())
    let threadID = try! XCTUnwrap(store.selectedThreadID)

    sendMessage("first", in: store)
    await waitUntil("first turn should start") { engine.state.starts.count == 1 }
    sendMessage("second", in: store)
    sendMessage("third", in: store)

    XCTAssertEqual(store.composerHistory(in: threadID), ["first", "second", "third"])
    engine.state.finishNext("r1")
    await waitUntil("second should start") { engine.state.starts.count == 2 }
    engine.state.finishNext("r2")
    await waitUntil("third should start") { engine.state.starts.count == 3 }
    engine.state.finishNext("r3")
  }

  /// Simulated app death: the first store accepts two messages (one running,
  /// one queued) and never reaches a terminal. A fresh store — same defaults —
  /// must replay both, in order.
  func testRestartReplaysAcceptedMessagesThatNeverReachedATerminal() async {
    let engine = GatedEngine()
    let store = AgentChatStore(engine: engine, threadsProvider: StubProvider())
    sendMessage("interrupted", in: store)
    await waitUntil("first turn should start") { engine.state.starts.count == 1 }
    sendMessage("queued-survivor", in: store)
    XCTAssertEqual(store.queuedTurns.map(\.text), ["queued-survivor"])
    // No terminal, no cleanup — the durable sidecar still holds both.

    let relaunchEngine = GatedEngine()
    let relaunched = AgentChatStore(engine: relaunchEngine, threadsProvider: StubProvider())

    await waitUntil("restart must replay the interrupted message first") {
      relaunchEngine.state.starts.count == 1
    }
    XCTAssertEqual(relaunchEngine.state.starts[0].text, "interrupted")
    XCTAssertEqual(relaunched.queuedTurns.map(\.text), ["queued-survivor"])

    relaunchEngine.state.finishNext("r1")
    await waitUntil("the queued survivor follows FIFO") {
      relaunchEngine.state.starts.count == 2
    }
    XCTAssertEqual(relaunchEngine.state.starts[1].text, "queued-survivor")
    relaunchEngine.state.finishNext("r2")
  }
}

/// Decoding helper for the sidecar-empty assertion: any element shape counts.
private struct SidecarProbe: Decodable {}
private struct SidecarTextProbe: Decodable { let text: String }
