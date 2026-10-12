import AppKit
import SwiftUI
import XCTest

@testable import Codescribe

/// Mounts the real Agent window composition. Symbols here exist on the
/// baseline, so root can run this file there: hidden title, unified toolbar,
/// sidebar material, and the divider stopping at the header all fail before
/// this cut. Geometry is measured on the live window, not from constants.
final class AgentWindowChromeTests: XCTestCase {
  private let sidebarKey = "AgentChat.sidebarExpanded.v1"

  func testIntegratedHeaderAtMinimumAndNormalWidths() {
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
          self.visibleControls(in: window, label: "Always on top").isEmpty == false
            && self.sidebarMaterial(in: window) != nil
        }

        XCTAssertEqual(window.title, "Agent", "width \(width) expanded \(expanded)")
        XCTAssertEqual(window.titleVisibility, .hidden, "width \(width) expanded \(expanded)")
        XCTAssertFalse(window.titlebarAppearsTransparent, "width \(width) expanded \(expanded)")
        XCTAssertEqual(window.toolbarStyle, .unified, "width \(width) expanded \(expanded)")

        let split = splitView(in: window)
        let sidebar = split?.splitViewItems.first { $0.behavior == .sidebar }
        XCTAssertEqual(sidebar?.behavior, .sidebar, "width \(width) expanded \(expanded)")
        XCTAssertEqual(
          sidebar?.minimumThickness, AgentSidebarMetrics.minimumWidth,
          "width \(width) expanded \(expanded)")
        XCTAssertFalse(
          sidebar?.allowsFullHeightLayout ?? true, "width \(width) expanded \(expanded)")
        XCTAssertFalse(
          split?.splitViewItems.last?.allowsFullHeightLayout ?? true,
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

        assertReachable(
          "Toggle Sidebar", in: window, band: true, label: "toggle \(width) \(expanded)")
        assertReachable(
          "Always on top", in: window, band: true, label: "pin \(width) \(expanded)")
        assertReachable(
          "More actions", in: window, band: true, label: "menu \(width) \(expanded)")
        assertReachable("Probe", in: window, band: true, label: "title \(width) \(expanded)")

        let pin = visibleControls(in: window, label: "Always on top")
        let menu = visibleControls(in: window, label: "More actions")
        let lights = trafficLights(in: window)
        for control in pin + menu {
          let controlFrame = frame(control)
          for light in lights {
            XCTAssertFalse(
              controlFrame.intersects(frame(light)),
              "control hits a traffic light at \(width) expanded \(expanded)")
          }
        }

        if expanded {
          let marks = visibleControls(in: window, text: "codescribe")
          XCTAssertEqual(marks.count, 1, "one wordmark at \(width)")
          assertInTitlebar(marks[0], window: window, label: "wordmark \(width)")
          let search = visibleControls(in: window, text: "Search threads")
          XCTAssertEqual(search.count, 1, "search at \(width)")
          let searchFrame = frame(search[0])
          XCTAssertLessThan(searchFrame.maxY, frame(pin[0]).minY)
          if let sidebarView = sidebar?.viewController.view, sidebarView.bounds.width > 1 {
            let edge = sidebarView.convert(
              NSPoint(x: sidebarView.bounds.maxX, y: sidebarView.bounds.midY), to: nil)
            XCTAssertGreaterThan(frame(pin[0]).minX, edge.x)
            XCTAssertGreaterThan(frame(menu[0]).minX, edge.x)
          }
        } else {
          XCTAssertTrue(
            visibleControls(in: window, text: "codescribe").isEmpty,
            "collapsed rail must not keep a wordmark at \(width)")
          let toggle = visibleControls(in: window, label: "Toggle Sidebar")
          let close = window.standardWindowButton(.closeButton)
          XCTAssertFalse(toggle.isEmpty)
          if let close, let toggle = toggle.first {
            let toggleFrame = frame(toggle)
            let closeFrame = frame(close)
            XCTAssertFalse(toggleFrame.intersects(closeFrame))
            XCTAssertGreaterThanOrEqual(toggleFrame.minX, closeFrame.maxX - 0.5)
          }
        }
      }
    }
  }

  func testSidebarMaterialFollowsReduceTransparencyAndIncreaseContrast() {
    let glass = mount(
      contentWidth: 1120,
      reduceTransparency: false,
      increaseContrast: false
    )
    defer {
      glass.orderOut(nil)
      glass.close()
    }
    pump(glass) { self.sidebarMaterial(in: glass) != nil }
    let plain = sidebarMaterial(in: glass)
    XCTAssertEqual(plain?.material, .sidebar)
    XCTAssertEqual(plain?.blendingMode, .behindWindow)
    XCTAssertEqual(plain?.isEmphasized, false)

    let reduced = mount(
      contentWidth: 1120,
      reduceTransparency: true,
      increaseContrast: false
    )
    defer {
      reduced.orderOut(nil)
      reduced.close()
    }
    pump(reduced) { self.sidebarMaterial(in: reduced)?.material == .contentBackground }
    let flat = sidebarMaterial(in: reduced)
    XCTAssertEqual(flat?.material, .contentBackground)
    XCTAssertEqual(flat?.blendingMode, .withinWindow)

    let contrast = mount(
      contentWidth: 1120,
      reduceTransparency: false,
      increaseContrast: true
    )
    defer {
      contrast.orderOut(nil)
      contrast.close()
    }
    pump(contrast) { self.sidebarMaterial(in: contrast)?.isEmphasized == true }
    let strong = sidebarMaterial(in: contrast)
    XCTAssertEqual(strong?.material, .sidebar)
    XCTAssertEqual(strong?.isEmphasized, true)
  }

  private func mount(
    contentWidth: CGFloat,
    reduceTransparency: Bool = false,
    increaseContrast: Bool = false
  ) -> NSWindow {
    let store = AgentChatStore(engine: nil)
    if let thread = store.currentThread {
      store.rename(thread, to: "Probe")
    }
    let root = AgentChatView(store: store)
      .environment(\.accessibilityReduceTransparency, reduceTransparency)
      .environment(\.colorSchemeContrast, increaseContrast ? .increased : .standard)
    let window = NSWindow(
      contentRect: NSRect(x: 80, y: 80, width: contentWidth, height: 720),
      styleMask: [.titled, .closable, .miniaturizable, .resizable, .fullSizeContentView],
      backing: .buffered,
      defer: false
    )
    window.contentViewController = NSHostingController(rootView: root)
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
    guard let theme = window.contentView?.superview else { return nil }
    return first(in: theme) { view in
      guard let effect = view as? NSVisualEffectView else { return false }
      return effect.material == .sidebar || effect.material == .contentBackground
    } as? NSVisualEffectView
  }

  private func visibleControls(in window: NSWindow, label: String) -> [NSView] {
    matching(in: window) { view in
      view.accessibilityLabel() == label
    }
  }

  private func visibleControls(in window: NSWindow, text: String) -> [NSView] {
    matching(in: window) { view in
      if let field = view as? NSTextField {
        return field.stringValue == text || field.placeholderString == text
      }
      if let button = view as? NSButton, button.title == text { return true }
      let value = view.accessibilityValue() as? String
      return value == text || view.accessibilityLabel() == text
        || view.accessibilityTitle() == text
    }
  }

  private func matching(in window: NSWindow, where matches: (NSView) -> Bool) -> [NSView] {
    guard let theme = window.contentView?.superview else { return [] }
    var hits: [NSView] = []
    func walk(_ view: NSView) {
      if matches(view) {
        let rect = frame(view)
        if rect.width > 1, rect.height > 1 { hits.append(view) }
      }
      for child in view.subviews { walk(child) }
    }
    walk(theme)
    return hits
  }

  private func assertReachable(
    _ label: String, in window: NSWindow, band: Bool, label note: String
  ) {
    let hits = visibleControls(in: window, label: label)
    if hits.isEmpty {
      let titled = visibleControls(in: window, text: label)
      XCTAssertFalse(titled.isEmpty, "missing \(note)")
      if let control = titled.first {
        if band { assertInTitlebar(control, window: window, label: note) }
        assertHit(control, in: window, label: note)
      }
      return
    }
    if band { assertInTitlebar(hits[0], window: window, label: note) }
    assertHit(hits[0], in: window, label: note)
  }

  private func assertInTitlebar(_ view: NSView, window: NSWindow, label: String) {
    let rect = frame(view)
    let windowBounds = NSRect(origin: .zero, size: window.frame.size)
    XCTAssertTrue(windowBounds.contains(rect), "\(label) leaves the window")
    XCTAssertGreaterThanOrEqual(
      rect.midY, window.contentLayoutRect.maxY - 1, "\(label) is below the titlebar")
  }

  private func assertHit(_ view: NSView, in window: NSWindow, label: String) {
    guard let theme = window.contentView?.superview else {
      XCTFail("no theme frame for \(label)")
      return
    }
    let rect = frame(view)
    let local = theme.convert(NSPoint(x: rect.midX, y: rect.midY), from: nil)
    let hit = theme.hitTest(local)
    var node = hit
    var reached = false
    while let current = node {
      if current === view || current.isDescendant(of: view) || view.isDescendant(of: current) {
        reached = true
        break
      }
      node = current.superview
    }
    XCTAssertTrue(reached, "hit test missed \(label)")
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
