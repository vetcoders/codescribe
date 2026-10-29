import AppKit
import SwiftUI
import XCTest

@testable import Codescribe

@MainActor
final class OverlayAgentRosterLayoutTests: XCTestCase {
  func testClockStaysAtTrailingEdgeAcrossWidthsAndTextScales() throws {
    try BackgroundTestWindow.requireIsolatedGUI()
    for width in [CGFloat(240), 300, 400] {
      for scale in [CGFloat(1), 1.5, 2] {
        let host = NSHostingView(
          rootView: OverlayAgentRosterLabel(
            title: "Assistant", channel: "3", conversationID: "fixture",
            descriptor: "Codex · model · repository",
            preview: "Founder → Astra: a message preview that can wrap onto two lines.",
            timestamp: Date(timeIntervalSince1970: 1_792_512_000), unreadCount: 8,
            dot: .idle, statusLabel: "Attached", statusDetail: "Fixture",
            palette: .dark, textScale: scale
          ).frame(width: width))
        let window = show(host, width: width)
        defer { window.close() }
        let clock = try XCTUnwrap(
          NativeAccessibilityProbe.waitForElements("overlay-agent-timestamp-fixture").elements.first
        )
        let title = try XCTUnwrap(
          NativeAccessibilityProbe.waitForElements("overlay-agent-title-3").elements.first)
        XCTAssertEqual(clock.frame.maxX, window.frame.maxX, accuracy: 1)
        XCTAssertGreaterThan(
          title.width, 0, "the identity retains space at \(width), scale \(scale)")
        XCTAssertLessThanOrEqual(title.frame.maxX, clock.frame.minX + 1)
        XCTAssertGreaterThan(clock.height, 0)
      }
    }
  }

  func testCompactRosterUsesPassiveRestrictionsWhileIntercomKeepsItsOwnControls() throws {
    let label = try sourceSection(
      "OverlayAgentRosterLabel.swift", from: "struct OverlayAgentRosterLabel: View {",
      to: "struct OverlayAgentUnreadBadge: View {")
    try assertOrder(
      [
        "Text(verbatim: channel)", "OverlayAgentStatusDot(",
        "Text(verbatim: title)", "titleAccessory", "Spacer(minLength: 4)",
      ], in: label)
    XCTAssertFalse(label.contains("OverlayReceptionMarks("))

    let card = try sourceSection(
      "OverlayIntercomWidget.swift", from: "private func cardHeader(",
      to: "private func audioSlot(")
    let richRow = try XCTUnwrap(card.range(of: "HStack(spacing: 6) {"))
    try assertOrder(
      [
        "Text(verbatim: identity.displayDigit)", "OverlayReceptionMarks(",
        "OverlayIntercomStatusMark(",
        "Text(verbatim: name)", "OverlayIntercomFocusButton(", "audioSlot(",
      ], in: String(card[richRow.lowerBound...]))

    let instance = try sourceSection(
      "OverlayIntercomWidget.swift", from: "private func headerRow(",
      to: "private struct OverlayIntercomResizeHandle:")
    let drawn = try XCTUnwrap(instance.range(of: "return HStack(spacing: 3) {"))
    try assertOrder(
      [
        "Text(verbatim: instance.displayDigit)", "OverlayReceptionMarks(",
        "OverlayIntercomStatusMark(",
        "if mini {", "OverlayIntercomFocusButton(", "OverlayIntercomAudioControls(",
      ], in: String(instance[drawn.lowerBound...]))
  }

  func testCompactRosterKeepsAudioAndMoreSeparateFromConversationNavigation() throws {
    let row = try sourceSection(
      "OverlayChannelStatusView.swift", from: "func channelRow(",
      to: "private func rosterRowLabel(")
    XCTAssertTrue(row.contains("compactAgentActions(for: channel)"))
    XCTAssertFalse(row.contains("rosterRowState("))
    XCTAssertFalse(row.contains("panelFocusAccessory("))

    let audio = try sourceSection(
      "OverlayChannelStatusView.swift", from: "struct OverlayAgentAudioControls: View {",
      to: "struct OverlayAgentPillMenuAction {")
    try assertOrder(["Button(action: onMicrophone)", "Button(action: onPlayback)"], in: audio)
  }

