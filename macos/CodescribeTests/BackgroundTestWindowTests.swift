import AppKit
import XCTest

@MainActor
final class BackgroundTestWindowTests: XCTestCase {
  func testHostSettingsStaysBelowThePersonsWindows() {
    let visibleSettings = NSApp.windows.filter {
      $0.isVisible && ($0.title == "Settings" || $0.title == String(localized: "Settings"))
    }
    for window in visibleSettings {
      XCTAssertLessThan(window.level.rawValue, NSWindow.Level.normal.rawValue)
      XCTAssertFalse(window.isKeyWindow)
    }
  }

  func testFixtureStaysBelowNormalWindowsWithoutTakingKeyboardOrActivation() {
    let window = NSWindow(
      contentRect: NSRect(x: 150, y: 150, width: 120, height: 80),
      styleMask: .borderless, backing: .buffered, defer: false)
    window.isReleasedWhenClosed = false
    defer { window.close() }
    BackgroundTestWindow.present(window)
    XCTAssertLessThan(window.level.rawValue, NSWindow.Level.normal.rawValue)
    XCTAssertTrue(window.isVisible, "AX can still inspect the fixture")
    XCTAssertFalse(window.isKeyWindow)
    XCTAssertFalse(NSApp.isActive, "the test host must not activate over the person's app")
  }
}
