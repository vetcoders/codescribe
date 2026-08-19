import AppKit
import Combine
import Foundation
import XCTest

@testable import Codescribe

/// Mechanism proof for the thread-rail live refresh (wave S, cut C).
///
/// A thread persisted OUTSIDE the store (assistive/overlay turn) must appear
/// in the rail when — and only when — a refresh trigger fires: window
/// activation (`NSWindow.didBecomeKeyNotification`) or the cross-surface
/// `ThreadsChangeBus.threadsDidChange`. Without a trigger the rail must NOT
/// pick it up; that asymmetry is the proof this is an event-driven mechanism,
/// not polling and not an accident of some unrelated refresh.
@MainActor
final class ThreadsRefreshTests: XCTestCase {
  private final class StubThreadsProvider: ChatThreadsProviding {
    /// (backendId, title) rows returned newest-first, mirroring the
    /// ThreadStore index order ("index top" = most recently updated).
    var stubbed: [(id: String, title: String)]
    private(set) var listCalls = 0

    init(_ stubbed: [(id: String, title: String)]) {
      self.stubbed = stubbed
    }

    func listThreads() -> [ChatThread] {
      listCalls += 1
      return stubbed.map { entry in
        var thread = ChatThread(title: entry.title, meta: "now")
        thread.backendId = entry.id
        thread.messagesLoaded = true
        thread.updatedAt = Date()
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

  private func backendIds(_ store: AgentChatStore) -> [String] {
    store.threads.compactMap(\.backendId)
  }

  /// Refresh triggers coalesce onto the next main-queue tick (so a popover
  /// close / window-ordering storm never runs disk re-reads inside the
  /// notification callout — sample 2026-08-07 10:43, main thread 93/93 in
  /// `_NSPopoverCloseAndAnimate → refreshThreadsFromExternalChange`).
  /// Tests drain that tick before asserting.
  private func drainMainQueue() {
    let drained = expectation(description: "main queue drained")
    DispatchQueue.main.async { drained.fulfill() }
    wait(for: [drained], timeout: 2)
  }

  // MARK: C1 — refresh on window activation, and ONLY on a trigger

  func testWindowActivationRefreshesRailAndNoTriggerDoesNot() {
    let provider = StubThreadsProvider([("t_old", "Old thread")])
    let store = AgentChatStore(threadsProvider: provider)
    XCTAssertEqual(backendIds(store), ["t_old"])

    // A turn outside this store persisted a new thread on top of the index.
    provider.stubbed.insert(("t_overlay", "Overlay reply"), at: 0)

    // No trigger → the rail must NOT see it (event-driven, not polling).
    XCTAssertFalse(
      backendIds(store).contains("t_overlay"),
      "rail refreshed without a trigger — mechanism is not event-driven"
    )

    // Window activation → the rail re-reads disk truth on the next
    // main-queue tick (coalesced out of the notification callout).
    NotificationCenter.default.post(name: NSWindow.didBecomeKeyNotification, object: nil)
    drainMainQueue()
    XCTAssertTrue(
      backendIds(store).contains("t_overlay"),
      "window activation must reload the persisted thread list"
    )
  }

  func testThreadsDidChangeBusRefreshesRail() {
    let provider = StubThreadsProvider([("t_old", "Old thread")])
    let store = AgentChatStore(threadsProvider: provider)

    provider.stubbed.insert(("t_saved_elsewhere", "Saved elsewhere"), at: 0)
    XCTAssertFalse(backendIds(store).contains("t_saved_elsewhere"))

    ThreadsChangeBus.postThreadsChanged()
    drainMainQueue()
    XCTAssertTrue(
      backendIds(store).contains("t_saved_elsewhere"),
      "threadsDidChange must reload the persisted thread list"
    )
  }

  // MARK: C1 guard — never yank the rail mid-turn

  func testActivationRefreshIsDeferredWhileVoiceTurnIsActive() {
    let provider = StubThreadsProvider([("t_old", "Old thread")])
    let store = AgentChatStore(threadsProvider: provider)

    // A live voice turn owns a freshly bound thread that is not on disk
    // yet — an activation refresh here would drop it from the rail.
    store.ingestVoiceTurn(threadId: "t_voice", userText: "hello")
    provider.stubbed.insert(("t_other", "Other"), at: 0)

    NotificationCenter.default.post(name: NSWindow.didBecomeKeyNotification, object: nil)
    drainMainQueue()
    XCTAssertFalse(
      backendIds(store).contains("t_other"),
      "activation refresh must be a no-op while a turn is in flight"
    )
    XCTAssertTrue(
      backendIds(store).contains("t_voice"),
      "the in-flight voice thread must survive an activation mid-turn"
    )
  }

  // MARK: C2 — in-window turn completion still refreshes (regression)

  func testVoiceTurnCompletionRefreshesRailWithoutStealingSelection() {
    let provider = StubThreadsProvider([("t_old", "Old thread")])
    let store = AgentChatStore(threadsProvider: provider)

    store.ingestVoiceTurn(threadId: "t_voice", userText: "hello")
    // Core persisted the voice thread AND another surface saved one more
    // in the meantime; the turn terminal must pull both from disk.
    provider.stubbed = [
      ("t_voice", "Voice turn"),
      ("t_saved_meanwhile", "Saved meanwhile"),
      ("t_old", "Old thread"),
    ]

    store.ingestVoiceDone()
    XCTAssertTrue(
      backendIds(store).contains("t_saved_meanwhile"),
      "turn completion must reload the persisted thread list"
    )
    XCTAssertEqual(
      store.currentThread?.backendId, "t_old",
      "turn completion must preserve the thread the user was already reading"
    )
  }

  // MARK: C4 — after activation the newest thread sits on top of the rail

  func testActivationRefreshPutsNewestThreadOnTopWithoutStealingSelection() {
    let provider = StubThreadsProvider([("t_old", "Old thread")])
    let store = AgentChatStore(threadsProvider: provider)
    XCTAssertEqual(store.currentThread?.backendId, "t_old")

    provider.stubbed.insert(("t_fresh", "Fresh overlay reply"), at: 0)
    NotificationCenter.default.post(name: NSWindow.didBecomeKeyNotification, object: nil)
    drainMainQueue()

    XCTAssertEqual(
      backendIds(store).first, "t_fresh",
      "the newest persisted thread must be on top of the rail"
    )
    XCTAssertEqual(
      store.currentThread?.backendId, "t_old",
      "an activation refresh must not steal the user's selection"
    )
  }

  // MARK: C5 — an activation storm collapses to ONE disk re-read
  //
  // Every window in the app posts `didBecomeKey` (observer has object: nil),
  // and AppKit fires it from INSIDE window-ordering operations (popover
  // close → orderOut → becomeKeyWindow). Sample 2026-08-07 10:43: main
  // thread pinned 93/93 in that callout chain. The storm must coalesce.

  func testActivationStormCoalescesIntoSingleRefresh() {
    let provider = StubThreadsProvider([("t_old", "Old thread")])
    let store = AgentChatStore(threadsProvider: provider)
    let baseline = provider.listCalls

    for _ in 0..<10 {
      NotificationCenter.default.post(name: NSWindow.didBecomeKeyNotification, object: nil)
    }
    drainMainQueue()

    XCTAssertEqual(
      provider.listCalls - baseline, 1,
      "10 activation signals in one tick must collapse to one disk re-read"
    )
    XCTAssertEqual(backendIds(store), ["t_old"])
  }

  // MARK: C6 — identical disk truth must not rebuild the window body
  //
  // The 937189fd lesson: publishing an unchanged value still tears down and
  // rebuilds the whole SwiftUI body. A refresh that finds the same rail
  // rows must not publish at all.

  func testUnchangedRefreshDoesNotPublish() {
    let provider = StubThreadsProvider([("t_old", "Old thread")])
    let store = AgentChatStore(threadsProvider: provider)
    drainMainQueue()

    var publishes = 0
    let subscription = store.objectWillChange.sink { _ in publishes += 1 }
    defer { subscription.cancel() }

    NotificationCenter.default.post(name: NSWindow.didBecomeKeyNotification, object: nil)
    drainMainQueue()

    XCTAssertEqual(
      publishes, 0,
      "a refresh that re-reads identical rail rows must not publish objectWillChange"
    )
  }

  // MARK: C3 — code-shape: the mechanism is event-driven, no timers

  func testRefreshMechanismContainsNoTimers() throws {
    let macosDir = URL(fileURLWithPath: #filePath)
      .deletingLastPathComponent()  // CodescribeTests/
      .deletingLastPathComponent()  // macos/
    let sources = [
      "Codescribe/Core/ThreadsChangeBus.swift",
      "Codescribe/Screens/AgentChat/AgentChatStore.swift",
      "Codescribe/Screens/AgentChat/ThreadRail.swift",
    ]
    let banned = [
      "Timer(",
      "Timer.publish",
      "scheduledTimer",
      "DispatchSourceTimer",
      "makeTimerSource",
    ]
    for relative in sources {
      let path = macosDir.appendingPathComponent(relative)
      let text = try String(contentsOf: path, encoding: .utf8)
      for token in banned {
        XCTAssertFalse(
          text.contains(token),
          "\(relative) must stay event-driven — found `\(token)`"
        )
      }
    }
  }
}
