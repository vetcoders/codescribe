import SwiftUI
import XCTest

@testable import Codescribe

@MainActor
final class OverlayDividerFoldTests: XCTestCase {
  private let scales: [CGFloat] = [1, 1.6]

  func testDrawerWidthUsesTheRowMinimumWhenTheDividerIsNotDragging() {
    for scale in scales {
      for preferred in [CGFloat(0), 120, 239] {
        XCTAssertEqual(
          OverlayAgentDrawerLayout.drawerWidth(
            textScale: scale, preferredWidth: preferred, windowWidth: 1_200 * scale),
          240 * scale, accuracy: 0.001)
      }
      XCTAssertEqual(
        OverlayAgentDrawerLayout.drawerWidth(textScale: scale, preferredWidth: 120),
        240 * scale, accuracy: 0.001)
    }
  }

  func testAWideDividerReleaseStillLeavesTheConversationMinimum() {
    for scale in scales {
      let width = OverlayAgentDrawerLayout.drawerWidth(
        textScale: scale, preferredWidth: 900, windowWidth: 1_200 * scale)
      XCTAssertEqual(width, 880 * scale, accuracy: 0.001)
      XCTAssertEqual(1_200 * scale - width, 320 * scale, accuracy: 0.001)
    }
  }

  func testFoldThresholdSeparatesTheRailFromTheRowsAtEachTextScale() {
    for scale in scales {
      let rail = OverlayAgentDrawerLayout.railWidth(textScale: scale)
      let fold = OverlayAgentDrawerLayout.railFoldWidth(textScale: scale)
      let rows = OverlayAgentDrawerLayout.contentMinimumWidth(textScale: scale)
      let conversation = OverlayAgentDrawerLayout.minimumConversationWidth(textScale: scale)
      XCTAssertEqual(fold, 160 * scale, accuracy: 0.001)
      XCTAssertEqual(rows, 240 * scale, accuracy: 0.001)
      XCTAssertLessThan(rail, fold)
      XCTAssertLessThan(fold, rows)
      XCTAssertEqual(
        OverlayAgentDrawerLayout.minimumSplitWindowWidth(textScale: scale),
        rows + conversation, accuracy: 0.001)
      XCTAssertEqual(
        OverlayAgentDrawerLayout.minimumSplitWindowWidth(textScale: scale),
        560 * scale, accuracy: 0.001)
    }
    XCTAssertEqual(OverlayAgentDrawerLayout.railUnfoldDrag, 40)
  }

  /// "ten niskobudżetowy układ w sensie przeciągania i resizingu z … floating
  /// przenieść" (Maciej, 2026-11-03): while the divider is dragged only its
  /// guide moves. Neither pane width reads the drag, and release commits it.
  func testADividerDragMovesOnlyItsGuideUntilRelease() throws {
    let source = try overlaySource("DictationOverlayView.swift")
    XCTAssertFalse(source.contains("agentDrawerResize:"), "no canvas state follows the pointer")
    let geometry = try section(
      source, from: "let drawerWidth =", to: "let canvasHidden =")
    XCTAssertEqual(
      geometry.components(separatedBy: "preferredWidth: CGFloat(preferredAgentDrawerWidth)")
        .count - 1, 2, "both pane projections read only the committed width")
    let divider = try section(
      source, from: "private struct OverlayAgentDrawerDivider", to: "\n}\n")
    XCTAssertTrue(divider.contains("@GestureState private var translation: CGFloat?"))
    XCTAssertTrue(divider.contains(".updating($translation)"))
    XCTAssertFalse(divider.contains(".onChanged"))
    XCTAssertTrue(
      compact(divider).contains(".onEnded { value in onCommit(value.translation.width) }"))
    let handle = try section(
      source, from: "private func agentDrawerResizeHandle(", to: ".accessibilityElement()")
    XCTAssertTrue(
      compact(handle).contains(
        "commitAgentDrawerWidth((width - translation) / textScale, windowWidth: windowWidth)"))
  }

