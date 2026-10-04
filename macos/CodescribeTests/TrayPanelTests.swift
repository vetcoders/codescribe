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

  func testSecondClickOnTheStatusButtonIsLeftToTheButtonAction() {
    let (button, onButton) = makeStatusButton()
    let panel = TrayPanel()
    var dismissals = 0
    panel.onDismiss = { dismissals += 1 }
    panel.present(from: button) { Text("Menu") }

    // The status bar's mouse-down reaches the global monitor and the key
    // change before the button's action fires; neither may close the menu.
    panel.mouseDownOutside(at: onButton)
    panel.windowDidResignKey(Notification(name: NSWindow.didResignKeyNotification, object: panel))
    XCTAssertNotNil(panel.contentViewController)
    XCTAssertEqual(dismissals, 0)

    // The action's toggle closes it exactly once.
    panel.dismiss()
    XCTAssertNil(panel.contentViewController)
    XCTAssertEqual(dismissals, 1)
  }

  func testClickAwayFromTheStatusButtonClosesTheMenu() {
    let (button, onButton) = makeStatusButton()
    let panel = TrayPanel()
    var dismissals = 0
    panel.onDismiss = { dismissals += 1 }
    panel.present(from: button) { Text("Menu") }
    panel.mouseDownOutside(at: NSPoint(x: onButton.x + 400, y: onButton.y - 300))
    XCTAssertNil(panel.contentViewController)
    XCTAssertEqual(dismissals, 1)
  }

  func testToggleClickDoesNotOutliveTheMenuItClosed() {
    let (button, onButton) = makeStatusButton()
    let panel = TrayPanel()
    var dismissals = 0
    panel.onDismiss = { dismissals += 1 }
    panel.present(from: button) { Text("Menu") }
    panel.mouseDownOutside(at: onButton)
    panel.dismiss()
    XCTAssertEqual(dismissals, 1)

    // Reopened by a later click, a keyboard app switch closes it again.
    panel.present(from: button) { Text("Menu") }
    panel.windowDidResignKey(Notification(name: NSWindow.didResignKeyNotification, object: panel))
    XCTAssertNil(panel.contentViewController)
    XCTAssertEqual(dismissals, 2)
  }

  /// A button hosted in a window, like the status item's, and its screen-space centre.
  private func makeStatusButton() -> (NSButton, NSPoint) {
    let window = NSWindow(
      contentRect: NSRect(x: 300, y: 700, width: 80, height: 24),
      styleMask: .borderless, backing: .buffered, defer: false)
    window.isReleasedWhenClosed = false
    hostWindows.append(window)
    let button = NSButton(frame: NSRect(x: 0, y: 0, width: 80, height: 24))
    window.contentView = button
    let rect = window.convertToScreen(button.convert(button.bounds, to: nil))
    return (button, NSPoint(x: rect.midX, y: rect.midY))
  }

  private var hostWindows: [NSWindow] = []

  func testKeyboardAppSwitchDismissesWithoutAMouseClick() {
    let panel = TrayPanel()
    panel.contentViewController = NSHostingController(rootView: Text("Menu"))
    var dismissals = 0
    panel.onDismiss = { dismissals += 1 }
    NotificationCenter.default.post(name: NSApplication.didResignActiveNotification, object: NSApp)
    XCTAssertNil(panel.contentViewController)
    XCTAssertEqual(dismissals, 1)
  }
}
