import AppKit
import XCTest

@testable import Codescribe

/// The Settings and Agent windows never miniaturise: a miniaturised Codescribe
/// window is an empty tile in App Exposé and Mission Control, with or without a
/// Dock icon. The Agent window keeps its minimise control and hides instead
/// (Founder decision, 2026-10-10); the SwiftUI-owned Settings window keeps none.
@MainActor
final class DockPresenceTests: XCTestCase {
  private let documentMask: NSWindow.StyleMask = [
    .titled, .closable, .miniaturizable, .resizable, .fullSizeContentView,
  ]

  func testStyleMaskKeepsMinimiseOnlyWhereItHides() {
    let hiding = DockPresence.styleMask(documentMask, hidesOnMinimise: true)
    XCTAssertTrue(hiding.contains(.miniaturizable))
    let plain = DockPresence.styleMask(documentMask, hidesOnMinimise: false)
    XCTAssertFalse(plain.contains(.miniaturizable))
    for mask in [hiding, plain] {
      XCTAssertTrue(mask.contains(.titled))
      XCTAssertTrue(mask.contains(.closable))
      XCTAssertTrue(mask.contains(.resizable))
      XCTAssertTrue(mask.contains(.fullSizeContentView))
    }
  }

  func testStyleMaskIsIdempotent() {
    for hides in [true, false] {
      let adopted = DockPresence.styleMask(documentMask, hidesOnMinimise: hides)
      XCTAssertEqual(DockPresence.styleMask(adopted, hidesOnMinimise: hides), adopted)
    }
  }

  func testAdoptedPlainWindowCannotMinimise() {
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

  /// The Agent window: an enabled minimise control whose every path hides the
  /// window and never leaves it miniaturised.
  func testHidingWindowHidesInsteadOfMiniaturising() {
    let window = HidingWindow(
      contentRect: NSRect(x: 0, y: 0, width: 320, height: 200),
      styleMask: documentMask, backing: .buffered, defer: false)
    window.isReleasedWhenClosed = false
    DockPresence.adopt(window)
    XCTAssertTrue(window.styleMask.contains(.miniaturizable))

    window.orderFront(nil)
    window.performMiniaturize(nil)
    XCTAssertFalse(window.isVisible, "the yellow button / ⌘M hides the window")
    XCTAssertFalse(window.isMiniaturized, "it never becomes an Exposé tile")

    window.orderFront(nil)
    window.miniaturize(nil)
    XCTAssertFalse(window.isVisible)
    XCTAssertFalse(window.isMiniaturized)

    window.orderFrontRegardless()
    XCTAssertTrue(window.isVisible, "the passive reveal brings a hidden window back")
    window.orderOut(nil)
  }

  /// The Agent window is created as a `HidingWindow`; a revert to a plain
  /// `NSWindow` would silently bring back either the dead button or the ghost.
  func testAgentWindowIsAHidingWindow() throws {
    let root = URL(fileURLWithPath: #filePath)
      .deletingLastPathComponent().deletingLastPathComponent()
    let app = try String(
      contentsOf: root.appendingPathComponent("Codescribe/App.swift"), encoding: .utf8)
    XCTAssertTrue(app.contains("HidingWindow(contentViewController:"))
  }
}
