import AppKit
import SwiftUI
import XCTest

@testable import Codescribe

/// Native regression suite for the sidebar material behind the shared sidebar
/// column of the Settings and Agent windows: translucency, hit-testing
/// transparency, and accessibility contrast contracts. The column keeps the
/// system glass (Monika, 2026-10-18).
@MainActor
final class CSSidebarMaterialTests: XCTestCase {

  // MARK: - Material Invariants

  func testCSSidebarVisualEffectViewConfiguresNativeSidebarMaterial() async throws {
    let host = NSHostingView(rootView: CSSidebarVisualEffectView())
    let window = NSWindow(
      contentRect: NSRect(x: 0, y: 0, width: 220, height: 600),
      styleMask: [.titled, .closable, .miniaturizable, .resizable],
      backing: .buffered,
      defer: false
    )
    window.isReleasedWhenClosed = false
    window.contentView = host
    defer { window.close() }

    host.layoutSubtreeIfNeeded()
    await nextMainQueueTurn()

    let view = try XCTUnwrap(findView(of: CSSidebarVisualEffectNSView.self, in: host))
    XCTAssertEqual(
      view.material, .sidebar,
      "Settings sidebar must use native macOS .sidebar material")
    XCTAssertEqual(
      view.blendingMode, .behindWindow,
      "Settings sidebar must blend behindWindow for native translucency")
    XCTAssertEqual(
      view.state, .followsWindowActiveState,
      "Settings sidebar material must respond to window key and active activation states")
  }

  func testCSSidebarVisualEffectViewDropsHitTesting() {
    let view = CSSidebarVisualEffectNSView(
      frame: NSRect(x: 0, y: 0, width: 220, height: 600))

    // Must return nil at all coordinates so clicks and scrolls pass through
    // directly to the SwiftUI List rows, search field, and health footer.
    XCTAssertNil(view.hitTest(NSPoint(x: 0, y: 0)))
    XCTAssertNil(view.hitTest(NSPoint(x: 110, y: 300)))
    XCTAssertNil(view.hitTest(NSPoint(x: 219, y: 599)))
  }

  // MARK: - Accessibility Policy

  func testReduceTransparencyAndIncreaseContrastPolicy() {
    XCTAssertFalse(
      CSSidebarMaterial.shouldReduceTransparency(
        reduceTransparency: false,
        contrast: .standard
      ),
      "Normal conditions must preserve translucency"
    )

    XCTAssertTrue(
      CSSidebarMaterial.shouldReduceTransparency(
        reduceTransparency: true,
        contrast: .standard
      ),
      "Reduce Transparency accessibility setting must force opaque canvas"
    )

    XCTAssertTrue(
      CSSidebarMaterial.shouldReduceTransparency(
        reduceTransparency: false,
        contrast: .increased
      ),
      "Increase Contrast accessibility setting must force opaque canvas"
    )

    XCTAssertTrue(
      CSSidebarMaterial.shouldReduceTransparency(
        reduceTransparency: true,
        contrast: .increased
      ),
      "Both accessibility flags active must force opaque canvas"
    )
  }

  // MARK: - Hierarchy Mounting

  func testSidebarMaterialMountsVisualEffectViewInNormalConditions() async {
    let materialView = CSSidebarMaterial(
      reduceTransparency: false,
      contrast: .standard
    )
    let host = NSHostingView(rootView: materialView)
    host.frame = NSRect(x: 0, y: 0, width: 220, height: 600)
    host.layoutSubtreeIfNeeded()
    await nextMainQueueTurn()

    let visualEffect = findView(of: NSVisualEffectView.self, in: host)
    XCTAssertNotNil(
      visualEffect,
      "Normal sidebar material must mount an NSVisualEffectView in the view hierarchy")
    XCTAssertEqual(visualEffect?.material, .sidebar)
    XCTAssertEqual(visualEffect?.blendingMode, .behindWindow)
    XCTAssertEqual(visualEffect?.state, .followsWindowActiveState)
  }

