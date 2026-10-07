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
    let size = NSSize(width: 720, height: 620)
    let window = NSWindow(
      contentRect: NSRect(origin: .zero, size: size),
      styleMask: [.borderless], backing: .buffered, defer: false)
    window.isReleasedWhenClosed = false
    let host = NSHostingView(rootView: AnyView(Color.clear))
    host.sizingOptions = []
    host.frame = NSRect(origin: .zero, size: size)
    window.contentView = host
    defer {
      window.contentView = nil
      window.close()
    }
    for dark in [false, true] {
      for progress in OnboardingStep.flow.indices {
        let model = OnboardingViewModel(
          engine: MockOnboardingEngine(progress: UInt32(progress)),
          hotkeys: MockHotkeysEngine(), agentStatus: MockAgentStatusEngine(),
          agentBridge: OfflineSetupBridge(),
          probe: MockPermissionProbe(.allGranted), languagePreferences: preferences)
        let originalStep = model.stepIndex
        let appearance = try XCTUnwrap(NSAppearance(named: dark ? .darkAqua : .aqua))
        window.appearance = appearance
        host.appearance = appearance
        host.frame = NSRect(origin: .zero, size: size)
        host.rootView = AnyView(
          OnboardingView(model: model)
            .environment(\.colorScheme, dark ? .dark : .light)
            .transaction { $0.disablesAnimations = true }
        )
        pumpUntilSetupInk(host, appearance: appearance)
        let bitmap = try XCTUnwrap(host.bitmapImageRepForCachingDisplay(in: host.bounds))
        appearance.performAsCurrentDrawingAppearance {
          host.cacheDisplay(in: host.bounds, to: bitmap)
        }
        let png = try XCTUnwrap(bitmap.representation(using: .png, properties: [:]))
        XCTAssertGreaterThan(png.count, 800)
        XCTAssertEqual(model.stepIndex, originalStep, "Rendering must not advance setup")
        XCTAssertFalse(window.isVisible, "Setup snapshots must stay offscreen")
      }
    }
  }

  func testTitledOnboardingWindowHasARealHeaderDragHitTargetAcrossAllSteps() throws {
    let hosting = NSHostingController(rootView: AnyView(Color.clear))
    let window = DragRecordingWindow(contentViewController: hosting)
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

    for progress in OnboardingStep.flow.indices {
      let model = OnboardingViewModel(
        engine: MockOnboardingEngine(progress: UInt32(progress)),
        hotkeys: MockHotkeysEngine(), agentStatus: MockAgentStatusEngine(),
        agentBridge: OfflineSetupBridge(),
        probe: MockPermissionProbe(.allGranted))
      window.title = model.windowTitle
      hosting.rootView = AnyView(
        OnboardingView(model: model)
          .transaction { $0.disablesAnimations = true }
      )
      contentView.layoutSubtreeIfNeeded()
      let dragView = try XCTUnwrap(
        waitForDragTarget(in: contentView),
        "step \(progress) header drag target did not mount"
      )
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
      let dragsBefore = window.performDragCallCount
      window.sendEvent(event)
      XCTAssertEqual(window.performDragCallCount, dragsBefore + 1, "step \(progress)")
    }
  }

  private func firstSubview<Child: NSView>(of type: Child.Type, in view: NSView) -> Child? {
    if let match = view as? Child { return match }
    for subview in view.subviews {
      if let match = firstSubview(of: type, in: subview) { return match }
    }
    return nil
  }

  /// Same 0.03s ceiling as the old fixed pulse. Returns once the mounted
  /// bitmap is not a flat fill. The caller still asserts `png.count > 800`.
  private func pumpUntilSetupInk(_ host: NSView, appearance: NSAppearance) {
    let deadline = Date().addingTimeInterval(0.03)
    host.layoutSubtreeIfNeeded()
    while Date() < deadline {
      let sliceEnd = min(deadline, Date().addingTimeInterval(0.008))
      RunLoop.main.run(mode: .default, before: sliceEnd)
      host.layoutSubtreeIfNeeded()
      guard let probe = host.bitmapImageRepForCachingDisplay(in: host.bounds) else { continue }
      appearance.performAsCurrentDrawingAppearance {
        host.cacheDisplay(in: host.bounds, to: probe)
      }
      if bitmapHasVariedInk(probe) { return }
    }
  }

  private func waitForDragTarget(in contentView: NSView) -> OnboardingDragView? {
    let deadline = Date().addingTimeInterval(0.03)
    while Date() < deadline {
      contentView.layoutSubtreeIfNeeded()
      if let drag = firstSubview(of: OnboardingDragView.self, in: contentView),
        drag.bounds.width > 500, drag.bounds.height > 60
      {
        return drag
      }
      let sliceEnd = min(deadline, Date().addingTimeInterval(0.008))
      RunLoop.main.run(mode: .default, before: sliceEnd)
    }
    contentView.layoutSubtreeIfNeeded()
    return firstSubview(of: OnboardingDragView.self, in: contentView)
  }

  private func bitmapHasVariedInk(_ bitmap: NSBitmapImageRep) -> Bool {
    guard !bitmap.isPlanar,
      bitmap.bitsPerSample == 8,
      bitmap.samplesPerPixel >= 3,
      let data = bitmap.bitmapData,
      bitmap.pixelsWide > 1,
      bitmap.pixelsHigh > 1
    else { return false }
    let samples = bitmap.samplesPerPixel
    let row = bitmap.bytesPerRow
    var minL = 255
    var maxL = 0
    let stepX = max(bitmap.pixelsWide / 16, 1)
    let stepY = max(bitmap.pixelsHigh / 12, 1)
    var y = 0
    while y < bitmap.pixelsHigh {
      var x = 0
      while x < bitmap.pixelsWide {
        let offset = y * row + x * samples
        let luminance = (Int(data[offset]) + Int(data[offset + 1]) + Int(data[offset + 2])) / 3
        minL = min(minL, luminance)
        maxL = max(maxL, luminance)
        x += stepX
      }
      y += stepY
    }
    return maxL - minL >= 12
  }

}

/// In-test status. `OnboardingViewModel` calls `status()` while initializing;
/// the real installer probes the payload on disk.
private struct OfflineSetupBridge: AgentBridgeInstalling {
  func status() -> AgentBridgeInstallationStatus {
    AgentBridgeInstallationStatus(
      payloadAvailable: true,
      bundleVersion: nil,
      installedClients: [],
      installedPaths: [],
      detail: "offline fixture"
    )
  }

  func install(selectedClients _: Set<AgentBridgeClient>) throws -> AgentBridgeInstallationStatus {
    throw AgentBridgeInstallationError.payloadUnavailable
  }

  func adoptManualSkill(client _: AgentBridgeClient) throws -> AgentBridgeAdoptionResult {
    throw AgentBridgeInstallationError.payloadUnavailable
  }
}
