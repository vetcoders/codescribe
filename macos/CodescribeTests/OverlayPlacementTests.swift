import AppKit
import XCTest

@testable import Codescribe

/// Pure-geometry contract for the overlay's anchored placement: six anchors
/// over a visible frame, margin respected, and the free-motion clamp keeping a
/// restored origin fully on-screen after display changes.
final class OverlayPlacementTests: XCTestCase {
  // A visible frame with a non-zero origin, as on a secondary display —
  // anchor math must respect minX/minY, not assume (0,0).
  private let visible = NSRect(x: 100, y: 50, width: 1600, height: 900)
  private let size = NSSize(width: 320, height: 600)
  private let m = OverlayPlacement.margin

  func testTopAnchorsSitUnderTheVisibleTopEdge() {
    for anchor in [OverlayAnchor.topLeft, .topCenter, .topRight] {
      let origin = OverlayPlacement.origin(for: anchor, size: size, in: visible)
      XCTAssertEqual(origin.y, visible.maxY - size.height - m, "\(anchor)")
    }
  }

  func testBottomAnchorsSitOnTheVisibleBottomEdge() {
    for anchor in [OverlayAnchor.bottomLeft, .bottomCenter, .bottomRight] {
      let origin = OverlayPlacement.origin(for: anchor, size: size, in: visible)
      XCTAssertEqual(origin.y, visible.minY + m, "\(anchor)")
    }
  }

  func testHorizontalLanesLeftCenterRight() {
    let left = OverlayPlacement.origin(for: .topLeft, size: size, in: visible)
    let center = OverlayPlacement.origin(for: .topCenter, size: size, in: visible)
    let right = OverlayPlacement.origin(for: .topRight, size: size, in: visible)
    XCTAssertEqual(left.x, visible.minX + m)
    XCTAssertEqual(center.x, visible.midX - size.width / 2)
    XCTAssertEqual(right.x, visible.maxX - size.width - m)
  }

  func testDefaultAnchorIsTopRightUnderTheTray() {
    XCTAssertEqual(OverlayPlacement.defaultAnchor, .topRight)
  }

  func testEveryAnchorKeepsThePanelFullyInsideTheVisibleFrame() {
    for anchor in OverlayAnchor.allCases {
      let origin = OverlayPlacement.origin(for: anchor, size: size, in: visible)
      let frame = NSRect(origin: origin, size: size)
      XCTAssertTrue(visible.contains(frame), "\(anchor): \(frame) escapes \(visible)")
    }
  }

  func testClampPullsAnOffscreenFreeMotionOriginBackInside() {
    let offscreen = NSPoint(x: visible.maxX + 500, y: visible.minY - 500)
    let clamped = OverlayPlacement.clampOrigin(offscreen, size: size, in: visible)
    XCTAssertTrue(visible.contains(NSRect(origin: clamped, size: size)))
  }

  func testAnchorsLandOnTheWholePointsAppKitGivesTheWindow() {
    // AppKit floors a window origin and rounds its size up. The anchor math
    // floors too, so the requested size and the size AppKit rounds it up to
    // resolve to one origin: the one the panel really lands on.
    let requested = NSSize(width: 470.3, height: 283.37)
    let landed = NSSize(width: 471, height: 284)
    for anchor in OverlayAnchor.allCases {
      let origin = OverlayPlacement.origin(for: anchor, size: requested, in: visible)
      XCTAssertEqual(origin.x, origin.x.rounded(.down), "\(anchor)")
      XCTAssertEqual(origin.y, origin.y.rounded(.down), "\(anchor)")
      XCTAssertEqual(
        OverlayPlacement.origin(for: anchor, size: landed, in: visible), origin, "\(anchor)")
    }
    XCTAssertEqual(
      OverlayPlacement.origin(for: .topCenter, size: landed, in: visible),
      NSPoint(x: 664, y: 654)
    )
  }

  func testClampIsIdentityForAnOriginAlreadyInside() {
    let inside = NSPoint(x: visible.midX, y: visible.minY + 20)
    let clamped = OverlayPlacement.clampOrigin(inside, size: size, in: visible)
    XCTAssertEqual(clamped, inside)
  }

