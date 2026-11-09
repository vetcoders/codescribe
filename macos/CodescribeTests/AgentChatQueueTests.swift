import Foundation
import XCTest
import os

@testable import Codescribe

/// Contract tests for the composer turn queue: messages accepted while a turn
/// is in flight are durable, FIFO per thread, cancellable while queued, and
/// survive an app death (restart replays what never reached a terminal).
@MainActor
final class AgentChatQueueTests: XCTestCase {
  private final class GatedState: Sendable {
    private struct Storage {
      var continuations: [CheckedContinuation<String, Error>] = []
      var starts: [(text: String, threadId: String)] = []
    }

    private let storage = OSAllocatedUnfairLock(initialState: Storage())

    var starts: [(text: String, threadId: String)] {
      storage.withLock { $0.starts }
    }

    func recordStart(text: String, threadId: String) {
      storage.withLock { $0.starts.append((text, threadId)) }
    }

    func suspend(_ continuation: CheckedContinuation<String, Error>) {
      storage.withLock { $0.continuations.append(continuation) }
    }

    func finishNext(_ result: String) {
      let continuation = storage.withLock {
        $0.continuations.isEmpty ? nil : $0.continuations.removeFirst()
      }
      continuation?.resume(returning: result)
    }

    func failNext(_ error: Error) {
      let continuation = storage.withLock {
        $0.continuations.isEmpty ? nil : $0.continuations.removeFirst()
      }
      continuation?.resume(throwing: error)
    }
  }

  /// Engine whose streams stay open until the test releases them, so a turn
  /// can be held "active" while more messages are accepted.
  private final class GatedEngine: ChatEngineFixture {
    let state = GatedState()

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

    func resolveToolApproval(
      _ request: PendingToolApproval, approved: Bool, remember: Bool
    ) -> Bool { true }
  }

  private final class StubProvider: ThreadsFixture {
    func generateThreadId() -> String { "t_\(UUID().uuidString)" }
  }

  override func setUp() async throws {
    try await super.setUp()
    await MainActor.run {
      UserDefaults.standard.removeObject(forKey: AgentChatStore.acceptedTurnsDefaultsKey)
    }
  }

