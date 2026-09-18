import Foundation
import XCTest

@testable import Codescribe

/// W1 contracts authored under the compile embargo. No provider or audio probe.
@MainActor
final class RCW1SpeechAgentTests: XCTestCase {
  private enum ReadFailure: Error { case unavailable }

  private final class Threads: ChatThreadsProviding {
    var failList = false
    var failSearch = false
    var records = [("alpha", "Alpha"), ("beta", "Beta"), ("gamma", "Gamma")]

    private func row(_ record: (String, String)) -> ChatThread {
      var result = ChatThread(title: record.1, meta: "now")
      result.backendId = record.0
      result.messagesLoaded = true
      return result
    }

    func listThreads() throws -> [ChatThread] {
      if failList { throw ReadFailure.unavailable }
      return records.map(row)
    }

    func searchThreads(query: String) throws -> [ChatThread] {
      if failSearch { throw ReadFailure.unavailable }
      return records.filter { $0.1.localizedStandardContains(query) }.map(row)
    }

    func loadMessages(backendId: String) -> [ChatMessage] { [] }
    func deleteThread(backendId: String) -> Bool {
      records.removeAll { $0.0 == backendId }
      return true
    }
    func setThreadFavorite(backendId: String, isFavorite: Bool) -> Bool { true }
    func renameThread(backendId: String, title: String) -> Bool { true }
    func setGeneratedTitle(backendId: String, title: String) -> Bool { true }
    func exportThreadMarkdown(backendId: String, assistantOnly: Bool) -> String? { nil }
    func generateThreadId() -> String { "rc-w1-draft" }
  }

  private final class SpeechEngine: AgentChatEngine {
    var pending: [CheckedContinuation<Void, Error>] = []
    var targets: [String?] = []
    var stopCount = 0
    let starts = AsyncStream<Void>.makeStream()
    func isAvailable() -> Bool { true }
    func availabilityDetail() -> String? { nil }
    func speechAvailability() -> String? { nil }
    func speak(text: String) async throws {
      try await withCheckedThrowingContinuation { continuation in
        pending.append(continuation)
        starts.continuation.yield(())
      }
    }
    func stopSpeaking() { stopCount += 1 }
    func setAssistiveTargetThread(backendId: String?) { targets.append(backendId) }
    func generateThreadTitle(_ text: String) async throws -> String? { nil }
    func cancelReply(threadId: String) -> Bool { false }
    func streamReply(
      _ text: String, threadId: String, attachmentPaths: [String],
      onDelta: @escaping @MainActor (String) -> Void,
      onReasoning: @escaping @MainActor (String) -> Void,
      onToolExecuting: @escaping @MainActor (String, String) -> Void,
      onToolResult: @escaping @MainActor (String, String, Bool, String) -> Void
    ) async throws -> String { text }
  }

  func testSearchNeverChangesSelectedIdentityOrAssistiveTarget() {
    let provider = Threads()
    let engine = SpeechEngine()
    let store = AgentChatStore(engine: engine, threadsProvider: provider)
    defer { store.invalidate() }
    let ids = store.threads.map(\.id)
    let selected = store.selectedThreadID
    let targetCount = engine.targets.count
    for query in ["Beta", "no match", "Gamma"] {
      store.searchThreads(query)
      XCTAssertEqual(store.selectedThreadID, selected)
      XCTAssertEqual(store.currentThread?.backendId, "alpha")
      XCTAssertEqual(engine.targets.count, targetCount)
    }
    store.searchThreads(" \n ")
    XCTAssertEqual(store.threads.map(\.id), ids)
    XCTAssertEqual(store.selectedThreadID, selected)
  }

  func testClearAfterReadFailureRestoresRowsAndDraftIdentity() {
    let provider = Threads()
    let store = AgentChatStore(threadsProvider: provider)
    defer { store.invalidate() }
    store.newThread()
    let draftID = store.selectedThreadID
    let ids = Set(store.threads.map(\.id))
    store.searchThreads("no match")
    XCTAssertTrue(store.threads.isEmpty)
    provider.failList = true
    store.searchThreads("")
    XCTAssertEqual(Set(store.threads.map(\.id)), ids)
    XCTAssertEqual(store.selectedThreadID, draftID)
    XCTAssertNil(store.currentThread?.backendId)
    XCTAssertNotNil(store.threadSearchError)
    XCTAssertEqual(store.threadSearchQuery, "")
  }