  func testIntercomWidthBudgetsEveryReorderedSlotAtEachScale() {
    for scale in [CGFloat(1), 1.5, 2] {
      let measuredText: CGFloat = 240
      let reception = 28 * scale + 3
      let statusDot = 8 * scale
      let identityGaps: CGFloat = 3 * 6
      let leadingPadding: CGFloat = 12
      let focusAndAudio = 3 * OverlayIntercomWidgetLayout.controlHeight(scale: scale) + 6
      let outerGaps: CGFloat = 2 * 3
      let expected =
        measuredText + reception + statusDot + identityGaps + leadingPadding
        + focusAndAudio + outerGaps
      XCTAssertEqual(
        OverlayIntercomWidgetLayout.restingWidth(
          headerText: 0, agentText: measuredText, scale: scale), expected, accuracy: 0.001)
      let miniExpected =
        measuredText + reception + statusDot
        + 3 * OverlayIntercomWidgetLayout.controlHeight(scale: scale)
        + 26 * max(1, scale) + 6 + 6 * 3 + 12 + 4
      XCTAssertEqual(
        OverlayIntercomWidgetLayout.minimizedInstanceWidth(text: measuredText, scale: scale),
        miniExpected, accuracy: 0.001)
    }
  }

  func testOnlyConfirmedDeadFollowersGreyRowsAndOverrideStaleOpenDots() {
    let channel = OverlayChannelDelivery(
      channel: "3", agent: "Assistant", deliveryID: "fixture", stage: .received, isOpen: false)
    let claims: [Bool?] = [false, true, nil]
    for alive in claims {
      for open in [false, true] {
        let roster = OverlayChannelStatusView(
          channels: [channel], unavailable: false, palette: .dark, animates: false,
          hudStates: [
            "3": OverlayChannelHudProjection(
              open: open, loud: false, autosealDeadline: nil, followerAlive: alive)
          ])
        XCTAssertEqual(roster.hasDeadFollower(channel), alive == false)
        let expected: OverlayAgentDotKind =
          alive == false ? .sleeping : open ? .listening : .received
        XCTAssertEqual(roster.statusDotKind(for: channel), expected)
      }
    }
  }

  /// The middle-click chooser lives on the vertical Intercom (Maciej,
  /// 2026-10-23 seal): its rows and its header carry the wheel's ring, and
  /// the app hosts the chooser there instead of summoning the big overlay.
  func testTheVerticalIntercomHostsTheMiddleClickChooser() throws {
    let cardBody = try sourceSection(
      "OverlayIntercomWidget.swift",
      from: "let expanded = controller.presentation.expanded.contains(identity)",
      to: "private func cardHeader(")
    XCTAssertTrue(
      cardBody.contains(
        "focused: identity.conversationID != nil && state.agentGestureFocusID == identity.conversationID"
      ))
    XCTAssertTrue(cardBody.contains("onHover: { state.hoverAgentGestureTarget($0) }"))

    let header = try sourceSection(
      "OverlayIntercomWidget.swift", from: "private struct OverlayIntercomHeader: View {",
      to: "private struct OverlayIntercomWindowProjection")
    XCTAssertTrue(header.contains("state.agentGestureFocusID == broadcast?.id"))
    XCTAssertTrue(header.contains("controller.revealChooserFocus(focus)"))

    // An agent in its own window stays on the wheel's path (Maciej, seal
    // 2026-10-23 20:31Z: "pomija — to bardzo źle") and wears the same ring.
    let targets = try sourceSection(
      "OverlayIntercomWidget.swift", from: "func chooserTargets()", to: "func presentChooser()")
    XCTAssertTrue(targets.contains("identities.compactMap"))
    XCTAssertFalse(targets.contains("groupIdentities"))
    let detached = try sourceSection(
      "OverlayIntercomWidget.swift", from: "private func headerRow(",
      to: "private func instanceLayout(")
    XCTAssertTrue(
      detached.contains("focused: state.agentGestureFocusID == instance.conversationID"))

    let app = try sourceSection(
      "../../Core/AppModel.swift", from: "private func syncIntercomPresentation(",
      to: "func handleAgentGesture(")
    XCTAssertTrue(app.contains("state.agentChooserHost = wantsWidget"))
    let stop = try sourceSection(
      "../../Core/AppModel.swift", from: "func stopIntercomPresentation()",
      to: "private func syncIntercomPresentation(")
    XCTAssertTrue(stop.contains("state.agentChooserHost = nil"))
    let gesture = try sourceSection(
      "../../Core/AppModel.swift", from: "func handleAgentGesture(",
      to: "if let panel, panel.isVisible")
    XCTAssertTrue(gesture.contains("intercomWidget.presentChooser()"))
  }