  @MainActor
  func testAnchorSelectionMovesTheRealPanelAndFreeMotionPersistsADrag() throws {
    let oldAnchor = OverlayPlacement.anchor
    let oldFreeMotion = OverlayPlacement.freeMotion
    let oldOrigin = OverlayPlacement.restoredOrigin(size: .zero, on: nil)
    defer {
      OverlayPlacement.anchor = oldAnchor
      OverlayPlacement.freeMotion = oldFreeMotion
      if let oldOrigin {
        OverlayPlacement.persistOrigin(oldOrigin)
      } else {
        OverlayPlacement.clearPersistedOrigin()
      }
    }

    let state = OverlayState.previewListening()
    let panel = try XCTUnwrap(
      DictationOverlayWindow.make(
        state: state,
        textScale: TextScaleController(key: "OverlayPlacementTests.textScale")
      ) as? FloatingOverlayPanel
    )
    defer { panel.invalidatePresence() }
    let controller = OverlayController(
      state: state,
      engine: nil,
      overlayEnabledProvider: { true },
      assistiveStatusProvider: { false },
      panelFactory: { _, _ in panel },
      orderPanelFront: { _ in },
      orderPanelOut: { _ in }
    )
    controller.show()

    let screen = try XCTUnwrap(NSScreen.main)
    state.selectPlacementAnchor(.bottomLeft)
    XCTAssertFalse(state.freeMotion)
    XCTAssertEqual(
      panel.frame.origin,
      OverlayPlacement.origin(for: .bottomLeft, size: panel.frame.size, on: screen)
    )

    state.selectFreeMotion()
    let dragged = OverlayPlacement.clampOrigin(
      NSPoint(x: screen.visibleFrame.midX, y: screen.visibleFrame.midY),
      size: panel.frame.size,
      in: screen.visibleFrame
    )
    panel.setFrameOrigin(dragged)
    panel.onUserMove?()

    XCTAssertTrue(state.freeMotion)
    let restored = try XCTUnwrap(
      OverlayPlacement.restoredOrigin(size: panel.frame.size, on: screen)
    )
    XCTAssertEqual(restored.x, dragged.x, accuracy: 1)
    XCTAssertEqual(restored.y, dragged.y, accuracy: 1)
  }

  @MainActor
  func testAnchoredMouseDragReturnsToChosenAnchorOnEveryShowAndFreeMotionUsesSavedDropPoint()
    throws
  {
    try withDragPanel(showsPanel: true) { state, panel, controller in
      let screen = try XCTUnwrap(NSScreen.main)
      var lastDrop: NSPoint?

      for anchor in OverlayAnchor.allCases {
        state.selectPlacementAnchor(anchor)
        XCTAssertFalse(state.freeMotion)
        XCTAssertFalse(OverlayPlacement.freeMotion)
        let anchoredOrigin = try XCTUnwrap(
          OverlayPlacement.origin(for: anchor, size: panel.frame.size, on: screen))
        assertFrameOrigin(anchoredOrigin, on: panel)

        let start = panel.frame.origin
        let events = try dragEvents(in: panel, delta: NSSize(width: -45, height: -30))
        for event in events.dropLast() {
          panel.sendEvent(event)
          XCTAssertFalse(state.freeMotion, "Dragging preserves the selected anchor")
        }
        XCTAssertNotEqual(panel.frame.origin, start)
        let drop = panel.frame.origin
        panel.sendEvent(try XCTUnwrap(events.last))

        XCTAssertEqual(state.placementAnchor, anchor)
        XCTAssertFalse(state.freeMotion)
        XCTAssertFalse(OverlayPlacement.freeMotion)
        XCTAssertEqual(panel.frame.origin, drop)
        XCTAssertEqual(OverlayPlacement.restoredOrigin(size: .zero, on: nil), drop)
        lastDrop = drop

        controller.hide()
        XCTAssertFalse(panel.isVisible)
        controller.show()
        XCTAssertTrue(panel.isVisible)
        XCTAssertEqual(state.placementAnchor, anchor)
        XCTAssertFalse(state.freeMotion)
        assertFrameOrigin(anchoredOrigin, on: panel)
      }

      let drop = try XCTUnwrap(lastDrop)
      controller.hide()
      state.selectFreeMotion()
      XCTAssertTrue(state.freeMotion)
      XCTAssertTrue(OverlayPlacement.freeMotion)
      let freeMotionOrigin = OverlayPlacement.clampOrigin(
        drop, size: panel.frame.size, in: screen.visibleFrame)
      assertFrameOrigin(freeMotionOrigin, on: panel)

      controller.show()
      XCTAssertTrue(panel.isVisible)
      XCTAssertTrue(state.freeMotion)
      assertFrameOrigin(freeMotionOrigin, on: panel)
    }
  }