  func testSidebarMaterialFallsBackToOpaqueUnderReducedTransparency() async {
    let materialView = CSSidebarMaterial(
      reduceTransparency: true,
      contrast: .standard
    )
    let host = NSHostingView(rootView: materialView)
    host.frame = NSRect(x: 0, y: 0, width: 220, height: 600)
    host.layoutSubtreeIfNeeded()
    await nextMainQueueTurn()

    let visualEffect = findView(of: NSVisualEffectView.self, in: host)
    XCTAssertNil(
      visualEffect,
      "Reduce Transparency must replace visual effect view with an opaque solid canvas")
  }

  func testSidebarMaterialFallsBackToOpaqueUnderIncreasedContrast() async {
    let materialView = CSSidebarMaterial(
      reduceTransparency: false,
      contrast: .increased
    )
    let host = NSHostingView(rootView: materialView)
    host.frame = NSRect(x: 0, y: 0, width: 220, height: 600)
    host.layoutSubtreeIfNeeded()
    await nextMainQueueTurn()

    let visualEffect = findView(of: NSVisualEffectView.self, in: host)
    XCTAssertNil(
      visualEffect,
      "Increase Contrast must replace visual effect view with an opaque solid canvas")
  }

  // MARK: - Window Adoption & Chrome Contract

  func testSettingsWindowAdoptionConfiguresTranslucencyBacking() async {
    let model = SettingsViewModel.preview(.creator)
    let settingsView = SettingsView(model: model)
    let host = NSHostingView(rootView: settingsView)
    let window = NSWindow(
      contentRect: NSRect(x: 0, y: 0, width: 880, height: 620),
      styleMask: [.titled, .closable, .miniaturizable, .resizable],
      backing: .buffered,
      defer: false
    )
    window.isReleasedWhenClosed = false
    window.contentView = host
    window.orderFrontRegardless()
    defer { window.close() }

    host.layoutSubtreeIfNeeded()
    await nextMainQueueTurn()
    await nextMainQueueTurn()

    // adoptHostWindow runs via HostingWindowReader
    XCTAssertEqual(window.titleVisibility, .hidden, "Toolbar wordmark serves as visible title")
    XCTAssertTrue(
      window.titlebarAppearsTransparent,
      "Titlebar must appear transparent for full-height sidebar material flow")
    XCTAssertTrue(
      window.styleMask.contains(.fullSizeContentView),
      "Window must use fullSizeContentView so sidebar material extends under titlebar")
    XCTAssertFalse(
      window.isOpaque,
      "Window must not be opaque so .behindWindow blending can sample desktop wallpaper")
    XCTAssertEqual(
      window.backgroundColor, .clear,
      "Window backgroundColor must be clear to avoid blocking behind-window translucency")
  }