  /// The small conversation shrinks on its own (Maciej, 2026-10-23: "czy ten
  /// tekst zawsze będzie taki wielki"): ⌘+/-/0 in an Intercom window adjusts
  /// the Intercom's scale, not the overlay's.
  func testTheIntercomHasItsOwnTextScale() throws {
    let model = try sourceSection(
      "../../Core/AppModel.swift", from: "final class OverlayController",
      to: "private func syncIntercomPresentation(")
    XCTAssertTrue(
      model.contains(
        "let intercomTextScale = TextScaleController(key: \"IntercomPanel.textScale.v1\")"))
    let sync = try sourceSection(
      "../../Core/AppModel.swift", from: "private func syncIntercomPresentation(",
      to: "func handleAgentGesture(")
    XCTAssertTrue(sync.contains("state: state, textScale: intercomTextScale,"))
    let shortcuts = try sourceSection(
      "../../App.swift", from: "private func textScaleController(for window: NSWindow?)",
      to: "func applicationShouldTerminateAfterLastWindowClosed(")
    XCTAssertTrue(
      shortcuts.contains(
        "if window is OverlayIntercomPanel { return model.overlay.intercomTextScale }"))
  }

  /// Maciej, 2026-10-23: Igor, Wiktoria and Olga read white while nobody read
  /// their channels; an inactive agent shows it explicitly. A follower that
  /// heartbeats over a backlog nobody read counts like a dead one.
  func testInactiveAgentsDimAndWearAnExplicitMark() throws {
    let channel = OverlayChannelDelivery(
      channel: "8", agent: "olga", deliveryID: "d8", stage: .received, isOpen: false)
    func roster(alive: Bool?, unreadSince: Date? = nil) -> OverlayChannelStatusView {
      OverlayChannelStatusView(
        channels: [channel], unavailable: false, palette: .dark, animates: false,
        hudStates: [
          "8": OverlayChannelHudProjection(
            open: false, loud: false, autosealDeadline: nil, followerAlive: alive,
            unreadSince: unreadSince)
        ])
    }
    let stale = Date().addingTimeInterval(-(OverlayState.followerStallHorizon + 60))
    XCTAssertTrue(roster(alive: false).isInactive(channel), "a gone follower")
    XCTAssertTrue(roster(alive: true, unreadSince: stale).isInactive(channel), "nobody reads")
    XCTAssertEqual(roster(alive: true, unreadSince: stale).statusSymbol(for: channel), "zzz")
    XCTAssertFalse(roster(alive: true).isInactive(channel), "a reading agent")
    XCTAssertFalse(
      roster(alive: true, unreadSince: Date()).isInactive(channel), "a fresh unread message")
    XCTAssertFalse(roster(alive: nil).isInactive(channel), "no claim is not a claim of death")

    let widget = try overlaySource("OverlayIntercomWidget.swift")
    XCTAssertFalse(widget.contains("roster.hasDeadFollower("), "every Intercom dim reads both")
    XCTAssertEqual(
      widget.components(separatedBy: "OverlayDisconnectedRowDim(disconnected: inactive)").count
        - 1, 4)
    XCTAssertEqual(widget.components(separatedBy: "OverlayIntercomStatusMark(").count - 1, 3)
    XCTAssertFalse(widget.contains("OverlayAgentStatusDot( kind: intercomStatusDot("))
  }

