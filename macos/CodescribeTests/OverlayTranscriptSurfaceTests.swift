import AppKit
import XCTest

@testable import Codescribe

/// Transcription keeps its own action column and measures the text beside it.
@MainActor
final class OverlayTranscriptSurfaceTests: XCTestCase {
  func testColumnWidthScalesIndependentlyOfTheAgentsDrawer() {
    XCTAssertEqual(OverlayTranscriptActionsLayout.columnWidthAtUnitScale, 52)
    XCTAssertEqual(OverlayTranscriptActionsLayout.columnWidth(textScale: 1), 52)
    XCTAssertEqual(OverlayTranscriptActionsLayout.columnWidth(textScale: 1.5), 78)
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
