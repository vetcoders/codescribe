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
///
/// What this mount cannot witness: the Settings window in the app is a SwiftUI
/// `Window` scene, and only a scene paints a visible window-toolbar background
/// as a strip across the whole titlebar width (the 2026-10-14 cut beneath the
/// titlebar). A hosting-controller mount never paints that strip, with or
/// without the background, and SwiftUI's toolbar bridging also rewrites
/// `NSWindow.toolbarStyle` here after `adoptHostWindow`, so neither is
/// asserted. The scene-side evidence is the probe in the 2026-10-14 report.
@MainActor
final class SettingsWindowChromeTests: XCTestCase {
  func testContinuousHeaderTracksTheSettingsSidebarAtBothWidths() throws {
    let key = "Settings.sidebarExpanded.v1"
    let previous = UserDefaults.standard.object(forKey: key)
    defer {
      if let previous {
        UserDefaults.standard.set(previous, forKey: key)
      } else {
        UserDefaults.standard.removeObject(forKey: key)
      }
    }
    for width in [880 as CGFloat, 1120] {
      for expanded in [true, false] {
        UserDefaults.standard.set(expanded, forKey: key)
        let window = mount(contentWidth: width)
        defer { window.close() }
        pump(window) {
          guard let root = window.contentView?.superview,
            let glass = self.first(
              in: root,
              where: {
                $0.identifier?.rawValue == "codescribe-header-glass"
              }),
            let sidebar = (self.splitView(in: window)?.delegate as? NSSplitViewController)?
              .splitViewItems.first
          else { return false }
          return sidebar.isCollapsed == !expanded
            && abs(self.frame(glass).maxX - window.frame.width) < 1
        }
        let root = try XCTUnwrap(window.contentView?.superview)
        let glass = try XCTUnwrap(
          first(in: root) {
            $0.identifier?.rawValue == "codescribe-header-glass"
          } as? NSGlassEffectView)
        let rect = frame(glass)
        XCTAssertEqual(
          window.titlebarAccessoryViewControllers.filter { $0.layoutAttribute == .top }.count, 1)
        XCTAssertEqual(rect.maxX, window.frame.width, accuracy: 1)
        XCTAssertEqual(rect.maxY, window.frame.height, accuracy: 1)
        XCTAssertEqual(rect.height, 52, accuracy: 1)
        XCTAssertEqual(rect.minY, window.contentLayoutRect.maxY, accuracy: 1)
        if expanded {
          let sidebar = try XCTUnwrap(
            (splitView(in: window)?.delegate as? NSSplitViewController)?.splitViewItems.first)
          XCTAssertEqual(rect.minX, frame(sidebar.viewController.view).maxX, accuracy: 1)
        }
        for type in [NSWindow.ButtonType.closeButton, .miniaturizeButton, .zoomButton] {
          let control = try XCTUnwrap(window.standardWindowButton(type))
          XCTAssertFalse(rect.intersects(frame(control)))
        }
        let content = try XCTUnwrap(glass.contentView)
        XCTAssertNotNil(content.hitTest(NSPoint(x: rect.width / 2, y: rect.height / 2)))
      }
    }
  }

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
    XCTAssertNil(
      first(in: theme) { $0 is CSSidebarVisualEffectNSView },
      "no second visual-effect material inside the native sidebar")
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
    window.contentViewController = NSHostingController(
      rootView: SettingsView(model: model)
        .environment(\.csReduceTransparencyOverride, false)
        .environment(\.csContrastOverride, .standard)
    )
    window.setContentSize(NSSize(width: contentWidth, height: 760))
    window.setFrameOrigin(NSPoint(x: 80, y: 80))
    BackgroundTestWindow.present(window)
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

  private func sidebarMaterial(in window: NSWindow) -> NSGlassEffectView? {
    guard let content = window.contentView else { return nil }
    return first(in: content) { $0 is NSGlassEffectView } as? NSGlassEffectView
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
