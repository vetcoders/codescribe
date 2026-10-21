import AppKit
import SwiftUI
import XCTest

@testable import Codescribe

/// Falsifiers for the compact overlay's channel map (bus intercom spec §5.1,
/// §5.4, §5.5, decision D1): one chip per bound channel plus 0, a counter when
/// the strip is narrow, `MIC → 5` for an open channel, hints that follow the
/// Agent channel modifier, and one pill view shared with the Agents rail.
@MainActor
final class OverlayChannelMapTests: XCTestCase {

  // MARK: - Fixtures

  private func hud(
    open: Bool = false, alive: Bool? = true, session: String? = nil
  ) -> OverlayChannelHudProjection {
    OverlayChannelHudProjection(
      open: open, loud: false, autosealDeadline: nil, followerAlive: alive,
      provider: session == nil ? nil : "codex", providerSessionID: session)
  }

  private func delivery(_ channel: String, _ agent: String) -> OverlayChannelDelivery {
    OverlayChannelDelivery(
      channel: channel, agent: agent, deliveryID: nil, stage: nil, isOpen: false)
  }

  private func model(
    channels: [OverlayChannelDelivery],
    hudStates: [String: OverlayChannelHudProjection],
    selectable: Set<String> = ["0", "2", "3", "5"],
    unread: Set<String> = []
  ) -> OverlayChannelMapModel {
    OverlayChannelMapModel(
      channels: channels, hudStates: hudStates, broadcastName: "All",
      selectableDigits: selectable, unreadDigits: unread)
  }

  /// The acceptance scenario's roster: Miksa on 2, Igor on 3, Kuba on 5.
  private func scenario(
    open: Set<String> = [], dead: Set<String> = []
  ) -> OverlayChannelMapModel {
    model(
      channels: [delivery("5", "Kuba"), delivery("2", "Miksa"), delivery("3", "Igor")],
      hudStates: [
        "0": hud(open: open.contains("0"), alive: nil),
        "2": hud(open: open.contains("2"), alive: !dead.contains("2"), session: "s2"),
        "3": hud(open: open.contains("3"), alive: !dead.contains("3"), session: "s3"),
        "5": hud(open: open.contains("5"), alive: !dead.contains("5"), session: "s5"),
      ])
  }

  private func ownerConversation(
    _ name: String, channel: String, session: String, lease: String
  ) throws -> OverlayConversation {
    let owner = try XCTUnwrap(
      OverlayConversationOwner(row: [
        "provider": "codex", "provider_session_id": session,
        "lease_id": String(repeating: lease, count: 32), "channel": channel, "name": name,
      ]))
    return OverlayConversation(
      id: owner.id, channel: channel, name: name, owner: owner, messages: [])
  }

  private func armedState() throws -> (
    state: OverlayState, broadcast: OverlayConversation, lena: OverlayConversation,
    astra: OverlayConversation
  ) {
    let broadcast = OverlayConversation(
      id: "map-all", channel: "0", name: "All", owner: nil, messages: [])
    let lena = try ownerConversation("lena", channel: "2", session: "lena-session", lease: "a")
    let astra = try ownerConversation("astra", channel: "3", session: "astra-session", lease: "b")
    let state = OverlayState.previewFormatted()
    state.applyConversationSnapshot(
      .init(
        deliveries: [
          OverlayChannelDelivery(
            channel: "2", agent: "lena", deliveryID: "map-lena", stage: .received, isOpen: false),
          OverlayChannelDelivery(
            channel: "3", agent: "astra", deliveryID: "map-astra", stage: .received,
            isOpen: false),
        ], conversations: [broadcast, lena, astra]))
    state.applyChannelRoster(
      [broadcast, lena, astra].map {
        CsChannelRosterState(
          channel: $0.channel, audience: $0.name, provider: $0.owner?.provider,
          providerSessionId: $0.owner?.providerSessionID, open: false, loud: false,
          autosealDeadlineUnixMs: nil, followerAlive: true, unreadSinceUnixMs: nil)
      })
    state.agentChannelModifierSetting = { "ctrl" }
    return (state, broadcast, lena, astra)
  }

