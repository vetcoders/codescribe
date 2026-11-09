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

  func testSettingsNativeToggleReopensAndKeepsCollapsedHeaderCovered() throws {
    try assertNativeToggleRoundTrip(key: keys[0]) {
      self.mount(NSHostingController(rootView: SettingsView(model: .preview(.creator))))
    }
  }

  func testAgentNativeToggleReopensAndKeepsCollapsedHeaderCovered() throws {
    try assertNativeToggleRoundTrip(key: keys[1]) {
      self.show(
        AgentWindowChrome.makeWindow(
          content: NSHostingController(rootView: AgentChatView(store: AgentChatStore(engine: nil))))
      )
    }
  }

  /// Keep event routing, native action dispatch and material coverage separate:
  /// AXPress proves the action, while hitTest probes its local view route.
  private func assertNativeToggleRoundTrip(key: String, make: () -> NSWindow) throws {
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
    func pump(_ ready: () -> Bool) {
      let deadline = Date().addingTimeInterval(1.5)
      while Date() < deadline {
        RunLoop.current.run(until: Date().addingTimeInterval(0.02))
        window.displayIfNeeded()
        if ready() { return }
      }
    }
    pump { window.toolbar?.items.isEmpty == false }
    let root = try XCTUnwrap(window.contentView?.superview)
    let split = try XCTUnwrap(first(in: root) { $0 is NSSplitView } as? NSSplitView)
    let controller = try XCTUnwrap(split.delegate as? NSSplitViewController)
    let sidebar = try XCTUnwrap(controller.splitViewItems.first { $0.behavior == .sidebar })
    for collapsed in [true, false, true, false] {
      let item = try XCTUnwrap(
        window.toolbar?.items.first {
          $0.itemIdentifier.rawValue.localizedCaseInsensitiveContains("toggleSidebar")
        })
      let itemView = try XCTUnwrap(item.view)
      func accessibilityElements(_ object: Any) -> [any NSAccessibilityProtocol] {
        guard let element = object as? any NSAccessibilityProtocol else { return [] }
        let native = (object as? NSView)?.subviews ?? []
        return [element]
          + ((element.accessibilityChildren() ?? []) + native).flatMap(accessibilityElements)
      }
      let elements = accessibilityElements(itemView)
      let button = try XCTUnwrap(elements.first { $0.accessibilityRole() == .button })
      let screenFrame = button.accessibilityFrame()
      let point = window.convertPoint(fromScreen: NSPoint(x: screenFrame.midX, y: screenFrame.midY))
      let hit = root.hitTest(root.convert(point, from: nil))
      let routed = hit.map { $0 === itemView || $0.isDescendant(of: itemView) } ?? false
      let evidence = XCTAttachment(
        string:
          "key=\(key) nextCollapsed=\(collapsed) item=\(itemView.convert(itemView.bounds, to: nil)) "
          + "AXframe=\(screenFrame) hit=\(String(describing: hit)) "
          + "action=\(String(describing: item.action)) target=\(String(describing: item.target))")
      evidence.name = "Native sidebar toggle route"
      evidence.lifetime = .keepAlways
      add(evidence)
      XCTAssertTrue(routed, "the native sidebar item must receive its pointer target")
      XCTAssertTrue(button.isAccessibilityEnabled())
      XCTAssertTrue(button.accessibilityPerformPress(), "native toolbar accessibility action")
      pump { sidebar.isCollapsed == collapsed }
      XCTAssertEqual(sidebar.isCollapsed, collapsed, "native action must change the sidebar")
      guard sidebar.isCollapsed == collapsed else { return }
      if collapsed {
        let viewport = try XCTUnwrap(
          first(in: root) {
            $0.identifier?.rawValue == "codescribe-header-viewport"
          })
        let frame = viewport.convert(viewport.bounds, to: nil)
        var ancestors: [String] = []
        var ancestor: NSView? = viewport
        while let view = ancestor {
          ancestors.append(
            "\(type(of: view)) frame=\(view.convert(view.bounds, to: nil)) "
              + "visible=\(view.visibleRect) clips=\(view.clipsToBounds)")
          ancestor = view.superview
        }
        let materialEvidence = XCTAttachment(string: ancestors.joined(separator: "\n"))
        materialEvidence.name = "Collapsed header clipping ancestry"
        materialEvidence.lifetime = .keepAlways
        add(materialEvidence)
        XCTAssertEqual(
          frame.minX, 0, accuracy: 1, "collapsed titlebar needs material at its left edge")
        XCTAssertEqual(frame.maxX, window.frame.width, accuracy: 1)
      }
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
