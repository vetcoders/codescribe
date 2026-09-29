import AppKit
import SwiftUI
import XCTest

@testable import Codescribe

@MainActor
final class TrayPanelTests: XCTestCase {
  func testMenuStaysUnderStatusItemWhenSectionsExpand() {
    let screen = NSRect(x: 0, y: 0, width: 1440, height: 880)
    let anchor = NSRect(x: 900, y: 880, width: 24, height: 24)
    let closed = TrayPanel.placement(anchor: anchor, visibleScreen: screen, contentHeight: 460)
    let expanded = TrayPanel.placement(anchor: anchor, visibleScreen: screen, contentHeight: 700)
    XCTAssertEqual(closed.maxY, expanded.maxY)
    XCTAssertEqual(closed.midX, anchor.midX)
    XCTAssertEqual(expanded.height, 700)
    XCTAssertLessThan(closed.maxY, anchor.minY)
  }

  func testLongMenuFitsAvailableHeightAndBothScreenEdges() {
    for screen in [
      NSRect(x: 0, y: 0, width: 1440, height: 880),
      NSRect(x: -1920, y: 300, width: 1920, height: 1056),
    ] {
      for x in [screen.minX, screen.midX, screen.maxX - 24] {
        let frame = TrayPanel.placement(
          anchor: NSRect(x: x, y: screen.maxY, width: 24, height: 24),
          visibleScreen: screen, contentHeight: 4000)
        XCTAssertTrue(screen.contains(frame))
        XCTAssertEqual(frame.width, 300)
        XCTAssertEqual(frame.minY, screen.minY + 6)
      }
    }
  }

  func testEscapeDismissesOnceAndReleasesHostedContent() {
    let panel = TrayPanel()
    var dismissals = 0
    panel.onDismiss = { dismissals += 1 }
    panel.contentViewController = NSHostingController(rootView: Text("Menu"))
    panel.cancelOperation(nil)
    panel.cancelOperation(nil)
    XCTAssertNil(panel.contentViewController)
    XCTAssertFalse(panel.isVisible)
    XCTAssertEqual(dismissals, 1)
  }

  func testClosingReleasesContentWithoutClosingOtherWindows() {
    let panel = TrayPanel()
    let other = NSWindow()
    let controller = NSHostingController(rootView: Text("Other window"))
    other.contentViewController = controller
    panel.contentViewController = NSHostingController(rootView: Text("Menu"))
    panel.close()
    XCTAssertNil(panel.contentViewController)
    XCTAssertTrue(other.contentViewController === controller)
    XCTAssertTrue(panel.canBecomeKey)
    XCTAssertFalse(panel.canBecomeMain)
    XCTAssertFalse(panel.isOpaque)
    XCTAssertEqual(panel.styleMask, [.borderless, .nonactivatingPanel])
  }

  func testDetachedAnchorCannotCreateAnOrphanMenu() {
    let panel = TrayPanel()
    panel.present(from: NSButton()) { Text("Detached") }
    XCTAssertNil(panel.contentViewController)
    XCTAssertFalse(panel.isVisible)
  }
}
