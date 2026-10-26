import AppKit
import XCTest

@testable import Codescribe

/// Native-window witnesses of the placement controller. Synthetic drop points
/// verify its lifecycle; they do not certify physical pointer tracking by AppKit.
@MainActor
final class OverlayIntercomDragAcceptanceTests: XCTestCase {
  @MainActor
  private final class Rig {
    let suite = "CodescribeTests.intercom-drag.\(UUID().uuidString)"
    let defaults: UserDefaults
    let state = OverlayState()
    let store: OverlayIntercomDesktopStore
    let controller: OverlayIntercomWidgetController
    let conversation: OverlayConversation
    let identity: OverlayIntercomCardIdentity
    let header: NSWindow
    let card: NSWindow
    let generation: UInt64

    init(secondRow: Bool = false, expanded: Bool = false) throws {
      defaults = try XCTUnwrap(UserDefaults(suiteName: suite))
      store = OverlayIntercomDesktopStore(defaults: defaults)
      conversation = try Self.conversation(session: "first", channel: "3")
      identity = .init(digit: "3", conversation: conversation)
      var conversations = [conversation]
      if secondRow { conversations.append(try Self.conversation(session: "second", channel: "4")) }
      Self.apply(conversations, to: state)
      state.publishConversationText = { _, _, _ in XCTFail("Placement must not publish") }
      state.conversationDrafts[conversation.id] = "Keep the exact-owner draft"
      generation = state.captureGeneration
      store.save(.init(groupWidth: 500, groupRowHeight: 80))
      controller = OverlayIntercomWidgetController(
        state: state, textScale: TextScaleController(key: suite), desktopStore: store)
      if expanded {
        _ = controller.offscreenCard(identity, width: 500, height: 80, scale: 1, expanded: true)
      }
      let before = Set(NSApp.windows.map(ObjectIdentifier.init))
      controller.setVisible(true, relativeTo: nil)
      let windows = NSApp.windows.filter {
        !before.contains(ObjectIdentifier($0)) && $0.isVisible
      }.sorted { $0.frame.maxY > $1.frame.maxY }
      XCTAssertEqual(windows.count, secondRow ? 3 : 2)
      header = try XCTUnwrap(windows.first)
      card = try XCTUnwrap(windows.dropFirst().first)
    }

    static func conversation(session: String, channel: String) throws -> OverlayConversation {
      let owner = try XCTUnwrap(
        OverlayConversationOwner(row: [
          "provider": "codex", "provider_session_id": session,
          "lease_id": AgentPlaybackIdentity.leaseIdentifier(provider: "codex", session: session),
          "channel": channel, "name": session,
        ]))
      return .init(id: owner.id, channel: channel, name: owner.name, owner: owner, messages: [])
    }

    static func apply(_ conversations: [OverlayConversation], to state: OverlayState) {
      state.applyConversationSnapshot(.init(deliveries: [], conversations: conversations))
      state.applyChannelRoster(
        conversations.map {
          .init(
            channel: $0.channel, audience: $0.name, provider: $0.owner?.provider,
            providerSessionId: $0.owner?.providerSessionID, open: false, loud: false,
            autosealDeadlineUnixMs: nil, followerAlive: true, unreadSinceUnixMs: nil)
        })
    }

    var headerPoint: CGPoint { CGPoint(x: header.frame.midX, y: header.frame.midY) }
    var instance: OverlayIntercomInstance? { store.load().instances.first }

    func verifyTruth(file: StaticString = #filePath, line: UInt = #line) {
      XCTAssertEqual(state.captureGeneration, generation, file: file, line: line)
      XCTAssertEqual(
        state.conversationDrafts[conversation.id], "Keep the exact-owner draft",
        file: file, line: line)
      XCTAssertTrue(state.pendingTextMessages.isEmpty, file: file, line: line)
    }

    func cleanup() {
      controller.invalidate()
      defaults.removePersistentDomain(forName: suite)
    }
  }

