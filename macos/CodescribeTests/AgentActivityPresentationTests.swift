import XCTest

@testable import Codescribe

/// Header phase follows lifecycle events. This file names the new projection,
/// so it compiles on the cut and not on the baseline. The window falsifier is
/// `AgentWindowChromeTests`.
@MainActor
final class AgentActivityPresentationTests: XCTestCase {
  func testVoiceLifecycleSurvivesToolBurstsAndThreadSelection() throws {
    let store = AgentChatStore(engine: nil)
    let threadID = try XCTUnwrap(store.selectedThreadID)

    store.ingestVoiceTurn(threadId: "backend-probe", userText: "probe")
    expect(store, .thinking, label("Thinking", comment: "Turn status"))

    store.ingestVoiceToolExecuting(name: "shell", id: "a")
    store.ingestVoiceToolExecuting(name: "shell", id: "b")
    expect(
      store, .usingTools(running: 2),
      label("tool", comment: "Tool activity row: a tool call is running"))

    store.ingestVoiceToolResult(name: "shell", id: "a", isError: false, reason: "ok")
    expect(
      store, .usingTools(running: 1),
      label("tool", comment: "Tool activity row: a tool call is running"))

    store.ingestVoiceDelta("The answer")
    expect(
      store, .usingTools(running: 1),
      label("tool", comment: "Tool activity row: a tool call is running"))
    store.ingestVoiceToolResult(name: "shell", id: "b", isError: false, reason: "ok")
    expect(store, .answering, label("Streaming", comment: "Turn status"))
    XCTAssertEqual(assistant(in: store)?.isStreaming, true)

    store.ingestVoiceToolExecuting(name: "shell", id: "c")
    expect(
      store, .usingTools(running: 1),
      label("tool", comment: "Tool activity row: a tool call is running"))
    store.ingestVoiceToolResult(name: "shell", id: "c", isError: false, reason: "ok")
    expect(store, .answering, label("Streaming", comment: "Turn status"))

    store.ingestVoiceReasoning("still working")
    expect(store, .thinking, label("Thinking", comment: "Turn status"))
    XCTAssertEqual(
      assistant(in: store)?.isStreaming, true,
      "the caret flag stays sticky; the header must not read it")

    store.newThread()
    let other = try XCTUnwrap(store.selectedThreadID)
    XCTAssertNotEqual(other, threadID)
    XCTAssertEqual(store.selectedHeaderActivity, .idle)
    XCTAssertNil(store.selectedHeaderActivity.statusLabel)

    store.select(threadID)
    expect(store, .thinking, label("Thinking", comment: "Turn status"))
    store.select(other)
    XCTAssertEqual(store.selectedHeaderActivity, .idle)

    store.select(threadID)
    store.ingestVoiceDone()
    XCTAssertEqual(store.selectedHeaderActivity, .idle)

    store.ingestVoiceTurn(threadId: "backend-probe", userText: "again")
    store.ingestVoiceError("nope")
    expect(store, .failed, label("failed", comment: "Tool activity row: tool call failed"))
  }

  func testComposerHeaderLeavesStreamingWhenReasoningOrToolsContinue() {
    let (store, engine) = scriptedStore(backendID: "backend-header")
    store.draft = "probe the header"
    store.send()
    pump { engine.onDelta != nil }
    expect(store, .thinking, label("Thinking", comment: "Turn status"))

    engine.onDelta?("partial")
    expect(store, .answering, label("Streaming", comment: "Turn status"))
    XCTAssertEqual(assistant(in: store)?.isStreaming, true)

    engine.onReasoning?("because")
    expect(store, .thinking, label("Thinking", comment: "Turn status"))
    XCTAssertEqual(assistant(in: store)?.isStreaming, true)

    engine.onToolExecuting?("shell", "a")
    engine.onToolExecuting?("shell", "b")
    expect(
      store, .usingTools(running: 2),
      label("tool", comment: "Tool activity row: a tool call is running"))
    engine.onToolResult?("shell", "a", false, "ok")
    expect(
      store, .usingTools(running: 1),
      label("tool", comment: "Tool activity row: a tool call is running"))

    let elsewhere = approval(threadID: "other-thread", callID: "foreign")
    let here = approval(threadID: "backend-header", callID: "local")
    engine.emit(elsewhere)
    XCTAssertTrue(store.currentToolApprovals.isEmpty)
    expect(
      store, .usingTools(running: 1),
      label("tool", comment: "Tool activity row: a tool call is running"))
    engine.emit(here)
    expect(store, .awaitingApproval(count: 1), label("Permission required", comment: nil))

    store.resolveToolApproval(here, approved: true)
    expect(
      store, .usingTools(running: 1),
      label("tool", comment: "Tool activity row: a tool call is running"))
    store.resolveToolApproval(elsewhere, approved: false)
    XCTAssertTrue(store.pendingToolApprovals.isEmpty)

    engine.onToolResult?("shell", "b", false, "ok")
    expect(store, .thinking, label("Thinking", comment: "Turn status"))

    store.stopActiveTurn()
    expect(store, .stopping, label("Stopping", comment: "Turn status"))
    engine.finish?(.failure(CancellationError()))
    pump { store.activeComposerTurn == nil }
    XCTAssertEqual(store.selectedHeaderActivity, .idle)
  }

