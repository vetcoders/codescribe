import AppKit
import SwiftUI
import XCTest

@testable import Codescribe

/// Transcription keeps its own action column and measures the text beside it.
@MainActor
final class OverlayTranscriptSurfaceTests: XCTestCase {
  func testPreviewOpensDictationFromAnAgentWithoutChangingCaptureOrDraft() {
    for mode in [OverlayPresentationMode.mini, .midi] {
      let state = OverlayState.previewListening()
      let agent = OverlayConversation(
        id: "preview-agent", channel: "3", name: "agent", owner: nil, messages: [])
      state.applyConversationSnapshot(.init(deliveries: [], conversations: [agent]))
      state.chooseConversation(agent.id)
      state.conversationDrafts[agent.id] = "Unsent agent draft"
      state.setPresentationMode(mode)
      let text = state.activeText
      let generation = state.captureGeneration
      let recording = state.recording
      let controls = OverlayRecordingControls(
        canFinish: recording, presentationMode: mode, compact: true, palette: .dark,
        onIntent: { _ in XCTFail("Preview must not send a recording command") },
        onPreviewToggle: state.toggleCollapsed)
      controls.togglePreview()
      XCTAssertEqual(state.presentationMode, .expanded)
      XCTAssertEqual(state.windowContentSurface, .transcription)
      XCTAssertNil(state.selectedConversationID)
      XCTAssertEqual(state.activeText, text)
      XCTAssertEqual(state.captureGeneration, generation)
      XCTAssertEqual(state.recording, recording)
      XCTAssertEqual(state.conversationDrafts[agent.id], "Unsent agent draft")
      state.toggleCollapsed()
      XCTAssertEqual(state.presentationMode, .mini)
      XCTAssertEqual(state.captureGeneration, generation)
    }
  }

  func testColumnWidthIsOneControlItsInsetsAndTheResizeBandAtEveryScale() {
    // The control chrome is unscaled, so the column is the same width at
    // every supported text scale: 28 control + 4 + 4 insets + 16 resize band.
    XCTAssertEqual(OverlayTranscriptActionsLayout.contentWidth, 36)
    for scale: CGFloat in [0.8, 1, 1.6] {
      XCTAssertEqual(OverlayTranscriptActionsLayout.columnWidth(textScale: scale), 52)
    }
    XCTAssertEqual(
      OverlayTranscriptActionsLayout.columnWidth(textScale: 1)
        - OverlayTranscriptActionsLayout.contentWidth,
      OverlayResizeHit.band, "the resize band is the only strip past the controls")
  }

  func testExpandedMinimumHeightFitsSevenSlotsAndKeepsTheFullInventory() {
    // The expanded floor still reserves seven controls. Available actions
    // use the offered height; the full inventory is only a sizing reference.
    XCTAssertEqual(OverlayTranscriptActionsLayout.columnHeight, 426)
    XCTAssertEqual(OverlayTranscriptActionsLayout.visibleSlotBudget, 7)
    XCTAssertEqual(OverlayTranscriptActionsLayout.viewportHeight, 242)
    XCTAssertEqual(OverlayTranscriptActionsLayout.minimumExpandedHeight, 308)
    XCTAssertEqual(
      DictationOverlayWindow.minSize.height,
      OverlayTranscriptActionsLayout.minimumExpandedHeight)
    // Mini is content-sized (bus intercom spec §13.4); its floor is its controls.
    XCTAssertEqual(
      DictationOverlayWindow.minimumSize(for: .mini),
      NSSize(
        width: DictationOverlayWindow.miniFloorWidth,
        height: DictationOverlayWindow.collapsedHeight))
    XCTAssertEqual(
      DictationOverlayWindow.minimumSize(for: .midi), DictationOverlayWindow.midiSize)
  }

