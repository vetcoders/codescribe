import AppKit
import SwiftUI
import XCTest

@testable import Codescribe

/// Mounts the real Agent window composition. Symbols here exist on the
/// baseline, so root can run this file there: hidden title, unified toolbar,
/// full-height sidebar material and clear titlebar are measured on the live
/// window, with toolbar controls still clear of the traffic lights.
@MainActor
final class AgentWindowChromeTests: XCTestCase {
  private let sidebarKey = "AgentChat.sidebarExpanded.v1"

  func testIntegratedHeaderAtMinimumAndNormalWidths() throws {
    let previous = UserDefaults.standard.object(forKey: sidebarKey)
    defer { restoreSidebarDefault(previous) }

    for width in [AgentWindowMetrics.minWidth, 1120 as CGFloat] {
      for expanded in [true, false] {
        UserDefaults.standard.set(expanded, forKey: sidebarKey)
        let window = mount(contentWidth: width)
        defer {
          window.orderOut(nil)
          window.close()
        }
        let theme = window.contentView?.superview
        XCTAssertNotNil(theme, "width \(width) expanded \(expanded)")

        pump(window) {
          (window.toolbar?.items.compactMap(\.view).count ?? 0) >= 3
            && (!expanded || self.sidebarMaterial(in: window) != nil)
            && window.title == "Agent"
            && window.titleVisibility == .hidden
            && window.toolbarStyle == .unified
        }

        XCTAssertEqual(window.title, "Agent", "width \(width) expanded \(expanded)")
        XCTAssertEqual(window.titleVisibility, .hidden, "width \(width) expanded \(expanded)")
        XCTAssertTrue(window.titlebarAppearsTransparent, "width \(width) expanded \(expanded)")
        XCTAssertEqual(window.toolbarStyle, .unified, "width \(width) expanded \(expanded)")

        let split = splitView(in: window)
        let controller = split?.delegate as? NSSplitViewController
        let sidebar = controller?.splitViewItems.first { $0.behavior == .sidebar }
        XCTAssertEqual(sidebar?.behavior, .sidebar, "width \(width) expanded \(expanded)")
        XCTAssertEqual(
          sidebar?.minimumThickness, AgentSidebarMetrics.minimumWidth,
          "width \(width) expanded \(expanded)")
        XCTAssertEqual(sidebar?.isCollapsed, !expanded, "width \(width) expanded \(expanded)")
        XCTAssertEqual(window.contentMinSize.width, 640)
        if expanded, let sidebar {
          XCTAssertEqual(
            frame(sidebar.viewController.view).maxY, window.frame.height, accuracy: 1,
            "sidebar must extend through the titlebar at \(width)")
          XCTAssertEqual(
            frame(sidebar.viewController.view).minY, 0, accuracy: 1,
            "sidebar must reach the bottom edge at \(width)")
          let search = try XCTUnwrap(first(in: try XCTUnwrap(theme)) { $0 is NSSearchField })
          XCTAssertLessThanOrEqual(
            frame(search).maxY, window.contentLayoutRect.maxY + 1,
            "full-height material must not move search under the toolbar")
        }

        if expanded {
          let material = try XCTUnwrap(sidebarMaterial(in: window))
          XCTAssertEqual(material.material, .sidebar, "width \(width) expanded \(expanded)")
          XCTAssertEqual(
            material.blendingMode, .behindWindow, "width \(width) expanded \(expanded)")
          XCTAssertEqual(frame(material).maxY, window.frame.height, accuracy: 1)
          XCTAssertEqual(frame(material).minY, 0, accuracy: 1)
        }

        // SwiftUI toolbar children are not projected through AppKit AX in
        // XCTest. Measure the public NSToolbarItem views instead of treating
        // absent AX children as absent production controls.
        let controls = try XCTUnwrap(window.toolbar).items.compactMap(\.view)
        XCTAssertGreaterThanOrEqual(controls.count, 3, "system toggle joins the native clusters")
        XCTAssertGreaterThanOrEqual(
          controls.filter { frame($0).height >= 51 }.count, 3,
          "brand, title and actions share the three-line native header rhythm")
        let bounds = NSRect(origin: .zero, size: window.frame.size)
        for control in controls {
          let rect = frame(control)
          XCTAssertGreaterThan(rect.width, 1)
          XCTAssertGreaterThan(rect.height, 1)
          XCTAssertTrue(bounds.contains(rect), "toolbar cluster leaves window: \(rect)")
          XCTAssertGreaterThanOrEqual(rect.midY, window.contentLayoutRect.maxY - 1)
          for light in trafficLights(in: window) {
            XCTAssertFalse(rect.intersects(frame(light)), "cluster overlaps traffic lights")
          }
          let parent = try XCTUnwrap(control.superview)
          let point = parent.convert(NSPoint(x: rect.midX, y: rect.midY), from: nil)
          XCTAssertNotNil(control.hitTest(point), "cluster must retain native hit testing")
        }

      }
    }
  }

