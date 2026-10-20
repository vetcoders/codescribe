import AppKit
import SwiftUI
import XCTest

@testable import Codescribe

/// Transcription keeps its own action column and measures the text beside it.
@MainActor
final class OverlayTranscriptSurfaceTests: XCTestCase {
  func testColumnWidthScalesIndependentlyOfTheAgentsDrawer() {
    XCTAssertEqual(OverlayTranscriptActionsLayout.columnWidthAtUnitScale, 52)
    XCTAssertEqual(OverlayTranscriptActionsLayout.columnWidth(textScale: 1), 68)
    XCTAssertEqual(OverlayTranscriptActionsLayout.columnWidth(textScale: 1.5), 94)
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

  func testGrowthMeasuresTheTextColumnBesideTheActionsBar() {
    let line = String(repeating: "recorded words ", count: 40)
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
