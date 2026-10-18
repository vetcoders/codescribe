import AppKit
import SwiftUI
import XCTest

@testable import Codescribe

/// The sidebar of the Settings and Agent windows is one flat palette tone,
/// `CSColor.sidebar`, a single step away from the detail sheet. A `.sidebar`
/// visual effect blending behind the window took its colour from the desktop,
/// so the seam beside the content was a wallpaper tone rather than one of ours
/// (Monika, 2026-10-18). These tests pin the tone, its step from the sheet in
/// both appearances, and the window adoption that lets the column run up
/// through the titlebar.
@MainActor
final class SettingsSidebarMaterialTests: XCTestCase {

  // MARK: - Palette

  func testSidebarIsOneStepFromTheSheetInBothAppearances() throws {
    let paper = try XCTUnwrap(NSAppearance(named: .aqua))
    let ink = try XCTUnwrap(NSAppearance(named: .darkAqua))
    let paperSidebar = luminance(CSPalette.sidebar, in: paper)
    let paperSheet = luminance(CSPalette.sheet, in: paper)
    let inkSidebar = luminance(CSPalette.sidebar, in: ink)
    let inkSheet = luminance(CSPalette.sheet, in: ink)

    XCTAssertLessThan(paperSidebar, paperSheet, "the sidebar sits deeper than the sheet on paper")
    XCTAssertGreaterThan(inkSidebar, inkSheet, "the sidebar sits lighter than the sheet on ink")
    XCTAssertLessThan(paperSheet - paperSidebar, 0.08, "one step on paper, not a second surface")
    XCTAssertLessThan(inkSidebar - inkSheet, 0.08, "one step on ink, not a second surface")
    XCTAssertGreaterThan(paperSidebar - inkSidebar, 0.5, "the tone follows the appearance")
  }

  // MARK: - Sidebar view

  func testSidebarPaintsTheFlatToneAndPassesEventsThrough() throws {
    let sidebar = try source("Screens/Settings/SettingsSidebarMaterial.swift")
    XCTAssertTrue(sidebar.contains("CSColor.sidebar"))
    XCTAssertTrue(
      sidebar.contains(".allowsHitTesting(false)"),
      "list rows, the search field and footer controls keep every click")
    XCTAssertFalse(sidebar.contains("NSViewRepresentable"), "no desktop-sampled material")
    XCTAssertFalse(sidebar.contains(".material ="), "no desktop-sampled material")
  }

  func testSidebarMountsNoVisualEffectView() async {
    let host = NSHostingView(rootView: SettingsSidebarMaterial())
    host.frame = NSRect(x: 0, y: 0, width: 220, height: 600)
    host.layoutSubtreeIfNeeded()
    await nextMainQueueTurn()

    XCTAssertNil(
      findView(of: NSVisualEffectView.self, in: host),
      "the sidebar is a flat palette tone, not a visual effect")
  }

  func testBothWindowsRunTheSidebarToneUnderTheTitlebar() throws {
    for path in ["Screens/Settings/SettingsView.swift", "Screens/AgentChat/ThreadRail.swift"] {
      XCTAssertTrue(
        try source(path).contains(".background { SettingsSidebarMaterial().ignoresSafeArea() }"),
        "\(path) backs its sidebar with the shared tone from the bottom edge to the top")
    }
  }

  // MARK: - Window Adoption & Chrome Contract

  func testSettingsWindowAdoptionLetsTheSidebarReachTheTitlebar() async {
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
      "Titlebar must appear transparent so the sidebar tone runs up beside the header")
    XCTAssertTrue(
      window.styleMask.contains(.fullSizeContentView),
      "Window must use fullSizeContentView so the sidebar extends under the titlebar")
    XCTAssertFalse(window.isOpaque, "adoptHostWindow keeps the window itself clear")
    XCTAssertEqual(
      window.backgroundColor, .clear,
      "the sheet and sidebar tones paint the window, not its background colour")
  }

  // MARK: - Header rhythm

  func testSettingsHeaderKeepsTheSharedHeight() async throws {
    let host = NSHostingController(rootView: SettingsView(model: .preview(.creator)))
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
  }

  // MARK: - Falsifier Contract

  func testFalsifierBaselineWindowKeepsTheSidebarBelowTheTitlebar() {
    // Proves that the default macOS/SwiftUI window configuration without
    // adoptHostWindow stops content under the titlebar, so the sidebar tone
    // could not run up beside the header.
    let baselineWindow = NSWindow(
      contentRect: NSRect(x: 0, y: 0, width: 880, height: 620),
      styleMask: [.titled, .closable, .miniaturizable, .resizable],
      backing: .buffered,
      defer: false
    )
    baselineWindow.isReleasedWhenClosed = false
    defer { baselineWindow.close() }

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

  private func luminance(_ color: NSColor, in appearance: NSAppearance) -> CGFloat {
    var resolved = color
    appearance.performAsCurrentDrawingAppearance {
      resolved = color.usingColorSpace(.sRGB) ?? color
    }
    return 0.2126 * resolved.redComponent + 0.7152 * resolved.greenComponent
      + 0.0722 * resolved.blueComponent
  }

  private func source(_ path: String) throws -> String {
    let codescribe = URL(fileURLWithPath: #filePath)
      .deletingLastPathComponent().deletingLastPathComponent()
      .appendingPathComponent("Codescribe")
    return try String(contentsOf: codescribe.appendingPathComponent(path), encoding: .utf8)
  }

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