  func testSidebarMaterialFollowsReduceTransparencyAndIncreaseContrast() throws {
    for (reduced, contrast) in [(false, false), (true, false), (false, true)] {
      let host = NSHostingView(
        rootView: SettingsSidebarMaterial(
          reduceTransparency: reduced, contrast: contrast ? .increased : .standard))
      let window = NSWindow(
        contentRect: NSRect(x: 80, y: 80, width: 300, height: 400),
        styleMask: [.titled], backing: .buffered, defer: false)
      window.isReleasedWhenClosed = false
      window.contentView = host
      window.orderFrontRegardless()
      defer { window.close() }
      pump(window) { host.fittingSize.height > 0 }
      if reduced || contrast {
        XCTAssertNil(sidebarMaterial(in: window), "shared policy uses an opaque system canvas")
      } else {
        let effect = try XCTUnwrap(sidebarMaterial(in: window))
        XCTAssertEqual(effect.material, .sidebar)
        XCTAssertEqual(effect.blendingMode, .behindWindow)
        XCTAssertNil(effect.hitTest(NSPoint(x: 10, y: 10)))
      }
    }
  }

  func testMessagesExtendUnderTheFloatingComposer() throws {
    let previous = UserDefaults.standard.object(forKey: sidebarKey)
    UserDefaults.standard.set(true, forKey: sidebarKey)
    defer { restoreSidebarDefault(previous) }
    let messages = (0..<30).map {
      ChatMessage(
        role: .assistant, timestamp: nil,
        text: "Turn \($0). " + String(repeating: "words ", count: 25))
    }
    let store = AgentChatStore(
      engine: nil, threads: [ChatThread(title: "Scroll fixture", meta: "", messages: messages)])
    let window = mount(contentWidth: 1120, store: store)
    defer { window.close() }
    let root = try XCTUnwrap(window.contentView)
    pump(window) {
      self.first(in: root) {
        ($0 as? NSTextView)?.accessibilityIdentifier() == "agent-composer-text"
      }
        != nil
    }
    let input = try XCTUnwrap(
      first(in: root) { ($0 as? NSTextView)?.accessibilityIdentifier() == "agent-composer-text" })
    let scroll = try XCTUnwrap(
      first(in: root) { view in
        guard let scroll = view as? NSScrollView, scroll.frame.height > 200 else { return false }
        return self.first(in: scroll) { $0 is NSTableView } == nil
      } as? NSScrollView)
    XCTAssertGreaterThan(scroll.documentView?.bounds.height ?? 0, scroll.contentSize.height)
    XCTAssertLessThan(
      frame(scroll).minY, frame(input).maxY,
      "conversation scroll surface must continue beneath the input, not stop above its strip")
    XCTAssertGreaterThanOrEqual(
      frame(scroll).maxY, window.contentLayoutRect.maxY,
      "messages must continue under the native header")
  }

  private func mount(contentWidth: CGFloat, store: AgentChatStore? = nil)
    -> NSWindow
  {
    let store = store ?? AgentChatStore(engine: nil)
    if let thread = store.currentThread {
      store.rename(thread, to: "Probe")
    }
    let window = NSWindow(
      contentRect: NSRect(x: 80, y: 80, width: contentWidth, height: 720),
      styleMask: [.titled, .closable, .miniaturizable, .resizable, .fullSizeContentView],
      backing: .buffered,
      defer: false
    )
    window.isReleasedWhenClosed = false
    window.contentViewController = NSHostingController(rootView: AgentChatView(store: store))
    window.setContentSize(NSSize(width: contentWidth, height: 720))
    window.setFrameOrigin(NSPoint(x: 80, y: 80))
    window.orderFrontRegardless()
    return window
  }

  private func restoreSidebarDefault(_ previous: Any?) {
    if let previous {
      UserDefaults.standard.set(previous, forKey: sidebarKey)
    } else {
      UserDefaults.standard.removeObject(forKey: sidebarKey)
    }
  }

  private func pump(_ window: NSWindow, until predicate: () -> Bool) {
    let deadline = Date().addingTimeInterval(2)
    while Date() < deadline {
      RunLoop.current.run(until: Date().addingTimeInterval(0.02))
      window.displayIfNeeded()
      if predicate() { return }
    }
  }

  private func splitView(in window: NSWindow) -> NSSplitView? {
    guard let content = window.contentView else { return nil }
    return first(in: content) { $0 is NSSplitView } as? NSSplitView
  }

  private func sidebarMaterial(in window: NSWindow) -> NSVisualEffectView? {
    guard let content = window.contentView else { return nil }
    return first(in: content) { view in
      view is SettingsSidebarVisualEffectNSView
    } as? NSVisualEffectView
  }

  private func trafficLights(in window: NSWindow) -> [NSView] {
    [NSWindow.ButtonType.closeButton, .miniaturizeButton, .zoomButton].compactMap {
      window.standardWindowButton($0)
    }
  }

  private func frame(_ view: NSView) -> NSRect {
    view.convert(view.bounds, to: nil)
  }

  private func first(in root: NSView, where matches: (NSView) -> Bool) -> NSView? {
    if matches(root) { return root }
    for child in root.subviews {
      if let found = first(in: child, where: matches) { return found }
    }
    return nil
  }
}
