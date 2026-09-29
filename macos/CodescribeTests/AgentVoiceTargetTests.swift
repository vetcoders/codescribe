import Foundation
import XCTest

@testable import Codescribe

@MainActor
final class AgentVoiceTargetTests: XCTestCase {
  private final class RoutingEngine: AgentChatEngine {
    var targets: [String?] = []
    var sentThreadIDs: [String] = []

    func isAvailable() -> Bool { true }
    func availabilityDetail() -> String? { nil }
    func generateThreadTitle(_ text: String) async throws -> String? { nil }
    func cancelReply(threadId: String) -> Bool { false }
    func setAssistiveTargetThread(backendId: String?) { targets.append(backendId) }
    func streamReply(
      _ text: String,
      threadId: String,
      attachmentPaths: [String],
      onDelta: @escaping @MainActor (String) -> Void,
      onReasoning: @escaping @MainActor (String) -> Void,
      onToolExecuting: @escaping @MainActor (String, String) -> Void,
      onToolResult: @escaping @MainActor (String, String, Bool, String) -> Void
    ) async throws -> String {
      sentThreadIDs.append(threadId)
      return "Composer reply"
    }
  }

  private final class ThreadsProvider: ChatThreadsProviding {
    var rows: [ChatThread]
    var loadedIDs: [String] = []

    init(_ rows: [ChatThread]) { self.rows = rows }
    func listThreads() -> [ChatThread] { rows }
    func searchThreads(query: String) -> [ChatThread] { rows }
    func loadMessages(backendId: String) -> [ChatMessage] {
      loadedIDs.append(backendId)
      return [ChatMessage(role: .assistant, timestamp: "earlier", text: "Stored conversation")]
    }
    func deleteThread(backendId: String) -> Bool {
      rows.removeAll { $0.backendId == backendId }
      return true
    }
    func setThreadFavorite(backendId: String, isFavorite: Bool) -> Bool { true }
    func renameThread(backendId: String, title: String) -> Bool { true }
    func setGeneratedTitle(backendId: String, title: String) -> Bool { true }
    func exportThreadMarkdown(backendId: String, assistantOnly: Bool) -> String? { nil }
    func generateThreadId() -> String { "t_at1_" + UUID().uuidString }
  }

  private func row(
    _ id: String, updated: TimeInterval = 1, mode: String = "agent", tags: [String] = []
  ) -> ChatThread {
    ChatThread(
      title: id, meta: "now", backendId: id,
      updatedAt: Date(timeIntervalSince1970: updated), mode: mode, tags: tags)
  }

  private func fixture(_ rows: [ChatThread]) -> (
    store: AgentChatStore, engine: RoutingEngine, provider: ThreadsProvider
  ) {
    let engine = RoutingEngine()
    let provider = ThreadsProvider(rows)
    let suite = "AgentVoiceTargetTests." + UUID().uuidString
    let defaults = UserDefaults(suiteName: suite)!
    addTeardownBlock { UserDefaults(suiteName: suite)?.removePersistentDomain(forName: suite) }
    let store = AgentChatStore(
      engine: engine, threadsProvider: provider, persistenceDefaults: defaults)
    return (store, engine, provider)
  }

  func testSummaryMappingPreservesBothIndependentMaxMarkers() {
    for (mode, tags) in [("max", ["agent"]), ("agent", ["agent", "max-consultation"])] {
      let summary = CsThreadSummary(
        id: "t_neutral", title: "Consultation", createdAtMs: 1, updatedAtMs: 2,
        messageCount: 0, mode: mode, tags: tags, summary: nil, hasNotes: false,
        latestMessage: nil, latestNote: nil, isFavorite: false, model: "test", totalTokens: nil)
      let mapped = RealThreadsEngine.thread(from: summary)
      XCTAssertEqual(mapped.mode, mode)
      XCTAssertEqual(mapped.tags, tags)
      XCTAssertTrue(mapped.isMaxConsultation)
    }
  }