  @MainActor
  func testProgrammaticMovementAndClickWithoutDragDoNotSelectFreeMotion() throws {
    try withDragPanel { state, panel, _ in
      let saved = NSPoint(x: -123, y: 456)
      OverlayPlacement.persistOrigin(saved)
      let start = panel.frame.origin
      panel.setFrameOrigin(NSPoint(x: start.x - 25, y: start.y - 15))
      panel.windowDidMove(Notification(name: NSWindow.didMoveNotification, object: panel))
      XCTAssertFalse(state.freeMotion)
      XCTAssertEqual(OverlayPlacement.restoredOrigin(size: .zero, on: nil), saved)

      let click = try dragEvents(in: panel, delta: .zero)
      for event in click { panel.sendEvent(event) }
      XCTAssertFalse(state.freeMotion, "A zero-distance drag is still only a click")
      XCTAssertEqual(OverlayPlacement.restoredOrigin(size: .zero, on: nil), saved)

      // An unrelated frame write between mouse down/up is not a dragged event.
      let interruptedClick = try dragEvents(in: panel, delta: .zero)
      panel.sendEvent(try XCTUnwrap(interruptedClick.first))
      panel.setFrameOrigin(start)
      panel.sendEvent(try XCTUnwrap(interruptedClick.last))
      XCTAssertFalse(state.freeMotion)
      XCTAssertEqual(OverlayPlacement.restoredOrigin(size: .zero, on: nil), saved)
    }
  }

  @MainActor
  func testDismissingHoveredMidiReopensAtParkedMiniOrigin() throws {
    try assertMidiDismissalPreservesMiniOrigin(drag: .zero, atLeftEdge: false)
  }

  @MainActor
  func testDismissingDraggedMidiReopensAtItsMovedMiniOrigin() throws {
    try assertMidiDismissalPreservesMiniOrigin(
      drag: NSSize(width: 35, height: -25), atLeftEdge: false)
  }

  @MainActor
  func testDismissingScreenClampedMidiDoesNotMoveParkedMini() throws {
    try assertMidiDismissalPreservesMiniOrigin(drag: .zero, atLeftEdge: true)
  }

  @MainActor
  private func assertMidiDismissalPreservesMiniOrigin(drag: NSSize, atLeftEdge: Bool) throws {
    let savedSize = DictationOverlayWindow.restoredContentSize()
    defer { DictationOverlayWindow.persist(size: savedSize) }
    try withDragPanel(showsPanel: true) { state, panel, controller in
      state.selectFreeMotion()
      state.setPresentationMode(.mini)
      panel.settleFrameTransition()
      let visible = try XCTUnwrap(panel.screen?.visibleFrame)
      let proposed = NSPoint(
        x: atLeftEdge ? visible.minX + 20 : visible.midX,
        y: visible.midY)
      panel.setFrameOrigin(proposed)
      panel.windowDidMove(Notification(name: NSWindow.didMoveNotification, object: panel))
      let parked = panel.frame.origin
      state.setPresentationMode(.midi)
      panel.settleFrameTransition()
      XCTAssertEqual(panel.frame.width, DictationOverlayWindow.midiSize.width)
      if drag != .zero {
        for event in try dragEvents(in: panel, delta: drag) { panel.sendEvent(event) }
      }
      let expected = NSPoint(x: parked.x + drag.width, y: parked.y + drag.height)
      controller.dismiss()
      XCTAssertFalse(panel.isVisible)
      XCTAssertEqual(
        OverlayPlacement.restoredOrigin(size: .zero, on: nil), expected,
        "Temporary hover width must not become the persisted mini origin")
      controller.showWidget()
      panel.settleFrameTransition()
      XCTAssertEqual(state.presentationMode, .mini)
      XCTAssertTrue(panel.isVisible)
      assertFrameOrigin(expected, on: panel)
    }
  }