  func testCompactAndExpandedDetachPreserveNativeSurfaceUntilRelease() throws {
    for expanded in [false, true] {
      let rig = try Rig(expanded: expanded)
      defer { rig.cleanup() }
      let root = try XCTUnwrap(rig.card.contentView)
      let frame = rig.card.frame
      XCTAssertTrue(rig.controller.detachForDrag(rig.identity))
      XCTAssertTrue(rig.card.isVisible)
      XCTAssertTrue(rig.card.contentView === root)
      XCTAssertEqual(rig.card.frame, frame)
      XCTAssertEqual(rig.controller.desktop.groupRows(of: [rig.identity]), [])
      XCTAssertFalse(rig.controller.detachForDrag(rig.identity), "One active native drag")
      let moved = frame.offsetBy(dx: -100, dy: -100)
      rig.card.setFrame(moved, display: false)
      rig.controller.windowDidMove(
        Notification(name: NSWindow.didMoveNotification, object: rig.card))
      XCTAssertEqual(rig.card.frame, moved, "Reconciliation cannot reset tracking geometry")
      rig.controller.finishInstanceDrag(at: CGPoint(x: -10000, y: -10000))
      XCTAssertFalse(try XCTUnwrap(rig.instance).closed)
      XCTAssertEqual(rig.controller.desktop.instances.filter { !$0.closed }.count, 1)
      XCTAssertEqual(rig.card.frame.maxY, moved.maxY, accuracy: 1)
      XCTAssertFalse(rig.card.contentView === root, "Mount conversation after tracking ends")
      rig.controller.finishInstanceDrag(at: rig.headerPoint)
      XCTAssertFalse(try XCTUnwrap(rig.instance).closed, "Late completion is idempotent")
      rig.verifyTruth()
    }
  }

  func testLastRowDocksOverVisibleHeaderIncludingFoldedGroup() throws {
    for folded in [false, true] {
      let rig = try Rig()
      defer { rig.cleanup() }
      XCTAssertTrue(rig.controller.detachForDrag(rig.identity))
      XCTAssertEqual(rig.controller.desktop.groupRows(of: [rig.identity]), [])
      if folded { rig.controller.toggleGroup() }
      XCTAssertTrue(rig.header.isVisible, "The header survives removal of its final row")
      rig.controller.finishInstanceDrag(at: rig.headerPoint)
      XCTAssertTrue(try XCTUnwrap(rig.instance).closed, "Visible header is a docking target")
      XCTAssertEqual(rig.controller.desktop.groupRows(of: [rig.identity]), [rig.identity])
      XCTAssertEqual(rig.controller.presentation.folded, folded)
      rig.verifyTruth()
    }
  }

  func testExistingDetachedWindowDocksOnlyOnRelease() throws {
    let rig = try Rig(secondRow: true)
    defer { rig.cleanup() }
    XCTAssertTrue(rig.controller.detachForDrag(rig.identity))
    rig.controller.finishInstanceDrag(at: CGPoint(x: -10000, y: -10000))
    let id = try XCTUnwrap(rig.instance).id
    XCTAssertTrue(rig.controller.beginInstanceDrag(id))
    rig.card.setFrameOrigin(rig.header.frame.origin)
    XCTAssertFalse(try XCTUnwrap(rig.instance).closed, "Crossing a target is not a drop")
    rig.controller.finishInstanceDrag(at: rig.headerPoint)
    XCTAssertTrue(try XCTUnwrap(rig.instance).closed)
    XCTAssertFalse(rig.controller.beginInstanceDrag(id), "Closed records cannot start tracking")
    rig.verifyTruth()
  }

