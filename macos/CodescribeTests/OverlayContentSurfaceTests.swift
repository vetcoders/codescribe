import AppKit
import SwiftUI
import XCTest

@testable import Codescribe

@MainActor
private final class SurfaceFoldRecorder {
  var frames = OverlayHeaderControlFrames()
}

@MainActor
private struct SurfaceFoldProbe: View {
  let state: OverlayState
  let recorder: SurfaceFoldRecorder

  var body: some View {
    DictationOverlayView(state: state)
      .onPreferenceChange(OverlayHeaderControlFramesPreferenceKey.self) { recorder.frames = $0 }
  }
}

/// No production presence monitor, recorder, activation or foreground window.
@MainActor
final class OverlayContentSurfaceTests: XCTestCase {
  private func withDefaults(_ body: (UserDefaults) throws -> Void) rethrows {
    let suite = "OverlayContentSurfaceTests.\(UUID().uuidString)"
    let defaults = UserDefaults(suiteName: suite)!
    defer { defaults.removePersistentDomain(forName: suite) }
    try body(defaults)
  }

  private func panel(defaults: UserDefaults) -> FloatingOverlayPanel {
    let panel = FloatingOverlayPanel(
      contentRect: NSRect(origin: .zero, size: DictationOverlayWindow.defaultSize),
      styleMask: [.borderless, .nonactivatingPanel, .resizable], backing: .buffered, defer: false)
    panel.contentSizeDefaults = defaults
    panel.delegate = panel
    panel.isReleasedWhenClosed = false
    panel.ignoresMouseEvents = true
    let visible = NSScreen.main!.visibleFrame
    panel.setFrameOrigin(
      NSPoint(
        x: visible.maxX - 510,
        y: visible.maxY - DictationOverlayWindow.defaultSize.height - 40))
    return panel
  }

  private func wireModes(_ state: OverlayState, to panel: FloatingOverlayPanel) {
    state.onPresentationModeChanged = { [weak panel, weak state] mode in
      guard let panel, let state else { return }
      panel.setContentSurface(state.windowContentSurface)
      panel.setPresentationMode(mode)
    }
    panel.onResizePresentationSelected = { [weak state] mode in
      state?.setPresentationMode(mode)
    }
  }

  func testProfilesSurviveRelaunchAndOnlyAgentsInheritTheOldSharedFrame() {
    withDefaults { defaults in
      defaults.set(900.0, forKey: "DictationOverlayPanel.contentSize.v6.w")
      defaults.set(700.0, forKey: "DictationOverlayPanel.contentSize.v6.h")
      XCTAssertEqual(
        DictationOverlayWindow.restoredContentSize(
          for: nil, surface: .transcription, defaults: defaults),
        DictationOverlayWindow.defaultSize)
      XCTAssertEqual(
        DictationOverlayWindow.restoredContentSize(for: nil, surface: .agents, defaults: defaults),
        NSSize(width: 900, height: 700))
      for (surface, size) in [
        (OverlayContentSurface.transcription, NSSize(width: 500, height: 560)),
        (.agents, NSSize(width: 760, height: 560)),
      ] {
        DictationOverlayWindow.persist(size: size, surface: surface, defaults: defaults)
        XCTAssertEqual(
          DictationOverlayWindow.restoredContentSize(
            for: nil, surface: surface, defaults: defaults),
          size)
      }
    }
  }

  func testNativeProfilesRoundTripThroughMiniAndMidiWithoutSavingStripSizes() {
    withDefaults { defaults in
      let agents = NSSize(width: 650, height: 560)
      DictationOverlayWindow.persist(size: agents, surface: .agents, defaults: defaults)
      let panel = panel(defaults: defaults)
      defer { panel.close() }
      let anchor = NSPoint(x: panel.frame.maxX, y: panel.frame.maxY)
      XCTAssertTrue(panel.setContentSurface(.agents))
      XCTAssertEqual(panel.frame.size, agents)
      XCTAssertEqual(NSPoint(x: panel.frame.maxX, y: panel.frame.maxY), anchor)
      panel.setPresentationMode(.mini)
      let strip = panel.frame
      XCTAssertTrue(panel.setContentSurface(.transcription))
      XCTAssertEqual(panel.frame, strip)
      panel.persistContentSize()
      XCTAssertEqual(
        DictationOverlayWindow.restoredContentSize(for: nil, defaults: defaults),
        DictationOverlayWindow.defaultSize)
      panel.setPresentationMode(.midi)
      panel.setPresentationMode(.expanded)
      XCTAssertEqual(panel.frame.size, DictationOverlayWindow.defaultSize)
      XCTAssertEqual(NSPoint(x: panel.frame.maxX, y: panel.frame.maxY), anchor)
      panel.setContentSurface(.agents)
      XCTAssertEqual(panel.frame.size, agents)
      panel.setPresentationMode(.midi)
      panel.persistContentSize()
      XCTAssertEqual(
        DictationOverlayWindow.restoredContentSize(for: nil, surface: .agents, defaults: defaults),
        agents)
    }
  }