  func testSelectingMaxShowsItsMessagesWithoutPublishingATarget() throws {
    for consultation in [row("t_mode", mode: "max"), row("t_tag", tags: ["max-consultation"])] {
      let agent = row("t_agent")
      let f = fixture([agent, consultation])
      defer { f.store.invalidate() }
      let before = f.engine.targets

      f.store.select(consultation.id)

      XCTAssertEqual(f.store.currentThread?.backendId, consultation.backendId)
      XCTAssertEqual(f.store.currentThread?.messages.last?.text, "Stored conversation")
      XCTAssertEqual(f.engine.targets, before, "browsing must publish nothing, including nil")
      f.store.endDictationSession()
      XCTAssertEqual(
        f.engine.targets, before, "terminal resync must also preserve the Agent target")
    }
  }

  func testCaptureFromMaxDoesNotPublishAMaxTarget() {
    let consultation = row("t_max", mode: "max")
    let f = fixture([row("t_agent"), consultation])
    defer { f.store.invalidate() }
    f.store.select(consultation.id)
    let before = f.engine.targets

    _ = f.store.beginComposerCaptureRequest(threadID: consultation.id)
    f.store.endDictationSession()

    XCTAssertEqual(f.engine.targets, before)
  }

  func testSelectingAgentAfterBrowsingMaxPublishesThatAgent() {
    let next = row("t_next", updated: 2)
    let consultation = row("t_max", updated: 4, mode: "max")
    let f = fixture([row("t_first", updated: 3), consultation, next])
    defer { f.store.invalidate() }
    f.store.select(consultation.id)
    f.store.select(next.id)
    XCTAssertEqual(f.engine.targets.last!, "t_next")
  }

  func testEagerSeedPrefersMostRecentlyUpdatedAgentAndLoadsItsMessages() {
    let f = fixture([
      row("t_max", updated: 10, mode: "max"),
      row("t_old", updated: 1), row("t_recent", updated: 5),
    ])
    defer { f.store.invalidate() }
    XCTAssertEqual(f.store.currentThread?.backendId, "t_recent")
    XCTAssertEqual(f.engine.targets, ["t_recent"])
    XCTAssertEqual(f.provider.loadedIDs, ["t_recent"])
  }

  func testOnlyMaxAtSeedCreatesANewThreadShell() {
    let f = fixture([row("t_max", mode: "max")])
    defer { f.store.invalidate() }
    XCTAssertEqual(f.store.currentThread?.title, "New thread")
    XCTAssertNil(f.store.currentThread?.backendId)
    XCTAssertEqual(f.engine.targets.count, 1)
    XCTAssertNil(f.engine.targets.last!)
    XCTAssertEqual(f.store.threads.filter(\.isMaxConsultation).count, 1)
  }

  func testExplicitSeedPublishesOnceAndLaterSelectionStillPublishes() {
    let recent = row("t_recent", updated: 5)
    let next = row("t_next", updated: 1)
    let engine = RoutingEngine()
    let store = AgentChatStore(
      engine: engine, threads: [row("t_max", updated: 10, mode: "max"), next, recent])
    defer { store.invalidate() }

    XCTAssertEqual(store.selectedThreadID, recent.id)
    XCTAssertEqual(engine.targets, ["t_recent"])

    store.select(next.id)

    XCTAssertEqual(engine.targets, ["t_recent", "t_next"])
  }

  func testDeletingSelectedAgentSkipsMaxAtTopOfIndex() throws {
    let selected = row("t_selected", updated: 8)
    let f = fixture([
      row("t_max", updated: 10, mode: "max"), selected,
      row("t_old", updated: 1), row("t_recent", updated: 5),
    ])
    defer { f.store.invalidate() }
    f.store.delete(try XCTUnwrap(f.store.currentThread))
    XCTAssertEqual(f.store.currentThread?.backendId, "t_recent")
    XCTAssertEqual(f.engine.targets.last!, "t_recent")
  }