  /// Rows drag the group and resize it by every edge; tear-off is gone
  /// from rows (Maciej, 2026-10-23 dictation).
  func testRowsMoveAndResizeTheGroupInsteadOfTearingOff() throws {
    let widget = try overlaySource("OverlayIntercomWidget.swift")
    XCTAssertFalse(widget.contains("tearOff"))
    XCTAssertFalse(widget.contains("tornOffTile"))
    let move = try sourceSection(
      "OverlayIntercomWidget.swift", from: "fileprivate func moveGroup(",
      to: "private func detachIntoInstance(")
    XCTAssertTrue(move.contains("let pointer = NSEvent.mouseLocation"), "screen points")
    XCTAssertTrue(move.contains("header.setFrameOrigin("), "the header carries the rows")
    let body = try sourceSection(
      "OverlayIntercomWidget.swift", from: "private struct OverlayIntercomCard: View {",
      to: "private func cardHeader(")
    XCTAssertTrue(body.contains(".onChanged { _ in controller.moveGroup() }"))
    XCTAssertTrue(body.contains(".onEnded { _ in controller.moveGroup(ended: true) }"))
    XCTAssertTrue(body.contains("sides: true,"))
    XCTAssertTrue(
      body.contains("from: frame, delta: delta, edge: edge, row: identity, gesture: gesture)"))
    XCTAssertTrue(
      body.contains(".frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .top)"),
      "progressive content starts under the fixed header")
    let handles = try sourceSection(
      "OverlayIntercomWidget.swift", from: "private struct OverlayIntercomResizeHandles: View {",
      to: "private struct OverlayIntercomInstanceCard: View {")
    for edge in [".left", ".right", ".top", ".bottom", ".bottomLeft", ".bottomRight"] {
      XCTAssertTrue(handles.contains("OverlayIntercomResizeHandle(edge: \(edge),"), edge)
    }
    let header = try sourceSection(
      "OverlayIntercomWidget.swift", from: "private struct OverlayIntercomHeader: View {",
      to: "private struct OverlayIntercomWindowProjection:")
    XCTAssertTrue(header.contains("OverlayIntercomResizeHandles(corners: false, right: true)"))
  }

  /// "Przydym też troszkę te hiperbiałe przyciski … nie tylko w pierwszym"
  /// and "można by się tego fioletu pozbyć … na rzecz rozjaśnienia" (Maciej,
  /// 2026-10-23): flat controls rest in the wordmark's muted ink, names rest
  /// smoked and brighten instead of turning violet.
  func testIntercomControlsRestSmokedAndNamesBrightenInsteadOfViolet() throws {
    let roster = try overlaySource("OverlayChannelStatusView.swift")
    XCTAssertFalse(roster.contains("AnyShapeStyle(.foreground)"), "an explicit ink is primary")
    XCTAssertTrue(roster.contains("return chrome ? AnyShapeStyle(palette.primaryText.color) : nil"))
    let focus = try sourceSection(
      "OverlayIntercomWidget.swift", from: "private struct OverlayIntercomFocusButton: View {",
      to: "private struct OverlayIntercomConversationBody: View {")
    XCTAssertFalse(focus.contains("action.enabled ? palette.primaryText.color"))
    XCTAssertTrue(
      focus.contains(
        "restingColor: palette.mutedText.color, hoverColor: palette.primaryText.color"))
    let widget = try overlaySource("OverlayIntercomWidget.swift")
    XCTAssertFalse(widget.contains("speaking ? palette.agentAccent.color"))
    XCTAssertFalse(widget.contains("hoverColor: palette.agentAccent.color"))
    XCTAssertEqual(
      widget.components(
        separatedBy: "speaking ? palette.primaryText.color : palette.bodyText.color"
      )
      .count - 1, 3)
  }