  func testComposerCompletionAndFailureSettleTheHeader() {
    let (store, engine) = scriptedStore(backendID: "backend-done")
    store.draft = "finish cleanly"
    store.send()
    pump { engine.onDelta != nil }
    engine.onDelta?("done")
    expect(store, .answering, label("Streaming", comment: "Turn status"))
    engine.finish?(.success("done"))
    pump { store.activeComposerTurn == nil }
    XCTAssertEqual(store.selectedHeaderActivity, .idle)

    let (failed, failing) = scriptedStore(backendID: "backend-fail")
    failed.draft = "finish badly"
    failed.send()
    pump { failing.onToolExecuting != nil }
    failing.onToolExecuting?("shell", "a")
    expect(
      failed, .usingTools(running: 1),
      label("tool", comment: "Tool activity row: a tool call is running"))
    failing.finish?(.failure(NSError(domain: "codescribe.test", code: 7)))
    pump { failed.activeComposerTurn == nil }
    expect(failed, .failed, label("failed", comment: "Tool activity row: tool call failed"))
  }

  func testSidebarSelectionEmphasisIsStrongerWhenContrastIncreases() {
    XCTAssertGreaterThan(
      AgentSidebarSelection.fillOpacity(increaseContrast: true),
      AgentSidebarSelection.fillOpacity(increaseContrast: false))
    XCTAssertGreaterThan(
      AgentSidebarSelection.strokeWidth(increaseContrast: true),
      AgentSidebarSelection.strokeWidth(increaseContrast: false))
  }

  private func label(_ key: String, comment _: String? = nil) -> String {
    switch key {
    case "Thinking":
      String(localized: "Thinking", comment: "Turn status")
    case "Streaming":
      String(localized: "Streaming", comment: "Turn status")
    case "Stopping":
      String(localized: "Stopping", comment: "Turn status")
    case "tool":
      String(localized: "tool", comment: "Tool activity row: a tool call is running")
    case "failed":
      String(localized: "failed", comment: "Tool activity row: tool call failed")
    case "Permission required":
      String(localized: "Permission required")
    default:
      key
    }
  }

  private func expect(
    _ store: AgentChatStore, _ activity: AgentHeaderActivity, _ localized: String,
    file: StaticString = #filePath, line: UInt = #line
  ) {
    XCTAssertEqual(store.selectedHeaderActivity, activity, file: file, line: line)
    XCTAssertEqual(store.selectedHeaderActivity.statusLabel, localized, file: file, line: line)
  }

  private func assistant(in store: AgentChatStore) -> ChatMessage? {
    store.currentThread?.messages.last { $0.role == .assistant }
  }

  private func approval(threadID: String, callID: String) -> PendingToolApproval {
    PendingToolApproval(
      callID: callID,
      sessionID: "session-\(callID)",
      threadID: threadID,
      tool: "shell",
      server: "local",
      risk: "read",
      summary: callID,
      command: nil,
      cwd: nil,
      paths: []
    )
  }

  private func scriptedStore(backendID: String) -> (AgentChatStore, ScriptedEngine) {
    let engine = ScriptedEngine()
    var thread = ChatThread(title: "Probe", meta: "now")
    thread.backendId = backendID
    thread.messagesLoaded = true
    let store = AgentChatStore(engine: engine, threads: [thread])
    return (store, engine)
  }

  private func pump(until predicate: () -> Bool) {
    let deadline = Date().addingTimeInterval(2)
    while Date() < deadline {
      RunLoop.current.run(until: Date().addingTimeInterval(0.02))
      if predicate() { return }
    }
  }
}

private final class ScriptedEngine: ChatEngineFixture, @unchecked Sendable {
  var onDelta: (@MainActor (String) -> Void)?
  var onReasoning: (@MainActor (String) -> Void)?
  var onToolExecuting: (@MainActor (String, String) -> Void)?
  var onToolResult: (@MainActor (String, String, Bool, String) -> Void)?
  var approval: (@MainActor (PendingToolApproval) -> Void)?
  var finish: ((Result<String, Error>) -> Void)?

  func streamReply(
    _ text: String,
    threadId: String,
    attachmentPaths: [String],
    onDelta: @escaping @MainActor (String) -> Void,
    onReasoning: @escaping @MainActor (String) -> Void,
    onToolExecuting: @escaping @MainActor (String, String) -> Void,
    onToolResult: @escaping @MainActor (String, String, Bool, String) -> Void
  ) async throws -> String {
    self.onDelta = onDelta
    self.onReasoning = onReasoning
    self.onToolExecuting = onToolExecuting
    self.onToolResult = onToolResult
    return try await withCheckedThrowingContinuation { continuation in
      self.finish = { continuation.resume(with: $0) }
    }
  }

  func cancelReply(threadId: String) -> Bool { true }

  func installToolApprovalHandler(
    _ handler: @escaping @MainActor (PendingToolApproval) -> Void
  ) {
    approval = handler
  }

  @MainActor
  func emit(_ request: PendingToolApproval) {
    approval?(request)
  }
}
