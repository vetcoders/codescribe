import AppKit
import Foundation
import XCTest

@testable import Codescribe

/// Selection-policy proof for voice/agent delivery.
///
/// Persisted events may update any thread, but selection belongs to explicit
/// rail actions. This guards both historical focus-switch sites:
/// `ingestVoiceTurn` and the completion refresh in `replaceThreads`.
@MainActor
final class AgentThreadContinuityTests: XCTestCase {
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
        thread.messages = [
          ChatMessage(role: .assistant, timestamp: "earlier", text: "\(row.id) history")
        ]
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

  private func thread(_ backendID: String, in store: AgentChatStore) -> ChatThread? {
    store.threads.first { $0.backendId == backendID }
  }

  private func drainMainQueue() {
    let drained = expectation(description: "main queue drained")
    DispatchQueue.main.async { drained.fulfill() }
    wait(for: [drained], timeout: 2)
  }

  func testActivationAppendsToCurrentlyOpenMatchingThreadWithoutChangingSelection() {
    let provider = StubThreadsProvider([
      ("t_active", "Active"),
      ("t_other", "Other"),
    ])
    let store = AgentChatStore(threadsProvider: provider)
    let activeID = store.selectedThreadID

    store.ingestVoiceTurn(threadId: "t_active", userText: "continue here")
    store.ingestVoiceDelta("answer")

    XCTAssertEqual(store.selectedThreadID, activeID)
    XCTAssertEqual(store.currentThread?.backendId, "t_active")
    XCTAssertTrue(
      store.currentThread?.messages.contains { message in
        message.role == .you && message.text == "continue here"
      } == true)
    XCTAssertEqual(store.currentThread?.messages.last?.text, "answer")
  }

  func testGenerationForBackgroundThreadNeverMovesThreadOrVisibleMessages() {
    let provider = StubThreadsProvider([
      ("t_visible", "Visible"),
      ("t_background", "Background"),
    ])
    let store = AgentChatStore(threadsProvider: provider)
    let visibleID = store.selectedThreadID
    let visibleMessageIDs = store.currentThread?.messages.map(\.id)

    store.ingestVoiceTurn(threadId: "t_background", userText: "background request")
    store.ingestVoiceDelta("background response")

    XCTAssertEqual(store.selectedThreadID, visibleID, "turn start must not move selection")
    XCTAssertEqual(store.currentThread?.messages.map(\.id), visibleMessageIDs)
    XCTAssertEqual(thread("t_background", in: store)?.messages.last?.text, "background response")

    store.ingestVoiceDone()
    ThreadsChangeBus.postThreadsChanged()

    XCTAssertEqual(
      store.selectedThreadID, visibleID, "terminal refresh and bus must preserve selection")
    XCTAssertEqual(store.currentThread?.backendId, "t_visible")
    XCTAssertEqual(store.currentThread?.messages.map(\.id), visibleMessageIDs)
  }

  func testVoiceTurnAdoptsSelectedEmptyDraftInsteadOfMintingParallelThread() {
    let provider = StubThreadsProvider([("t_history", "History")])
    let store = AgentChatStore(threadsProvider: provider)
    store.newThread()
    let draftID = store.selectedThreadID
    let threadCount = store.threads.count

    store.ingestVoiceTurn(threadId: "t_voice_session", userText: "Ze względu na fakt że…")
    store.ingestVoiceDelta("odpowiedź")

    XCTAssertEqual(store.selectedThreadID, draftID, "voice turn must land in the open draft")
    XCTAssertEqual(store.currentThread?.backendId, "t_voice_session")
    XCTAssertEqual(store.threads.count, threadCount, "no parallel thread may be minted")
    XCTAssertTrue(
      store.currentThread?.messages.contains { message in
        message.role == .you && message.text.contains("Ze względu")
      } == true)
    XCTAssertEqual(store.currentThread?.messages.last?.text, "odpowiedź")
    XCTAssertNotEqual(store.currentThread?.title, "New thread", "adopted draft takes a real title")
  }

