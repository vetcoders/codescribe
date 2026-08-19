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