  func testFoldRemembersRosterAndExplicitTranscriptionChangesTheSurface() {
    let state = OverlayState()
    state.showAgentMonitor()
    XCTAssertEqual(state.windowContentSurface, .agents)
    state.setPresentationMode(.mini)
    XCTAssertFalse(state.showsAgentMonitor)
    XCTAssertEqual(state.windowContentSurface, .agents)
    state.setPresentationMode(.expanded)
    XCTAssertTrue(state.showsAgentMonitor)
    state.showTranscription()
    XCTAssertEqual(state.windowContentSurface, .transcription)
    XCTAssertFalse(state.showsAgentMonitor)
  }

  func testOrdinaryCaptureLeavesRosterBeforeApplyingExpansion() {
    for preparing in [true, false] {
      let state = OverlayState()
      state.showAgentMonitor()
      state.setPresentationMode(.mini)
      if preparing { state.handleRecordingPreparing() } else { state.handleRecordingStarted() }
      XCTAssertEqual(state.windowContentSurface, .transcription)
      XCTAssertFalse(state.showsAgentMonitor)
      state.finishControllerRecording()
    }
  }

  func testLatestSurfaceWaitsForUserResizeAndForAnInFlightMorph() {
    withDefaults { defaults in
      let state = OverlayState()
      state.setPresentationMode(.expanded)
      let panel = panel(defaults: defaults)
      defer { panel.close() }
      wireModes(state, to: panel)
      let controller = OverlayController(
        state: state, overlayEnabledProvider: { true }, assistiveStatusProvider: { false },
        panelFactory: { _, _ in panel }, orderPanelFront: { _ in }, orderPanelOut: { _ in })
      controller.show()
      XCTAssertTrue(panel.beginUserResize(edge: .left, at: .zero))
      state.showAgentMonitor()
      XCTAssertEqual(panel.contentSurface, .transcription)
      panel.endUserResize()
      XCTAssertEqual(panel.contentSurface, .agents)
      state.onPresentationModeChanged = { [weak panel] mode in
        panel?.setPresentationMode(mode, animated: true)
      }
      state.setPresentationMode(.mini)
      state.showTranscription()
      XCTAssertEqual(panel.contentSurface, .agents, "a morph retains its original surface")
      panel.settleFrameTransition()
      XCTAssertEqual(
        panel.contentSurface, .transcription, "completion retries the latest projection")
    }
  }

  func testAgentsManualSizeDoesNotDisableTranscriptGrowthOrMoveItsFoldAnchor() {
    withDefaults { defaults in
      let state = OverlayState()
      state.setPresentationMode(.expanded)
      let panel = panel(defaults: defaults)
      defer { panel.close() }
      wireModes(state, to: panel)
      let controller = OverlayController(
        state: state, overlayEnabledProvider: { true }, assistiveStatusProvider: { false },
        panelFactory: { _, _ in panel }, orderPanelFront: { _ in }, orderPanelOut: { _ in })
      controller.show()
      state.showAgentMonitor()
      XCTAssertTrue(panel.beginUserResize(edge: .left, at: .zero))
      panel.updateUserResize(to: NSPoint(x: -120, y: 0))
      panel.endUserResize()
      let agentSize = panel.frame.size
      state.showTranscription()
      let visible = NSScreen.main!.visibleFrame
      OverlayController.isApplyingFrame = true
      panel.setFrameOrigin(
        NSPoint(x: visible.maxX - 510, y: visible.maxY - panel.frame.height - 40))
      OverlayController.isApplyingFrame = false
      let anchor = NSPoint(x: panel.frame.maxX, y: panel.frame.maxY)
      let before = panel.frame.height
      project((1...80).map { "projected line \($0)" }.joined(separator: "\n"), to: state)
      XCTAssertGreaterThan(panel.frame.height, before)
      XCTAssertEqual(NSPoint(x: panel.frame.maxX, y: panel.frame.maxY), anchor)
      let grown = panel.frame.size
      state.showAgentMonitor()
      XCTAssertEqual(panel.frame.size, agentSize)
      state.showTranscription()
      XCTAssertEqual(panel.frame.size, grown, "return restores the last transcript size")
      XCTAssertTrue(panel.beginUserResize(edge: .bottom, at: .zero))
      panel.updateUserResize(to: NSPoint(x: 0, y: 40))
      panel.endUserResize()
      let manual = panel.frame.size
      state.showAgentMonitor()
      state.showTranscription()
      project(String(repeating: "more text\n", count: 120), to: state, sequence: 2)
      XCTAssertEqual(
        panel.frame.size, manual, "the transcript's own manual size stays authoritative")
    }
  }

