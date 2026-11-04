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

  func testDrawerWidthAllowsDraggingBelowTheRowMinimumButStopsAtTheRail() {
    for scale in scales {
      let rail = OverlayAgentDrawerLayout.railWidth(textScale: scale)
      let dragged = OverlayAgentDrawerLayout.drawerWidth(
        textScale: scale, preferredWidth: 120, windowWidth: 1_200 * scale,
        minimumWidth: rail)
      XCTAssertEqual(dragged, 120 * scale, accuracy: 0.001)
      XCTAssertLessThan(dragged, OverlayAgentDrawerLayout.contentMinimumWidth(textScale: scale))
      XCTAssertEqual(
        OverlayAgentDrawerLayout.drawerWidth(
          textScale: scale, preferredWidth: 0, windowWidth: 1_200 * scale,
          minimumWidth: rail),
        rail, accuracy: 0.001)
      XCTAssertEqual(
        OverlayAgentDrawerLayout.drawerWidth(
          textScale: scale, preferredWidth: 0, minimumWidth: 100 * scale),
        100 * scale, accuracy: 0.001, "the supplied minimum is already in window points")
    }
  }

  func testDraggingStillLeavesTheConversationMinimumAtWideWindowSizes() {
    for scale in scales {
      let width = OverlayAgentDrawerLayout.drawerWidth(
        textScale: scale, preferredWidth: 900, windowWidth: 1_200 * scale,
        minimumWidth: OverlayAgentDrawerLayout.railWidth(textScale: scale))
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

  func testBothPaneWidthProjectionsUseTheRailMinimumOnlyDuringDragging() throws {
    let source = try overlaySource("DictationOverlayView.swift")
    let geometry = try section(
      source, from: "let dragMinimumWidth: CGFloat?", to: "let canvasHidden =")
    XCTAssertTrue(
      compact(geometry).contains(
        "let dragMinimumWidth: CGFloat? = agentDrawerResize == nil ? nil : railWidth"))
    XCTAssertEqual(
      geometry.components(separatedBy: "minimumWidth: dragMinimumWidth").count - 1, 2,
      "both the visible column and the conversation's stable split width follow the drag")
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
    XCTAssertEqual(OverlayConversationMessageContent.bodyFontSize, 13, accuracy: 0.001)
  }

  func testOnlyTheExpandedRosterMultipliesTheWindowTextScaleByMessageScale() throws {
    let source = try overlaySource("OverlayChannelStatusView.swift")
    let monitor = try section(source, from: "func monitorBody(", to: "func railBody(")
    XCTAssertTrue(
      compact(monitor).contains(
        "installed.textScale = scale return installed.scaledMonitorBody("))
    XCTAssertFalse(monitor.contains("messageListScale"))
    let rail = try section(source, from: "func railBody(", to: "private func scaledRailBody(")
    XCTAssertTrue(compact(rail).contains("installed.textScale = scale return installed.scaledRailBody("))
    XCTAssertFalse(rail.contains("messageListScale"))
  }

  func testCompactReceptionIndicatorsGiveMoonPrimaryContrastAndAntennaMutedContrast() throws {
    let source = try overlaySource("OverlayChannelStatusView.swift")
    let indicators = try section(
      source, from: "func compactReceptionIndicators(for channel: OverlayChannelDelivery)",
      to: "func deliveryReceiptDisplay(")
    let compactIndicators = compact(indicators)
    XCTAssertTrue(
      compactIndicators.contains(
        "Image(systemName: \"moon.fill\") .foregroundStyle(palette.primaryText.color)"))
    XCTAssertTrue(
      compactIndicators.contains(
        "Image(systemName: \"antenna.radiowaves.left.and.right.slash\") .foregroundStyle(palette.mutedText.color)"
      ))
  }

  func testMessageBubbleWrapsFooterActionsAndKeepsLeadingFrameAlignmentOnNarrowWidth() throws {
    let source = try overlaySource("OverlayConversationView.swift")
    let row = try section(
      source, from: "private func messageRow(", to: "func messageBubble(")
    XCTAssertTrue(
      compact(row).contains(
        "if message.kind == .user { Spacer(minLength: 0) } messageBubble(message) .frame(maxWidth: maxBubbleWidth, alignment: .leading)"
      ))
    let bubble = try section(
      source, from: "func messageBubble(", to: "private func messageActions(")
    XCTAssertTrue(
      compact(bubble).contains(
        "ViewThatFits(in: .horizontal) { messageActions(message, stacked: false) messageActions(message, stacked: true) }"
      ))
    let footer = try section(
      source, from: "private struct OverlayConversationMessageFooter",
      to: "private var footerRow: some View")
    XCTAssertTrue(
      compact(footer).contains("if adaptsToWidth { ViewThatFits(in: .horizontal) { footerRow"))
  }

  func testPerformCopyChordTriesSelectedBubbleFragmentWhenFirstResponderIsAnEmptyTextView() throws {
    let source = try overlaySource("DictationOverlayWindow.swift")
    let chord = try section(
      source, from: "func performCopyChord(_ event: NSEvent) -> Bool {",
      to: "return NSApp.sendAction(#selector(NSText.copy(_:)), to: nil, from: self)")
    let compactChord = compact(chord)
    XCTAssertTrue(compactChord.contains("let before = NSPasteboard.general.changeCount"))
    XCTAssertTrue(
      compactChord.contains(
        "_ = textView.nextResponder?.tryToPerform(#selector(NSText.copy(_:)), with: nil)"))
    XCTAssertTrue(
      compactChord.contains(
        "if NSPasteboard.general.changeCount == before { _ = NSApp.sendAction(#selector(NSText.copy(_:)), to: nil, from: self) }"
      ))
    XCTAssertTrue(
      compactChord.contains("if NSPasteboard.general.changeCount != before { return true }"))
  }

  func testNarrowSplitColumnsDoNotClipRightEdgesAtRest() throws {
    let overlay = try overlaySource("DictationOverlayView.swift")
    let column = try section(
      overlay, from: "private func agentDrawerColumn(",
      to: "private func commitAgentDrawerWidth(")
    XCTAssertTrue(
      compact(column).contains(
        "let contentMinimum = agentDrawerResize == nil ? width : OverlayAgentDrawerLayout.contentMinimumWidth(textScale: textScale)"
      ))
    let conversation = try overlaySource("OverlayConversationView.swift")
    let body = try section(
      conversation, from: "ScrollViewReader { proxy in", to: ".onChange(of: replyNavigationID)")
    XCTAssertTrue(
      compact(body).contains(
        "maxBubbleWidth: Self.bubbleCap( contentWidth: max(0, geometry.size.width - OverlayResizeHit.scrollbarInset), textScale: textScale)"
      ))
  }

  @MainActor
  func testRosterPreviewSessionBrowsesLastFiveMessagesWithoutWrappingAndFreezesWhileOpen() {
    let makeMessage = { (id: String, text: String) in
      OverlayConversationMessage(
        id: id, kind: .reply, text: text, order: 1,
        emittedAt: "2026-11-04T02:00:00Z", owner: nil, recipients: [],
        deliveryID: id, replyTo: nil, unsolicited: false, playback: nil, busPath: "")
    }
    let emptyConversation = OverlayConversation(
      id: "2", channel: "2", name: "Tola", owner: nil, messages: [])
    let emptySession = OverlayRosterPreviewSession(conversation: emptyConversation)
    XCTAssertTrue(emptySession.ids.isEmpty)
    XCTAssertFalse(emptySession.canStepOlder)
    XCTAssertFalse(emptySession.canStepNewer)

    let messages = (1...6).map { makeMessage("m\($0)", "Body \($0)") }
    let conversation = OverlayConversation(
      id: "2", channel: "2", name: "Tola", owner: nil, messages: messages)
    let session = OverlayRosterPreviewSession(conversation: conversation)
    session.openLatest(ownerID: "2")
    XCTAssertEqual(session.ids, ["m2", "m3", "m4", "m5", "m6"])
    XCTAssertEqual(session.index, 4)
    XCTAssertTrue(session.canStepOlder)
    XCTAssertFalse(session.canStepNewer)

    session.stepNewer()
    XCTAssertEqual(session.index, 4, "must not wrap past newest")

    session.stepOlder()
    session.stepOlder()
    XCTAssertEqual(session.index, 2)
    XCTAssertEqual(session.ids[session.index], "m4")

    // A new message arriving while browsing must not shift the viewed slot.
    let updatedMessages = messages + [makeMessage("m7", "Body 7")]
    let updatedConversation = OverlayConversation(
      id: "2", channel: "2", name: "Tola", owner: nil, messages: updatedMessages)
    session.update(updatedConversation, withdrawnIDs: ["m3"])
    XCTAssertEqual(session.ids, ["m2", "m3", "m4", "m5", "m6"])
    XCTAssertEqual(session.index, 2)
    XCTAssertEqual(session.message(for: "m4")?.text, "Body 4")
    XCTAssertTrue(session.isWithdrawn("m3"))
    XCTAssertNil(session.message(for: "m3"))

    session.stepOlder()
    session.stepOlder()
    XCTAssertEqual(session.index, 0)
    XCTAssertFalse(session.canStepOlder)
    session.stepOlder()
    XCTAssertEqual(session.index, 0, "must not wrap past oldest")

    session.close()
    XCTAssertEqual(session.ids, ["m3", "m4", "m5", "m6", "m7"])
    XCTAssertEqual(session.index, 4)
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
