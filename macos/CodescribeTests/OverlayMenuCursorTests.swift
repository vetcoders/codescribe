import AppKit
import XCTest

@testable import Codescribe

@MainActor
final class OverlayMenuCursorTests: XCTestCase {
  func testMountedMenuOwnsArrowAboveSelectableTextAndReleasesOnRemoval() {
    let panel = FloatingOverlayPanel(
      contentRect: NSRect(x: 0, y: 0, width: 600, height: 500),
      styleMask: [.borderless, .nonactivatingPanel], backing: .buffered, defer: false)
    defer { panel.close() }
    let content = NSView(frame: NSRect(x: 0, y: 0, width: 600, height: 500))
    let text = NSTextView(frame: content.bounds)
    text.string = "Selectable transcript behind the drawer"
    content.addSubview(text)
    panel.contentView = content
    let menu = OverlayMenuCursorRegionView(frame: NSRect(x: 300, y: 100, width: 280, height: 350))
    content.addSubview(menu)

    XCTAssertTrue(panel.menuCursorRegion === menu)
    XCTAssertEqual(panel.cursor(at: NSPoint(x: 400, y: 200)), .arrow)
    XCTAssertEqual(panel.cursor(at: NSPoint(x: 100, y: 200)), .iBeam)
    XCTAssertNil(menu.hitTest(NSPoint(x: 10, y: 10)), "Drawer controls retain their own hit path")

    menu.isHidden = true
    XCTAssertEqual(panel.cursor(at: NSPoint(x: 400, y: 200)), .iBeam)
    menu.isHidden = false
    menu.removeFromSuperview()
    XCTAssertNil(panel.menuCursorRegion)
    XCTAssertEqual(panel.cursor(at: NSPoint(x: 400, y: 200)), .iBeam)
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
