import AppKit
import SwiftUI
import XCTest

@testable import Codescribe

/// Mounts the real Agent window composition. Symbols here exist on the
/// baseline, so root can run this file there: hidden title, unified toolbar,
/// sidebar material, and the divider stopping at the header all fail before
/// this cut. Geometry is measured on the live window, not from constants.
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
          window.toolbar?.items.compactMap(\.view).count == 3
            && self.sidebarMaterial(in: window) != nil
            && window.title == "Agent"
            && window.titleVisibility == .hidden
            && window.toolbarStyle == .unified
        }

        XCTAssertEqual(window.title, "Agent", "width \(width) expanded \(expanded)")
        XCTAssertEqual(window.titleVisibility, .hidden, "width \(width) expanded \(expanded)")
        XCTAssertFalse(window.titlebarAppearsTransparent, "width \(width) expanded \(expanded)")
        XCTAssertEqual(window.toolbarStyle, .unified, "width \(width) expanded \(expanded)")

        let split = splitView(in: window)
        let controller = split?.delegate as? NSSplitViewController
        let sidebar = controller?.splitViewItems.first { $0.behavior == .sidebar }
        XCTAssertEqual(sidebar?.behavior, .sidebar, "width \(width) expanded \(expanded)")
        XCTAssertEqual(
          sidebar?.minimumThickness, AgentSidebarMetrics.minimumWidth,
          "width \(width) expanded \(expanded)")
        XCTAssertFalse(
          sidebar?.allowsFullHeightLayout ?? true, "width \(width) expanded \(expanded)")
        XCTAssertFalse(
          controller?.splitViewItems.last?.allowsFullHeightLayout ?? true,
          "width \(width) expanded \(expanded)")
        XCTAssertEqual(sidebar?.isCollapsed, !expanded, "width \(width) expanded \(expanded)")
        if let split {
          XCTAssertLessThanOrEqual(
            frame(split).maxY,
            window.contentLayoutRect.maxY + 1,
            "divider runs through the titlebar at \(width) expanded \(expanded)")
        }

        let material = sidebarMaterial(in: window)
        XCTAssertEqual(material?.material, .sidebar, "width \(width) expanded \(expanded)")
        XCTAssertEqual(material?.blendingMode, .behindWindow, "width \(width) expanded \(expanded)")

        // SwiftUI toolbar children are not projected through AppKit AX in
        // XCTest. Measure the public NSToolbarItem views instead of treating
        // absent AX children as absent production controls.
        let controls = try XCTUnwrap(window.toolbar).items.compactMap(\.view)
        XCTAssertEqual(controls.count, 3, "navigation, identity and actions clusters")
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
        rootView: AgentSidebarMaterial(
          reduceTransparency: reduced, increaseContrast: contrast))
      let window = NSWindow(
        contentRect: NSRect(x: 80, y: 80, width: 300, height: 400),
        styleMask: [.titled], backing: .buffered, defer: false)
      window.isReleasedWhenClosed = false
      window.contentView = host
      window.orderFrontRegardless()
      defer { window.close() }
      pump(window) { self.sidebarMaterial(in: window) != nil }
      let effect = try XCTUnwrap(sidebarMaterial(in: window))
      XCTAssertEqual(effect.material, reduced ? .contentBackground : .sidebar)
      XCTAssertEqual(effect.blendingMode, reduced ? .withinWindow : .behindWindow)
      XCTAssertEqual(effect.isEmphasized, contrast)
    }
  }

  private func mount(contentWidth: CGFloat) -> NSWindow {
    let store = AgentChatStore(engine: nil)
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
      guard let effect = view as? NSVisualEffectView else { return false }
      return effect.material == .sidebar || effect.material == .contentBackground
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