  func testDeadRowsDimAndDisableAudioWhileReceptionRemainsActionable() throws {
    let row = try sourceSection(
      "OverlayChannelStatusView.swift", from: "func channelRow(",
      to: "private func rosterRowLabel(")
    XCTAssertEqual(row.components(separatedBy: "&& !hasDeadFollower(channel)").count - 1, 2)
    XCTAssertTrue(row.contains("OverlayDisconnectedRowDim(disconnected: hasDeadFollower(channel))"))

    let audio = try sourceSection(
      "OverlayIntercomWidget.swift", from: "private struct OverlayIntercomAudioControls: View {",
      to: "private func intercomStatusDot(")
    XCTAssertEqual(audio.components(separatedBy: "followerAlive != false").count - 1, 2)
    XCTAssertFalse(audio.contains("state.conversationMicrophoneOpen(conversation) ||"))

    let card = try sourceSection(
      "OverlayIntercomWidget.swift", from: "private func cardHeader(",
      to: "private func audioSlot(")
    // An agent in its own window leaves the stack (Maciej 17:48Z): no row
    // dims for a detached window, and the row carries no window glyph.
    XCTAssertTrue(card.contains("OverlayDisconnectedRowDim(disconnected: inactive)"))
    XCTAssertFalse(card.contains("detachedInstance"))
    XCTAssertFalse(card.contains("Image(systemName: \"macwindow\")"))
    // A collapsed row moves the group; it never tears off (Maciej,
    // 2026-10-23: only an unrolled card detaches, by its grip).
    XCTAssertEqual(card.components(separatedBy: ".simultaneousGesture(moveGesture").count - 1, 2)
    XCTAssertFalse(card.contains("tearOff"))
    XCTAssertTrue(
      card.contains("if expanded { Button(\"Detach conversation\") { controller.detach(identity) }")
    )
    XCTAssertTrue(
      card.contains(
        "OverlayReceptionMarks(channel: channelRow, palette: palette, textScale: scale)"))
    // A narrow row keeps the microphone and speaker at its right edge; the
    // antenna and moon wait for the width (Maciej, 2026-10-23 seal).
    XCTAssertTrue(card.contains("if let channelRow, row.marks {"))
    XCTAssertTrue(card.contains("shared: row.slots == 1)"))
    // Hover moves no row out of the column and adds no preview line; the
    // icon square names its agent in the hint panel instead.
    XCTAssertFalse(card.contains("hovered"))
    XCTAssertTrue(
      card.contains(
        "inactive ? name + \" · \" + roster.detail(for: $0) : nil } ?? name, palette: palette, leading: true"
      ))
    let cardBody = try sourceSection(
      "OverlayIntercomWidget.swift",
      from: "let expanded = controller.presentation.expanded.contains(identity)",
      to: "private func cardHeader(")
    XCTAssertFalse(cardBody.contains(".onHover"))
    XCTAssertTrue(cardBody.contains("OverlayIntercomWidgetLayout.rowCornerRadius(scale: scale)"))

    let miniFrame = try sourceSection(
      "OverlayIntercomWidget.swift", from: "private func instanceFrame(",
      to: "fileprivate func setInstanceMinimized(")
    XCTAssertTrue(miniFrame.contains("text: nameWidth + digitWidth"))
    // A saved fiszka controls only its exact owner (Copilot on #183): once the
    // digit is reassigned, its row belongs to the replacement agent.
    let instanceHeader = try sourceSection(
      "OverlayIntercomWidget.swift", from: "private func headerRow(",
      to: "private func instanceLayout(")
    XCTAssertTrue(
      instanceHeader.contains(
        "let channelRow = current ? intercomChannelRow(state: state, digit: instance.digit) : nil"))
    let instanceControls = try sourceSection(
      "OverlayIntercomWidget.swift", from: "private func instanceControls(",
      to: "private struct OverlayIntercomResizeHandle")
    XCTAssertTrue(
      instanceControls.contains(
        "let channel = current ? intercomChannelRow(state: state, digit: instance.digit) : nil"))
    let dim = try sourceSection(
      "OverlayChannelStatusView.swift", from: "struct OverlayDisconnectedRowDim: ViewModifier {",
      to: "struct OverlayAgentsSidebarControls {")
    XCTAssertEqual(OverlayDisconnectedRowDim.opacity, 0.45)
    XCTAssertTrue(dim.contains(".opacity(disconnected ? Self.opacity : 1)"))
    XCTAssertTrue(dim.contains(".grayscale(disconnected ? 1 : 0)"))
    let marks = try sourceSection(
      "OverlayChannelStatusView.swift", from: "struct OverlayReceptionMarks: View {",
      to: "struct OverlayChannelStatusView: View {")
    XCTAssertTrue(
      marks.contains(".disabled(!enabled || pending || channel.broadcastIdentity == nil)"))
    XCTAssertFalse(marks.contains("followerAlive"), "policy writes do not need a live follower")
  }

