import AppKit
import XCTest

@testable import Codescribe

@MainActor
final class OverlayIntercomHistoryRestoreTests: XCTestCase {
  @MainActor
  private final class Fixture {
    let suite = "CodescribeTests.history-restore.\(UUID().uuidString)"
    let defaults: UserDefaults
    let state = OverlayState()
    let store: OverlayIntercomDesktopStore
    let owner: OverlayConversationOwner
    let conversation: OverlayConversation
    var controller: OverlayIntercomWidgetController
    let instanceID: UUID

    init() throws {
      defaults = try XCTUnwrap(UserDefaults(suiteName: suite))
      store = OverlayIntercomDesktopStore(defaults: defaults)
      let session = "history-restore-fixture"
      owner = try XCTUnwrap(OverlayConversationOwner(row: [
        "provider": "codex", "provider_session_id": session,
        "lease_id": AgentPlaybackIdentity.leaseIdentifier(provider: "codex", session: session),
        "channel": "2", "name": "Retained owner",
      ]))
      conversation = .init(id: owner.id, channel: "2", name: owner.name, owner: owner, messages: [])
      state.applyConversationSnapshot(.init(deliveries: [], conversations: [conversation]))
      state.applyChannelRoster([
        .init(channel: "2", audience: owner.name, provider: owner.provider,
          providerSessionId: owner.providerSessionID, open: false, loud: false,
          autosealDeadlineUnixMs: nil, followerAlive: false, unreadSinceUnixMs: nil)
      ])
      state.archiveAgentCommand = { _, _ in }
      state.publishConversationText = { _, _, _, _ in XCTFail("Window restoration must not send") }
      state.conversationDrafts[conversation.id] = "History draft stays with its exact owner"
      let identity = OverlayIntercomCardIdentity(digit: "2", conversation: conversation)
      let instance = OverlayIntercomInstance(
        identity: identity, frame: CGRect(x: 120, y: 180, width: 380, height: 320))
      instanceID = instance.id
      var desktop = OverlayIntercomDesktopState()
      desktop.upsert(instance)
      store.save(desktop)
      controller = OverlayIntercomWidgetController(
        state: state, textScale: TextScaleController(key: suite), desktopStore: store)
      controller.setVisible(true, relativeTo: nil)
    }

    func archive() async throws {
      await state.archiveAgent(owner)
      for _ in 0..<100 {
        if store.load().instances.first?.closed == true { break }
        try await Task.sleep(for: .milliseconds(20))
      }
      XCTAssertTrue(state.archivedAgentOwners.contains(owner))
      XCTAssertTrue(try record().closed)
    }

    func record() throws -> OverlayIntercomInstance {
      try XCTUnwrap(store.load().instances.first { $0.id == instanceID })
    }

    func checkHistoryIsOpen() throws {
      let saved = try record()
      XCTAssertFalse(saved.closed)
      XCTAssertEqual(saved.ownerID, owner.id)
      XCTAssertTrue(controller.presentsConversation(conversation.id))
      XCTAssertTrue(state.visibleChannelRows.isEmpty)
      XCTAssertFalse(state.canToggleConversationMicrophone(conversation))
      XCTAssertEqual(state.conversationDrafts[conversation.id], "History draft stays with its exact owner")
      XCTAssertTrue(state.pendingTextMessages.isEmpty)
    }

    func cleanup() {
      controller.invalidate()
      defaults.removePersistentDomain(forName: suite)
    }
  }

  func testHistoryOpenedBeforeDockReturnsWithTheSavedLayout() async throws {
    let f = try Fixture()
    defer { f.cleanup() }
    try await f.archive()
    f.controller.reopenInstance(f.instanceID)
    try f.checkHistoryIsOpen()
    let before = try f.record()
    f.controller.dockAll()
    XCTAssertTrue(try f.record().closed)
    f.controller.restoreDockedLayout()
    try f.checkHistoryIsOpen()
    XCTAssertEqual(try f.record().frame, before.frame)
  }

  func testLaterArchiveCannotBeUndoneByRestoringAnOldLiveLayout() async throws {
    let f = try Fixture()
    defer { f.cleanup() }
    f.controller.dockAll()
    try await f.archive()
    f.controller.restoreDockedLayout()
    XCTAssertTrue(try f.record().closed)
    XCTAssertFalse(f.controller.presentsConversation(f.conversation.id))
    XCTAssertTrue(f.state.visibleChannelRows.isEmpty)
  }

  func testExplicitHistoryReopenOfTheSameInstanceSurvivesRestore() async throws {
    let f = try Fixture()
    defer { f.cleanup() }
    f.controller.dockAll()
    try await f.archive()
    f.controller.reopenInstance(f.instanceID)
    try f.checkHistoryIsOpen()
    f.controller.restoreDockedLayout()
    try f.checkHistoryIsOpen()
    XCTAssertEqual(try f.record().id, f.instanceID)
  }

  func testRestoreRespectsAWindowThatWasClosedInTheSavedLayout() async throws {
    let f = try Fixture()
    defer { f.cleanup() }
    try await f.archive()
    f.controller.dockAll()
    f.controller.reopenInstance(f.instanceID)
    try f.checkHistoryIsOpen()
    f.controller.restoreDockedLayout()
    XCTAssertTrue(try f.record().closed, "Restore reinstates the deliberately saved closed layout")
  }

  func testExplicitHistoryOpeningSurvivesANewController() async throws {
    let f = try Fixture()
    defer { f.cleanup() }
    try await f.archive()
    f.controller.reopenInstance(f.instanceID)
    try f.checkHistoryIsOpen()
    f.controller.invalidate()
    f.controller = OverlayIntercomWidgetController(
      state: f.state, textScale: TextScaleController(key: f.suite), desktopStore: f.store)
    f.controller.setVisible(true, relativeTo: nil)
    try await Task.sleep(for: .milliseconds(80))
    try f.checkHistoryIsOpen()
  }
}
