import AppKit
import XCTest

@testable import Codescribe

/// Presentation owner for native fixtures. The normal suite has no visible
/// windows or input targets; its geometry stays unchanged for layout/AX probes.
@MainActor
enum BackgroundTestWindow {
  static func present(_ window: NSWindow) {
    if QualityCaptureHost.interactiveWindows() {
      window.orderFrontRegardless()
      return
    }
    window.level = NSWindow.Level(rawValue: NSWindow.Level.normal.rawValue - 1)
    window.alphaValue = 0
    window.ignoresMouseEvents = true
    window.orderBack(nil)
  }

  static func requireIsolatedGUI() throws {
    try XCTSkipUnless(
      QualityCaptureHost.interactiveWindows(),
      "Native keyboard acceptance requires CODESCRIBE_TEST_ISOLATED_GUI=1 in an isolated GUI session; it has not passed in the background suite."
    )
  }

  static func takeKeyboard(_ window: NSWindow) throws {
    try requireIsolatedGUI()
    window.makeKeyAndOrderFront(nil)
  }
}