  func testReleasingBelowTheFoldThresholdFoldsBeforeRememberingAWidth() throws {
    let source = try overlaySource("DictationOverlayView.swift")
    let commit = try section(
      source, from: "private func commitAgentDrawerWidth(", to: "private func agentRail(")
    let fold = try section(commit, from: "if preferred * textScale", to: "guard")
    XCTAssertEqual(
      compact(fold),
      "if preferred * textScale < OverlayAgentDrawerLayout.railFoldWidth(textScale: textScale) { state.foldAgentList() return }")
    XCTAssertTrue(
      compact(commit).contains(
        "OverlayAgentDrawerLayout.rememberedWidth( textScale: textScale, preferredWidth: preferred, windowWidth: windowWidth)"))
    XCTAssertTrue(commit.contains("preferredAgentDrawerWidth = width"))
  }

  func testTheRailMountsItsDividerAsTheUnfoldGestureTarget() throws {
    let source = try overlaySource("DictationOverlayView.swift")
    let rail = try section(
      source, from: "private func agentRail(", to: "private func agentRailUnfoldHandle(")
    XCTAssertTrue(
      compact(rail).contains(
        ".overlay(alignment: .leading) { agentRailUnfoldHandle(width: width, windowWidth: windowWidth) }"))
    let handle = try section(
      source, from: "private func agentRailUnfoldHandle(", to: "private func agentDrawerResizeHandle(")
    XCTAssertTrue(handle.contains(".accessibilityIdentifier(\"overlay-agent-rail-resize\")"))
    XCTAssertTrue(handle.contains("OverlayMenuCursorRegion(cursor: .resizeLeftRight)"))
    let drag = try section(handle, from: ".onEnded { value in", to: ".accessibilityElement()")
    XCTAssertTrue(
      compact(drag).contains(
        "guard -value.translation.width >= OverlayAgentDrawerLayout.railUnfoldDrag else { return }"))
    XCTAssertTrue(
      compact(drag).contains("let dragged = (width - value.translation.width) / textScale"))
    XCTAssertTrue(
      compact(drag).contains(
        "if dragged * textScale >= OverlayAgentDrawerLayout.contentMinimumWidth(textScale: textScale), let remembered = OverlayAgentDrawerLayout.rememberedWidth( textScale: textScale, preferredWidth: dragged, windowWidth: windowWidth) { preferredAgentDrawerWidth = remembered } state.unfoldAgentList()"))
  }

  func testDecrementingAtTheRowMinimumCommitsAWidthThatFoldsTheList() throws {
    let source = try overlaySource("DictationOverlayView.swift")
    let handle = try section(
      source, from: "private func agentDrawerResizeHandle(", to: "// MARK: Header")
    let adjustable = try section(
      handle, from: ".accessibilityAdjustableAction", to: ".accessibilityIdentifier(")
    XCTAssertTrue(adjustable.contains("case .decrement: step = -20"))
    XCTAssertTrue(
      compact(adjustable).contains(
        "width <= OverlayAgentDrawerLayout.contentMinimumWidth(textScale: textScale) + 0.5"))
    XCTAssertTrue(
      compact(adjustable).contains(
        ": atMinimum && step < 0 ? 0 : width / textScale + step"))
    XCTAssertTrue(adjustable.contains("commitAgentDrawerWidth(next, windowWidth: windowWidth)"))
  }

  func testIncrementingTheRailDividerUnfoldsTheList() throws {
    let source = try overlaySource("DictationOverlayView.swift")
    let handle = try section(
      source, from: "private func agentRailUnfoldHandle(", to: "private func agentDrawerResizeHandle(")
    let adjustable = try section(
      handle, from: ".accessibilityAdjustableAction", to: ".accessibilityIdentifier(")
    XCTAssertTrue(
      compact(adjustable).contains(
        "if direction == .increment { state.unfoldAgentList() }"))
  }

  func testTheRosterInboxUsesAnEnvelopeInBothTheRailAndConversationRow() throws {
    let source = try overlaySource("OverlayChannelStatusView.swift")
    let pill = try section(source, from: "func railPillDisplay(", to: "var channelMapModel:")
    XCTAssertTrue(pill.contains("symbol: inbox ? \"envelope\" : nil"))
    let row = try section(
      source, from: "private func conversationRow(", to: "private func savedConversationDetail(")
    XCTAssertTrue(
      row.contains(
        "Image(systemName: conversation.isHumanInbox ? \"envelope\" : saved ? \"clock\" : \"bubble.left\")"))
    XCTAssertFalse(source.contains("\"tray\""))
  }