  override func tearDown() async throws {
    await MainActor.run {
      UserDefaults.standard.removeObject(forKey: AgentChatStore.acceptedTurnsDefaultsKey)
    }
    try await super.tearDown()
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

/// Integration seam: two independently suspended provider calls, not a global
/// continuation. Cancellation acknowledges immediately but terminal is gated.
@MainActor
private final class IndependentThreadEngine: ChatEngineFixture {
  struct Start {
    let text: String
    let threadID: String
    let configuration: AgentTurnConfiguration?
  }
  var selection = AgentTurnConfiguration(
    provider: "provider-A", model: "model-A", reasoningEffort: "high", receipt: "sealed-A")
  var starts: [Start] = []
  var cancelled: [String] = []
  var acknowledgeAndFinishAfter: Int?
  var continuations: [String: CheckedContinuation<String, Error>] = [:]
  var deltas: [String: @MainActor (String) -> Void] = [:]

  func captureTurnConfiguration() -> AgentTurnConfiguration? { selection }

  func streamReply(
    _ text: String, threadId: String, attachmentPaths: [String],
    configuration: AgentTurnConfiguration?,
    onDelta: @escaping @MainActor (String) -> Void,
    onReasoning: @escaping @MainActor (String) -> Void,
    onToolExecuting: @escaping @MainActor (String, String) -> Void,
    onToolResult: @escaping @MainActor (String, String, Bool, String) -> Void
  ) async throws -> String {
    starts.append(Start(text: text, threadID: threadId, configuration: configuration))
    deltas[threadId] = onDelta
    return try await withCheckedThrowingContinuation { continuations[threadId] = $0 }
  }

  func cancelReply(threadId: String) -> Bool {
    cancelled.append(threadId)
    if let threshold = acknowledgeAndFinishAfter {
      guard cancelled.count >= threshold else { return false }
      finish(threadId, result: .failure(CancellationError()))
    }
    return true
  }

  func finish(_ threadID: String, result: Result<String, Error>) {
    continuations.removeValue(forKey: threadID)?.resume(with: result)
  }
}

extension AgentChatQueueTests {
  /// Exercise the live owner with two concurrent calls and stale disk metadata.
  /// SwiftUI's accessibility subtree is not exposed through AppKit under this
  /// test host; the final wiring guards pair the behavioral proof with the view
  /// that must consume it, without showing a window on the Founder's desktop.
  func testContextPanelTracksSelectedExecutionQueueAndFrozenModelAcrossRefresh() async throws {
    final class RefreshingProvider: ThreadsFixture {
      func listThreads() -> [ChatThread] {
        PairThreadsProvider().listThreads().map { row in
          var thread = row
          thread.provider = "stale-provider"
          thread.model = "stale-model"
          return thread
        }
      }
    }
    let engine = IndependentThreadEngine()
    let suite = "AgentContextLive-\(UUID())"
    let defaults = try XCTUnwrap(UserDefaults(suiteName: suite))
    defer { defaults.removePersistentDomain(forName: suite) }
    let store = AgentChatStore(
      engine: engine, threadsProvider: RefreshingProvider(), persistenceDefaults: defaults)
    let a = try XCTUnwrap(store.threads.first { $0.backendId == "t_a" }?.id)
    let b = try XCTUnwrap(store.threads.first { $0.backendId == "t_b" }?.id)
    store.selectedThreadID = a
    sendMessage("A1", in: store)
    await waitUntil { engine.starts.count == 1 }
    sendMessage("A2", in: store)
    engine.deltas["t_a"]?("A is answering")
    XCTAssertEqual(store.selectedHeaderActivity, .answering)
    XCTAssertEqual(store.selectedModelIdentity, "provider-A/model-A")
    XCTAssertEqual(store.queuedTurns(in: a).count, 1)

    engine.selection = AgentTurnConfiguration(
      provider: "provider-B", model: "model-B", reasoningEffort: "low", receipt: "sealed-B")
    store.selectedThreadID = b
    sendMessage("B1", in: store)
    await waitUntil { engine.starts.count == 2 }
    store.refreshThreads()
    XCTAssertEqual(store.selectedThreadID, b, "refresh must not select a background turn")
    XCTAssertEqual(store.selectedHeaderActivity, .thinking)
    XCTAssertEqual(store.selectedModelIdentity, "provider-B/model-B")
    XCTAssertTrue(store.queuedTurns(in: b).isEmpty)
    store.selectedThreadID = a
    XCTAssertEqual(store.selectedHeaderActivity, .answering)
    XCTAssertEqual(store.selectedModelIdentity, "provider-A/model-A")
    XCTAssertEqual(store.queuedTurns(in: a).count, 1)
    engine.finish("t_a", result: .success("A1 complete"))
    await waitUntil { engine.starts.count == 3 }
    XCTAssertEqual(store.selectedModelIdentity, "provider-A/model-A")
    XCTAssertTrue(store.queuedTurns(in: a).isEmpty)
    engine.finish("t_a", result: .success("A2 complete"))
    engine.finish("t_b", result: .success("B1 complete"))
    await waitUntil { store.activeComposerTurns.isEmpty }
    XCTAssertEqual(store.selectedHeaderActivity, .idle)

    let screens = URL(fileURLWithPath: #filePath)
      .deletingLastPathComponent().deletingLastPathComponent()
      .appendingPathComponent("Codescribe/Screens/AgentChat")
    let panel = try String(
      contentsOf: screens.appendingPathComponent("AgentContextPanel.swift"), encoding: .utf8)
    let context = try XCTUnwrap(
      panel.components(separatedBy: "struct AgentContextCard: View").last?
        .components(separatedBy: "struct AgentContextCardLayer: View").first)
    XCTAssertTrue(context.contains("@ObservedObject var store: AgentChatStore"))
    XCTAssertTrue(context.contains("if let thread = store.currentThread"))
    XCTAssertTrue(context.contains("store.queuedTurns(in: thread.id).count"))
    // Live activity and the model belong to the header; the card repeats neither.
    XCTAssertFalse(context.contains("AgentLiveStatusPill"))
    XCTAssertFalse(context.contains("modelIdentity"))
    XCTAssertFalse(context.contains("Self.model(of:"), "no independent historical model formatter")
    let view = try String(
      contentsOf: screens.appendingPathComponent("AgentChatView.swift"), encoding: .utf8)
    XCTAssertTrue(view.contains("AgentContextCardLayer(\n          store: store,"))
  }

  func testFailedTurnKeepsItsModelAfterStaleIndexRefreshUntilNextTurnSettles() async throws {
    final class RefreshingProvider: ThreadsFixture {
      var provider = "old-provider"
      var model = "old-model"

      func listThreads() -> [ChatThread] {
        PairThreadsProvider().listThreads().map { row in
          var thread = row
          thread.provider = provider
          thread.model = model
          return thread
        }
      }
    }
    let engine = IndependentThreadEngine()
    let provider = RefreshingProvider()
    let suite = "AgentFailedModel-\(UUID())"
    let defaults = try XCTUnwrap(UserDefaults(suiteName: suite))
    defer { defaults.removePersistentDomain(forName: suite) }
    let store = AgentChatStore(
      engine: engine, threadsProvider: provider, persistenceDefaults: defaults)
    let a = try XCTUnwrap(store.threads.first { $0.backendId == "t_a" }?.id)
    store.selectedThreadID = a
    sendMessage("attempt on model A", in: store)
    await waitUntil { engine.starts.count == 1 }
    engine.finish("t_a", result: .failure(NSError(domain: "Provider", code: 400)))
    await waitUntil { store.activeComposerTurns.isEmpty }
    XCTAssertEqual(store.selectedHeaderActivity, .failed)
    store.refreshThreads()
    XCTAssertEqual(store.selectedHeaderActivity, .failed)
    XCTAssertEqual(store.selectedModelIdentity, "provider-A/model-A")

    engine.selection = AgentTurnConfiguration(
      provider: "provider-B", model: "model-B", reasoningEffort: "low", receipt: "sealed-B")
    sendMessage("retry on model B", in: store)
    await waitUntil { engine.starts.count == 2 }
    XCTAssertEqual(store.selectedModelIdentity, "provider-B/model-B")
    engine.finish("t_a", result: .success("retry complete"))
    await waitUntil { store.activeComposerTurns.isEmpty }
    provider.provider = "provider-B"
    provider.model = "model-B"
    store.refreshThreads()
    XCTAssertEqual(store.selectedHeaderActivity, .idle)
    XCTAssertEqual(store.selectedModelIdentity, "provider-B/model-B")
  }

  func testIndependentABStopWaitsForItsTerminalAndPreservesOtherThreadAndFIFO() async throws {
    let engine = IndependentThreadEngine()
    let suite = "AgentChatAB-\(UUID())"
    let defaults = try XCTUnwrap(UserDefaults(suiteName: suite))
    defer { defaults.removePersistentDomain(forName: suite) }
    let store = AgentChatStore(
      engine: engine, threadsProvider: PairThreadsProvider(), persistenceDefaults: defaults)
    let a = try XCTUnwrap(store.threads.first { $0.backendId == "t_a" }?.id)
    let b = try XCTUnwrap(store.threads.first { $0.backendId == "t_b" }?.id)
    store.selectedThreadID = a
    sendMessage("A1", in: store)
    await waitUntil { engine.starts.count == 1 }
    sendMessage("A2", in: store)
    store.selectedThreadID = b
    sendMessage("B1", in: store)
    await waitUntil("B starts while A is still suspended") { engine.starts.count == 2 }
    XCTAssertEqual(Set(engine.continuations.keys), ["t_a", "t_b"])
    engine.deltas["t_b"]?("B continues")
    store.selectedThreadID = a
    let oldDelta = engine.deltas["t_a"]
    store.stopActiveTurn()
    XCTAssertEqual(engine.cancelled, ["t_a"])
    XCTAssertEqual(store.headerActivity(in: b), .answering)
    XCTAssertEqual(store.headerActivity(in: a), .stopping)
    XCTAssertEqual(store.selectedHeaderActivity, .stopping)
    try? await Task.sleep(for: .milliseconds(60))
    XCTAssertEqual(engine.starts.map(\.text), ["A1", "B1"], "ACK is not a terminal")
    XCTAssertNotNil(engine.continuations["t_b"])
    engine.finish("t_a", result: .failure(CancellationError()))
    await waitUntil { engine.starts.count == 3 }
    XCTAssertEqual(engine.starts[2].text, "A2")
    oldDelta?("obsolete A1 text")
    XCTAssertFalse(store.currentThread?.messages.last?.text.contains("obsolete") == true)
    engine.finish("t_a", result: .success("A2 done"))
    engine.finish("t_b", result: .success("B done"))
    await waitUntil { store.queuedTurns.isEmpty && store.activeComposerTurn == nil }
    XCTAssertTrue(
      store.threads.first { $0.id == b }?.messages.contains { $0.text == "B continues" } == true)
  }

  func testQueuedTurnRetainsAcceptedModelAfterSelectionChanges() async throws {
    let engine = IndependentThreadEngine()
    let suite = "AgentChatModel-\(UUID())"
    let defaults = try XCTUnwrap(UserDefaults(suiteName: suite))
    defer { defaults.removePersistentDomain(forName: suite) }
    let store = AgentChatStore(
      engine: engine, threadsProvider: PairThreadsProvider(), persistenceDefaults: defaults)
    store.selectedThreadID = try XCTUnwrap(store.threads.first { $0.backendId == "t_a" }?.id)
    sendMessage("first", in: store)
    await waitUntil { engine.starts.count == 1 }
    sendMessage("accepted model A", in: store)
    let receipt = try XCTUnwrap(defaults.data(forKey: AgentChatStore.acceptedTurnsDefaultsKey))
    XCTAssertTrue(String(decoding: receipt, as: UTF8.self).contains("sealed-A"))
    engine.selection = AgentTurnConfiguration(
      provider: "provider-B", model: "model-B", reasoningEffort: "low", receipt: "sealed-B")
    engine.finish("t_a", result: .success("done"))
    await waitUntil { engine.starts.count == 2 }
    XCTAssertEqual(engine.starts[1].configuration?.model, "model-A")
    XCTAssertEqual(engine.starts[1].configuration?.reasoningEffort, "high")
    XCTAssertEqual(store.currentThread?.model, "model-A")
    engine.finish("t_a", result: .success("done"))
    await waitUntil { store.activeComposerTurn == nil }
  }
  func testRestartUsesPersistedAcceptedModelInsteadOfCurrentSelection() async throws {
    let engine = IndependentThreadEngine()
    let suite = "AgentChatRestoreModel-\(UUID())"
    let restoredSuite = "AgentChatRestoredModel-\(UUID())"
    let defaults = try XCTUnwrap(UserDefaults(suiteName: suite))
    let restoredDefaults = try XCTUnwrap(UserDefaults(suiteName: restoredSuite))
    defer {
      defaults.removePersistentDomain(forName: suite)
      restoredDefaults.removePersistentDomain(forName: restoredSuite)
    }
    let store = AgentChatStore(
      engine: engine, threadsProvider: PairThreadsProvider(), persistenceDefaults: defaults)
    store.selectedThreadID = try XCTUnwrap(store.threads.first { $0.backendId == "t_a" }?.id)
    sendMessage("blocking", in: store)
    await waitUntil { engine.starts.count == 1 }
    sendMessage("queued before death", in: store)
    let data = try XCTUnwrap(defaults.data(forKey: AgentChatStore.acceptedTurnsDefaultsKey))
    let records = try XCTUnwrap(JSONSerialization.jsonObject(with: data) as? [[String: Any]])
    let queued = records.filter { $0["text"] as? String == "queued before death" }
    restoredDefaults.set(
      try JSONSerialization.data(withJSONObject: queued),
      forKey: AgentChatStore.acceptedTurnsDefaultsKey)
    // Separate defaults represent the relaunched process's durable snapshot;
    // drain the first process so the fixture owns no abandoned continuations.
    engine.finish("t_a", result: .success("done"))
    await waitUntil { engine.starts.count == 2 }
    engine.finish("t_a", result: .success("done"))
    await waitUntil { store.activeComposerTurn == nil }
    let restartedEngine = IndependentThreadEngine()
    restartedEngine.selection = AgentTurnConfiguration(
      provider: "new-provider", model: "new-model", reasoningEffort: "low", receipt: "new")
    let restarted = AgentChatStore(
      engine: restartedEngine, threadsProvider: PairThreadsProvider(),
      persistenceDefaults: restoredDefaults)
    await waitUntil { restartedEngine.starts.count == 1 }
    XCTAssertEqual(restartedEngine.starts[0].text, "queued before death")
    XCTAssertEqual(restartedEngine.starts[0].configuration?.model, "model-A")
    XCTAssertEqual(restartedEngine.starts[0].configuration?.receipt, "sealed-A")
    restartedEngine.finish("t_a", result: .success("done"))
    await waitUntil { restarted.activeComposerTurns.isEmpty }
  }

  func testStopStillCancelsWhenBackendRegistrationTakesMoreThanTwoSeconds() async throws {
    let engine = IndependentThreadEngine()
    engine.acknowledgeAndFinishAfter = 90
    let suite = "AgentChatSlowCancel-\(UUID())"
    let defaults = try XCTUnwrap(UserDefaults(suiteName: suite))
    defer { defaults.removePersistentDomain(forName: suite) }
    let store = AgentChatStore(
      engine: engine, threadsProvider: PairThreadsProvider(), persistenceDefaults: defaults)
    store.selectedThreadID = try XCTUnwrap(store.threads.first { $0.backendId == "t_a" }?.id)
    sendMessage("slow preparation", in: store)
    await waitUntil { engine.starts.count == 1 }
    store.stopActiveTurn()
    await waitUntil(timeout: 4, "Stop must not abandon a late registry") {
      engine.cancelled.count == 90 && store.activeComposerTurns.isEmpty
    }
    XCTAssertEqual(Set(engine.cancelled), ["t_a"])
  }

  func testImmediateStopBeforeEngineEntryNeverDispatchesTheAcceptedTurn() async throws {
    let engine = IndependentThreadEngine()
    let suite = "AgentChatImmediateStop-\(UUID())"
    let defaults = try XCTUnwrap(UserDefaults(suiteName: suite))
    defer { defaults.removePersistentDomain(forName: suite) }
    let store = AgentChatStore(
      engine: engine, threadsProvider: PairThreadsProvider(), persistenceDefaults: defaults)
    store.selectedThreadID = try XCTUnwrap(store.threads.first { $0.backendId == "t_a" }?.id)
    sendMessage("stop before dispatch", in: store)
    store.stopActiveTurn()
    await waitUntil { store.activeComposerTurns.isEmpty }
    XCTAssertTrue(engine.starts.isEmpty)
    XCTAssertTrue(store.currentThread?.messages.last?.wasStopped == true)
  }

}
