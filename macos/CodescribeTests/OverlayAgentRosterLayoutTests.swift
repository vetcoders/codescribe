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
        "Text(verbatim: identity.displayDigit)",
        "OverlayIntercomStatusMark(",
        // The name and its inline descriptor share OverlayIntercomNameLine.
        "OverlayIntercomNameLine( name: name,", "OverlayReceptionMarks(",
        "OverlayIntercomFocusButton(", "audioSlot(",
      ], in: String(card[richRow.lowerBound...]))

    let instance = try sourceSection(
      "OverlayIntercomWidget.swift", from: "private func headerRow(",
      to: "private struct OverlayIntercomResizeHandle:")
    let drawn = try XCTUnwrap(instance.range(of: "return HStack(spacing: 3) {"))
    try assertOrder(
      [
        "Text(verbatim: instance.displayDigit)",
        "OverlayIntercomStatusMark(",
        "if mini {", "OverlayReceptionMarks(",
        "OverlayIntercomFocusButton(", "OverlayIntercomAudioControls(",
      ], in: String(instance[drawn.lowerBound...]))
  }

  func testCompactRosterKeepsAudioAndMoreSeparateFromConversationNavigation() throws {
    let row = try sourceSection(
      "OverlayChannelStatusView.swift", from: "func channelRow(",
      to: "private func rosterRowLabel(")
    XCTAssertTrue(row.contains("compactAgentActions(for: channel)"))
    // The delivery state is the line under the preview, beside the receipt
    // glyph the name row carries again (Maciej, 2026-10-30 on build 2981:
    // "ja bym zostawił te ptaszki, ale one naprawdę muszą dawać status").
    XCTAssertTrue(row.contains("rosterRowState("))
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
        // Presence alone: the fixture's received stage never reaches the dot.
        let expected: OverlayAgentDotKind =
          alive == false ? .sleeping : open ? .listening : alive == true ? .present : .idle
        XCTAssertEqual(roster.presenceDotKind(for: channel), expected)
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
      to: "struct OverlayIntercomWindowProjection")
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
    // The stall's mark is the presence dot's "!", never the moon's "zzz" (L6).
    XCTAssertEqual(roster(alive: true, unreadSince: stale).presenceDotKind(for: channel), .stalled)
    XCTAssertNotEqual(roster(alive: true, unreadSince: stale).statusSymbol(for: channel), "zzz")
    XCTAssertFalse(roster(alive: true).isInactive(channel), "a reading agent")
    XCTAssertFalse(
      roster(alive: true, unreadSince: Date()).isInactive(channel), "a fresh unread message")
    XCTAssertFalse(roster(alive: nil).isInactive(channel), "no claim is not a claim of death")

    let widget = try overlaySource("OverlayIntercomWidget.swift")
    XCTAssertEqual(
      widget.components(separatedBy: "roster.hasDeadFollower(").count - 1, 3,
      "only a gone follower dims the row or replaces the presence dot")
    XCTAssertEqual(
      widget.components(
        separatedBy: "OverlayDisconnectedRowDim(disconnected: current && disconnected)"
      ).count
        - 1, 2, "current cards and instances dim only when the follower is gone")
    XCTAssertEqual(
      widget.components(separatedBy: "OverlayDisconnectedRowDim(disconnected: !current)").count
        - 1, 3, "stale owner chrome has its own dim, independently of the current follower")
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
    XCTAssertTrue(body.contains("OverlayIntercomResizeHandles("))
    XCTAssertTrue(
      body.contains("from: frame, delta: delta, edge: edge, row: identity, gesture: gesture)"))
    XCTAssertTrue(
      body.contains(".frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .top)"),
      "progressive content starts under the fixed header")
    let handles = try sourceSection(
      "OverlayIntercomWidget.swift", from: "private struct OverlayIntercomResizeHandles: View {",
      to: "private struct OverlayIntercomInstanceCard: View {")
    for edge in [".topRight", ".bottomLeft", ".bottomRight"] {
      XCTAssertTrue(handles.contains("handle(\(edge))"), edge)
    }
    for edge in [".left", ".right", ".top", ".bottom", ".topLeft"] {
      XCTAssertFalse(handles.contains("handle(\(edge))"), "no invisible band at \(edge)")
    }
    let header = try sourceSection(
      "OverlayIntercomWidget.swift", from: "private struct OverlayIntercomHeader: View {",
      to: "struct OverlayIntercomWindowProjection:")
    XCTAssertTrue(header.contains("OverlayResizeAnchor()"))
    XCTAssertTrue(header.contains("onDrag: { controller.beginGroupDrag() }"))
  }

  /// "Przydym też troszkę te hiperbiałe przyciski … nie tylko w pierwszym"
  /// and "można by się tego fioletu pozbyć … na rzecz rozjaśnienia" (Maciej,
  /// 2026-10-23): flat controls rest in the wordmark's muted ink, names rest
  /// smoked and brighten instead of turning violet.
  func testIntercomControlsRestSmokedAndNamesBrightenInsteadOfViolet() throws {
    let roster = try overlaySource("OverlayChannelStatusView.swift")
    XCTAssertFalse(roster.contains("AnyShapeStyle(.foreground)"), "an explicit ink is primary")
    XCTAssertTrue(roster.contains("private var idleControlInk: Color { palette.mutedText.color }"))
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
    XCTAssertTrue(row.contains("compactAgentActions(for: channel)"))
    let actions = try sourceSection(
      "OverlayChannelStatusView.swift", from: "func compactAgentActions(",
      to: "func compactReceptionIndicators(")
    XCTAssertTrue(actions.contains("agentMicrophoneEnabled(for: channel)"))
    XCTAssertTrue(actions.contains("agentPlaybackEnabled(for: channel)"))
    XCTAssertTrue(actions.contains("connected: !hasDeadFollower(channel)"))
    XCTAssertTrue(row.contains("OverlayDisconnectedRowDim(disconnected: hasDeadFollower(channel))"))

    let audio = try sourceSection(
      "OverlayIntercomWidget.swift", from: "private struct OverlayIntercomAudioControls: View {",
      to: "private func intercomStatusDot(")
    XCTAssertEqual(audio.components(separatedBy: "followerAlive != false").count - 1, 2)
    XCTAssertFalse(audio.contains("state.conversationMicrophoneOpen(conversation) ||"))

    let card = try sourceSection(
      "OverlayIntercomWidget.swift", from: "private func cardHeader(",
      to: "private func audioSlot(")
    // Parent cards own follower dimming; the title independently marks a
    // stale owner and an already detached conversation.
    XCTAssertTrue(card.contains("OverlayDisconnectedRowDim(disconnected: !current)"))
    XCTAssertFalse(card.contains("detachedInstance"))
    XCTAssertTrue(card.contains("if detached { Image(systemName: \"macwindow\")"))
    // A collapsed row moves the group; it never tears off (Maciej,
    // 2026-10-23: only an unrolled card detaches, by its grip).
    XCTAssertEqual(card.components(separatedBy: ".highPriorityGesture(moveGesture").count - 1, 2)
    XCTAssertFalse(card.contains("tearOff"))
    XCTAssertTrue(
      card.contains(
        "if identity.canExpand { Button(\"Detach conversation\") { controller.detach(identity) }")
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
    XCTAssertTrue(card.contains("showsHint: !expanded && !detached"))
    XCTAssertTrue(card.contains("!hasInlinePreview"))
    let cardBody = try sourceSection(
      "OverlayIntercomWidget.swift",
      from: "let expanded = controller.presentation.expanded.contains(identity)",
      to: "private func cardHeader(")
    let hoverStart = try XCTUnwrap(cardBody.range(of: ".onHover {"))
    let hoverEnd = try XCTUnwrap(
      cardBody.range(of: ".onChange(of:", range: hoverStart.upperBound..<cardBody.endIndex))
    let hover = String(cardBody[hoverStart.lowerBound..<hoverEnd.lowerBound])
    XCTAssertTrue(hover.contains("hovering = $0"))
    XCTAssertTrue(hover.contains("controller.noteGroupedActivity()"))
    for forbidden in ["expand(", "resize(", "setFrame", "setPresentationMode"] {
      XCTAssertFalse(
        hover.contains(forbidden), "hover paints the grips without changing card geometry")
    }
    XCTAssertTrue(cardBody.contains("OverlayIntercomWidgetLayout.rowCornerRadius(scale: scale)"))

    let miniFrame = try sourceSection(
      "OverlayIntercomWidget.swift", from: "private func instanceFrame(",
      to: "func setInstanceMinimized(")
    XCTAssertTrue(miniFrame.contains("text: nameWidth + digitWidth"))
    // A saved fiszka controls only its exact owner (Copilot on #183): once the
    // digit is reassigned, its row belongs to the replacement agent.
    let instanceHeader = try sourceSection(
      "OverlayIntercomWidget.swift", from: "private func headerRow(",
      to: "private func instanceLayout(")
    XCTAssertTrue(
      instanceHeader.contains(
        "let channelRow = current ? intercomChannelRow(state: state, digit: instance.digit) : nil"))
    XCTAssertTrue(instanceHeader.contains("OverlayIntercomAudioControls("))
    XCTAssertTrue(instanceHeader.contains("conversation: current ? conversation : nil"))
    // A gone agent fades but keeps half its colour (L5 variant 3).
    XCTAssertEqual(OverlayDisconnectedRowDim.opacity, 0.65)
    XCTAssertEqual(OverlayDisconnectedRowDim.grayscale, 0.5)
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
    XCTAssertTrue(
      marks.contains("palette: palette, lit: lit ? litColor : nil, unlitOpacity: unlitOpacity"))
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

  func testAgentsFirstRevealWithNoConversationReturnsToTranscriptionOnFold() {
    for width in [CGFloat(700), 1200] {
      let state = OverlayState.previewFormatted()
      XCTAssertNil(state.selectedConversationID)
      let rail = FirstRevealRailRecorder()
      let drawer = FirstRevealRailRecorder()
      let host = NSHostingView(
        rootView: DictationOverlayView(state: state)
          .onPreferenceChange(OverlayAgentRailFramePreferenceKey.self) { rail.frame = $0 }
          .onPreferenceChange(OverlayDrawerFramePreferenceKey.self) { drawer.frame = $0 }
          .frame(width: width, height: 500))
      host.sizingOptions = []
      let window = show(host, width: width, origin: NSPoint(x: -10000, y: -10000))
      defer { window.close() }
      XCTAssertEqual(state.windowContentSurface, .transcription)
      XCTAssertEqual(rail.frame, .zero)
      XCTAssertEqual(drawer.frame, .zero)
      state.unfoldAgentList()
      settle(host)
      RunLoop.main.run(until: Date().addingTimeInterval(CSMotion.Tempo.enter + 0.05))
      settle(host)
      XCTAssertEqual(state.windowContentSurface, .agents)
      XCTAssertTrue(state.showsAgentMonitor)
      XCTAssertGreaterThan(drawer.frame.width, 0, "the explicit Agents action mounts the roster")
      XCTAssertEqual(drawer.frame.maxX, width, accuracy: 1)
      state.foldAgentList()
      settle(host)
      RunLoop.main.run(until: Date().addingTimeInterval(CSMotion.Tempo.enter + 0.05))
      settle(host)
      XCTAssertEqual(state.windowContentSurface, .transcription)
      XCTAssertFalse(state.showsAgentMonitor)
      XCTAssertEqual(rail.frame, .zero, "agent chrome never occupies the transcription")
      XCTAssertEqual(drawer.frame, .zero)
      XCTAssertNil(state.selectedConversationID)
    }
  }

  /// Monika, 2026-11-07 ("to spychanie nie jest akceptowalne"; then, from
  /// her screenshot, the empty band above the rows): the list's fold stands
  /// in the pane's top corner, in the band between the window header and the
  /// first row, so the button moves no row. The rail's unfold stands in the
  /// same band, centred on the pills' column, and the pills start just below
  /// it, each lower by the same drop. The list renders under the column's
  /// gap, as in the Bus window. Ink bands are the pixel rows that carry text or glyphs.
  func testFoldButtonsLeaveTheRowsAndPillsWhereTheyStand() throws {
    let all = OverlayConversation(id: "0", channel: "0", name: "All", owner: nil, messages: [])
    let inbox = OverlayConversation(
      id: "10", channel: "10", name: "Your Inbox", owner: nil, messages: [])
    let lena = OverlayChannelDelivery(
      channel: "3", agent: "Lena", deliveryID: nil, stage: nil, isOpen: false)
    func roster(folds: Bool) -> OverlayChannelStatusView {
      var view = OverlayChannelStatusView(
        channels: [lena], unavailable: false, palette: .light, animates: false,
        onToggleChannel: { _ in }, conversations: [all, inbox], onSelectConversation: { _ in })
      if folds {
        view.onFoldList = {}
        view.onUnfoldList = {}
      }
      return view
    }
    let gap = OverlayChannelStatusView.listColumnTopGap
    let band = OverlayChannelStatusView.listTopBand
    let side = OverlayChannelStatusView.listFoldSide
    XCTAssertGreaterThanOrEqual(side, 20, "the fold keeps a 20 pt hit target")
    XCTAssertLessThan(side, band, "the fold fits the band above the rows")
    var report: [String] = ["band (header to first row frame) = \(band) pt at every scale"]
    for scale in [CGFloat(1), 1.5, 2] {
      let width = 360 * scale
      let height = 300 * scale
      let plain = try renderBitmap(
        roster(folds: false).monitorBody(maximumHeight: height).padding(.top, gap), width: width,
        height: height, scale: scale)
      let folded = try renderBitmap(
        roster(folds: true).monitorBody(maximumHeight: height).padding(.top, gap), width: width,
        height: height, scale: scale)
      let pixels = CGFloat(plain.pixelsWide) / width
      let strip = 40 * scale
      let left = 0..<(width - strip)
      let titleBand = try XCTUnwrap(inkBands(plain, width: width, columns: left).first)
      XCTAssertEqual(
        inkBands(folded, width: width, columns: left), inkBands(plain, width: width, columns: left),
        "the rows keep their places beside the fold button at scale \(scale)")
      let plainStrip = inkBands(plain, width: width, columns: (width - strip)..<width)
      let foldedStrip = inkBands(folded, width: width, columns: (width - strip)..<width)
      XCTAssertEqual(
        Array(foldedStrip.dropFirst()), plainStrip,
        "the button is a band of its own above the row controls at scale \(scale)")
      let button = try XCTUnwrap(foldedStrip.first)
      // The rows' frames start where the band ends; the button's ink stays
      // inside the band, above every row's ink.
      XCTAssertGreaterThan(CGFloat(button.lowerBound), 0)
      XCTAssertLessThan(CGFloat(button.upperBound), band * pixels, "scale \(scale)")
      XCTAssertLessThan(button.upperBound, titleBand.lowerBound, "scale \(scale)")
      if let firstControl = plainStrip.first {
        XCTAssertLessThan(button.upperBound, firstControl.lowerBound, "scale \(scale)")
      }
      XCTAssertEqual(
        CGFloat(button.lowerBound + button.upperBound) / 2 / pixels, band / 2, accuracy: 2,
        "the button is centred in the band at scale \(scale)")
      let ink = try XCTUnwrap(inkColumns(folded, width: width, rows: button))
      let contentEdge = width - OverlayChannelStatusView.listFoldTrailingInset
      XCTAssertLessThanOrEqual(CGFloat(ink.upperBound) / pixels, contentEdge, "scale \(scale)")
      XCTAssertGreaterThan(
        CGFloat(ink.lowerBound) / pixels, contentEdge - side,
        "the button ends at the rows' trailing content at scale \(scale)")
      let frameTop = (band - side) / 2
      report.append(
        "scale \(scale): title ink y \(CGFloat(titleBand.lowerBound) / pixels)-"
          + "\(CGFloat(titleBand.upperBound + 1) / pixels) pt; button frame y \(frameTop)-"
          + "\(frameTop + side) pt, x \(contentEdge - side)-\(contentEdge) of \(width) pt; "
          + "button ink y \(CGFloat(button.lowerBound) / pixels)-"
          + "\(CGFloat(button.upperBound + 1) / pixels) pt")

      let railWidth = 88 * scale
      let plainRail = try renderBitmap(
        roster(folds: false).railBody(maximumHeight: height).padding(.top, gap), width: railWidth,
        height: height, scale: scale)
      let foldedRail = try renderBitmap(
        roster(folds: true).railBody(maximumHeight: height).padding(.top, gap), width: railWidth,
        height: height, scale: scale)
      let plainPills = inkBands(plainRail, width: railWidth, columns: 0..<railWidth)
      let foldedPills = inkBands(foldedRail, width: railWidth, columns: 0..<railWidth)
      XCTAssertFalse(plainPills.isEmpty)
      XCTAssertEqual(
        foldedPills.count, plainPills.count + 1,
        "the unfold button is one band above the pills at scale \(scale)")
      let unfold = try XCTUnwrap(foldedPills.first)
      let railPixels = CGFloat(plainRail.pixelsWide) / railWidth
      // The button stands in the top band, where the list's fold stands.
      XCTAssertGreaterThan(unfold.lowerBound, 0)
      XCTAssertLessThan(CGFloat(unfold.upperBound) / railPixels, band, "scale \(scale)")
      XCTAssertEqual(
        CGFloat(unfold.lowerBound + unfold.upperBound) / 2 / railPixels, band / 2, accuracy: 2,
        "the unfold is centred in the band at scale \(scale)")
      // Centred on the pills' column: the pills' ink is the column's width.
      let firstRow = try XCTUnwrap(plainPills.first).lowerBound
      let lastRow = try XCTUnwrap(plainPills.last).upperBound
      let pillsInk = try XCTUnwrap(
        inkColumns(plainRail, width: railWidth, rows: firstRow...lastRow))
      let unfoldInk = try XCTUnwrap(inkColumns(foldedRail, width: railWidth, rows: unfold))
      XCTAssertEqual(
        CGFloat(unfoldInk.lowerBound + unfoldInk.upperBound) / 2 / railPixels,
        CGFloat(pillsInk.lowerBound + pillsInk.upperBound) / 2 / railPixels, accuracy: 1.5,
        "the unfold is centred on the pills' column at scale \(scale)")
      // The pills start below the button, every one lower by the same drop:
      // their own spacing is unchanged.
      let drop = OverlayChannelStatusView.railFoldDrop * railPixels
      let pills = Array(foldedPills.dropFirst())
      XCTAssertGreaterThan(try XCTUnwrap(pills.first).lowerBound, unfold.upperBound)
      for (moved, original) in zip(pills, plainPills) {
        XCTAssertEqual(CGFloat(moved.lowerBound - original.lowerBound), drop, accuracy: 1)
        XCTAssertEqual(CGFloat(moved.upperBound - original.upperBound), drop, accuracy: 1)
      }
      let firstPill = try XCTUnwrap(pills.first)
      let pillFrameTop = gap + 8 + OverlayChannelPill.railBadgeRise * scale + drop / railPixels
      report.append(
        "scale \(scale): rail unfold frame y 1-23 pt, ink y "
          + "\(CGFloat(unfold.lowerBound) / railPixels)-"
          + "\(CGFloat(unfold.upperBound + 1) / railPixels) pt, ink x "
          + "\(CGFloat(unfoldInk.lowerBound) / railPixels)-"
          + "\(CGFloat(unfoldInk.upperBound + 1) / railPixels) of \(railWidth) pt; "
          + "first pill frame top y \(pillFrameTop) pt, ink y "
          + "\(CGFloat(firstPill.lowerBound) / railPixels) pt (without the button "
          + "\(CGFloat(plainPills[0].lowerBound) / railPixels) pt); pills' column ink x "
          + "\(CGFloat(pillsInk.lowerBound) / railPixels)-"
          + "\(CGFloat(pillsInk.upperBound + 1) / railPixels) pt")
    }
    let measurements = XCTAttachment(string: report.joined(separator: "\n"))
    measurements.name = "fold-band-measurements"
    measurements.lifetime = .keepAlways
    add(measurements)
  }

  private func renderBitmap<Content: View>(
    _ view: Content, width: CGFloat, height: CGFloat, scale: CGFloat
  ) throws -> NSBitmapImageRep {
    let host = NSHostingView(
      rootView:
        view
        .frame(width: width, height: height, alignment: .top)
        .background(Color.white)
        .environment(\.csTextScale, scale)
        .environment(\.colorScheme, .light)
        .transaction { $0.disablesAnimations = true })
    host.sizingOptions = []
    host.frame = NSRect(x: 0, y: 0, width: width, height: height)
    let panel = NSPanel(
      contentRect: host.frame, styleMask: [.borderless, .nonactivatingPanel],
      backing: .buffered, defer: false)
    panel.isReleasedWhenClosed = false
    panel.appearance = NSAppearance(named: .aqua)
    panel.contentView = host
    defer {
      panel.contentView = nil
      panel.close()
    }
    settle(host)
    let bitmap = try XCTUnwrap(host.bitmapImageRepForCachingDisplay(in: host.bounds))
    host.cacheDisplay(in: host.bounds, to: bitmap)
    XCTAssertFalse(panel.isVisible, "the fixture does not claim the user's desktop")
    return bitmap
  }

  /// Runs of pixel rows (top down) holding dark ink within `columns` (points).
  private func inkBands(
    _ bitmap: NSBitmapImageRep, width: CGFloat, columns: Range<CGFloat>
  ) -> [ClosedRange<Int>] {
    guard bitmap.bitsPerSample == 8, bitmap.samplesPerPixel >= 3, let data = bitmap.bitmapData
    else {
      XCTFail("unexpected bitmap layout")
      return []
    }
    let pixels = CGFloat(bitmap.pixelsWide) / width
    let first = Int((columns.lowerBound * pixels).rounded(.down))
    let last = min(bitmap.pixelsWide, Int((columns.upperBound * pixels).rounded(.down)))
    let offset = bitmap.bitmapFormat.contains(.alphaFirst) ? 1 : 0
    var bands: [ClosedRange<Int>] = []
    var start: Int?
    for y in 0..<bitmap.pixelsHigh {
      var ink = false
      for x in first..<last {
        let base = y * bitmap.bytesPerRow + x * bitmap.samplesPerPixel + offset
        let sum = Int(data[base]) + Int(data[base + 1]) + Int(data[base + 2])
        if sum < 3 * 180 {
          ink = true
          break
        }
      }
      if ink, start == nil { start = y }
      if !ink, let open = start {
        bands.append(open...(y - 1))
        start = nil
      }
    }
    if let open = start { bands.append(open...(bitmap.pixelsHigh - 1)) }
    return bands
  }

  /// The span of columns (pixels) holding dark ink within `rows` (pixels).
  private func inkColumns(
    _ bitmap: NSBitmapImageRep, width: CGFloat, rows: ClosedRange<Int>
  ) -> ClosedRange<Int>? {
    guard let data = bitmap.bitmapData else { return nil }
    let offset = bitmap.bitmapFormat.contains(.alphaFirst) ? 1 : 0
    var first: Int?
    var last: Int?
    for y in rows {
      for x in 0..<bitmap.pixelsWide {
        let base = y * bitmap.bytesPerRow + x * bitmap.samplesPerPixel + offset
        let sum = Int(data[base]) + Int(data[base + 1]) + Int(data[base + 2])
        guard sum < 3 * 180 else { continue }
        first = min(first ?? x, x)
        last = max(last ?? x, x)
      }
    }
    guard let first, let last else { return nil }
    return first...last
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