  func testMinimizedTitleReachesNativeDragSurfaceAndClickRestores() throws {
    let rig = try Rig()
    defer { rig.cleanup() }
    XCTAssertTrue(rig.controller.detachForDrag(rig.identity))
    rig.controller.finishInstanceDrag(at: CGPoint(x: -10000, y: -10000))
    let id = try XCTUnwrap(rig.instance).id
    rig.controller.setInstanceMinimized(id, true)
    RunLoop.main.run(until: Date().addingTimeInterval(0.08))
    let root = try XCTUnwrap(rig.card.contentView)
    root.layoutSubtreeIfNeeded()
    XCTAssertTrue(try XCTUnwrap(rig.instance).minimized)
    // The title occupies the free leading part of this 500pt bar. The old
    // restore Button consumed this point before the native drag view saw it.
    let point = NSPoint(x: 80, y: root.bounds.midY)
    let drag = try XCTUnwrap(root.hitTest(point) as? OverlayIntercomDragView)
    XCTAssertEqual(drag.minimumDragDistance, 4)
    let location = root.convert(point, to: nil)
    let topRight = CGPoint(x: rig.card.frame.maxX, y: rig.card.frame.maxY)
    for type in [NSEvent.EventType.leftMouseDown, .leftMouseUp] {
      let event = try XCTUnwrap(NSEvent.mouseEvent(
        with: type, location: location, modifierFlags: [], timestamp: 0,
        windowNumber: rig.card.windowNumber, context: nil, eventNumber: 1,
        clickCount: 1, pressure: type == .leftMouseDown ? 1 : 0))
      if type == .leftMouseDown { drag.mouseDown(with: event) }
      else { drag.mouseUp(with: event) }
    }
    XCTAssertFalse(try XCTUnwrap(rig.instance).minimized)
    XCTAssertEqual(rig.card.frame.maxX, topRight.x, accuracy: 1)
    XCTAssertEqual(rig.card.frame.maxY, topRight.y, accuracy: 1)
    rig.verifyTruth()
  }

  func testHidingOrClosingDuringDragCannotResurrectAWindow() throws {
    for close in [false, true] {
      let rig = try Rig()
      defer { rig.cleanup() }
      XCTAssertTrue(rig.controller.detachForDrag(rig.identity))
      if close {
        rig.controller.closeInstance(try XCTUnwrap(rig.instance).id)
      } else {
        rig.controller.setVisible(false, relativeTo: nil)
      }
      rig.controller.finishInstanceDrag(at: rig.headerPoint)
      XCTAssertFalse(rig.card.isVisible)
      XCTAssertEqual(try XCTUnwrap(rig.instance).closed, close)
      rig.verifyTruth()
    }
  }

  func testReplacementOwnerCannotReceiveOldWindowsDrop() async throws {
    let rig = try Rig()
    defer { rig.cleanup() }
    XCTAssertTrue(rig.controller.detachForDrag(rig.identity))
    let replacement = try Rig.conversation(session: "replacement", channel: "3")
    Rig.apply([replacement], to: rig.state)
    try await Task.sleep(for: .milliseconds(60))
    rig.controller.finishInstanceDrag(at: rig.headerPoint)
    let old = try XCTUnwrap(rig.instance)
    XCTAssertFalse(old.closed, "Old-owner history cannot dock under the replacement")
    XCTAssertEqual(old.conversationID, rig.conversation.id)
    let successor = OverlayIntercomCardIdentity(digit: "3", conversation: replacement)
    XCTAssertEqual(rig.controller.desktop.groupRows(of: [successor]), [successor])
    rig.verifyTruth()
  }

  func testArchiveDuringDragCannotResurrectOrRebindTheWindow() async throws {
    let rig = try Rig()
    defer { rig.cleanup() }
    rig.state.archiveAgentCommand = { _, _ in }
    XCTAssertTrue(rig.controller.detachForDrag(rig.identity))
    await rig.state.archiveAgent(try XCTUnwrap(rig.conversation.owner))
    for _ in 0..<50 {
      if rig.instance?.closed == true { break }
      try await Task.sleep(for: .milliseconds(20))
    }
    rig.controller.finishInstanceDrag(at: rig.headerPoint)
    XCTAssertTrue(try XCTUnwrap(rig.instance).closed)
    XCTAssertFalse(rig.card.isVisible)
    rig.verifyTruth()
  }
}
