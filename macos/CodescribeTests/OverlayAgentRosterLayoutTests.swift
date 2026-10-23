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
            reception: nil, palette: .dark, textScale: scale
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

  func testRosterAndIntercomReadAddressReceptionActivityThenName() throws {
    let label = try sourceSection(
      "OverlayAgentRosterLabel.swift", from: "struct OverlayAgentRosterLabel: View {",
      to: "struct OverlayAgentUnreadBadge: View {")
    try assertOrder(
      [
        "Text(verbatim: channel)", "OverlayReceptionMarks(", "OverlayAgentStatusDot(",
        "Text(verbatim: title)", "Spacer(minLength: 4)",
      ], in: label)

    let card = try sourceSection(
      "OverlayIntercomWidget.swift", from: "private func cardHeader(",
      to: "private func audioSlot(")
    let richRow = try XCTUnwrap(card.range(of: "HStack(spacing: 6) {"))
    try assertOrder(
      [
        "Text(verbatim: identity.digit)", "OverlayReceptionMarks(", "OverlayAgentStatusDot(",
        "Text(verbatim: name)", "OverlayIntercomFocusButton(", "audioSlot(conversation,",
      ], in: String(card[richRow.lowerBound...]))

    let instance = try sourceSection(
      "OverlayIntercomWidget.swift", from: "private func headerRow(",
      to: "private struct OverlayIntercomResizeHandle:")
    let drawn = try XCTUnwrap(instance.range(of: "return HStack(spacing: 3) {"))
    try assertOrder(
      [
        "Text(verbatim: instance.digit)", "OverlayReceptionMarks(", "OverlayAgentStatusDot(",
        "if mini {", "OverlayIntercomFocusButton(", "OverlayIntercomAudioControls(",
      ], in: String(instance[drawn.lowerBound...]))
  }

  func testRosterActionsLeadWithFocusThenSpeakerAndMicrophone() throws {
    let row = try sourceSection(
      "OverlayChannelStatusView.swift", from: "func channelRow(",
      to: "private func rosterRowLabel(")
    try assertOrder(
      [".accessibilityIdentifier(\"overlay-focus-panel-", "OverlayAgentAudioControls("], in: row)
    XCTAssertTrue(row.contains("title: conversation?.name ?? channel.agent"))

    let audio = try sourceSection(
      "OverlayChannelStatusView.swift", from: "struct OverlayAgentAudioControls: View {",
      to: "struct OverlayAgentPillMenuAction {")
    try assertOrder(["Button(action: onPlayback)", "Button(action: onMicrophone)"], in: audio)
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
      let detachedWindowMark = 9 * scale + 5
      XCTAssertEqual(
        OverlayIntercomWidgetLayout.restingWidth(
          headerText: 0, agentText: measuredText + detachedWindowMark, scale: scale),
        expected + detachedWindowMark, accuracy: 0.001,
        "the detached-window glyph keeps its own measured slot")
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
    XCTAssertTrue(card.contains("OverlayDisconnectedRowDim(disconnected: dead || detached != nil)"))
    XCTAssertTrue(card.contains("Image(systemName: \"macwindow\")"))
    XCTAssertTrue(
      card.contains(
        "OverlayReceptionMarks(channel: channelRow, palette: palette, textScale: scale)"))

    let miniFrame = try sourceSection(
      "OverlayIntercomWidget.swift", from: "private func instanceFrame(",
      to: "fileprivate func setInstanceMinimized(")
    XCTAssertTrue(miniFrame.contains("text: nameWidth + digitWidth"))
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
