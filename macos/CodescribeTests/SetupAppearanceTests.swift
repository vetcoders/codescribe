import AppKit
import SwiftUI
import XCTest

@testable import Codescribe

@MainActor
private final class DragRecordingWindow: NSWindow {
  private(set) var performDragCallCount = 0

  override func performDrag(with event: NSEvent) {
    performDragCallCount += 1
  }
}

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
    for language in [InterfaceLanguage.english, .polish] {
      try renderSetupSteps(language: language)
    }
  }

  private func renderSetupSteps(language: InterfaceLanguage) throws {
    let suite = "codescribe-setup-snapshots-\(UUID().uuidString)"
    let preferences = try XCTUnwrap(UserDefaults(suiteName: suite))
    preferences.setVolatileDomain([:], forName: UserDefaults.argumentDomain)
    preferences.set([language.rawValue], forKey: "AppleLanguages")
    defer { preferences.removePersistentDomain(forName: suite) }
    for dark in [false, true] {
      for progress in 0...12 {
        let model = OnboardingViewModel(
          engine: MockOnboardingEngine(progress: UInt32(progress)),
          hotkeys: MockHotkeysEngine(), agentStatus: MockAgentStatusEngine(),
          probe: MockPermissionProbe(.allGranted), languagePreferences: preferences)
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
          .appendingPathComponent(
            "codescribe-setup-\(language.rawValue)-\(progress)-\(dark ? "dark" : "light").png")
        try png.write(to: file)
        XCTAssertGreaterThan(png.count, 800)
        XCTAssertEqual(model.stepIndex, originalStep, "Rendering must not advance setup")
        XCTAssertFalse(window.isVisible, "Setup snapshots must stay offscreen")
      }
    }
  }

  func testTitledOnboardingWindowHasARealHeaderDragHitTargetAcrossAllSteps() throws {
    for progress in 0...12 {
      do {
        let model = OnboardingViewModel(
          engine: MockOnboardingEngine(progress: UInt32(progress)),
          hotkeys: MockHotkeysEngine(), agentStatus: MockAgentStatusEngine(),
          probe: MockPermissionProbe(.allGranted))
        let hosting = NSHostingController(rootView: OnboardingView(model: model))
        let window = DragRecordingWindow(contentViewController: hosting)
        window.title = model.windowTitle
        window.setContentSize(NSSize(width: 720, height: 620))
        window.styleMask = [.titled, .closable, .fullSizeContentView]
        window.titlebarAppearsTransparent = true
        window.isOpaque = false
        window.backgroundColor = .clear
        window.isReleasedWhenClosed = false
        defer {
          window.orderOut(nil)
          window.close()
        }
        window.orderFrontRegardless()

        let contentView = try XCTUnwrap(window.contentView)
        contentView.layoutSubtreeIfNeeded()
        RunLoop.main.run(until: Date().addingTimeInterval(0.03))

        let dragView = try XCTUnwrap(firstSubview(of: OnboardingDragView.self, in: contentView))
        XCTAssertTrue(window.styleMask.contains(.titled), "step \(progress)")
        XCTAssertTrue(window.styleMask.contains(.fullSizeContentView), "step \(progress)")
        XCTAssertTrue(window.styleMask.contains(.closable), "step \(progress)")
        XCTAssertTrue(window.titlebarAppearsTransparent, "step \(progress)")
        XCTAssertFalse(window.isOpaque, "step \(progress)")
        XCTAssertTrue(dragView.window === window, "step \(progress)")
        XCTAssertGreaterThan(dragView.bounds.width, 500, "step \(progress)")
        XCTAssertGreaterThan(dragView.bounds.height, 60, "step \(progress)")
        XCTAssertTrue(dragView.acceptsFirstMouse(for: nil), "step \(progress)")

        let pointInDragView = NSPoint(x: dragView.bounds.maxX - 3, y: dragView.bounds.midY)
        let pointInWindow = dragView.convert(pointInDragView, to: nil)
        let event = try XCTUnwrap(
          NSEvent.mouseEvent(
            with: .leftMouseDown,
            location: pointInWindow,
            modifierFlags: [],
            timestamp: ProcessInfo.processInfo.systemUptime,
            windowNumber: window.windowNumber,
            context: nil,
            eventNumber: progress + 1,
            clickCount: 1,
            pressure: 1
          )
        )
        window.sendEvent(event)
        XCTAssertEqual(window.performDragCallCount, 1, "step \(progress)")
      }
    }
  }

  private func firstSubview<Child: NSView>(of type: Child.Type, in view: NSView) -> Child? {
    if let match = view as? Child { return match }
    for subview in view.subviews {
      if let match = firstSubview(of: type, in: subview) { return match }
    }
    return nil
  }

}