  @MainActor
  private func withDragPanel(
    showsPanel: Bool = false,
    _ check: (OverlayState, FloatingOverlayPanel, OverlayController) throws -> Void
  ) throws {
    let oldAnchor = OverlayPlacement.anchor
    let oldFreeMotion = OverlayPlacement.freeMotion
    let oldOrigin = OverlayPlacement.restoredOrigin(size: .zero, on: nil)
    defer {
      OverlayPlacement.anchor = oldAnchor
      OverlayPlacement.freeMotion = oldFreeMotion
      if let oldOrigin {
        OverlayPlacement.persistOrigin(oldOrigin)
      } else {
        OverlayPlacement.clearPersistedOrigin()
      }
    }
    let state = OverlayState()
    state.selectPlacementAnchor(.topRight)
    let panel = try XCTUnwrap(
      DictationOverlayWindow.make(
        state: state, textScale: TextScaleController(key: "OverlayPlacementTests.dragScale")
      ) as? FloatingOverlayPanel)
    defer {
      panel.orderOut(nil)
      panel.invalidatePresence()
    }
    // The panel size is part of every anchor's origin. Pin it instead of
    // inheriting whatever size an earlier run or the app persisted; the odd
    // width keeps the center anchors on the half-point case AppKit rounds.
    panel.setContentSize(NSSize(width: 471, height: 283))
    let controller = OverlayController(
      state: state, engine: nil,
      overlayEnabledProvider: { true }, assistiveStatusProvider: { false },
      panelFactory: { _, _ in panel },
      orderPanelFront: { panel in
        if showsPanel { panel.orderFrontRegardless() }
      },
      orderPanelOut: { panel in
        if showsPanel { panel.orderOut(nil) }
      })
    controller.show()
    panel.contentView?.layoutSubtreeIfNeeded()
    RunLoop.main.run(until: Date().addingTimeInterval(0.05))
    panel.contentView?.layoutSubtreeIfNeeded()
    try check(state, panel, controller)
    withExtendedLifetime(controller) {}
  }

  @MainActor
  private func assertFrameOrigin(
    _ expected: NSPoint,
    on panel: NSWindow,
    file: StaticString = #filePath,
    line: UInt = #line
  ) {
    let deadline = Date().addingTimeInterval(2)
    while panel.frame.origin != expected && Date() < deadline {
      RunLoop.main.run(until: min(deadline, Date().addingTimeInterval(0.01)))
    }
    XCTAssertEqual(panel.frame.origin, expected, file: file, line: line)
  }

  @MainActor
  private func dragEvents(in panel: FloatingOverlayPanel, delta: NSSize) throws -> [NSEvent] {
    let start = try XCTUnwrap(
      stride(from: CGFloat(20), to: panel.frame.width - 20, by: 10)
        .map { NSPoint(x: $0, y: panel.frame.height - 20) }
        .first { panel.isWindowDragHit(at: $0) },
      "The real overlay header must expose a drag region")
    let end = NSPoint(x: start.x + delta.width, y: start.y + delta.height)
    let types: [NSEvent.EventType] = [.leftMouseDown, .leftMouseDragged, .leftMouseUp]
    return try types.enumerated().map { index, type in
      let event = try XCTUnwrap(
        NSEvent.mouseEvent(
          with: type, location: index == 0 ? start : end, modifierFlags: [],
          timestamp: ProcessInfo.processInfo.systemUptime + Double(index) * 0.01,
          windowNumber: panel.windowNumber, context: nil, eventNumber: 3500 + index,
          clickCount: 1, pressure: type == .leftMouseUp ? 0 : 1))
      // Freeze screen coordinates before any event moves the panel.
      let cgEvent = try XCTUnwrap(event.cgEvent?.copy())
      return try XCTUnwrap(NSEvent(cgEvent: cgEvent))
    }
  }
}
