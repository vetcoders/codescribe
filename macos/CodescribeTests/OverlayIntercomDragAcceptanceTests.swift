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

    init(
      secondRow: Bool = false, expanded: Bool = false, clock: ManualAutoCollapseScheduler? = nil
    ) throws {
      defaults = try XCTUnwrap(UserDefaults(suiteName: suite))
      store = OverlayIntercomDesktopStore(defaults: defaults)
      conversation = try Self.conversation(session: "first", channel: "3")
      identity = .init(digit: "3", conversation: conversation)
      var conversations = [conversation]
      if secondRow { conversations.append(try Self.conversation(session: "second", channel: "4")) }
      Self.apply(conversations, to: state)
      state.publishConversationText = { _, _, _, _ in XCTFail("Placement must not publish") }
      state.conversationDrafts[conversation.id] = "Keep the exact-owner draft"
      generation = state.captureGeneration
      store.save(.init(groupWidth: 500, groupRowHeight: 80))
      controller = OverlayIntercomWidgetController(
        state: state, textScale: TextScaleController(key: suite), desktopStore: store,
        idleNow: { clock?.now ?? Date.timeIntervalSinceReferenceDate },
        idleSchedule: clock?.scheduler ?? OverlayAutoCollapse.liveScheduler)
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

  func testNativeDragDoesNotRewriteObservableOrPersistedDesktopAtPointerFrequency() throws {
    let rig = try Rig(secondRow: true)
    defer { rig.cleanup() }
    XCTAssertTrue(rig.controller.detachForDrag(rig.identity))
    rig.controller.finishInstanceDrag(at: CGPoint(x: -10000, y: -10000))
    let id = try XCTUnwrap(rig.instance).id
    XCTAssertTrue(rig.controller.beginInstanceDrag(id))
    let stored = rig.store.load()
    let desktop = rig.controller.desktop
    let root = rig.card.contentView
    let initial = rig.card.frame
    for tick in 1...30 {
      rig.card.setFrameOrigin(
        CGPoint(x: initial.minX - CGFloat(tick), y: initial.minY - CGFloat(tick)))
      rig.controller.windowDidMove(
        Notification(name: NSWindow.didMoveNotification, object: rig.card))
      XCTAssertEqual(rig.store.load(), stored, "Pointer movement must not serialize desktop JSON")
      XCTAssertEqual(rig.controller.desktop, desktop, "Pointer movement must not invalidate card bodies")
      XCTAssertTrue(rig.card.contentView === root, "The mounted editor survives tracking")
    }
    rig.controller.finishInstanceDrag(at: CGPoint(x: -10000, y: -10000))
    XCTAssertEqual(try XCTUnwrap(rig.instance).frame, rig.card.frame)
    XCTAssertNotEqual(rig.store.load(), stored, "Release commits the final geometry")
    rig.verifyTruth()
  }

  func testGroupedReturnUsesChosenIntervalAndDefersWhileVoiceIsOpen() throws {
    let clock = ManualAutoCollapseScheduler()
    let rig = try Rig(expanded: true, clock: clock)
    defer { rig.cleanup() }
    rig.state.engine = OverlayChromePolicyEngine()
    rig.state.setAutoFoldSeconds(5)
    // A menu interaction ends through the controller's real notification owner.
    NotificationCenter.default.post(name: NSMenu.didEndTrackingNotification, object: nil)
    XCTAssertEqual(clock.outstanding, 1)
    clock.advance(by: 4)
    XCTAssertTrue(rig.controller.presentation.expanded.contains(rig.identity))
    let owner = try XCTUnwrap(rig.conversation.owner)
    rig.state.applyChannelRoster([
      .init(
        channel: "3", audience: owner.name, provider: owner.provider,
        providerSessionId: owner.providerSessionID, open: true, loud: false,
        autosealDeadlineUnixMs: nil, followerAlive: true, unreadSinceUnixMs: nil)
    ])
    clock.advance(by: 1)
    XCTAssertTrue(rig.controller.presentation.expanded.contains(rig.identity))
    Rig.apply([rig.conversation], to: rig.state)
    clock.advance(by: 5)
    XCTAssertTrue(rig.controller.presentation.expanded.isEmpty)
    rig.verifyTruth()
  }

  func testNeverReturnAndDetachedCardsRemainOpen() throws {
    let clock = ManualAutoCollapseScheduler()
    let rig = try Rig(expanded: true, clock: clock)
    defer { rig.cleanup() }
    rig.state.engine = OverlayChromePolicyEngine()
    rig.state.setAutoFoldSeconds(0)
    NotificationCenter.default.post(name: NSMenu.didEndTrackingNotification, object: nil)
    clock.advance(by: 1_000)
    XCTAssertTrue(rig.controller.presentation.expanded.contains(rig.identity))
    rig.state.setAutoFoldSeconds(5)
    NotificationCenter.default.post(name: NSMenu.didEndTrackingNotification, object: nil)
    XCTAssertTrue(rig.controller.detachForDrag(rig.identity))
    rig.controller.finishInstanceDrag(at: CGPoint(x: -10000, y: -10000))
    clock.advance(by: 1_000)
    XCTAssertFalse(try XCTUnwrap(rig.instance).closed)
    XCTAssertFalse(try XCTUnwrap(rig.instance).minimized)
    rig.verifyTruth()
  }

  func testNativeResizeCornersAndDeferredGestureLifecycle() throws {
    for cancelled in [false, true] {
      let rig = try Rig(secondRow: true)
      defer { rig.cleanup() }
      let root = try XCTUnwrap(rig.card.contentView)
      root.layoutSubtreeIfNeeded()
      RunLoop.main.run(until: Date().addingTimeInterval(0.2))
      let top = root.isFlipped ? root.bounds.minY + 7 : root.bounds.maxY - 7
      let bottom = root.isFlipped ? root.bounds.maxY - 7 : root.bounds.minY + 7
      let left = root.bounds.minX + 7
      let right = root.bounds.maxX - 7
      // NSView.hitTest takes its superview's coordinates. Hosting and theme
      // frames can disagree on flippedness; convert before testing a corner.
      func target(_ point: CGPoint) -> NSView? {
        root.hitTest(root.convert(point, to: root.superview))
      }
      func isResize(_ point: CGPoint) -> Bool {
        target(point).map { String(describing: type(of: $0)).contains("OverlayIntercomResizeView") }
          ?? false
      }
      XCTAssertTrue(isResize(CGPoint(x: right, y: top)), "Top-right is a visible resize corner")
      XCTAssertTrue(isResize(CGPoint(x: left, y: bottom)), "Bottom-left is a visible resize corner")
      XCTAssertTrue(isResize(CGPoint(x: right, y: bottom)), "Bottom-right is a visible resize corner")
      XCTAssertFalse(isResize(CGPoint(x: left, y: top)), "Top-left belongs to detach")
      XCTAssertFalse(isResize(CGPoint(x: root.bounds.midX, y: bottom)), "No invisible edge band")
      let point = CGPoint(x: right, y: bottom)
      let handle = try XCTUnwrap(target(point))
      let desktop = rig.controller.desktop
      let stored = rig.store.load()
      let frame = rig.card.frame
      let alpha = rig.card.alphaValue
      let reconciliations = rig.controller.reconciliationRequests
      func event(_ type: NSEvent.EventType) throws -> NSEvent {
        try XCTUnwrap(NSEvent.mouseEvent(
          with: type, location: root.convert(point, to: nil), modifierFlags: [], timestamp: 0,
          windowNumber: rig.card.windowNumber, context: nil, eventNumber: 1,
          clickCount: 1, pressure: type == .leftMouseUp ? 0 : 1))
      }
      handle.mouseDown(with: try event(.leftMouseDown))
      for _ in 0..<5 { handle.mouseDragged(with: try event(.leftMouseDragged)) }
      XCTAssertEqual(rig.controller.desktop, desktop)
      XCTAssertEqual(rig.store.load(), stored)
      XCTAssertEqual(rig.controller.reconciliationRequests, reconciliations)
      XCTAssertEqual(rig.card.frame, frame, "Only native preview changes during tracking")
      XCTAssertLessThan(rig.card.alphaValue, alpha)
      if cancelled {
        rig.controller.setVisible(false, relativeTo: nil)
        XCTAssertEqual(rig.store.load(), stored)
        XCTAssertEqual(rig.controller.desktop, desktop)
        XCTAssertEqual(rig.card.alphaValue, alpha, accuracy: 0.001)
      } else {
        handle.mouseUp(with: try event(.leftMouseUp))
        XCTAssertEqual(rig.controller.reconciliationRequests, reconciliations + 1)
        XCTAssertEqual(rig.store.load(), rig.controller.desktop)
        XCTAssertEqual(try XCTUnwrap(rig.controller.desktop.groupWidth), Double(rig.header.frame.width), accuracy: 0.01)
      }
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
