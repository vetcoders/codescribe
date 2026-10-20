import AppKit

/// Native UI fixtures can render and expose AX below the person's windows.
/// This never activates the test host or requests keyboard ownership.
@MainActor
enum BackgroundTestWindow {
  static func present(_ window: NSWindow) {
    window.level = NSWindow.Level(rawValue: NSWindow.Level.normal.rawValue - 1)
    window.orderBack(nil)
  }
}
