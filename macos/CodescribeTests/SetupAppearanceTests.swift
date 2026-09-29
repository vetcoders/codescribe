import AppKit
import SwiftUI
import XCTest

@testable import Codescribe

@MainActor
final class SetupAppearanceTests: XCTestCase {
  func testAllSemanticIconsHaveSystemGlyphs() {
    for icon in CSIcon.allCases {
      XCTAssertNotNil(
        NSImage(systemSymbolName: icon.systemName, accessibilityDescription: nil),
        "Missing system glyph: \(icon.systemName)")
    }
  }

  func testSetupStepsRenderInBothSystemAppearances() throws {
    for dark in [false, true] {
      for progress in 0...12 {
        let model = OnboardingViewModel(
          engine: MockOnboardingEngine(progress: UInt32(progress)),
          hotkeys: MockHotkeysEngine(), agentStatus: MockAgentStatusEngine(),
          probe: MockPermissionProbe(.allGranted))
        let originalStep = model.stepIndex
        let size = NSSize(width: 720, height: 620)
        let appearance = try XCTUnwrap(NSAppearance(named: dark ? .darkAqua : .aqua))
        let host = NSHostingView(
          rootView: OnboardingView(model: model)
            .environment(\.colorScheme, dark ? .dark : .light))
        let window = NSWindow(
          contentRect: NSRect(origin: .zero, size: size),
          styleMask: [.borderless], backing: .buffered, defer: false)
        window.isReleasedWhenClosed = false
        defer {
          window.contentView = nil
          window.close()
        }
        window.appearance = appearance
        host.appearance = appearance
        host.sizingOptions = []
        host.frame = NSRect(origin: .zero, size: size)
        window.contentView = host
        host.layoutSubtreeIfNeeded()
        RunLoop.main.run(until: Date().addingTimeInterval(0.03))
        let bitmap = try XCTUnwrap(host.bitmapImageRepForCachingDisplay(in: host.bounds))
        appearance.performAsCurrentDrawingAppearance {
          host.cacheDisplay(in: host.bounds, to: bitmap)
        }
        let png = try XCTUnwrap(bitmap.representation(using: .png, properties: [:]))
        let file = FileManager.default.temporaryDirectory
          .appendingPathComponent("codescribe-setup-\(progress)-\(dark ? "dark" : "light").png")
        try png.write(to: file)
        XCTAssertGreaterThan(png.count, 800)
        XCTAssertEqual(model.stepIndex, originalStep, "Rendering must not advance setup")
        XCTAssertFalse(window.isVisible, "Setup snapshots must stay offscreen")
      }
    }
  }
}
