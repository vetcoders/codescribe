import AppKit
import XCTest

@testable import Codescribe

@MainActor
final class OverlayMenuCursorTests: XCTestCase {
  func testProductionHeaderRegistersItsVisibleControlsWithThePanel() throws {
    let state = OverlayState()
    state.setPresentationMode(.midi)
    let panel = try XCTUnwrap(
      DictationOverlayWindow.make(
        state: state, textScale: TextScaleController(key: "OverlayMenuCursorTests.header"))
        as? FloatingOverlayPanel)
    defer {
      panel.orderOut(nil)
      panel.invalidatePresence()
    }
    panel.orderFrontRegardless()
    let content = try XCTUnwrap(panel.contentView)
    content.layoutSubtreeIfNeeded()
    RunLoop.main.run(until: Date().addingTimeInterval(0.05))
    content.layoutSubtreeIfNeeded()
    let controls = panel.menuCursorRegions.allObjects.filter {
      $0.window === panel && !$0.isHiddenOrHasHiddenAncestor && !$0.visibleRect.isEmpty
    }
    XCTAssertGreaterThanOrEqual(
      controls.count, 4, "The real header must wire its controls, not only the drawer")
    let saved = NSCursor.current
    defer { saved.set() }
    for control in controls {
      let point = control.convert(NSPoint(x: control.bounds.midX, y: control.bounds.midY), to: nil)
      NSCursor.iBeam.set()
      XCTAssertTrue(panel.refreshCursor(at: point), "Mounted control at \(point)")
      XCTAssertEqual(NSCursor.current, .arrow)
    }
  }

  func testDrawerTextInputKeepsItsTextCursor() {
    let panel = FloatingOverlayPanel(
      contentRect: NSRect(x: 0, y: 0, width: 600, height: 500),
      styleMask: [.borderless, .nonactivatingPanel], backing: .buffered, defer: false)
    panel.isReleasedWhenClosed = false
    defer { panel.close() }
    let content = NSView(frame: NSRect(x: 0, y: 0, width: 600, height: 500))
    panel.contentView = content
    content.addSubview(
      OverlayMenuCursorRegionView(
        frame: NSRect(x: 300, y: 100, width: 280, height: 350)))
    let search = NSTextField(frame: NSRect(x: 320, y: 400, width: 220, height: 24))
    search.isEditable = true
    content.addSubview(search)
    let point = NSPoint(x: 350, y: 412)
    XCTAssertTrue(content.hitTest(point) === search)
    XCTAssertEqual(
      panel.cursor(at: point), .iBeam, "Live search remains a text input inside chrome")
    let saved = NSCursor.current
    defer { saved.set() }
    NSCursor.iBeam.set()
    XCTAssertFalse(panel.refreshCursor(at: point), "The panel leaves native input tracking alone")
    XCTAssertEqual(NSCursor.current, .iBeam)
  }

  func testHeaderAndDrawerKeepIndependentCursorRegions() {
    let panel = FloatingOverlayPanel(
      contentRect: NSRect(x: 0, y: 0, width: 600, height: 500),
      styleMask: [.borderless, .nonactivatingPanel, .resizable],
      backing: .buffered, defer: false)
    panel.isReleasedWhenClosed = false
    let content = NSView(frame: NSRect(x: 0, y: 0, width: 600, height: 500))
    panel.contentView = content
    let header = OverlayMenuCursorRegionView(frame: NSRect(x: 100, y: 450, width: 22, height: 22))
    let drawer = OverlayMenuCursorRegionView(frame: NSRect(x: 300, y: 100, width: 280, height: 350))
    content.addSubview(header)
    content.addSubview(drawer)
    let saved = NSCursor.current
    defer {
      panel.close()
      saved.set()
    }

    NSCursor.iBeam.set()
    XCTAssertTrue(panel.refreshCursor(at: NSPoint(x: 110, y: 460)))
    XCTAssertEqual(NSCursor.current, .arrow, "Mounting the drawer cannot disown header controls")
    NSCursor.iBeam.set()
    XCTAssertTrue(panel.refreshCursor(at: NSPoint(x: 400, y: 200)))
    XCTAssertEqual(NSCursor.current, .arrow)

    drawer.removeFromSuperview()
    NSCursor.iBeam.set()
    XCTAssertTrue(panel.refreshCursor(at: NSPoint(x: 110, y: 460)))
    XCTAssertEqual(NSCursor.current, .arrow, "Removing one region retains the other")
    header.isHidden = true
    NSCursor.iBeam.set()
    XCTAssertFalse(panel.refreshCursor(at: NSPoint(x: 110, y: 460)))
    XCTAssertEqual(NSCursor.current, .iBeam, "Hidden chrome cannot own the cursor")
    header.isHidden = false
    header.removeFromSuperview()
    XCTAssertFalse(panel.refreshCursor(at: NSPoint(x: 110, y: 460)))
  }