  func testCaptureOwnerSurvivesDoneQueuedRefreshAndSummonUntilExplicitSelection() {
    let provider = StubThreadsProvider([
      ("t_history", "History"),
      ("t_other", "Other"),
    ])
    let engine = SpyRoutingEngine()
    let store = AgentChatStore(engine: engine, threadsProvider: provider)
    XCTAssertEqual(engine.assistiveTargets.compactMap { $0 }.last, "t_history")

    // Starting from an explicit empty thread publishes nil: the controller
    // must mint a backend for this selected logical owner.
    let targetsBeforeNewThread = engine.assistiveTargets.count
    store.newThread()
    let captureOwnerID = store.selectedThreadID
    XCTAssertGreaterThan(engine.assistiveTargets.count, targetsBeforeNewThread)
    XCTAssertNil(engine.assistiveTargets.last!)

    // TurnStarted binds the backend to that same local row and immediately
    // republishes the now-stable backend routing target.
    store.ingestVoiceTurn(threadId: "t_voice_owner", userText: "keep this thread")
    XCTAssertEqual(store.selectedThreadID, captureOwnerID)
    XCTAssertEqual(store.currentThread?.backendId, "t_voice_owner")
    XCTAssertEqual(engine.assistiveTargets.compactMap { $0 }.last, "t_voice_owner")

    provider.rows = [
      ("t_voice_owner", "Persisted voice turn"),
      ("t_other", "Other"),
      ("t_history", "History"),
    ]
    store.ingestVoiceDone()

    // Reproduce the persistence/index seam: a queued bus refresh and the
    // didBecomeKey emitted by a later summon both observe a transient index
    // snapshot without the just-completed owner row.
    provider.rows = [
      ("t_other", "Other"),
      ("t_history", "History"),
    ]
    ThreadsChangeBus.postThreadsChanged()
    let summon = AgentSummonAction(store: store) {
      NotificationCenter.default.post(name: NSWindow.didBecomeKeyNotification, object: nil)
    }
    summon.perform()
    drainMainQueue()

    XCTAssertEqual(store.selectedThreadID, captureOwnerID)
    XCTAssertEqual(store.currentThread?.backendId, "t_voice_owner")
    XCTAssertEqual(engine.assistiveTargets.compactMap { $0 }.last, "t_voice_owner")

    // Explicit navigation remains authoritative and is preserved by the same
    // later refresh path; continuity must never become a UI pin.
    let otherID = thread("t_other", in: store)!.id
    store.select(otherID)
    provider.rows = [
      ("t_voice_owner", "Persisted voice turn"),
      ("t_other", "Other"),
      ("t_history", "History"),
    ]
    ThreadsChangeBus.postThreadsChanged()
    drainMainQueue()

    XCTAssertEqual(store.selectedThreadID, otherID)
    XCTAssertEqual(store.currentThread?.backendId, "t_other")
    XCTAssertEqual(engine.assistiveTargets.compactMap { $0 }.last, "t_other")
  }

  func testStoppedCaptureHasNoComposerPreviewCallbackSurfaceToResurrect() throws {
    let macosDir = URL(fileURLWithPath: #filePath)
      .deletingLastPathComponent()  // CodescribeTests/
      .deletingLastPathComponent()  // macos/
    let sources = [
      "Codescribe/Core/ComposerDictation.swift",
      "Codescribe/Screens/AgentChat/AgentChatStore.swift",
      "Codescribe/Screens/AgentChat/Composer.swift",
    ]
    let orphanedPreviewTokens = [
      "ComposerDictationListener",
      "CsTranscriptionListener",
      "dictationPreview",
      "onPreview(",
      "onVadState(",
    ]

    for relative in sources {
      let text = try String(
        contentsOf: macosDir.appendingPathComponent(relative), encoding: .utf8)
      for token in orphanedPreviewTokens {
        XCTAssertFalse(
          text.contains(token),
          "\(relative) must not retain `\(token)`; a late callback after stop could recreate the orphaned composer preview"
        )
      }
    }
  }

  func testVoiceTurnWithUnknownIdStillMintsThreadWhenSelectionIsBound() {
    let provider = StubThreadsProvider([("t_history", "History")])
    let store = AgentChatStore(threadsProvider: provider)
    let selectedID = store.selectedThreadID
    let threadCount = store.threads.count

    store.ingestVoiceTurn(threadId: "t_new_session", userText: "fresh voice")

    XCTAssertEqual(store.selectedThreadID, selectedID, "selection must not move")
    XCTAssertEqual(store.threads.count, threadCount + 1)
    XCTAssertEqual(thread("t_new_session", in: store)?.messages.first?.role, .you)
  }

  func testOnlyExplicitNewThreadActionSelectsFreshThread() {
    let provider = StubThreadsProvider([
      ("t_first", "First"),
      ("t_second", "Second"),
    ])
    let store = AgentChatStore(threadsProvider: provider)
    let secondID = thread("t_second", in: store)!.id
    store.select(secondID)

    store.ingestVoiceTurn(threadId: "t_first", userText: "do not switch")
    XCTAssertEqual(store.selectedThreadID, secondID)

    store.newThread()

    XCTAssertNotEqual(store.selectedThreadID, secondID)
    XCTAssertNil(store.currentThread?.backendId)
    XCTAssertEqual(store.currentThread?.title, "New thread")
  }
}