  func testRefreshWithRemovedSelectionPrefersAgentOrNewShell() {
    for remaining in [[row("t_recent", updated: 5), row("t_old", updated: 1)], []] {
      let selected = row("t_removed", updated: 8)
      let f = fixture([selected])
      defer { f.store.invalidate() }
      // The logical row has already been removed before index reconciliation.
      f.store.threads.removeAll { $0.id == selected.id }
      f.provider.rows = [row("t_max", updated: 10, mode: "max")] + remaining
      f.store.refreshThreads()
      XCTAssertFalse(f.store.currentThread?.isMaxConsultation ?? true)
      if remaining.isEmpty {
        XCTAssertEqual(f.store.currentThread?.title, "New thread")
        XCTAssertNil(f.engine.targets.last!)
      } else {
        XCTAssertEqual(f.store.currentThread?.backendId, "t_recent")
        XCTAssertEqual(f.engine.targets.last!, "t_recent")
      }
    }
  }

  func testDeletingLastAgentLeavesMaxVisibleAndSelectsNewShell() throws {
    let f = fixture([row("t_max", updated: 10, mode: "max"), row("t_agent")])
    defer { f.store.invalidate() }
    f.store.delete(try XCTUnwrap(f.store.currentThread))
    XCTAssertEqual(f.store.currentThread?.title, "New thread")
    XCTAssertNil(f.engine.targets.last!)
    XCTAssertEqual(f.store.threads.filter(\.isMaxConsultation).count, 1)
  }

  func testRefreshCarriesClassificationWithoutRepublishingSelectedMax() throws {
    let f = fixture([row("t_selected")])
    defer { f.store.invalidate() }
    let selected = f.store.selectedThreadID
    let before = f.engine.targets
    f.provider.rows[0].tags = ["max-consultation"]
    f.store.refreshThreads()
    XCTAssertEqual(f.store.selectedThreadID, selected, "explicit selection remains readable")
    XCTAssertTrue(try XCTUnwrap(f.store.currentThread).isMaxConsultation)
    XCTAssertEqual(f.engine.targets, before)
  }

  func testMaxComposerStillSendsToSelectedConsultation() async {
    let consultation = row("t_max_" + UUID().uuidString, updated: 10, mode: "max")
    let f = fixture([row("t_agent"), consultation])
    defer { f.store.invalidate() }
    f.store.select(consultation.id)
    let before = f.engine.targets
    f.store.draft = "Continue this consultation"
    f.store.send()
    await f.store.waitForComposerTurns(in: consultation.id)
    XCTAssertEqual(f.engine.sentThreadIDs, [consultation.backendId!])
    XCTAssertEqual(f.store.currentThread?.messages.last?.text, "Composer reply")
    XCTAssertEqual(f.engine.targets, before)
  }

  func testRailGroupsEveryMaxAfterAllAgentRecencySections() {
    let now = Date(timeIntervalSince1970: 1_000_000)
    let newestMax = row("t_mode", updated: now.timeIntervalSince1970, mode: "max")
    let olderAgent = row("t_old", updated: 1)
    let taggedMax = row("t_tag", updated: 2, tags: ["max-consultation"])
    let todayAgent = row("t_today", updated: now.timeIntervalSince1970)
    let groups = ThreadSection.railGroups(
      [newestMax, todayAgent, taggedMax, olderAgent], now: now)
    XCTAssertEqual(groups.map(\.section), [.today, .older, .maxConsultations])
    XCTAssertEqual(groups.last?.section.title, "Max consultations")
    XCTAssertEqual(groups.last?.threads.map(\.id), [newestMax.id, taggedMax.id])
    XCTAssertTrue(groups.dropLast().flatMap(\.threads).allSatisfy { !$0.isMaxConsultation })
  }
}
