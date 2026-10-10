import AppKit
import XCTest

@testable import Codescribe

/// Minimising follows the Dock icon: an accessory app has no Dock tile for a
/// minimised window to land in, so its windows must not offer the button.
@MainActor
final class DockPresenceTests: XCTestCase {
  private let documentMask: NSWindow.StyleMask = [
    .titled, .closable, .miniaturizable, .resizable, .fullSizeContentView,
  ]

  func testStyleMaskDropsMinimiseWithoutDockIconAndRestoresItWithOne() {
    let hidden = DockPresence.styleMask(documentMask, dockIconShown: false)
    XCTAssertFalse(hidden.contains(.miniaturizable))
    XCTAssertTrue(hidden.contains(.titled))
    XCTAssertTrue(hidden.contains(.closable))
    XCTAssertTrue(hidden.contains(.resizable))
    XCTAssertTrue(hidden.contains(.fullSizeContentView))

    let shown = DockPresence.styleMask(hidden, dockIconShown: true)
    XCTAssertEqual(shown, documentMask)
  }

  func testStyleMaskIsIdempotent() {
    XCTAssertEqual(
      DockPresence.styleMask(documentMask, dockIconShown: true), documentMask)
    let hidden = DockPresence.styleMask(documentMask, dockIconShown: false)
    XCTAssertEqual(DockPresence.styleMask(hidden, dockIconShown: false), hidden)
  }

  func testAdoptedWindowFollowsTheAccessoryPolicyOfTheTestHost() throws {
    try XCTSkipUnless(
      NSApp.activationPolicy() == .accessory,
      "the test host runs with a Dock icon; the accessory contract is not observable")
    let window = NSWindow(
      contentRect: NSRect(x: 0, y: 0, width: 320, height: 200),
      styleMask: documentMask, backing: .buffered, defer: true)
    // Never shown, so it simply deallocates with the test; closing it would
    // release it a second time under the default `isReleasedWhenClosed`.
    window.isReleasedWhenClosed = false

    DockPresence.adopt(window)

    XCTAssertFalse(window.styleMask.contains(.miniaturizable))
    XCTAssertTrue(window.styleMask.contains(.closable))
    XCTAssertTrue(window.styleMask.contains(.resizable))
  }
}
