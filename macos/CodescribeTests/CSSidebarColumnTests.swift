import AppKit
import SwiftUI
import XCTest

@testable import Codescribe

/// Mounts both windows that share `CSSidebarColumn` and measures the native
/// list it builds. On 2026-10-18 an empty slot wrapped in `safeAreaInset`
/// claimed the rest of the column: Settings showed a search field over an
/// empty sidebar with its rows pushed under the footer, and the Agent list
/// lost its lower half. A hand-placed toggle also sat beside the system one.
/// The Agent window is built by the app's own factory: mounted in a plain
/// window it showed the system toggle while the shipped window had none.
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

    // Settings is a SwiftUI scene the test cannot open, so it is mounted in a
    // plain window; the Agent window comes from the factory the app uses.
    let windows: [(String, () -> NSWindow)] = [
      (
        "Settings",
        { self.mount(NSHostingController(rootView: SettingsView(model: .preview(.creator)))) }
      ),
      (
        "Agent",
        {
          self.show(
            AgentWindowChrome.makeWindow(
              content: NSHostingController(
                rootView: AgentChatView(store: AgentChatStore(engine: nil)))))
        }
      ),
    ]
    for (name, make) in windows {
      let window = make()
      defer { window.orderOut(nil) }
      let list = try XCTUnwrap(
        sidebarList(in: window, until: { $0.documentView.map { $0.frame.height > 0 } ?? false }),
        "\(name) sidebar list")
      // The toolbar, the search field and at most one slot sit above the
      // rows; the footer is the only thing below them.
      XCTAssertLessThan(list.contentInsets.top, 200, "\(name) rows start under the search field")
      XCTAssertLessThan(
        list.contentInsets.bottom, 120, "\(name) rows reach the bottom of the column")
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

  func testSettingsToolbarPointerRouteSurvivesTopAccessory() throws {
    try assertNativeToolbarRoute(key: keys[0]) {
      self.mount(NSHostingController(rootView: SettingsView(model: .preview(.creator))))
    }
  }

  func testAgentToolbarPointerRouteSurvivesTopAccessory() throws {
    try assertNativeToolbarRoute(key: keys[1]) {
      self.show(
        AgentWindowChrome.makeWindow(
          content: NSHostingController(rootView: AgentChatView(store: AgentChatStore(engine: nil))))
      )
    }
  }

  /// A controlled removal of our own public accessory distinguishes routing
  /// obstruction from an unavailable synthetic button/action in this fixture.
  private func assertNativeToolbarRoute(key: String, make: () -> NSWindow) throws {
    let previous = UserDefaults.standard.object(forKey: key)
    UserDefaults.standard.set(true, forKey: key)
    let window = make()
    defer {
      window.close()
      if let previous {
        UserDefaults.standard.set(previous, forKey: key)
      } else {
        UserDefaults.standard.removeObject(forKey: key)
      }
    }
    func pump() {
      let deadline = Date().addingTimeInterval(0.35)
      while Date() < deadline {
        RunLoop.current.run(until: Date().addingTimeInterval(0.02))
        window.displayIfNeeded()
      }
    }
    func route(_ phase: String) throws -> Bool {
      let root = try XCTUnwrap(window.contentView?.superview)
      let item = try XCTUnwrap(
        window.toolbar?.items.first {
          $0.itemIdentifier.rawValue.localizedCaseInsensitiveContains("toggleSidebar")
        })
      let view = try XCTUnwrap(item.view)
      let point = view.convert(NSPoint(x: view.bounds.midX, y: view.bounds.midY), to: root)
      let hit = root.hitTest(point)
      let isNative = hit.map { $0 === view || $0.isDescendant(of: view) } ?? false
      var ancestry: [String] = []
      var ancestor = hit
      while let current = ancestor {
        ancestry.append("\(type(of: current)) frame=\(current.convert(current.bounds, to: nil))")
        ancestor = current.superview
      }
      let attachment = XCTAttachment(
        string:
          "phase=\(phase) item=\(view.convert(view.bounds, to: nil)) native=\(isNative) "
          + "hit=\(String(describing: hit))\n" + ancestry.joined(separator: "\n"))
      attachment.name = "Sidebar pointer route \(phase)"
      attachment.lifetime = .keepAlways
      add(attachment)
      return isNative
    }
    pump()
    let before = try route("with-accessory")
    let index = try XCTUnwrap(
      window.titlebarAccessoryViewControllers.firstIndex {
        $0.layoutAttribute == .top
      })
    let owner = window.titlebarAccessoryViewControllers[index]
    window.removeTitlebarAccessoryViewController(at: index)
    pump()
    let without = try route("without-accessory")
    window.addTitlebarAccessoryViewController(owner)
    pump()
    let restored = try route("restored-accessory")
    XCTAssertTrue(without, "negative control: native toolbar target without our accessory")
    XCTAssertTrue(before, "our header must leave the system toolbar pointer target reachable")
    XCTAssertTrue(restored, "reattaching our header must preserve the native pointer target")
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
    return show(window)
  }

  private func show(_ window: NSWindow) -> NSWindow {
    window.setFrameOrigin(NSPoint(x: 80, y: 80))
    BackgroundTestWindow.present(window)
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
        found =
          first(in: root) {
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