  func testMountedMenuOwnsArrowAboveSelectableTextAndReleasesOnRemoval() {
    let panel = FloatingOverlayPanel(
      contentRect: NSRect(x: 0, y: 0, width: 600, height: 500),
      styleMask: [.borderless, .nonactivatingPanel], backing: .buffered, defer: false)
    defer { panel.close() }
    let content = NSView(frame: NSRect(x: 0, y: 0, width: 600, height: 500))
    let text = NSTextView(frame: content.bounds)
    // This fixture tests panel hit ownership, not TextKit's fit-to-content
    // resizing. Keep the selectable surface fixed while layout is realized.
    text.isHorizontallyResizable = false
    text.isVerticallyResizable = false
    text.isEditable = false
    text.string = "Selectable transcript behind the drawer"
    content.addSubview(text)
    panel.contentView = content
    text.frame = content.bounds
    if let container = text.textContainer {
      text.layoutManager?.ensureLayout(for: container)
    }
    content.layoutSubtreeIfNeeded()
    let menu = OverlayMenuCursorRegionView(frame: NSRect(x: 300, y: 100, width: 280, height: 350))
    content.addSubview(menu)

    let outsideMenu = text.convert(NSPoint(x: 100, y: 200), to: nil)
    XCTAssertTrue(
      content.hitTest(outsideMenu) === text,
      "Fixture must hit native text outside menu; text frame=\(text.frame), visible=\(text.visibleRect), menu frame=\(menu.frame)"
    )
    XCTAssertTrue(panel.menuCursorRegions.contains(menu))
    XCTAssertEqual(panel.cursor(at: NSPoint(x: 400, y: 200)), .arrow)
    XCTAssertEqual(panel.cursor(at: NSPoint(x: 100, y: 200)), .iBeam)
    XCTAssertEqual(panel.cursor(at: outsideMenu), .iBeam)
    XCTAssertNil(menu.hitTest(NSPoint(x: 10, y: 10)), "Drawer controls retain their own hit path")

    menu.isHidden = true
    XCTAssertEqual(panel.cursor(at: NSPoint(x: 400, y: 200)), .iBeam)
    menu.isHidden = false
    menu.removeFromSuperview()
    XCTAssertTrue(panel.menuCursorRegions.allObjects.isEmpty)
    XCTAssertEqual(panel.cursor(at: NSPoint(x: 400, y: 200)), .iBeam)
  }

