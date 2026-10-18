import AppKit
import SwiftUI
import XCTest

@testable import Codescribe

/// Mounts both windows that share `CSSidebarColumn` and measures the native
/// list it builds. On 2026-10-18 an empty slot wrapped in `safeAreaInset`
/// claimed the rest of the column: Settings showed a search field over an
/// empty sidebar with its rows pushed under the footer, and the Agent list
/// lost its lower half. A hand-placed toggle also sat beside the system one.
@MainActor
final class CSSidebarColumnTests: XCTestCase {
  private let keys = ["Settings.sidebarExpanded.v1", "AgentChat.sidebarExpanded.v1"]

  func testEmptySlotsLeaveTheRowsWhereTheyBelong() throws {
    let previous = keys.map { UserDefaults.standard.object(forKey: $0) }
    defer {
      for (key, value) in zip(keys, previous) {
        if let value {
          UserDefaults.standard.set(value, forKey: key)
        } else {
          UserDefaults.standard.removeObject(forKey: key)
        }
      }
    }
    for key in keys { UserDefaults.standard.set(true, forKey: key) }

    let windows: [(String, NSViewController)] = [
      ("Settings", NSHostingController(rootView: SettingsView(model: .preview(.creator)))),
      ("Agent", NSHostingController(rootView: AgentChatView(store: AgentChatStore(engine: nil)))),
    ]
    for (name, host) in windows {
      let window = mount(host)
      defer { window.orderOut(nil) }
      let list = try XCTUnwrap(
        sidebarList(in: window, until: { $0.documentView.map { $0.frame.height > 0 } ?? false }),
        "\(name) sidebar list")
      // The toolbar, the search field and at most one slot sit above the
      // rows; the footer is the only thing below them.
      XCTAssertLessThan(list.contentInsets.top, 200, "\(name) rows start under the search field")
      XCTAssertLessThan(list.contentInsets.bottom, 120, "\(name) rows reach the bottom of the column")
      let rows = try XCTUnwrap(list.documentView as? NSTableView, "\(name) native rows")
      XCTAssertGreaterThan(rows.numberOfRows, 0, "\(name) has rows")
      let first = rows.convert(rows.rect(ofRow: 0), to: nil)
      XCTAssertGreaterThan(
        first.maxY, window.frame.height * 0.6, "\(name) first row is in the top part of the window")

      let toggles = try XCTUnwrap(window.toolbar).items.filter {
        $0.itemIdentifier.rawValue.localizedCaseInsensitiveContains("toggleSidebar")
          || $0.label.localizedCaseInsensitiveContains("sidebar")
      }
      XCTAssertEqual(toggles.count, 1, "\(name) has exactly one sidebar toggle")
    }
  }

  private func mount(_ host: NSViewController) -> NSWindow {
    let window = NSWindow(
      contentRect: NSRect(x: 80, y: 80, width: 1120, height: 720),
      styleMask: [.titled, .closable, .miniaturizable, .resizable, .fullSizeContentView],
      backing: .buffered,
      defer: false
    )
    window.isReleasedWhenClosed = false
    window.contentViewController = host
    window.setContentSize(NSSize(width: 1120, height: 720))
    window.setFrameOrigin(NSPoint(x: 80, y: 80))
    window.orderFrontRegardless()
    return window
  }

  /// The sidebar's own scroll view: the narrow one whose document is the
  /// native list.
  private func sidebarList(in window: NSWindow, until ready: (NSScrollView) -> Bool)
    -> NSScrollView?
  {
    let deadline = Date().addingTimeInterval(2)
    var found: NSScrollView?
    while Date() < deadline {
      RunLoop.current.run(until: Date().addingTimeInterval(0.02))
      window.displayIfNeeded()
      if let root = window.contentView?.superview ?? window.contentView {
        found = first(in: root) {
          guard let scroll = $0 as? NSScrollView else { return false }
          return scroll.documentView is NSTableView && scroll.frame.width <= 320
        } as? NSScrollView
      }
      if let found, ready(found) { break }
    }
    return found
  }

  private func first(in root: NSView, where matches: (NSView) -> Bool) -> NSView? {
    if matches(root) { return root }
    for child in root.subviews {
      if let found = first(in: child, where: matches) { return found }
    }
    return nil
  }
}