  func testUnlitMoonRecedesWhileLitMoonFillsInTheTextColour() throws {
    XCTAssertEqual(AgentBroadcastPolicy.on.quietSymbol, "moon")
    XCTAssertEqual(AgentBroadcastPolicy.quiet.quietSymbol, "moon.fill")
    let marks = try sourceSection(
      "OverlayChannelStatusView.swift", from: "struct OverlayReceptionMarks: View {",
      to: "struct OverlayChannelStatusView: View {")
    try assertOrder(["symbol: policy.statusSymbol", "symbol: policy.quietSymbol"], in: marks)
    // Shape is the whole signal: no amber, no accent on the lit moon.
    XCTAssertTrue(
      marks.contains(
        "symbol: policy.quietSymbol, lit: policy.moon, litColor: palette.primaryText.color, unlitOpacity: 0.45"
      ))
    XCTAssertFalse(
      marks.contains("litColor: palette.processingStatus"), "the lit moon is not a warning")
    XCTAssertTrue(marks.contains("unlitOpacity: Double = 1"), "the antenna keeps its existing tone")
    XCTAssertTrue(marks.contains("lit ? litColor : palette.mutedText.color.opacity(unlitOpacity)"))
  }

  func testActionButtonsSitBelowThePrimaryLabel() throws {
    try BackgroundTestWindow.requireIsolatedGUI()
    let owner = try XCTUnwrap(
      OverlayConversationOwner(row: [
        "provider": "codex", "provider_session_id": "roster-layout",
        "lease_id": String(repeating: "a", count: 32), "channel": "3", "name": "Astra",
      ]))
    let conversation = OverlayConversation(
      id: owner.id, channel: "3", name: "Astra", owner: owner, messages: [])
    let channel = OverlayChannelDelivery(
      channel: "3", agent: "Astra", deliveryID: nil, stage: nil, isOpen: false)
    var row = OverlayChannelStatusView(
      channels: [channel], unavailable: false, palette: .dark, animates: false,
      hudStates: [
        "3": OverlayChannelHudProjection(
          open: false, loud: false, autosealDeadline: nil, followerAlive: true,
          provider: owner.provider, providerSessionID: owner.providerSessionID, frameDetected: true)
      ],
      onToggleChannel: { _ in }, conversations: [conversation], onSelectConversation: { _ in })
    row.archiveCandidates = ["3": owner]
    row.onArchiveAgent = { _ in }
    row.panelFocusAvailable = true
    row.onFocusAgentPanel = { _ in }
    for width in [CGFloat(240), 300, 400] {
      let host = NSHostingView(rootView: row.channelRow(channel).frame(width: width))
      let window = show(host, width: width)
      defer { window.close() }
      let label = try XCTUnwrap(
        NativeAccessibilityProbe.waitForElements("overlay-view-conversation-\(owner.id)")
          .elements.first)
      let controls = try NativeAccessibilityProbe.waitForElements("overlay-agent-row-controls-3")
      XCTAssertFalse(controls.elements.isEmpty, controls.trace.joined(separator: "\n"))
      XCTAssertGreaterThan(label.width, width * 0.8, "controls do not reserve a full-height column")
      // Native AX coordinates grow down from the top of the screen.
      for control in controls.elements {
        XCTAssertGreaterThanOrEqual(control.y, label.frame.maxY - 1)
        XCTAssertLessThanOrEqual(control.frame.maxX, window.frame.maxX + 1)
      }
    }
  }