  func testSearchFailureKeepsPreviousResultsAndClearStillWorks() {
    let provider = Threads()
    let store = AgentChatStore(threadsProvider: provider)
    defer { store.invalidate() }
    store.searchThreads("Beta")
    let matchedIDs = store.threads.map(\.id)
    provider.failSearch = true
    store.searchThreads("Gamma")
    XCTAssertEqual(store.threads.map(\.id), matchedIDs)
    XCTAssertNotNil(store.threadSearchError)
    store.searchThreads("")
    XCTAssertEqual(store.threads.compactMap(\.backendId), ["alpha", "beta", "gamma"])
    XCTAssertNil(store.threadSearchError)
  }

  func testExternalRefreshKeepsSearchAndExplicitSelectionSurvivesClear() throws {
    let store = AgentChatStore(threadsProvider: Threads())
    defer { store.invalidate() }
    store.searchThreads("Beta")
    let beta = try XCTUnwrap(store.threads.first)
    store.select(beta.id)
    store.refreshThreadsFromExternalChange()
    XCTAssertEqual(store.threads.compactMap(\.backendId), ["beta"])
    store.searchThreads("")
    XCTAssertEqual(store.selectedThreadID, beta.id)
    XCTAssertEqual(store.currentThread?.backendId, "beta")
  }

  func testLateInitialIndexCannotOverwriteNewerSearch() async {
    let store = AgentChatStore(threadsProvider: Threads(), loadsThreadIndexEagerly: false)
    defer { store.invalidate() }
    store.searchThreads("Beta")
    let ids = store.threads.map(\.id)
    await store.initialThreadIndexTask?.value
    XCTAssertEqual(store.threads.compactMap(\.backendId), ["beta"])
    XCTAssertEqual(store.threads.map(\.id), ids)
    XCTAssertEqual(store.threadSearchQuery, "Beta")
  }

  func testCaptureOwnershipSurvivesSearchAndClear() {
    let store = AgentChatStore(threadsProvider: Threads())
    defer { store.invalidate() }
    let selected = store.selectedThreadID
    store.searchThreads("Beta")
    store.setDictationPhase(.recording)
    store.searchThreads("Gamma")
    XCTAssertNotNil(store.threadSearchError)
    XCTAssertEqual(store.dictationThreadID, selected)
    store.searchThreads("")
    XCTAssertEqual(store.dictationThreadID, selected)
    XCTAssertEqual(store.selectedThreadID, selected)
    XCTAssertEqual(store.threads.compactMap(\.backendId), ["alpha", "beta", "gamma"])
    store.endDictationSession()
  }

  func testVoiceTurnRestoresHiddenSelectedRowWithoutDuplicatingIdentity() {
    let store = AgentChatStore(threadsProvider: Threads())
    defer { store.invalidate() }
    let selected = store.selectedThreadID
    store.searchThreads("no match")
    store.ingestVoiceTurn(threadId: "alpha", userText: "Hello")
    XCTAssertEqual(store.threadSearchQuery, "")
    XCTAssertEqual(store.selectedThreadID, selected)
    XCTAssertEqual(store.threads.filter { $0.backendId == "alpha" }.count, 1)
    XCTAssertTrue(store.currentThread?.messages.contains { $0.role == .you && $0.text == "Hello" } == true)
    store.ingestVoiceCancelled(threadId: "alpha")
  }

  func testDeletedSearchResultDoesNotReturnFromRestorationSnapshot() throws {
    let store = AgentChatStore(threadsProvider: Threads())
    defer { store.invalidate() }
    store.searchThreads("Beta")
    store.delete(try XCTUnwrap(store.threads.first))
    store.searchThreads("")
    XCTAssertFalse(store.threads.contains { $0.backendId == "beta" })
  }

  func testStoppedSpeechLateFailureCannotClearOrFailTheNextRequest() async {
    let engine = SpeechEngine()
    let store = AgentChatStore(engine: engine)
    let first = ChatMessage(role: .assistant, timestamp: "now", text: "First")
    let second = ChatMessage(role: .assistant, timestamp: "now", text: "Second")
    var starts = engine.starts.stream.makeAsyncIterator()
    let firstTask = Task { await store.speak(first) }
    _ = await starts.next()
    store.stopSpeaking()
    XCTAssertNil(store.speakingMessageID)
    XCTAssertEqual(engine.stopCount, 1)
    let secondTask = Task { await store.speak(second) }
    _ = await starts.next()
    engine.pending[0].resume(throwing: ReadFailure.unavailable)
    await firstTask.value
    XCTAssertEqual(store.speakingMessageID, second.id)
    XCTAssertNil(store.speechError)
    engine.pending[1].resume()
    await secondTask.value
    XCTAssertNil(store.speakingMessageID)
    XCTAssertNil(store.speechError)
  }
}
