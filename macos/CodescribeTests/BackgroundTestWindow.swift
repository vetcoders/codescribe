import AppKit
import XCTest

@testable import Codescribe

/// Presentation owner for native fixtures. The normal suite has no visible
/// windows or input targets; its geometry stays unchanged for layout probes.
@MainActor
enum BackgroundTestWindow {
  static func present(_ window: NSWindow) {
    if QualityCaptureHost.interactiveWindows() {
      window.orderFrontRegardless()
      return
    }
    window.alphaValue = 0
    window.ignoresMouseEvents = true
    window.orderBack(nil)
  }

  static func requireIsolatedGUI() throws {
    try XCTSkipUnless(
      QualityCaptureHost.interactiveWindows(),
      "Native window/AX acceptance requires CODESCRIBE_TEST_ISOLATED_GUI=1 in an isolated GUI session; it has not passed in the background suite."
    )
  }

  static func takeKeyboard(_ window: NSWindow) throws {
    try requireIsolatedGUI()
    window.makeKeyAndOrderFront(nil)
  }

  /// Ends an overlay panel built by `DictationOverlayWindow.make` together
  /// with its state. `make` wires the state to the panel, and an ordered-out
  /// panel that is never closed can outlive its test: a state left behind (its
  /// automatic fold, a roster or preference change) would still drive that
  /// panel's presentation, and every transition of it lands in the shared
  /// `OverlayPresentationTrace` while a later test reads the ledger. Disconnect
  /// the state first, then stop the panel's monitors and close it.
  static func tearDown(_ panel: FloatingOverlayPanel, state: OverlayState?) {
    if let state {
      state.suspendAutoCollapse()
      state.onPresentationModeChanged = nil
      state.onChannelMapLayoutChanged = nil
      state.onPinChanged = nil
      state.onAgentSidebarPresented = nil
    }
    panel.invalidatePresence()
    panel.orderOut(nil)
    panel.isReleasedWhenClosed = false
    panel.close()
  }
}