  func testFirstRevealHasRailBeforeAnyConversationWasOpened() {
    for width in [CGFloat(700), 1200] {
      let state = OverlayState.previewFormatted()
      XCTAssertNil(state.selectedConversationID)
      let recorder = FirstRevealRailRecorder()
      let host = NSHostingView(
        rootView: DictationOverlayView(state: state)
          .onPreferenceChange(OverlayAgentRailFramePreferenceKey.self) { recorder.frame = $0 }
          .frame(width: width, height: 500))
      host.sizingOptions = []
      // This case measures first-layout preferences, not AX. Keep it away
      // from the physical pointer, whose legitimate rail hover opens the list.
      let window = show(host, width: width, origin: NSPoint(x: -10000, y: -10000))
      defer { window.close() }
      XCTAssertFalse(state.showsAgentMonitor)
      XCTAssertEqual(
        recorder.frame.width, OverlayAgentDrawerLayout.railWidth(textScale: 1), accuracy: 1)
      XCTAssertEqual(recorder.frame.maxX, width, accuracy: 1)
      state.unfoldAgentList()
      settle(host)
      XCTAssertTrue(state.showsAgentMonitor)
      state.foldAgentList()
      settle(host)
      XCTAssertEqual(
        recorder.frame.width, OverlayAgentDrawerLayout.railWidth(textScale: 1), accuracy: 1)
      XCTAssertNil(state.selectedConversationID)
    }
  }

  private func overlaySource(_ file: String) throws -> String {
    let url = URL(fileURLWithPath: #filePath).deletingLastPathComponent()
      .deletingLastPathComponent().appendingPathComponent("Codescribe/Screens/Overlay/\(file)")
    return try String(contentsOf: url, encoding: .utf8).split(whereSeparator: \.isWhitespace)
      .joined(separator: " ")
  }

  private func sourceSection(_ file: String, from start: String, to end: String) throws -> String {
    let url = URL(fileURLWithPath: #filePath).deletingLastPathComponent()
      .deletingLastPathComponent().appendingPathComponent("Codescribe/Screens/Overlay/\(file)")
    let source = try String(contentsOf: url, encoding: .utf8)
    let first = try XCTUnwrap(source.range(of: start))
    let last = try XCTUnwrap(source.range(of: end, range: first.upperBound..<source.endIndex))
    return source[first.lowerBound..<last.lowerBound].split(whereSeparator: \.isWhitespace)
      .joined(separator: " ")
  }

  private func assertOrder(_ fragments: [String], in source: String) throws {
    var cursor = source.startIndex
    for fragment in fragments {
      let match = try XCTUnwrap(
        source.range(of: fragment, range: cursor..<source.endIndex),
        "expected \(fragment) after the preceding row element")
      cursor = match.upperBound
    }
  }

  private func show<Content: View>(
    _ host: NSHostingView<Content>, width: CGFloat, origin: NSPoint = NSPoint(x: 150, y: 150)
  ) -> NSWindow {
    let window = NSWindow(
      contentRect: NSRect(origin: origin, size: NSSize(width: width, height: 500)),
      styleMask: .borderless, backing: .buffered, defer: false)
    window.isReleasedWhenClosed = false
    window.contentView = host
    BackgroundTestWindow.present(window)
    settle(host)
    return window
  }

  private func settle(_ host: NSView) {
    host.layoutSubtreeIfNeeded()
    RunLoop.main.run(until: Date().addingTimeInterval(0.1))
    host.layoutSubtreeIfNeeded()
  }
}

@MainActor
private final class FirstRevealRailRecorder {
  var frame = CGRect.zero
}
