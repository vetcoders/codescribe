import AppKit
import SwiftUI
import XCTest

@testable import Codescribe

/// The Settings window carries the same native chrome contract as the Agent
/// window: the sidebar material runs through the titlebar from the bottom
/// edge to the top of the window, the search field stays below the header,
/// and the split view keeps a sidebar column with the system toggle.
/// `SettingsChromeContractTests` reads source text; this one measures the
/// mounted window, as `AgentWindowChromeTests` does for Agent.
@MainActor
final class SettingsWindowChromeTests: XCTestCase {
  func testSidebarMaterialRunsThroughTheTitlebar() throws {
    let window = mount(contentWidth: 1120)
    defer {
      window.orderOut(nil)
      window.close()
    }
    let theme = try XCTUnwrap(window.contentView?.superview)

    pump(window) {
      self.sidebarMaterial(in: window) != nil
        && (window.toolbar?.items.count ?? 0) >= 1
    }

    XCTAssertEqual(window.toolbarStyle, .unified, "one unified header plane, as in Agent")

    let split = splitView(in: window)
    let controller = split?.delegate as? NSSplitViewController
    let sidebar = try XCTUnwrap(controller?.splitViewItems.first { $0.behavior == .sidebar })
    XCTAssertFalse(sidebar.isCollapsed)
    XCTAssertEqual(
      frame(sidebar.viewController.view).maxY, window.frame.height, accuracy: 1,
      "sidebar column must extend through the titlebar")
    XCTAssertEqual(
      frame(sidebar.viewController.view).minY, 0, accuracy: 1,
      "sidebar column must reach the bottom edge")

    let material = try XCTUnwrap(sidebarMaterial(in: window))
    XCTAssertEqual(material.material, .sidebar)
    XCTAssertEqual(material.blendingMode, .behindWindow)
    XCTAssertEqual(
      frame(material).maxY, window.frame.height, accuracy: 1,
      "sidebar material must run from the bottom to the top of the window")
    XCTAssertEqual(frame(material).minY, 0, accuracy: 1)

    let search = try XCTUnwrap(first(in: theme) { $0 is NSSearchField })
    XCTAssertLessThanOrEqual(
      frame(search).maxY, window.contentLayoutRect.maxY + 1,
      "search must sit below the header, not under it")

    let toggle = try XCTUnwrap(window.toolbar).items.first {
      $0.itemIdentifier.rawValue.localizedCaseInsensitiveContains("sidebar")
    }
    XCTAssertNotNil(toggle, "the split view supplies the system sidebar toggle")
  }

  private func mount(contentWidth: CGFloat) -> NSWindow {
    let model = SettingsViewModel(
      engine: MockSettingsEngine(), permissionProbe: MockPermissionProbe())
    let window = NSWindow(
      contentRect: NSRect(x: 80, y: 80, width: contentWidth, height: 760),
      styleMask: [.titled, .closable, .miniaturizable, .resizable, .fullSizeContentView],
      backing: .buffered,
      defer: false
    )
    window.isReleasedWhenClosed = false
    window.contentViewController = NSHostingController(rootView: SettingsView(model: model))
    window.setContentSize(NSSize(width: contentWidth, height: 760))
    window.setFrameOrigin(NSPoint(x: 80, y: 80))
    window.orderFrontRegardless()
    return window
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
    return first(in: content) { $0 is SettingsSidebarVisualEffectNSView } as? NSVisualEffectView
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
