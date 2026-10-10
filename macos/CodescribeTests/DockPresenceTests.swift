import AppKit
import XCTest

@testable import Codescribe

/// The Settings and Agent windows never minimise: a minimised Codescribe
/// window is an empty tile in App Exposé and Mission Control, with or without
/// a Dock icon.
@MainActor
final class DockPresenceTests: XCTestCase {
  private let documentMask: NSWindow.StyleMask = [
    .titled, .closable, .miniaturizable, .resizable, .fullSizeContentView,
  ]

  func testStyleMaskDropsMinimiseAndKeepsTheRest() {
    let adopted = DockPresence.styleMask(documentMask)
    XCTAssertFalse(adopted.contains(.miniaturizable))
    XCTAssertTrue(adopted.contains(.titled))
    XCTAssertTrue(adopted.contains(.closable))
    XCTAssertTrue(adopted.contains(.resizable))
    XCTAssertTrue(adopted.contains(.fullSizeContentView))
  }

  func testStyleMaskIsIdempotent() {
    let adopted = DockPresence.styleMask(documentMask)
    XCTAssertEqual(DockPresence.styleMask(adopted), adopted)
  }

  func testAdoptedWindowCannotMinimise() {
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