  func testAvailableActionsScrollOnlyWhenTheWindowIsTooShort() throws {
    for height: CGFloat in [90, OverlayTranscriptActionsLayout.viewportHeight] {
      let host = NSHostingView(
        rootView: OverlayIntentRail(
          phase: "formatted", intents: [.copy, .retranscribe, .format, .sendToAgent],
          palette: .dark, onIntent: { _ in }
        )
        .frame(width: OverlayTranscriptActionsLayout.contentWidth, height: height))
      host.sizingOptions = []
      host.frame = NSRect(
        x: 0, y: 0, width: OverlayTranscriptActionsLayout.contentWidth, height: height)
      // A window supplies AppKit's layout context, but is never ordered on
      // screen, made key, or admitted to the person's input surface.
      let window = NSWindow(
        contentRect: host.frame, styleMask: .borderless, backing: .buffered, defer: false)
      window.isReleasedWhenClosed = false
      window.contentView = host
      defer { window.close() }
      host.layoutSubtreeIfNeeded()
      RunLoop.main.run(until: Date().addingTimeInterval(0.1))
      host.layoutSubtreeIfNeeded()
      func descendants(_ view: NSView) -> [NSView] {
        view.subviews.flatMap { [$0] + descendants($0) }
      }
      let scroll = try XCTUnwrap(descendants(host).compactMap { $0 as? NSScrollView }.first)
      let document = try XCTUnwrap(scroll.documentView)
      XCTAssertFalse(window.isVisible, "this fixture must never present a native window")
      XCTAssertFalse(window.isKeyWindow)
      XCTAssertEqual(scroll.contentView.bounds.height, height, accuracy: 1)
      if height == 90 {
        XCTAssertGreaterThan(document.bounds.height, height)
        let bottom = document.bounds.height - scroll.contentView.bounds.height
        scroll.contentView.scroll(to: NSPoint(x: 0, y: bottom))
        scroll.reflectScrolledClipView(scroll.contentView)
        XCTAssertEqual(scroll.contentView.bounds.minY, bottom, accuracy: 1)
        XCTAssertEqual(scroll.contentView.bounds.maxY, document.bounds.maxY, accuracy: 1)
      } else {
        XCTAssertLessThanOrEqual(
          document.bounds.height, height + 1,
          "unavailable controls must not reserve scroll space ahead of format/retranscribe")
      }
    }
  }

  func testConversationInventoryCannotChooseTheTranscriptionCanvas() {
    let state = OverlayState.previewFormatted()
    let agent = OverlayConversation(
      id: "retiring-agent", channel: "3", name: "agent", owner: nil, messages: [])
    state.applyConversationSnapshot(.init(deliveries: [], conversations: [agent]))
    state.chooseConversation(agent.id)
    state.conversationDrafts[agent.id] = "keep the draft"
    let generation = state.captureGeneration
    state.applyConversationSnapshot(.init(deliveries: [], conversations: []))
    XCTAssertEqual(state.windowContentSurface, .agents)
    XCTAssertTrue(state.showsAgentMonitor)
    XCTAssertNil(state.selectedConversationID)
    XCTAssertEqual(state.conversationDrafts[agent.id], "keep the draft")
    XCTAssertEqual(state.captureGeneration, generation)
    state.showTranscription()
    state.applyConversationSnapshot(.init(deliveries: [], conversations: [agent]))
    XCTAssertEqual(state.windowContentSurface, .transcription)
    XCTAssertFalse(state.showsAgentMonitor)
  }

  func testTranscriptActionHitTargetsStayOutsideTheWindowResizeBand() throws {
    try BackgroundTestWindow.requireIsolatedGUI()
    for scale: CGFloat in [0.8, 1, 1.6] {
      let state = OverlayState.previewFormatted()
      let host = NSHostingView(
        rootView: DictationOverlayView(state: state)
          .environment(\.csTextScale, scale)
          .defaultAppStorage(try XCTUnwrap(UserDefaults(suiteName: UUID().uuidString))))
      host.sizingOptions = []
      let panel = FloatingOverlayPanel(
        contentRect: NSRect(x: 100, y: 100, width: 900, height: 600),
        styleMask: [.borderless, .nonactivatingPanel, .resizable], backing: .buffered,
        defer: false)
      panel.contentView = OverlayContentContainer(hosting: host)
      BackgroundTestWindow.present(panel)
      defer { panel.orderOut(nil) }
      host.layoutSubtreeIfNeeded()
      RunLoop.main.run(until: Date().addingTimeInterval(0.1))
      let button = try XCTUnwrap(
        NativeAccessibilityProbe.waitForElements("overlay-intent-copy").elements.first)
      XCTAssertEqual(button.role, "AXButton")
      let screenTop = try XCTUnwrap(NSScreen.screens.first).frame.maxY
      let rect = panel.convertFromScreen(
        NSRect(
          x: button.x, y: screenTop - button.y - button.height,
          width: button.width, height: button.height))
      XCTAssertLessThan(rect.maxX, panel.frame.width - OverlayResizeHit.band)
      let point = NSPoint(x: rect.maxX - 1, y: rect.midY)
      XCTAssertNil(OverlayResizeHit.edge(at: point, in: host.bounds))
      XCTAssertTrue(panel.cursor(at: point) === NSCursor.arrow)
      XCTAssertFalse(panel.isKeyWindow)
    }
  }