  func testIntercomInboxUsesAnEnvelopeInItsMenuAndEveryCardPresentation() throws {
    let source = try overlaySource("OverlayIntercomWidget.swift")
    XCTAssertTrue(
      compact(source).contains(
        "inbox, title: String(localized: \"Your Inbox\"), symbol: \"envelope\", identifier: \"overlay-intercom-inbox\""))
    XCTAssertEqual(
      compact(source).components(
        separatedBy: "if identity.isHumanInbox { Image(systemName: \"envelope\").csFont(11)")
        .count - 1,
      2, "both icon and row cards identify the inbox with an envelope")
    XCTAssertTrue(
      compact(source).contains(
        "if instance.isHumanInbox { Image(systemName: \"envelope\").csFont(11)"))
    XCTAssertFalse(source.contains("\"tray\""))
  }

  func testJumpToCurrentAlwaysClearsTheMeasuredComposer() throws {
    let source = try overlaySource("OverlayConversationView.swift")
    let jump = try section(
      source,
      from: ".overlay(alignment: .bottom) {\n          ZStack {",
      to: ".overlay(alignment: .top) {")
    XCTAssertTrue(jump.contains(".accessibilityIdentifier(\"overlay-conversation-jump-to-current\")"))
    XCTAssertTrue(jump.contains(".padding(.bottom, visibleComposerHeight + 10)"))
    XCTAssertFalse(
      compact(jump).contains("presentation"), "the jump offset applies to both presentations")
  }

  func testStandardComposerReportsItsHeightInsideTheBottomSafeAreaInset() throws {
    let source = try overlaySource("OverlayConversationView.swift")
    let inset = try section(
      source, from: ".safeAreaInset(edge: .bottom, spacing: 0) {", to: ".overlay(alignment: .bottom) {")
    XCTAssertTrue(
      compact(inset).contains(
        "if showsComposer && presentation == .standard { composer .onGeometryChange(for: CGFloat.self) { $0.size.height } action: { composerHeight = $0 } }"))
  }

  func testRosterMessageScaleMatchesTheConversationBodyFont() {
    XCTAssertEqual(
      OverlayChannelStatusView.messageListScale,
      OverlayConversationMessageContent.bodyFontSize / 13, accuracy: 0.001)
  }

  func testOnlyTheExpandedRosterMultipliesTheWindowTextScaleByMessageScale() throws {
    let source = try overlaySource("OverlayChannelStatusView.swift")
    let monitor = try section(source, from: "func monitorBody(", to: "static var messageListScale:")
    XCTAssertTrue(monitor.contains("installed.textScale = scale * Self.messageListScale"))
    XCTAssertTrue(monitor.contains("return installed.scaledMonitorBody("))
    let rail = try section(source, from: "func railBody(", to: "private func scaledRailBody(")
    XCTAssertTrue(compact(rail).contains("installed.textScale = scale return installed.scaledRailBody("))
    XCTAssertFalse(rail.contains("messageListScale"))
  }

  private func overlaySource(_ name: String) throws -> String {
    let root = URL(fileURLWithPath: #filePath).deletingLastPathComponent()
      .deletingLastPathComponent()
    return try String(
      contentsOf: root.appendingPathComponent("Codescribe/Screens/Overlay/\(name)"),
      encoding: .utf8)
  }

  private func section(_ source: String, from: String, to: String) throws -> String {
    let start = try XCTUnwrap(source.range(of: from), "missing section start: \(from)")
    let end = try XCTUnwrap(
      source.range(of: to, range: start.upperBound..<source.endIndex),
      "missing section end: \(to)")
    return String(source[start.lowerBound..<end.lowerBound])
  }

  private func compact(_ source: String) -> String {
    source.components(separatedBy: .newlines)
      .filter { !$0.trimmingCharacters(in: .whitespaces).hasPrefix("//") }
      .joined(separator: " ")
      .split(whereSeparator: \.isWhitespace).joined(separator: " ")
  }
}
