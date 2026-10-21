import AppKit
import XCTest

@testable import Codescribe

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

  func testFixturePreservesWindowLevelWithoutTakingKeyboardOrActivation() throws {
    try XCTSkipIf(QualityCaptureHost.interactiveWindows(), "This checks the default invisible mode")
    let window = NSWindow(
      contentRect: NSRect(x: 150, y: 150, width: 120, height: 80),
      styleMask: .borderless, backing: .buffered, defer: false)
    window.isReleasedWhenClosed = false
    defer { window.close() }
    let originalLevel = window.level
    BackgroundTestWindow.present(window)
    XCTAssertEqual(window.level, originalLevel, "Presentation must not alter window policy")
    XCTAssertTrue(window.isVisible, "The fixture remains ordered for layout")
    XCTAssertEqual(window.alphaValue, 0, "No fixture pixels reach the desktop")
    XCTAssertTrue(window.ignoresMouseEvents, "The fixture cannot intercept the person's input")
    XCTAssertEqual(window.frame.origin, NSPoint(x: 150, y: 150), "Layout geometry stays intact")
    XCTAssertFalse(window.isKeyWindow)
    XCTAssertFalse(NSApp.isActive, "the test host must not activate over the person's app")
  }
}