  private func overlaySource(_ name: String) throws -> String {
    let root = URL(fileURLWithPath: #filePath).deletingLastPathComponent()
      .deletingLastPathComponent()
    return try String(
      contentsOf: root.appendingPathComponent("Codescribe/Screens/Overlay/\(name)"),
      encoding: .utf8)
  }

  private func section(of source: String, from: String, to: String) throws -> String {
    let start = try XCTUnwrap(source.range(of: from), "missing section start: \(from)")
    let end = try XCTUnwrap(
      source.range(of: to, range: start.upperBound..<source.endIndex),
      "missing section end: \(to)")
    return String(source[start.lowerBound..<end.lowerBound])
  }

  // MARK: - Projection

  func testChannelZeroLeadsAndOnlyBoundChordDigitsFollowInOrder() {
    let map = model(
      channels: [
        delivery("5", "Kuba"), delivery("3", "Igor"),
        // Delivery history alone does not bind a digit.
        delivery("7", "ghost"),
        // Channel 10 has no chord, so it stays off the map.
        delivery("10", "Founder"),
      ],
      hudStates: [
        "0": hud(alive: nil), "3": hud(session: "s3"), "5": hud(session: "s5"),
        "10": hud(session: "s10"),
      ])
    XCTAssertEqual(map.chips.map(\.digit), ["0", "3", "5"])
    XCTAssertEqual(map.chips.map(\.name), ["All", "Igor", "Kuba"])
    XCTAssertTrue(map.chips[0].isBroadcast)
    XCTAssertEqual(map.count, 3)
  }

  func testNoRosterMeansNoMap() {
    let map = model(channels: [delivery("2", "Miksa")], hudStates: [:])
    XCTAssertTrue(map.chips.isEmpty)
    XCTAssertEqual(
      OverlayChannelMapModel.presentation(availableWidth: 400, rowWidth: 0, counterWidth: 30),
      .hidden)
  }

  func testRosterWithOnlyChannelZeroMapsAll() {
    let map = model(channels: [], hudStates: ["0": hud(alive: nil)])
    XCTAssertEqual(map.chips.map(\.digit), ["0"])
  }

  func testDeadFollowerDimsItsChipAndKeepsItsDigit() {
    let map = scenario(dead: ["3"])
    XCTAssertEqual(map.chips.map(\.digit), ["0", "2", "3", "5"])
    XCTAssertEqual(map.chips.map(\.disconnected), [false, false, true, false])
  }

  func testUnreadAndSelectableComeFromTheRoster() {
    let map = model(
      channels: [delivery("2", "Miksa"), delivery("3", "Igor")],
      hudStates: ["0": hud(alive: nil), "2": hud(session: "s2"), "3": hud(session: "s3")],
      selectable: ["0", "2"], unread: ["3"])
    XCTAssertEqual(map.chips.map(\.selectable), [true, true, false])
    XCTAssertEqual(map.chips.map(\.unread), [false, false, true])
    XCTAssertTrue(map.anyUnread)
  }

  // MARK: - Active microphone (§5.4)

  func testOpenChannelCarriesTheMicrophoneRouteInPlace() {
    let map = scenario(open: ["5"])
    XCTAssertEqual(map.chips.map(\.digit), ["0", "2", "3", "5"], "no second row, no reorder")
    XCTAssertEqual(map.chips.map(\.microphoneOpen), [false, false, false, true])
    XCTAssertEqual(map.openDigits, ["5"])
    XCTAssertEqual(map.counterFigure, "5", "the collapsed counter reads MIC → 5")
    XCTAssertEqual(
      map.chips[3].accessibilityLabel(modifier: .control),
      "Channel 5, Kuba, shortcut Ctrl 5, microphone open")
    XCTAssertTrue(map.counterAccessibilityValue.hasPrefix("Microphone open on channel 5"))
  }

  func testChannelZeroOpenIsTheBroadcastRoute() {
    let map = scenario(open: ["0"])
    let zero = map.chips[0]
    XCTAssertTrue(zero.microphoneOpen)
    XCTAssertTrue(zero.isBroadcast, "channel 0 speaks through the megaphone")
    XCTAssertEqual(map.openDigits, ["0"])
    XCTAssertEqual(map.counterFigure, "0")
  }

  func testClosedChannelReturnsThePlainChipAndCounter() {
    let map = scenario()
    XCTAssertFalse(map.microphoneOpen)
    XCTAssertEqual(map.counterFigure, "4", "channel 0 counts")
    XCTAssertEqual(
      map.chips[3].accessibilityLabel(modifier: .control), "Channel 5, Kuba, shortcut Ctrl 5")
  }

  // MARK: - Collapse (§5.1)

  func testCollapseFollowsTheMeasuredRowNotAConstant() {
    let widths: [CGFloat] = [48, 62, 51, 57]
    let row = OverlayChannelMapModel.rowWidth(chipWidths: widths, spacing: 4)
    XCTAssertEqual(row, 48 + 62 + 51 + 57 + 3 * 4)
    func presentation(_ available: CGFloat?) -> OverlayChannelMapPresentation {
      OverlayChannelMapModel.presentation(
        availableWidth: available, chipWidths: widths, spacing: 4, counterWidth: 30)
    }
    XCTAssertEqual(presentation(nil), .chips, "the ideal size is the whole row")
    XCTAssertEqual(presentation(row), .chips)
    XCTAssertEqual(presentation(row - 0.4), .chips, "fractional rounding does not collapse")
    XCTAssertEqual(presentation(row - 1), .counter)
    XCTAssertEqual(presentation(30), .counter)
    XCTAssertEqual(presentation(29), .hidden, "the map yields before the actions do")
    // A wider chip moves the threshold with it.
    let wider = OverlayChannelMapModel.presentation(
      availableWidth: row, chipWidths: [48, 62, 51, 80], spacing: 4, counterWidth: 30)
    XCTAssertEqual(wider, .counter)
  }

  func testSlotClaimsOnlyWhatItShows() {
    let slot = OverlayChannelMapSlot(
      rowWidth: 230, counterWidth: 30, leadingGap: OverlayChannelMapView.leadingGap,
      height: OverlayChannelPill.headerHeight)
    let gap = OverlayChannelMapView.leadingGap
    XCTAssertEqual(slot.width(proposed: nil), 230 + gap)
    XCTAssertEqual(slot.width(proposed: 500), 230 + gap, "the leftover stays with the waveform")
    XCTAssertEqual(slot.width(proposed: 200), 30 + gap, "the counter hands the rest back")
    XCTAssertEqual(slot.width(proposed: 30 + gap - 1), 0, "no width when nothing fits")
    XCTAssertEqual(slot.width(proposed: 0), 0)
  }

  func testChipWidthsAreMeasuredFromTheirText() {
    let short = OverlayChannelPill.chipWidth(name: "Igor", microphoneOpen: false, scale: 1)
    let long = OverlayChannelPill.chipWidth(name: "Miksa-Igor", microphoneOpen: false, scale: 1)
    XCTAssertGreaterThan(long, short)
    let recording = OverlayChannelPill.chipWidth(name: "Igor", microphoneOpen: true, scale: 1)
    XCTAssertGreaterThan(recording, short, "MIC → takes its own room")
    let huge = OverlayChannelPill.chipWidth(
      name: String(repeating: "W", count: 80), microphoneOpen: false, scale: 1)
    XCTAssertLessThanOrEqual(
      huge,
      ceil(
        2 * OverlayChannelPill.chipPadding + OverlayChannelPill.digitCircle
          + OverlayChannelPill.chipGap + OverlayChannelPill.maxNameWidth),
      "a long name truncates instead of collapsing the map")
    XCTAssertGreaterThan(
      OverlayChannelPill.chipWidth(name: "Igor", microphoneOpen: false, scale: 1.2), short)
  }

  func testStripPillsStopScalingWhereTheRowWouldGrow() {
    XCTAssertEqual(OverlayChannelPill.headerScale(1), 1)
    XCTAssertEqual(OverlayChannelPill.headerScale(0.8), 0.8)
    XCTAssertEqual(OverlayChannelPill.headerScale(2), OverlayChannelPill.headerScaleCeiling)
    XCTAssertLessThanOrEqual(
      OverlayChannelPill.digitCircle * OverlayChannelPill.headerScaleCeiling,
      OverlayChannelPill.headerHeight)
    XCTAssertEqual(OverlayChannelPill.headerHeight, OverlayMicrophoneGlyph.diameter)
  }

  // MARK: - Shortcut hints (§5.1, §6)

  func testModifierFollowsTheSettingsNormalization() {
    XCTAssertEqual(OverlayChannelChordModifier(setting: "ctrl"), .control)
    XCTAssertEqual(OverlayChannelChordModifier(setting: "fn"), .function)
    XCTAssertEqual(OverlayChannelChordModifier(setting: "FN"), .function)
    XCTAssertEqual(OverlayChannelChordModifier(setting: "cmd"), .control, "Command is not a value")
    XCTAssertEqual(OverlayChannelChordModifier(setting: ""), .control)
  }

  func testChipHintNamesTheChordFromTheSetting() {
    let kuba = scenario().chips[3]
    XCTAssertEqual(kuba.hint(modifier: .control), "5 · Kuba · Ctrl+5")
    XCTAssertEqual(kuba.hint(modifier: .function), "5 · Kuba · Fn+5")
    XCTAssertEqual(
      kuba.accessibilityLabel(modifier: .function), "Channel 5, Kuba, shortcut Fn 5")
    XCTAssertEqual(scenario().chips[0].hint(modifier: .control), "0 · All · Ctrl+0")
  }

  func testCounterHintListsTheWholeMap() {
    XCTAssertEqual(scenario().counterHint, "0 All · 2 Miksa · 3 Igor · 5 Kuba")
    XCTAssertEqual(scenario().counterAccessibilityValue, "0 All · 2 Miksa · 3 Igor · 5 Kuba")
  }

  func testOverlayReadsTheModifierFromSettings() {
    let state = OverlayState.previewFormatted()
    XCTAssertEqual(state.agentChannelModifier, .control)
    state.agentChannelModifierSetting = { "fn" }
    state.refreshAgentChannelModifier()
    XCTAssertEqual(state.agentChannelModifier, .function)
    state.agentChannelModifierSetting = { "ctrl" }
    state.refreshAgentChannelModifier()
    XCTAssertEqual(state.agentChannelModifier, .control)
  }

  // MARK: - Click (decision D1)

  func testChipSelectsItsConversationLikeTheRailPillAndNeverTouchesCapture() throws {
    let (state, broadcast, _, astra) = try armedState()
    state.setPresentationMode(.midi)
    let generation = state.captureGeneration
    let view = DictationOverlayView(state: state)
    let roster = view.channelStatusView
    let map = roster.channelMapModel
    XCTAssertEqual(map.chips.map(\.digit), ["0", "2", "3"])
    XCTAssertEqual(map.chips.map(\.name), ["All", "lena", "astra"])
    XCTAssertEqual(map.chips.map(\.selectable), [true, true, true])

    roster.selectChannelConversation("3")
    XCTAssertEqual(state.selectedConversationID, astra.id, "the exact owner's conversation")
    XCTAssertEqual(state.windowContentSurface, .agents)
    XCTAssertEqual(state.presentationMode, .expanded, "the Agents canvas unfolds")
    XCTAssertEqual(state.captureGeneration, generation)
    XCTAssertTrue(state.pendingChannelToggles.isEmpty, "a chip is not a microphone")
    XCTAssertFalse(state.channelHudStates.values.contains(where: \.open))

    roster.selectChannelConversation("0")
    XCTAssertEqual(state.selectedConversationID, broadcast.id)
    // A digit with no current conversation is not an identity.
    roster.selectChannelConversation("7")
    XCTAssertEqual(state.selectedConversationID, broadcast.id)
    XCTAssertTrue(state.pendingChannelToggles.isEmpty)
  }

  func testCounterOpensTheAgentsListWithoutSelectingOrCapturing() throws {
    let (state, _, _, _) = try armedState()
    state.setPresentationMode(.midi)
    let generation = state.captureGeneration
    let map = try XCTUnwrap(DictationOverlayView(state: state).channelMap)
    XCTAssertEqual(map.model.counterFigure, "3")
    map.onOpenMap()
    XCTAssertNil(state.selectedConversationID)
    XCTAssertEqual(state.windowContentSurface, .agents)
    XCTAssertEqual(state.captureGeneration, generation)
    XCTAssertTrue(state.pendingChannelToggles.isEmpty)
  }

  func testNoMapBeforeTheRosterNamesAChannel() {
    let state = OverlayState.previewFormatted()
    XCTAssertNil(DictationOverlayView(state: state).channelMap)
  }

  // MARK: - Hosted compact strip

  func testWideCompactStripShowsChipsAndAChipClickSelects() throws {
    let (state, _, _, astra) = try armedState()
    state.setPresentationMode(.midi)
    let host = NSHostingView(
      rootView: DictationOverlayView(state: state)
        .defaultAppStorage(try XCTUnwrap(UserDefaults(suiteName: UUID().uuidString))))
    host.sizingOptions = []
    let panel = FloatingOverlayPanel(
      contentRect: NSRect(x: 100, y: 100, width: 900, height: DictationOverlayWindow.midiSize.height),
      styleMask: [.borderless, .nonactivatingPanel, .resizable], backing: .buffered,
      defer: false)
    panel.contentView = OverlayContentContainer(hosting: host)
    BackgroundTestWindow.present(panel)
    defer { panel.orderOut(nil) }
    host.layoutSubtreeIfNeeded()
    RunLoop.main.run(until: Date().addingTimeInterval(0.1))
    for digit in ["0", "2", "3"] {
      let chip = try XCTUnwrap(
        NativeAccessibilityProbe.waitForElements("overlay-channel-chip-\(digit)").elements.first,
        "chip \(digit)")
      XCTAssertEqual(chip.role, "AXButton")
      XCTAssertLessThanOrEqual(chip.height, OverlayChannelPill.headerHeight + 0.5)
    }
    XCTAssertTrue(
      try NativeAccessibilityProbe.query("overlay-channel-map-toggle").elements.isEmpty,
      "a wide strip shows the chips, not the counter")
    _ = try NativeAccessibilityProbe.query("overlay-channel-chip-3", press: true)
    XCTAssertEqual(state.selectedConversationID, astra.id)
    XCTAssertTrue(state.pendingChannelToggles.isEmpty)
    XCTAssertFalse(panel.isKeyWindow)
  }

  // MARK: - Source shape (§5.5, invariants)

  func testRailPillsAndMapChipsShareOnePillView() throws {
    let status = try overlaySource("OverlayChannelStatusView.swift")
    let rail = try section(
      of: status, from: "private func railPill(", to: "var channelMapModel")
    XCTAssertTrue(rail.contains("OverlayChannelPill("))
    XCTAssertTrue(rail.contains("form: .rail"))
    XCTAssertFalse(rail.contains("Text(verbatim: digit)"), "the rail does not draw its own digit")
    let mapView = try overlaySource("OverlayChannelMapView.swift")
    XCTAssertTrue(mapView.contains("form: .chip"))
    XCTAssertTrue(mapView.contains("form: .counter"))
    XCTAssertFalse(mapView.contains("Text(verbatim:"), "the map does not draw a digit itself")
    let pill = try overlaySource("OverlayChannelPill.swift")
    XCTAssertEqual(
      pill.components(separatedBy: "Text(verbatim: display.figure)").count - 1, 1,
      "one drawing of the digit")
    for forbidden in [
      "OverlayState", "onToggleChannel", "toggleAgentChannel", "OverlayConversation",
      "onSelectConversation",
    ] {
      XCTAssertFalse(pill.contains(forbidden), "the pill takes display values only: \(forbidden)")
      XCTAssertFalse(
        mapView.contains(forbidden), "the map routes through the roster: \(forbidden)")
    }
    let label = try overlaySource("OverlayAgentRosterLabel.swift")
    XCTAssertFalse(label.contains("OverlayState"))
  }

  func testMapLivesInTheCompactStripOnlyAndNeverOpensAMicrophone() throws {
    let overlay = try overlaySource("DictationOverlayView.swift")
    let header = try section(
      of: overlay, from: "private func justifiedHeader(density: OverlayHeaderDensity)",
      to: "private var transcriptionPreviewButton")
    XCTAssertTrue(header.contains("if density == .midi, let map = channelMap {"))
    XCTAssertTrue(header.contains(".frame(height: OverlayHeaderDensity.rowHeight)"))
    let map = try section(
      of: overlay, from: "var channelMap: OverlayChannelMapView?",
      to: "var channelStatusView: OverlayChannelStatusView")
    XCTAssertTrue(map.contains("roster.selectChannelConversation($0)"))
    XCTAssertTrue(map.contains("state.showAgentMonitor()"))
    XCTAssertFalse(map.contains("toggleAgentChannel"))
    XCTAssertFalse(map.contains("onToggleChannel"))
    // The intent rail stays as it is.
    XCTAssertFalse(header.contains("OverlayIntentRail("))
  }
}