  func testRealFoldControlKeepsItsScreenPointAcrossAllFormsAndSurfaces() throws {
    try withDefaults { defaults in
      DictationOverlayWindow.persist(
        size: NSSize(width: 650, height: 560), surface: .agents, defaults: defaults)
      let state = OverlayState.previewListening()
      state.setPresentationMode(.expanded)
      let recorder = SurfaceFoldRecorder()
      let host = NSHostingView(rootView: SurfaceFoldProbe(state: state, recorder: recorder))
      host.sizingOptions = []
      host.autoresizingMask = []
      let panel = panel(defaults: defaults)
      panel.contentView = OverlayContentContainer(hosting: host)
      wireModes(state, to: panel)
      defer { panel.close() }
      BackgroundTestWindow.present(panel)
      @MainActor func foldPoint() throws -> NSPoint {
        host.layoutSubtreeIfNeeded()
        RunLoop.main.run(until: Date().addingTimeInterval(0.08))
        host.layoutSubtreeIfNeeded()
        let measured = recorder.frames.preview
        let rect = try XCTUnwrap(measured)
        XCTAssertGreaterThan(rect.width, 0)
        // The preference is in the shared, top-leading header coordinate space.
        return NSPoint(x: panel.frame.minX + rect.midX, y: panel.frame.maxY - rect.midY)
      }
      let reference = try foldPoint()
      for mode in [OverlayPresentationMode.mini, .midi, .expanded] {
        state.setPresentationMode(mode)
        let point = try foldPoint()
        XCTAssertEqual(point.x, reference.x, accuracy: 0.5, "\(mode)")
        XCTAssertEqual(point.y, reference.y, accuracy: 0.5, "\(mode)")
      }
      state.showAgentMonitor()
      panel.setContentSurface(state.windowContentSurface)
      let agentsPoint = try foldPoint()
      XCTAssertEqual(agentsPoint.x, reference.x, accuracy: 0.5)
      XCTAssertEqual(agentsPoint.y, reference.y, accuracy: 0.5)
      XCTAssertEqual(panel.alphaValue, 0, "the fixture stays invisible without changing its level")
      XCTAssertFalse(panel.isKeyWindow)
      XCTAssertFalse(NSApp.isActive)
    }
  }

  func testCaptureShowKeepsAnAlreadyVisibleFoldPoint() {
    withDefaults { defaults in
      let state = OverlayState()
      state.setPresentationMode(.expanded)
      let panel = panel(defaults: defaults)
      defer { panel.close() }
      wireModes(state, to: panel)
      let controller = OverlayController(
        state: state, overlayEnabledProvider: { true }, assistiveStatusProvider: { false },
        panelFactory: { _, _ in panel }, orderPanelFront: { BackgroundTestWindow.present($0) },
        orderPanelOut: { $0.orderOut(nil) })
      controller.show()
      OverlayController.isApplyingFrame = true
      panel.setFrameOrigin(NSPoint(x: panel.frame.minX - 80, y: panel.frame.minY - 60))
      OverlayController.isApplyingFrame = false
      let before = panel.frame
      controller.showForRecording()
      XCTAssertEqual(panel.frame, before)
      XCTAssertEqual(panel.alphaValue, 0, "the fixture stays invisible without changing its level")
      XCTAssertFalse(panel.isKeyWindow)
      XCTAssertFalse(NSApp.isActive)
    }
  }

  private func project(_ text: String, to state: OverlayState, sequence: UInt64 = 1) {
    state.applyTranscriptProjection(
      transcriptProjection(
        sequence: sequence, emittedAt: "2026-10-20T18:00:00Z", sessionId: "surface-size-fixture",
        renderedText: text, phase: "listening", terminal: false,
        reducerAction: "surface_size_fixture"))
  }
}
