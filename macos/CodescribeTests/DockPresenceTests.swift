import AppKit
import XCTest

@testable import Codescribe

/// The Settings and Agent windows never miniaturise: a miniaturised Codescribe
/// window is an empty tile in App Exposé and Mission Control, with or without a
/// Dock icon. Both keep their minimise control and hide instead: the Agent
/// window through its own class (Founder decision, 2026-10-10), the
/// SwiftUI-owned Settings window through the hider `DockPresence` installs on
/// adoption (Founder decision, 2026-10-20).
@MainActor
final class DockPresenceTests: XCTestCase {
  private let documentMask: NSWindow.StyleMask = [
    .titled, .closable, .miniaturizable, .resizable, .fullSizeContentView,
  ]

  func testStyleMaskKeepsTheMinimiseControlOnEveryAdoptedWindow() {
    let plain: NSWindow.StyleMask = [.titled, .closable, .resizable, .fullSizeContentView]
    let mask = DockPresence.styleMask(plain)
    XCTAssertTrue(mask.contains(.miniaturizable), "the yellow button is back on Settings")
    XCTAssertEqual(DockPresence.styleMask(documentMask), documentMask)
    for mask in [mask, DockPresence.styleMask(documentMask)] {
      XCTAssertTrue(mask.contains(.titled))
      XCTAssertTrue(mask.contains(.closable))
      XCTAssertTrue(mask.contains(.resizable))
      XCTAssertTrue(mask.contains(.fullSizeContentView))
    }
  }

  func testStyleMaskIsIdempotent() {
    let adopted = DockPresence.styleMask(documentMask)
    XCTAssertEqual(DockPresence.styleMask(adopted), adopted)
  }

  private func plainWindow() -> NSWindow {
    let window = NSWindow(
      contentRect: NSRect(x: 0, y: 0, width: 320, height: 200),
      styleMask: [.titled, .closable, .resizable], backing: .buffered, defer: false)
    // Closing it would release it a second time under the default
    // `isReleasedWhenClosed`; the tests order it out instead.
    window.isReleasedWhenClosed = false
    return window
  }

  /// The Settings window: SwiftUI owns its class, so the minimise control is
  /// retargeted at the hider and the yellow button orders the window out.
  func testAdoptedPlainWindowKeepsAMinimiseControlThatHides() throws {
    let window = plainWindow()
    DockPresence.adopt(window)
    XCTAssertTrue(window.styleMask.contains(.miniaturizable))
    XCTAssertTrue(window.styleMask.contains(.closable))
    XCTAssertTrue(window.styleMask.contains(.resizable))
    XCTAssertTrue(DockPresence.hidesOnMinimise(window))

    let button = try XCTUnwrap(window.standardWindowButton(.miniaturizeButton))
    XCTAssertTrue(button.isEnabled)
    window.orderFront(nil)
    XCTAssertTrue(window.isVisible)
    XCTAssertTrue(NSApp.sendAction(try XCTUnwrap(button.action), to: button.target, from: button))
    XCTAssertFalse(window.isVisible, "the yellow button hides the window")
    XCTAssertFalse(window.isMiniaturized, "it never becomes an Exposé tile")
  }

  /// Minimize All and the title-bar double-click preference reach AppKit's own
  /// miniaturise; the hider undoes it into a hide at once.
  func testAdoptedPlainWindowMiniaturiseEndsHidden() {
    let window = plainWindow()
    DockPresence.adopt(window)
    window.orderFront(nil)

    window.miniaturize(nil)

    XCTAssertFalse(window.isMiniaturized, "no path leaves an Exposé tile")
    XCTAssertFalse(window.isVisible)
    window.orderOut(nil)
  }

  /// `SettingsView` adopts on every `viewDidMoveToWindow`; one hider per window.
  func testAdoptIsIdempotentForAPlainWindow() throws {
    let window = plainWindow()
    DockPresence.adopt(window)
    let first = try XCTUnwrap(window.standardWindowButton(.miniaturizeButton)?.target)
    DockPresence.adopt(window)
    let second = try XCTUnwrap(window.standardWindowButton(.miniaturizeButton)?.target)
    XCTAssertTrue(first === second)
    XCTAssertTrue(DockPresence.hidesOnMinimise(window))
  }

  /// A window the app never adopted keeps AppKit's native minimise.
  func testUnadoptedWindowIsNotClaimed() {
    let window = plainWindow()
    XCTAssertFalse(DockPresence.hidesOnMinimise(window))
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
    let window = AgentWindowChrome.makeWindow(content: NSViewController())
    XCTAssertTrue(window is HidingWindow)
    XCTAssertTrue(window.styleMask.contains(.miniaturizable))
    let root = URL(fileURLWithPath: #filePath)
      .deletingLastPathComponent().deletingLastPathComponent()
    let app = try String(
      contentsOf: root.appendingPathComponent("Codescribe/App.swift"), encoding: .utf8)
    XCTAssertTrue(app.contains("AgentWindowChrome.makeWindow(content:"))
  }
}