  /// Availability changes the mounted controls while preserving their order.
  /// Finished-take engine actions must be reachable without disabled slots.
  func testOnlyAvailableActionsRenderAcrossListeningFinishedAndEditingStates() throws {
    try BackgroundTestWindow.requireIsolatedGUI()
    let host = NSHostingView(
      rootView: OverlayIntentRail(
        phase: "listening", intents: [], palette: .dark, onIntent: { _ in }))
    host.sizingOptions = []
    let window = NSWindow(
      contentRect: NSRect(
        x: -10000, y: -10000, width: 900,
        height: OverlayTranscriptActionsLayout.minimumExpandedHeight),
      styleMask: .borderless, backing: .buffered, defer: false)
    window.isReleasedWhenClosed = false
    window.contentView = host
    BackgroundTestWindow.present(window)
    defer { window.orderOut(nil) }
    host.layoutSubtreeIfNeeded()
    RunLoop.main.run(until: Date().addingTimeInterval(0.1))

    let identifiers = [
      "overlay-history-menu", "overlay-versions-menu", "overlay-intent-undo",
      "overlay-intent-redo", "overlay-previous-take-menu",
      "overlay-intent-commit-revision", "overlay-intent-discard-revision",
      "overlay-intent-insert-paste", "overlay-intent-copy",
      "overlay-intent-retranscribe", "overlay-intent-format",
      "overlay-intent-send-to-agent",
    ]
    let cases: [(String, [OverlayIntent], [String])] = [
      ("listening", [], ["overlay-history-menu"]),
      (
        "formatted", [.copy, .retranscribe, .format, .sendToAgent],
        [
          "overlay-history-menu", "overlay-intent-copy", "overlay-intent-retranscribe",
          "overlay-intent-format", "overlay-intent-send-to-agent",
        ]
      ),
      (
        "formatted", [.commitRevision, .discardRevision],
        [
          "overlay-history-menu", "overlay-intent-commit-revision",
          "overlay-intent-discard-revision",
        ]
      ),
    ]
    for (phase, intents, expected) in cases {
      host.rootView = OverlayIntentRail(
        phase: phase, intents: intents, palette: .dark, onIntent: { _ in })
      host.layoutSubtreeIfNeeded()
      RunLoop.main.run(until: Date().addingTimeInterval(0.1))
      var tops: [Double] = []
      for identifier in identifiers {
        if expected.contains(identifier) {
          let element = try XCTUnwrap(
            NativeAccessibilityProbe.waitForElements(identifier).elements.first,
            "\(identifier) is missing in \(phase)")
          tops.append(element.y)
        } else {
          XCTAssertTrue(
            try NativeAccessibilityProbe.query(identifier).elements.isEmpty,
            "\(identifier) must not leave a disabled placeholder in \(phase)")
        }
      }
      XCTAssertEqual(tops, tops.sorted(), "available actions changed order: \(tops)")
      XCTAssertEqual(Set(tops).count, expected.count, "two controls share one place")
    }
  }

  func testGrowthMeasuresTheTextColumnBesideTheActionsBar() {
    // Long enough that the measured text clears the expanded floor at both
    // widths; the growth under test is the narrowing column, not the floor.
    let line = String(repeating: "recorded words ", count: 240)
    let narrow = OverlayContentSizePolicy.preferredHeight(
      for: line, width: 470, textScale: 1, screen: nil)
    let wide = OverlayContentSizePolicy.preferredHeight(
      for: line, width: 900, textScale: 1, screen: nil)
    XCTAssertGreaterThan(narrow, wide)
    XCTAssertGreaterThan(narrow, DictationOverlayWindow.minSize.height)
  }

  func testTranscriptionMountsActionsAndAgentsMountsTheRoster() throws {
    let overlay = try String(
      contentsOf: URL(fileURLWithPath: #filePath)
        .deletingLastPathComponent()
        .deletingLastPathComponent()
        .appendingPathComponent("Codescribe/Screens/Overlay/DictationOverlayView.swift"),
      encoding: .utf8)
    let canvasStart = try XCTUnwrap(overlay.range(of: "private func canvasStack"))
    let canvasEnd = try XCTUnwrap(
      overlay.range(
        of: "private func transcriptActionsColumn",
        range: canvasStart.upperBound..<overlay.endIndex))
    let canvas = String(overlay[canvasStart.lowerBound..<canvasEnd.lowerBound])
    XCTAssertTrue(
      canvas.contains(
        "let listOnScreen = agentsSurface && !state.isCollapsed && state.showsAgentMonitor"))
    XCTAssertTrue(
      canvas.contains("let panelOverlayShown = listOnScreen && !columnShown"))
    XCTAssertTrue(
      canvas.contains(
        "let transcriptActionsShown = transcriptionSurface && !state.isCollapsed"))
    XCTAssertTrue(canvas.contains("if transcriptActionsShown {"))
    XCTAssertTrue(canvas.contains("if columnShown {"))
    XCTAssertTrue(canvas.contains("if panelOverlayShown {"))
    XCTAssertFalse(canvas.contains("overlay-agent-rail"))
    XCTAssertEqual(canvas.components(separatedBy: "transcriptActionsColumn(").count - 1, 1)
  }
}
