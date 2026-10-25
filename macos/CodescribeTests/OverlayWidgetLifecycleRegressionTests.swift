import AppKit
import SwiftUI
import XCTest

@testable import Codescribe

/// Cross-generation witnesses: this file uses APIs available on the baseline
/// and candidate, so a baseline failure is behavioral rather than an API error.
@MainActor
final class OverlayWidgetLifecycleRegressionTests: XCTestCase {
  private func owner() throws -> OverlayConversationOwner {
    let session = "widget-lifecycle-fixture"
    return try XCTUnwrap(OverlayConversationOwner(row: [
      "provider": "codex", "provider_session_id": session,
      "lease_id": AgentPlaybackIdentity.leaseIdentifier(provider: "codex", session: session),
      "channel": "2", "name": "Fixture agent",
    ]))
  }

  func testCanonicalArchiveRetiresDetachedWindowAndPreservesSavedConversation() async throws {
    let suite = "CodescribeTests.widget-archive.\(UUID().uuidString)"
    let defaults = try XCTUnwrap(UserDefaults(suiteName: suite))
    defer { defaults.removePersistentDomain(forName: suite) }
    let state = OverlayState()
    let owner = try owner()
    let conversation = OverlayConversation(
      id: owner.id, channel: owner.channel, name: owner.name, owner: owner, messages: [])
    state.applyConversationSnapshot(.init(deliveries: [], conversations: [conversation]))
    state.applyChannelRoster([
      .init(channel: owner.channel, audience: owner.name, provider: owner.provider,
        providerSessionId: owner.providerSessionID, open: false, loud: false,
        autosealDeadlineUnixMs: nil, followerAlive: false, unreadSinceUnixMs: nil)
    ])
    let store = OverlayIntercomDesktopStore(defaults: defaults)
    let controller = OverlayIntercomWidgetController(
      state: state, textScale: TextScaleController(key: suite), desktopStore: store)
    defer { controller.invalidate() }
    state.publishConversationText = { _, _, _ in XCTFail("Presentation must not send") }
    var archiveCalls = 0
    state.archiveAgentCommand = { actual, release in
      XCTAssertEqual(actual, owner)
      XCTAssertFalse(release)
      archiveCalls += 1
    }
    state.conversationDrafts[conversation.id] = "Retained exact-owner draft"
    let generation = state.captureGeneration
    controller.setVisible(true, relativeTo: nil)
    XCTAssertTrue(controller.detach(.init(digit: owner.channel, conversation: conversation)))
    let saved = try XCTUnwrap(store.load().instances.first)
    XCTAssertTrue(controller.presentsConversation(conversation.id))

    await state.archiveAgent(owner)
    for _ in 0..<100 {
      if store.load().instances.first?.closed == true { break }
      try await Task.sleep(for: .milliseconds(20))
    }
    let archived = try XCTUnwrap(store.load().instances.first)
    XCTAssertTrue(archived.closed, "Canonical archive must retire its detached native window")
    XCTAssertFalse(controller.presentsConversation(conversation.id))
    XCTAssertEqual(archived.id, saved.id)
    XCTAssertEqual(archived.ownerID, saved.ownerID)
    XCTAssertEqual(archived.frame, saved.frame)
    XCTAssertEqual(state.conversationDrafts[conversation.id], "Retained exact-owner draft")
    XCTAssertEqual(state.conversations.map(\.id), [conversation.id])
    XCTAssertEqual(state.captureGeneration, generation)
    XCTAssertEqual(archiveCalls, 1)
    XCTAssertTrue(state.pendingTextMessages.isEmpty)
  }

  func testInactiveCurrentOwnerCanDockWithoutSending() throws {
    let suite = "CodescribeTests.widget-inactive.\(UUID().uuidString)"
    let defaults = try XCTUnwrap(UserDefaults(suiteName: suite))
    defer { defaults.removePersistentDomain(forName: suite) }
    let state = OverlayState()
    let owner = try owner()
    let conversation = OverlayConversation(
      id: owner.id, channel: owner.channel, name: owner.name, owner: owner, messages: [])
    state.applyConversationSnapshot(.init(deliveries: [], conversations: [conversation]))
    state.applyChannelRoster([
      .init(channel: owner.channel, audience: owner.name, provider: owner.provider,
        providerSessionId: owner.providerSessionID, open: false, loud: false,
        autosealDeadlineUnixMs: nil, followerAlive: false, unreadSinceUnixMs: nil)
    ])
    let store = OverlayIntercomDesktopStore(defaults: defaults)
    let controller = OverlayIntercomWidgetController(
      state: state, textScale: TextScaleController(key: suite), desktopStore: store)
    defer { controller.invalidate() }
    state.publishConversationText = { _, _, _ in XCTFail("Docking must not send") }
    XCTAssertTrue(controller.detach(.init(digit: owner.channel, conversation: conversation)))
    let instance = try XCTUnwrap(store.load().instances.first)
    controller.closeInstance(instance.id)
    XCTAssertEqual(store.load().instances.count, 1)
    XCTAssertTrue(try XCTUnwrap(store.load().instances.first).closed)
    XCTAssertEqual(state.visibleChannelRows.map(\.channel), [owner.channel])
    XCTAssertTrue(state.pendingTextMessages.isEmpty)
  }

  func testCompactHintUsesFreeSideWithoutCoveringItsAnchor() {
    for screen in [CGRect(x: 0, y: 0, width: 1440, height: 900),
      CGRect(x: -1920, y: 100, width: 1920, height: 1080)] {
      for y in [screen.minY + 10, screen.midY, screen.maxY - 40] {
        let anchor = CGRect(x: screen.minX + 10, y: y, width: 150, height: 30)
        let frame = OverlayHintPanel.frame(
          size: CGSize(width: 360, height: 240), leading: anchor, within: screen)
        XCTAssertGreaterThanOrEqual(frame.minX, anchor.maxX, "Use the free side at the left edge")
        XCTAssertFalse(frame.intersects(anchor), "Preview must leave its control reachable")
        XCTAssertTrue(screen.contains(frame))
      }
    }
  }

  func testIncreasingHeightNeverMovesActionsBelowTheAgentName() {
    for scale in [CGFloat(1), 1.5, 2] {
      let metrics = IntercomContentMetrics(names: ["Agent"], digits: ["2"], scale: scale)
      let width = max(metrics.minimumNameWidth, metrics.actionsWidth - 1)
      let expected = metrics.tier(width: width, cardHeight: metrics.iconSide)
      for height in [2 * metrics.iconSide, 8 * metrics.iconSide, 1800] {
        XCTAssertEqual(metrics.tier(width: width, cardHeight: height), expected)
      }
    }
  }
}