  func testMenuMotionDoesNotDispatchOverlappingTextTrackingAndReleasesOwnership() throws {
    let panel = FloatingOverlayPanel(
      contentRect: NSRect(x: 100, y: 100, width: 600, height: 500),
      styleMask: [.borderless, .nonactivatingPanel, .resizable],
      backing: .buffered, defer: false)
    panel.isReleasedWhenClosed = false
    let content = NSView(frame: NSRect(x: 0, y: 0, width: 600, height: 500))
    let text = DrawerCursorTrackingTextView(frame: content.bounds)
    text.isHorizontallyResizable = false
    text.isVerticallyResizable = false
    text.isEditable = false
    text.string = "Selectable transcript behind the agent drawer"
    content.addSubview(text)
    panel.contentView = content
    text.frame = content.bounds
    panel.acceptsMouseMovedEvents = true
    panel.orderFrontRegardless()
    let saved = NSCursor.current
    defer {
      panel.orderOut(nil)
      saved.set()
    }
    text.addTrackingArea(
      NSTrackingArea(
        rect: text.bounds, options: [.mouseMoved, .activeAlways], owner: text, userInfo: nil))
    let selection = NSRange(location: 2, length: 5)
    text.setSelectedRange(selection)
    let menu = OverlayMenuCursorRegionView(frame: NSRect(x: 300, y: 100, width: 280, height: 350))
    content.addSubview(menu)
    let button = NSButton(title: "Agent", target: nil, action: nil)
    button.frame = NSRect(x: 320, y: 120, width: 120, height: 30)
    content.addSubview(button)
    XCTAssertTrue(content.hitTest(NSPoint(x: 350, y: 135)) === button)

    func move(to point: NSPoint) throws {
      let event = try XCTUnwrap(
        NSEvent.mouseEvent(
          with: .mouseMoved, location: point, modifierFlags: [],
          timestamp: ProcessInfo.processInfo.systemUptime, windowNumber: panel.windowNumber,
          context: nil, eventNumber: 1, clickCount: 0, pressure: 0))
      panel.sendEvent(event)
    }

    // Prove the mounted native tracking path is active before testing its exclusion.
    try move(to: NSPoint(x: 100, y: 200))
    XCTAssertGreaterThan(text.motions, 0)
    text.motions = 0
    let inside = NSPoint(x: 400, y: 200)
    for _ in 0..<20 {
      try move(to: inside)
      XCTAssertEqual(NSCursor.current, .arrow)
    }
    XCTAssertEqual(text.motions, 0, "Drawer motion must not first set the text cursor")
    XCTAssertEqual(text.selectedRange(), selection)
    XCTAssertFalse(panel.refreshCursor(at: inside), "An unchanged arrow needs no second write")

    try move(to: NSPoint(x: 100, y: 200))
    XCTAssertGreaterThan(text.motions, 0, "Text outside the drawer retains native motion")
    text.motions = 0
    menu.isHidden = true
    try move(to: inside)
    XCTAssertGreaterThan(text.motions, 0, "Hiding the drawer releases native tracking")
    text.motions = 0
    menu.isHidden = false
    try move(to: inside)
    XCTAssertEqual(text.motions, 0, "Showing the drawer restores exclusive cursor ownership")
    menu.removeFromSuperview()
    try move(to: inside)
    XCTAssertGreaterThan(text.motions, 0, "Removing the drawer releases native tracking")
    XCTAssertEqual(text.selectedRange(), selection)
  }

  func testMenuDoesNotTakeResizeOwnershipFromPanelEdges() {
    let panel = FloatingOverlayPanel(
      contentRect: NSRect(x: 0, y: 0, width: 600, height: 500),
      styleMask: [.borderless, .nonactivatingPanel, .resizable], backing: .buffered, defer: false)
    defer { panel.close() }
    let content = NSView(frame: NSRect(x: 0, y: 0, width: 600, height: 500))
    panel.contentView = content
    content.addSubview(OverlayMenuCursorRegionView(frame: content.bounds))

    XCTAssertEqual(panel.cursor(at: NSPoint(x: 400, y: 200)), .arrow)
    XCTAssertEqual(
      panel.cursor(at: NSPoint(x: 599, y: 200)), OverlayResizeHit.cursor(for: .right))
    XCTAssertEqual(
      panel.cursor(at: NSPoint(x: 400, y: 499)), OverlayResizeHit.cursor(for: .top))
  }
}

@MainActor
private final class DrawerCursorTrackingTextView: NSTextView {
  var motions = 0

  override func mouseMoved(with event: NSEvent) {
    motions += 1
    NSCursor.iBeam.set()
    super.mouseMoved(with: event)
  }
}
