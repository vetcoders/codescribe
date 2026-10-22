import AppKit
import SwiftUI
import XCTest

@testable import Codescribe

/// Transcription keeps its own action column and measures the text beside it.
@MainActor
final class OverlayTranscriptSurfaceTests: XCTestCase {
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
    // All twelve commands remain in the scroll content. The viewport ends
    // after the seventh, without revealing a clipped eighth control. Header
    // clearance and bottom padding are paid once: 54 + 242 + 12.
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

  func testSevenSlotViewportScrollsTheRemainingCommandsWithoutPresentingAWindow() throws {
    let height = OverlayTranscriptActionsLayout.viewportHeight
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
    XCTAssertEqual(document.bounds.height, OverlayTranscriptActionsLayout.columnHeight, accuracy: 1)
    let bottom = document.bounds.height - scroll.contentView.bounds.height
    scroll.contentView.scroll(to: NSPoint(x: 0, y: bottom))
    scroll.reflectScrolledClipView(scroll.contentView)
    XCTAssertEqual(scroll.contentView.bounds.minY, bottom, accuracy: 1)
    XCTAssertEqual(scroll.contentView.bounds.maxY, document.bounds.maxY, accuracy: 1)
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

  /// The whole fixed action set renders from the first frame, in its fixed
  /// top-to-bottom order, even in a listening take where almost none of it
  /// can run yet. Availability must never add or remove a control.
  func testFixedActionSetRendersInFullFromTheFirstRender() throws {
    try BackgroundTestWindow.requireIsolatedGUI()
    let state = OverlayState.previewListening()
    let host = NSHostingView(
      rootView: DictationOverlayView(state: state)
        .defaultAppStorage(try XCTUnwrap(UserDefaults(suiteName: UUID().uuidString))))
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
    var tops: [Double] = []
    for identifier in identifiers {
      let element = try XCTUnwrap(
        NativeAccessibilityProbe.waitForElements(identifier).elements.first,
        "\(identifier) is not rendered from the first render")
      tops.append(element.y)
    }
    XCTAssertEqual(tops, tops.sorted(), "the fixed column changed its order: \(tops)")
    XCTAssertEqual(Set(tops).count, identifiers.count, "two controls share one place")
    // First-to-last top distance of the rendered stack: eleven 28 pt controls
    // stepped over inner spacing 14, 4 separators and 64 of group spacing.
    XCTAssertEqual(
      tops.last! - tops.first!, 390, accuracy: 1,
      "the rendered column does not match the laid-out column height")
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
      canvas.contains("let listOnScreen =\n          agentsSurface"))
    XCTAssertTrue(
      canvas.contains("let railShown = agentsSurface && !state.isCollapsed && !listOnScreen"))
    XCTAssertTrue(
      canvas.contains(
        "let transcriptActionsShown = transcriptionSurface && !state.isCollapsed"))
    XCTAssertTrue(canvas.contains("if transcriptActionsShown {"))
    XCTAssertTrue(canvas.contains("if railShown {"))
    XCTAssertFalse(canvas.contains("overlay-agent-rail"))
    XCTAssertEqual(canvas.components(separatedBy: "transcriptActionsColumn(").count - 1, 1)
  }
}