  func testBothWindowsRunTheMaterialUnderTheTitlebar() throws {
    let codescribe = URL(fileURLWithPath: #filePath)
      .deletingLastPathComponent().deletingLastPathComponent()
      .appendingPathComponent("Codescribe")
    let column = try String(
      contentsOf: codescribe.appendingPathComponent("DesignSystem/CSSidebarColumn.swift"),
      encoding: .utf8)
    XCTAssertTrue(
      column.contains(".background { CSSidebarMaterial().ignoresSafeArea() }"),
      "the shared column backs every sidebar with the material from the bottom edge to the top")
    for path in ["Screens/Settings/SettingsView.swift", "Screens/AgentChat/ThreadRail.swift"] {
      let source = try String(
        contentsOf: codescribe.appendingPathComponent(path), encoding: .utf8)
      XCTAssertTrue(
        source.contains("CSSidebarColumn("), "\(path) builds its sidebar from the shared column")
      XCTAssertFalse(
        source.contains("CSSidebarMaterial()"), "\(path) never reaches for the material on its own")
    }
  }

  // MARK: - Appearance Adaptation

  func testSettingsHeaderKeepsTheSharedHeightAndFullSidebarMaterial() async throws {
    let host = NSHostingController(
      rootView: SettingsView(model: .preview(.creator))
        .environment(\.csReduceTransparencyOverride, false)
        .environment(\.csContrastOverride, .standard)
    )
    let window = NSWindow(contentViewController: host)
    window.isReleasedWhenClosed = false
    window.setContentSize(NSSize(width: 1120, height: 720))
    window.orderFrontRegardless()
    defer { window.close() }
    for _ in 0..<5 {
      await nextMainQueueTurn()
      host.view.layoutSubtreeIfNeeded()
    }
    let controls = try XCTUnwrap(window.toolbar).items.compactMap(\.view)
    XCTAssertGreaterThanOrEqual(
      controls.filter { $0.frame.height >= 51 }.count, 2,
      "Settings brand and pane title use the same header rhythm as Agent")
    for view in controls {
      let rect = view.convert(view.bounds, to: nil)
      XCTAssertLessThanOrEqual(rect.maxY, window.frame.height)
      XCTAssertGreaterThanOrEqual(rect.minY, window.contentLayoutRect.maxY)
    }
    let material = try XCTUnwrap(
      findView(of: CSSidebarVisualEffectNSView.self, in: host.view))
    let frame = material.convert(material.bounds, to: nil)
    XCTAssertEqual(frame.maxY, window.frame.height, accuracy: 1)
    XCTAssertEqual(frame.minY, 0, accuracy: 1)
  }

  func testSystemAppearanceAdaptationBetweenLightAndDark() {
    let aqua = NSAppearance(named: .aqua)!
    let darkAqua = NSAppearance(named: .darkAqua)!

    var aquaColor = NSColor.clear
    var darkAquaColor = NSColor.clear

    aqua.performAsCurrentDrawingAppearance {
      aquaColor =
        NSColor.windowBackgroundColor.usingColorSpace(.sRGB)
        ?? NSColor.windowBackgroundColor
    }
    darkAqua.performAsCurrentDrawingAppearance {
      darkAquaColor =
        NSColor.windowBackgroundColor.usingColorSpace(.sRGB)
        ?? NSColor.windowBackgroundColor
    }

    XCTAssertNotEqual(
      aquaColor.redComponent, darkAquaColor.redComponent,
      "System fallback background must adapt dynamically across light and dark appearances"
    )
  }

  // MARK: - Falsifier Contract

  func testFalsifierBaselineWindowConfigurationBlocksTranslucency() {
    // Proves that the default macOS/SwiftUI window configuration without adoptHostWindow
    // is opaque and prevents .behindWindow material sampling.
    let baselineWindow = NSWindow(
      contentRect: NSRect(x: 0, y: 0, width: 880, height: 620),
      styleMask: [.titled, .closable, .miniaturizable, .resizable],
      backing: .buffered,
      defer: false
    )
    baselineWindow.isReleasedWhenClosed = false
    defer { baselineWindow.close() }

    // Baseline unadopted window properties:
    XCTAssertTrue(
      baselineWindow.isOpaque,
      "Falsifier: default window is opaque, which stops behind-window blending from sampling"
    )
    XCTAssertFalse(
      baselineWindow.styleMask.contains(.fullSizeContentView),
      "Falsifier: default window lacks fullSizeContentView, clipping content below titlebar"
    )
    XCTAssertFalse(
      baselineWindow.titlebarAppearsTransparent,
      "Falsifier: default titlebar is not transparent"
    )
  }

  // MARK: - Helpers

  private func findView<T: NSView>(of type: T.Type, in root: NSView) -> T? {
    if let match = root as? T {
      return match
    }
    for subview in root.subviews {
      if let match = findView(of: type, in: subview) {
        return match
      }
    }
    return nil
  }

  private func nextMainQueueTurn() async {
    await withCheckedContinuation { (continuation: CheckedContinuation<Void, Never>) in
      DispatchQueue.main.async {
        continuation.resume()
      }
    }
  }
}
