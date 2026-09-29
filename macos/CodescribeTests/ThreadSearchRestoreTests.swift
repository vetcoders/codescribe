import Foundation
import XCTest

@testable import Codescribe

/// Founder 2026-09-09 14:14: "wyszukiwarka threads odsiewa wyniki i super
/// działa, ale już nie wraca pierwotna lista threads". Clearing the search
/// field must restore the full rail, whatever the last query matched.
@MainActor
final class ThreadSearchRestoreTests: XCTestCase {
  private final class StubThreadsProvider: ChatThreadsProviding {
    var stubbed: [(id: String, title: String)]
    private(set) var listCalls = 0
    private(set) var searchQueries: [String] = []

    init(_ stubbed: [(id: String, title: String)]) { self.stubbed = stubbed }

    private func row(_ entry: (id: String, title: String)) -> ChatThread {
      var thread = ChatThread(title: entry.title, meta: "now")
      thread.backendId = entry.id
      thread.messagesLoaded = true
      thread.updatedAt = Date()
      return thread
    }

    func listThreads() -> [ChatThread] {
      listCalls += 1
      return stubbed.map(row)
    }

    func searchThreads(query: String) -> [ChatThread] {
      searchQueries.append(query)
      let q = query.lowercased()
      return stubbed.filter { $0.title.lowercased().contains(q) }.map(row)
    }

    func loadMessages(backendId: String) -> [ChatMessage] { [] }
    func deleteThread(backendId: String) -> Bool { true }
    func setThreadFavorite(backendId: String, isFavorite: Bool) -> Bool { true }
    func renameThread(backendId: String, title: String) -> Bool { true }
    func setGeneratedTitle(backendId: String, title: String) -> Bool { true }
    func exportThreadMarkdown(backendId: String, assistantOnly: Bool) -> String? { nil }
    func generateThreadId() -> String { "t_generated" }
  }

  private func backendIds(_ store: AgentChatStore) -> [String] {
    store.threads.compactMap(\.backendId)
  }

  private func makeStore() -> (AgentChatStore, StubThreadsProvider) {
    let provider = StubThreadsProvider([
      ("t_dmg", "w tym gościem jest DMG"),
      ("t_shake", "Prawidłowy shake"),
      ("t_hello", "Powitanie użytkownika"),
    ])
    let store = AgentChatStore(threadsProvider: provider)
    XCTAssertEqual(backendIds(store), ["t_dmg", "t_shake", "t_hello"])
    return (store, provider)
  }

  func testClearingAfterMatchRestoresFullRail() {
    let (store, _) = makeStore()
    store.searchThreads("shake")
    XCTAssertEqual(backendIds(store), ["t_shake"])
    store.searchThreads("")
    XCTAssertEqual(backendIds(store), ["t_dmg", "t_shake", "t_hello"])
  }

  func testClearingAfterNoMatchRestoresFullRail() {
    let (store, _) = makeStore()
    store.searchThreads("zzz-nothing")
    XCTAssertEqual(backendIds(store), [])
    store.searchThreads("")
    XCTAssertEqual(backendIds(store), ["t_dmg", "t_shake", "t_hello"])
    XCTAssertNotNil(store.selectedThreadID)
  }

  func testNarrowingThenWideningQueryTracksProvider() {
    let (store, provider) = makeStore()
    store.searchThreads("p")
    XCTAssertEqual(backendIds(store), ["t_shake", "t_hello"])
    store.searchThreads("po")
    XCTAssertEqual(backendIds(store), ["t_hello"])
    store.searchThreads("p")
    XCTAssertEqual(backendIds(store), ["t_shake", "t_hello"])
    store.searchThreads("   ")
    XCTAssertEqual(backendIds(store), ["t_dmg", "t_shake", "t_hello"])
    XCTAssertEqual(provider.searchQueries, ["p", "po", "p"])
  }

  func testClearingWithLocalDraftSelectedRestoresRail() {
    let (store, _) = makeStore()
    store.newThread()
    XCTAssertNil(store.currentThread?.backendId)
    store.searchThreads("dmg")
    store.searchThreads("")
    XCTAssertEqual(backendIds(store), ["t_dmg", "t_shake", "t_hello"])
  }
}
