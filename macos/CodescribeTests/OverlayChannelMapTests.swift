import AppKit
import SwiftUI
import XCTest

@testable import Codescribe

/// Falsifiers for the collapsed overlay's channel map (bus intercom spec §5.1,
/// §5.5, §13, decision D1): one chip per bound agent, channel 0 only while it
/// broadcasts, named chips, then digit-only chips, then a counter as the room
/// shrinks, the violet dot on an open channel's digit, capitalised names, hints
/// that follow the Agent channel modifier, a counter whose click changes the
/// form or the width and nothing else, and one pill view shared with the
/// Agents rail.
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
    selected: String? = nil,
    selectable: Set<String> = ["0", "2", "3", "5"],
    unread: Set<String> = []
  ) -> OverlayChannelMapModel {
    OverlayChannelMapModel(
      channels: channels, hudStates: hudStates, selectedDigit: selected,
      selectableDigits: selectable, unreadDigits: unread)
  }

  /// The acceptance scenario's roster: Miksa on 2, Igor on 3, Kuba on 5.
  private func scenario(
    open: Set<String> = [], dead: Set<String> = [], selected: String? = nil
  ) -> OverlayChannelMapModel {
    model(
      channels: [delivery("5", "Kuba"), delivery("2", "Miksa"), delivery("3", "Igor")],
      hudStates: [
        "0": hud(open: open.contains("0"), alive: nil),
        "2": hud(open: open.contains("2"), alive: !dead.contains("2"), session: "s2"),
        "3": hud(open: open.contains("3"), alive: !dead.contains("3"), session: "s3"),
        "5": hud(open: open.contains("5"), alive: !dead.contains("5"), session: "s5"),
      ], selected: selected)
  }

  /// Nine agents, every chord digit bound.
  private func fullRoster(selected: String? = nil) -> OverlayChannelMapModel {
    let names = ["Miksa", "Nora", "Igor", "Hania", "Kuba", "Konfig", "Klara", "Leon", "Zofia"]
    var hudStates: [String: OverlayChannelHudProjection] = ["0": hud(alive: nil)]
    var channels: [OverlayChannelDelivery] = []
    for (index, name) in names.enumerated() {
      let digit = String(index + 1)
      hudStates[digit] = hud(session: "s\(digit)")
      channels.append(delivery(digit, name))
    }
    return model(channels: channels, hudStates: hudStates, selected: selected)
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
    optIntoChannelMap(state)
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

  // MARK: - Projection (§13.2)

  func testOnlyBoundAgentDigitsFollowInOrderWithoutChannelZero() {
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
    XCTAssertEqual(map.chips.map(\.digit), ["3", "5"], "no 0 All on the map")
    XCTAssertEqual(map.chips.map(\.name), ["Igor", "Kuba"])
    XCTAssertFalse(map.chips.contains(where: \.isBroadcast))
    XCTAssertEqual(map.count, 2, "the counter counts agents")
    XCTAssertEqual(map.counterFigure, "2")
  }

  func testNoRosterMeansNoMap() {
    let map = model(channels: [delivery("2", "Miksa")], hudStates: [:])
    XCTAssertTrue(map.chips.isEmpty)
    XCTAssertEqual(
      OverlayChannelMapModel.presentation(
        availableWidth: 400, namedWidth: 0, digitsWidth: 0, counterWidth: 30),
      .hidden)
  }

  func testChannelZeroAloneIsNoMap() {
    let map = model(channels: [], hudStates: ["0": hud(alive: nil)])
    XCTAssertTrue(map.chips.isEmpty, "channel 0 is not an agent")
  }

  func testChannelZeroAppearsOnlyWhileTheMicrophoneBroadcasts() {
    let map = scenario(open: ["0"])
    XCTAssertEqual(map.chips.map(\.digit), ["0", "2", "3", "5"])
    let zero = map.chips[0]
    XCTAssertTrue(zero.isBroadcast)
    XCTAssertTrue(zero.microphoneOpen)
    XCTAssertEqual(zero.name, "", "no All label")
    for presentation in [OverlayChannelMapPresentation.chips, .digits] {
      XCTAssertFalse(zero.showsName(in: presentation), "channel 0 is a digit with its badge")
    }
    XCTAssertEqual(map.count, 3, "channel 0 is not counted")
    XCTAssertEqual(map.agents.map(\.digit), ["2", "3", "5"])
    XCTAssertEqual(map.openDigits, ["0"])
    XCTAssertEqual(map.counterFigure, "0", "the counter shows 0 with the badge")
    XCTAssertEqual(map.counterHint, "2 Miksa · 3 Igor · 5 Kuba", "the hint lists agents only")
    XCTAssertEqual(zero.hint(modifier: .control), "0 · Ctrl+0")
    XCTAssertEqual(
      zero.accessibilityLabel(modifier: .control), "Channel 0, shortcut Ctrl 0, microphone open")
    // Broadcasting with no agent bound still shows the open channel.
    let alone = model(channels: [], hudStates: ["0": hud(open: true, alive: nil)])
    XCTAssertEqual(alone.chips.map(\.digit), ["0"])
    XCTAssertEqual(alone.count, 0)
    XCTAssertEqual(alone.counterAccessibilityValue, "Microphone open on channel 0")
  }

  func testDeadFollowerDimsItsChipAndKeepsItsDigit() {
    let map = scenario(dead: ["3"])
    XCTAssertEqual(map.chips.map(\.digit), ["2", "3", "5"])
    XCTAssertEqual(map.chips.map(\.disconnected), [false, true, false])
  }

  func testUnreadSelectableAndSelectedComeFromTheRoster() {
    let map = model(
      channels: [delivery("2", "Miksa"), delivery("3", "Igor")],
      hudStates: ["0": hud(alive: nil), "2": hud(session: "s2"), "3": hud(session: "s3")],
      selected: "3", selectable: ["0", "2"], unread: ["3"])
    XCTAssertEqual(map.chips.map(\.selectable), [true, false])
    XCTAssertEqual(map.chips.map(\.unread), [false, true])
    XCTAssertEqual(map.chips.map(\.selected), [false, true])
    XCTAssertTrue(map.anyUnread)
  }

  // MARK: - Names (§13.5)

  func testLeaseNamesAreCapitalisedOnTheMapOnly() {
    let map = model(
      channels: [delivery("3", "igor"), delivery("4", "hania"), delivery("6", "konfig")],
      hudStates: [
        "0": hud(alive: nil), "3": hud(session: "s3"), "4": hud(session: "s4"),
        "6": hud(session: "s6"),
      ])
    XCTAssertEqual(map.chips.map(\.name), ["Igor", "Hania", "Konfig"])
    XCTAssertEqual(map.chips.map(\.digit), ["3", "4", "6"], "identity stays the digit")
    XCTAssertEqual(map.counterHint, "3 Igor · 4 Hania · 6 Konfig")
    XCTAssertEqual(map.chips[1].hint(modifier: .control), "4 · Hania · Ctrl+4")
    XCTAssertEqual(OverlayChannelMapChip.displayName("ąbc"), "Ąbc", "the first grapheme only")
    XCTAssertEqual(OverlayChannelMapChip.displayName("miksa-igor"), "Miksa-igor")
    XCTAssertEqual(OverlayChannelMapChip.displayName("Kuba"), "Kuba")
    XCTAssertEqual(OverlayChannelMapChip.displayName(""), "")
  }

  func testCapitalisationLeavesTheRosterLeaseNamesAlone() {
    let state = OverlayState.previewFormatted()
    state.applyChannelRoster([
      CsChannelRosterState(
        channel: "3", audience: "igor", provider: "codex", providerSessionId: "igor-session",
        open: false, loud: false, autosealDeadlineUnixMs: nil, followerAlive: true,
        unreadSinceUnixMs: nil)
    ])
    XCTAssertEqual(state.channelMapLayoutModel.chips.map(\.name), ["Igor"])
    XCTAssertEqual(state.channelRosterNames["3"], "igor", "the lease keeps its name")
  }

  // MARK: - Open-channel dot (§13.3, §14)

  func testOpenChannelCarriesTheDotInPlace() {
    let map = scenario(open: ["5"])
    XCTAssertEqual(map.chips.map(\.digit), ["2", "3", "5"], "no second row, no reorder")
    XCTAssertEqual(map.chips.map(\.microphoneOpen), [false, false, true])
    XCTAssertEqual(map.openDigits, ["5"])
    XCTAssertEqual(map.counterFigure, "5", "the collapsed counter shows the open digit")
    XCTAssertEqual(map.count, 3, "the agent count returns when the channel closes")
    XCTAssertEqual(
      map.chips[2].accessibilityLabel(modifier: .control),
      "Channel 5, Kuba, shortcut Ctrl 5, microphone open")
    XCTAssertTrue(map.counterAccessibilityValue.hasPrefix("Microphone open on channel 5"))
    XCTAssertEqual(
      map.rowWidth(for: .chips, scale: 1, spacing: OverlayChannelMapView.chipSpacing),
      scenario().rowWidth(for: .chips, scale: 1, spacing: OverlayChannelMapView.chipSpacing),
      "the dot overlaps the circle and adds no width")
  }

  func testClosedChannelReturnsThePlainChipAndCounter() {
    let map = scenario()
    XCTAssertFalse(map.microphoneOpen)
    XCTAssertEqual(map.counterFigure, "3", "the counter counts agents")
    XCTAssertEqual(
      map.chips[2].accessibilityLabel(modifier: .control), "Channel 5, Kuba, shortcut Ctrl 5")
  }

  func testRecordingCounterHugsItsCircledDigit() {
    let one = OverlayChannelPill.counterWidth(figure: "3", openDigitCount: 1, scale: 1)
    XCTAssertEqual(
      one,
      ceil(2 * OverlayChannelPill.counterRecordingPadding + OverlayChannelPill.digitCircle))
    XCTAssertGreaterThan(
      OverlayChannelPill.counterWidth(figure: "3", openDigitCount: 2, scale: 1), one)
    XCTAssertLessThan(
      one, OverlayChannelPill.counterWidth(figure: "3", openDigitCount: 0, scale: 1),
      "the open digit is narrower than 3 ▾")
  }

  // MARK: - Mini width (§13.4)

  /// Mini is as wide as its content: no fixed 200 or 164 pt. The floor is its
  /// controls; the counter adds its brand gap and its own measured width.
  func testMiniWidthIsItsControlsPlusTheCounterAsDrawn() {
    let floor = DictationOverlayWindow.miniFloorWidth
    XCTAssertEqual(
      floor,
      DictationOverlayWindow.miniOuterPadding + DictationOverlayWindow.miniBrandDot
        + 2 * DictationOverlayWindow.miniRowSpacing + DictationOverlayWindow.miniTrailingControls)
    XCTAssertEqual(floor, 130, "32 padding + 9 dot + 2 × 4 gaps + 81 trailing controls")
    XCTAssertEqual(
      2 * DictationOverlayWindow.miniRowSpacing, 8,
      "across the closed centre, the gap the trailing controls keep")
    XCTAssertEqual(
      DictationOverlayWindow.miniNaturalWidth(
        map: model(channels: [], hudStates: [:]), textScale: 1),
      floor, "no channel: mini is its controls")

    let resting = scenario()
    XCTAssertEqual(
      DictationOverlayWindow.miniNaturalWidth(map: resting, textScale: 1),
      ceil(
        floor + DictationOverlayWindow.miniBrandGap
          + OverlayChannelPill.counterWidth(figure: "3", openDigitCount: 0, scale: 1)))
    let recording = scenario(open: ["5"])
    XCTAssertEqual(
      DictationOverlayWindow.miniNaturalWidth(map: recording, textScale: 1),
      ceil(
        floor + DictationOverlayWindow.miniBrandGap
          + OverlayChannelPill.counterWidth(figure: "3", openDigitCount: 1, scale: 1)),
      "the recording variant is the same rule with the recording counter")
    XCTAssertLessThan(
      DictationOverlayWindow.miniNaturalWidth(map: recording, textScale: 1),
      DictationOverlayWindow.miniNaturalWidth(map: resting, textScale: 1))
    XCTAssertGreaterThan(
      DictationOverlayWindow.miniNaturalWidth(map: scenario(open: ["2", "5"]), textScale: 1),
      DictationOverlayWindow.miniNaturalWidth(map: recording, textScale: 1),
      "two open digits widen the capsule instead of clipping it")
    // Broadcasting with no agent bound still shows the open 0.
    XCTAssertGreaterThan(
      DictationOverlayWindow.miniNaturalWidth(
        map: model(channels: [], hudStates: ["0": hud(open: true, alive: nil)]), textScale: 1),
      floor)
    // Larger text grows the counter, and the capsule with it, up to the
    // strip's scale ceiling.
    XCTAssertGreaterThan(
      DictationOverlayWindow.miniNaturalWidth(map: resting, textScale: 1.2),
      DictationOverlayWindow.miniNaturalWidth(map: resting, textScale: 1))
    XCTAssertEqual(
      DictationOverlayWindow.minimumSize(for: .mini).width, floor,
      "mini's floor is its controls")
  }

  // MARK: - Tiers (§13.1)

  func testTiersFollowTheMeasuredRowsNotConstants() {
    func presentation(_ available: CGFloat?) -> OverlayChannelMapPresentation {
      OverlayChannelMapModel.presentation(
        availableWidth: available, namedWidth: 300, digitsWidth: 120, counterWidth: 30)
    }
    XCTAssertEqual(presentation(nil), .chips, "the ideal size is the named row")
    XCTAssertEqual(presentation(300), .chips)
    XCTAssertEqual(presentation(299.6), .chips, "fractional rounding does not drop names")
    XCTAssertEqual(presentation(299), .digits, "names go before the chips do")
    XCTAssertEqual(presentation(120), .digits)
    XCTAssertEqual(presentation(119), .counter, "the counter only when digits do not fit")
    XCTAssertEqual(presentation(30), .counter)
    XCTAssertEqual(presentation(29), .hidden, "the map yields before the actions do")
    XCTAssertEqual(OverlayChannelMapModel.rowWidth(chipWidths: [48, 62, 51], spacing: 4), 169)
  }

  func testDigitsTierKeepsOnlyTheSelectedConversationsName() {
    let spacing = OverlayChannelMapView.chipSpacing
    let digit = OverlayChannelPill.chipWidth(name: nil, scale: 1)
    let plain = scenario()
    XCTAssertEqual(plain.chips.map { $0.showsName(in: .digits) }, [false, false, false])
    XCTAssertEqual(plain.chips.map { $0.showsName(in: .chips) }, [true, true, true])
    XCTAssertEqual(
      plain.rowWidth(for: .digits, scale: 1, spacing: spacing), 3 * digit + 2 * spacing)
    let selected = scenario(selected: "3")
    XCTAssertEqual(selected.chips.map { $0.showsName(in: .digits) }, [false, true, false])
    XCTAssertEqual(
      selected.rowWidth(for: .digits, scale: 1, spacing: spacing),
      2 * digit + OverlayChannelPill.chipWidth(name: "Igor", scale: 1) + 2 * spacing)
    XCTAssertLessThan(
      selected.rowWidth(for: .digits, scale: 1, spacing: spacing),
      selected.rowWidth(for: .chips, scale: 1, spacing: spacing))
    // Unread, dimming and the badge stay on a digit-only chip.
    let marked = model(
      channels: [delivery("2", "Miksa"), delivery("3", "Igor")],
      hudStates: [
        "2": hud(open: true, session: "s2"), "3": hud(alive: false, session: "s3"),
      ], unread: ["3"])
    XCTAssertEqual(marked.chips.map(\.microphoneOpen), [true, false])
    XCTAssertEqual(marked.chips.map(\.disconnected), [false, true])
    XCTAssertEqual(marked.chips.map(\.unread), [false, true])
  }

  func testNineAgentsDropToDigitsWithinTheBudget() throws {
    XCTAssertEqual(DictationOverlayWindow.midiChannelMapBudget, 820)
    XCTAssertEqual(
      DictationOverlayWindow.midiChannelMapBudget, 2 * DictationOverlayWindow.midiSize.width)
    let map = fullRoster()
    let spacing = OverlayChannelMapView.chipSpacing
    let digit = OverlayChannelPill.chipWidth(name: nil, scale: 1)
    let base =
      DictationOverlayWindow.midiSize.width + OverlayChannelMapView.leadingGap + digit + spacing
    XCTAssertGreaterThan(
      base + map.rowWidth(for: .chips, scale: 1, spacing: spacing),
      DictationOverlayWindow.midiChannelMapBudget, "nine named chips overflow the budget")
    let natural = DictationOverlayWindow.midiNaturalWidth(map: map, textScale: 1)
    XCTAssertLessThanOrEqual(natural, DictationOverlayWindow.midiChannelMapBudget)
    let longest = try XCTUnwrap(
      map.chips.map { OverlayChannelPill.chipWidth(name: $0.name, scale: 1) }.max())
    XCTAssertEqual(
      natural,
      ceil(base + 9 * digit + 8 * spacing + longest - digit),
      "the digits row, with room for the selected chip's name")
    // Whatever conversation is selected, its named chip fits that room.
    for selected in map.chips.map(\.digit) {
      XCTAssertLessThanOrEqual(
        base + fullRoster(selected: selected).rowWidth(for: .digits, scale: 1, spacing: spacing),
        natural + 0.5)
    }
  }

  func testSlotClaimsOnlyWhatItShows() {
    let slot = OverlayChannelMapSlot(
      namedWidth: 230, digitsWidth: 100, counterWidth: 30,
      leadingGap: OverlayChannelMapView.leadingGap, height: OverlayChannelPill.headerHeight)
    let gap = OverlayChannelMapView.leadingGap
    XCTAssertEqual(slot.width(proposed: nil), 230 + gap)
    XCTAssertEqual(slot.width(proposed: 500), 230 + gap, "the leftover stays with the waveform")
    XCTAssertEqual(slot.width(proposed: 200), 100 + gap, "digits hand the rest back")
    XCTAssertEqual(slot.width(proposed: 60), 30 + gap, "the counter hands the rest back")
    XCTAssertEqual(slot.width(proposed: 30 + gap - 1), 0, "no width when nothing fits")
    XCTAssertEqual(slot.width(proposed: 0), 0)
  }

  func testChipWidthsAreMeasuredFromTheirText() {
    let short = OverlayChannelPill.chipWidth(name: "Igor", scale: 1)
    let long = OverlayChannelPill.chipWidth(name: "Miksa-Igor", scale: 1)
    XCTAssertGreaterThan(long, short)
    let digit = OverlayChannelPill.chipWidth(name: nil, scale: 1)
    XCTAssertEqual(
      digit, ceil(2 * OverlayChannelPill.chipPadding + OverlayChannelPill.digitCircle))
    XCTAssertLessThan(digit, short)
    let huge = OverlayChannelPill.chipWidth(name: String(repeating: "W", count: 80), scale: 1)
    XCTAssertLessThanOrEqual(
      huge,
      ceil(
        2 * OverlayChannelPill.chipPadding + OverlayChannelPill.digitCircle
          + OverlayChannelPill.chipGap + OverlayChannelPill.maxNameWidth),
      "a long name truncates instead of collapsing the map")
    XCTAssertGreaterThan(OverlayChannelPill.chipWidth(name: "Igor", scale: 1.2), short)
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
    let kuba = scenario().chips[2]
    XCTAssertEqual(kuba.hint(modifier: .control), "5 · Kuba · Ctrl+5")
    XCTAssertEqual(kuba.hint(modifier: .function), "5 · Kuba · Fn+5")
    XCTAssertEqual(
      kuba.accessibilityLabel(modifier: .function), "Channel 5, Kuba, shortcut Fn 5")
  }

  /// Both keys record dictation: the core resolves the modifier to `off`,
  /// and no hint names a chord that does not exist.
  func testNoChordWhenBothKeysRecord() {
    XCTAssertEqual(OverlayChannelChordModifier(setting: "off"), .unbound)
    let kuba = scenario().chips[2]
    XCTAssertEqual(kuba.hint(modifier: .unbound), "5 · Kuba")
    XCTAssertEqual(kuba.accessibilityLabel(modifier: .unbound), "Channel 5, Kuba")
    XCTAssertFalse(kuba.accessibilityLabel(modifier: .unbound).contains("shortcut"))
  }

  func testCounterHintListsTheAgents() {
    XCTAssertEqual(scenario().counterHint, "2 Miksa · 3 Igor · 5 Kuba")
    XCTAssertEqual(scenario().counterAccessibilityValue, "2 Miksa · 3 Igor · 5 Kuba")
    XCTAssertFalse(scenario().counterHint.contains("All"))
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

  // MARK: - Click (decision D1, §13.4)

  func testChipSelectsItsConversationLikeTheRailPillAndNeverTouchesCapture() throws {
    let (state, broadcast, _, astra) = try armedState()
    state.setPresentationMode(.midi)
    let generation = state.captureGeneration
    let view = DictationOverlayView(state: state)
    let roster = view.channelStatusView
    let map = roster.channelMapModel
    XCTAssertEqual(map.chips.map(\.digit), ["2", "3"])
    XCTAssertEqual(map.chips.map(\.name), ["Lena", "Astra"])
    XCTAssertEqual(map.chips.map(\.selectable), [true, true])
    XCTAssertEqual(map.chips.map(\.selected), [false, false])

    roster.selectChannelConversation("3")
    XCTAssertEqual(state.selectedConversationID, astra.id, "the exact owner's conversation")
    XCTAssertEqual(state.windowContentSurface, .agents)
    XCTAssertEqual(state.presentationMode, .expanded, "the Agents canvas unfolds")
    XCTAssertEqual(state.captureGeneration, generation)
    XCTAssertTrue(state.pendingChannelToggles.isEmpty, "a chip is not a microphone")
    XCTAssertFalse(state.channelHudStates.values.contains(where: \.open))
    XCTAssertEqual(
      DictationOverlayView(state: state).channelStatusView.channelMapModel.chips.map(\.selected),
      [false, true], "the selected conversation's chip keeps its name in the digits tier")

    roster.selectChannelConversation("0")
    XCTAssertEqual(state.selectedConversationID, broadcast.id)
    // A digit with no current conversation is not an identity.
    roster.selectChannelConversation("7")
    XCTAssertEqual(state.selectedConversationID, broadcast.id)
    XCTAssertTrue(state.pendingChannelToggles.isEmpty)
  }

  func testMiniCounterSwitchesToCompactAndNothingElse() throws {
    let (state, _, _, _) = try armedState()
    state.setPresentationMode(.mini)
    let surface = state.windowContentSurface
    let generation = state.captureGeneration
    var restored = 0
    state.onRestoreMidiNaturalWidth = { restored += 1 }
    let map = try XCTUnwrap(DictationOverlayView(state: state).channelMap)
    XCTAssertEqual(map.placement, .mini, "mini shows the counter, never the chips")
    XCTAssertEqual(map.model.counterFigure, "2")
    XCTAssertEqual(map.model.counterHint, "2 Lena · 3 Astra")
    map.onCounter()
    XCTAssertEqual(state.presentationMode, .midi, "the counter opens compact, as the toggle does")
    XCTAssertEqual(restored, 0)
    XCTAssertNil(state.selectedConversationID, "no conversation is chosen")
    XCTAssertEqual(state.windowContentSurface, surface)
    XCTAssertEqual(state.captureGeneration, generation)
    XCTAssertTrue(state.pendingChannelToggles.isEmpty, "the counter is not a microphone")
  }

  func testVerticalMiniCounterOpensTheIntercomAndNothingElse() throws {
    let (state, _, _, _) = try armedState()
    state.channelMapPreference.selectLayout(.vertical)
    state.setPresentationMode(.mini)
    let surface = state.windowContentSurface
    let generation = state.captureGeneration
    var revealed = 0
    var restored = 0
    state.onRevealIntercom = { revealed += 1 }
    state.onRestoreMidiNaturalWidth = { restored += 1 }
    let map = try XCTUnwrap(DictationOverlayView(state: state).channelMap)
    XCTAssertEqual(map.placement, .mini)
    XCTAssertTrue(map.stacked)
    XCTAssertEqual(map.model.counterFigure, "2", "the badge carries the same agent count")
    map.onCounter()
    XCTAssertEqual(revealed, 1, "the counter leads to the Intercom group")
    XCTAssertEqual(state.presentationMode, .mini, "mini stays mini")
    XCTAssertEqual(restored, 0)
    XCTAssertNil(state.selectedConversationID)
    XCTAssertEqual(state.windowContentSurface, surface)
    XCTAssertEqual(state.captureGeneration, generation)
    XCTAssertTrue(state.pendingChannelToggles.isEmpty)
  }

  @MainActor
  func testMiniIsAsWideInBothLayoutsUpToTheCounter() throws {
    let (state, _, _, _) = try armedState()
    let map = state.channelMapLayoutModel
    let strip = DictationOverlayWindow.miniNaturalWidth(map: map, textScale: 1)
    let stacked = DictationOverlayWindow.miniNaturalWidth(map: map, textScale: 1, stacked: true)
    XCTAssertEqual(
      stacked,
      ceil(
        DictationOverlayWindow.miniFloorWidth + DictationOverlayWindow.miniBrandGap
          + OverlayChannelPill.headerHeight))
    XCTAssertLessThanOrEqual(abs(stacked - strip), 12, "one mini, one counter slot")
  }

  func testCompactCounterRestoresTheNaturalWidthAndNothingElse() throws {
    let (state, _, _, _) = try armedState()
    state.setPresentationMode(.midi)
    let generation = state.captureGeneration
    var restored = 0
    state.onRestoreMidiNaturalWidth = { restored += 1 }
    let map = try XCTUnwrap(DictationOverlayView(state: state).channelMap)
    XCTAssertEqual(map.placement, .strip)
    map.onCounter()
    XCTAssertEqual(restored, 1, "the window owner forgets the user's width")
    XCTAssertEqual(state.presentationMode, .midi, "compact stays compact")
    XCTAssertNil(state.selectedConversationID)
    XCTAssertEqual(state.captureGeneration, generation)
    XCTAssertTrue(state.pendingChannelToggles.isEmpty)
    // The expanded canvas has no counter; a stray call changes nothing.
    state.setPresentationMode(.expanded)
    state.activateChannelMapCounter()
    XCTAssertEqual(state.presentationMode, .expanded)
    XCTAssertEqual(restored, 1)
  }

  func testNoMapBeforeTheRosterNamesAChannel() {
    let state = OverlayState.previewFormatted()
    optIntoChannelMap(state)
    XCTAssertNil(DictationOverlayView(state: state).channelMap)
  }

  // MARK: - The chord keeps the collapsed form (§1, §14 W3)

  /// The controller's roster after a chord: the conversations' channels, with
  /// `open` naming the channels the microphone feeds.
  private func chordRoster(
    _ conversations: [OverlayConversation], open: Set<String>
  ) -> [CsChannelRosterState] {
    conversations.map {
      CsChannelRosterState(
        channel: $0.channel, audience: $0.name, provider: $0.owner?.provider,
        providerSessionId: $0.owner?.providerSessionID, open: open.contains($0.channel),
        loud: false, autosealDeadlineUnixMs: nil, followerAlive: true, unreadSinceUnixMs: nil)
    }
  }

  /// With the map on, a chord in mini or compact keeps the form: the strip's
  /// chip or counter shows the open digit, so the chord is readable without
  /// unfolding. The selection follows the open channel as before.
  func testChordKeepsMiniAndCompactWhileTheMapIsOn() throws {
    for form in [OverlayPresentationMode.mini, .midi] {
      let (state, broadcast, lena, astra) = try armedState()
      let conversations = [broadcast, lena, astra]
      XCTAssertTrue(state.channelMapEnabled)
      state.setPresentationMode(form)
      XCTAssertEqual(state.presentationMode, form)

      state.applyChannelRoster(chordRoster(conversations, open: ["3"]))
      XCTAssertEqual(state.presentationMode, form, "\(form): the chord keeps the form")
      let map = state.channelMapLayoutModel
      XCTAssertEqual(map.openDigits, ["3"], "\(form): the strip shows the open digit")
      XCTAssertEqual(map.counterFigure, "3", "\(form): the counter shows the open digit")
      XCTAssertEqual(map.chips.first { $0.digit == "3" }?.microphoneOpen, true)
      XCTAssertEqual(state.selectedConversationID, astra.id, "the selection follows the chord")

      state.applyChannelRoster(chordRoster(conversations, open: []))
      XCTAssertEqual(state.presentationMode, form, "\(form): closing keeps the form")
      let closed = state.channelMapLayoutModel
      XCTAssertFalse(closed.microphoneOpen)
      XCTAssertEqual(closed.counterFigure, String(closed.count), "the agent count returns")

      // A broadcast on 0 by chord keeps the form as well.
      state.applyChannelRoster(chordRoster(conversations, open: ["0"]))
      XCTAssertEqual(state.presentationMode, form, "\(form): broadcasting keeps the form")
      XCTAssertEqual(state.channelMapLayoutModel.openDigits, ["0"])
    }
  }

  /// With the map off, the chord unfolds mini and compact onto the Agents
  /// canvas, as before the map; an expanded overlay stays expanded either way.
  func testChordExpandsAsBeforeWithTheMapOffOrWhenAlreadyExpanded() throws {
    for form in [OverlayPresentationMode.mini, .midi] {
      let (state, broadcast, lena, astra) = try armedState()
      state.channelMapPreference.select(false)
      XCTAssertFalse(state.channelMapEnabled)
      state.setPresentationMode(form)
      state.applyChannelRoster(chordRoster([broadcast, lena, astra], open: ["3"]))
      XCTAssertEqual(state.presentationMode, .expanded, "\(form): the map is off")
      XCTAssertEqual(state.windowContentSurface, .agents)
      XCTAssertEqual(state.selectedConversationID, astra.id)
    }
    let (state, broadcast, lena, astra) = try armedState()
    XCTAssertTrue(state.channelMapEnabled)
    state.setPresentationMode(.expanded)
    state.applyChannelRoster(chordRoster([broadcast, lena, astra], open: ["3"]))
    XCTAssertEqual(state.presentationMode, .expanded)
    XCTAssertEqual(state.windowContentSurface, .agents)
    XCTAssertEqual(state.selectedConversationID, astra.id)
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
      contentRect: NSRect(
        x: 100, y: 100, width: 900, height: DictationOverlayWindow.midiSize.height),
      styleMask: [.borderless, .nonactivatingPanel, .resizable], backing: .buffered,
      defer: false)
    panel.contentView = OverlayContentContainer(hosting: host)
    BackgroundTestWindow.present(panel)
    defer { panel.orderOut(nil) }
    host.layoutSubtreeIfNeeded()
    RunLoop.main.run(until: Date().addingTimeInterval(0.1))
    for digit in ["2", "3"] {
      // The probe's XCTSkip (no isolated GUI session) must reach the runner;
      // inside XCTUnwrap's autoclosure it would be recorded as a failure.
      let elements = try NativeAccessibilityProbe.waitForElements(
        "overlay-channel-chip-\(digit)"
      ).elements
      let chip = try XCTUnwrap(elements.first, "chip \(digit)")
      XCTAssertEqual(chip.role, "AXButton")
      XCTAssertLessThanOrEqual(chip.height, OverlayChannelPill.headerHeight + 0.5)
    }
    XCTAssertTrue(
      try NativeAccessibilityProbe.query("overlay-channel-chip-0").elements.isEmpty,
      "no 0 All chip while channel 0 is closed")
    XCTAssertTrue(
      try NativeAccessibilityProbe.query("overlay-channel-map-toggle").elements.isEmpty,
      "a wide strip shows the chips, not the counter")
    _ = try NativeAccessibilityProbe.query("overlay-channel-chip-3", press: true)
    XCTAssertEqual(state.selectedConversationID, astra.id)
    XCTAssertTrue(state.pendingChannelToggles.isEmpty)
    XCTAssertFalse(panel.isKeyWindow)
  }

  // MARK: - Source shape (§5.5, §13.3, §13.4, invariants)

  func testPillWriteKeepsTheSelectedOwnerAndDoesNotFocusItsTerminalOrRecord() throws {
    let (state, _, _, astra) = try armedState()
    state.chooseConversation(astra.id)
    let generation = state.captureGeneration
    var selections: [String?] = []
    var focused: [OverlayConversationOwner] = []
    var toggles: [UInt8] = []
    var roster = OverlayChannelStatusView(
      channels: state.visibleChannelRows.filter { $0.channel != "0" }, unavailable: false,
      palette: .dark, animates: false, hudStates: state.channelHudStates,
      onToggleChannel: { toggles.append($0) }, conversations: state.conversations,
      selectedConversationID: astra.id,
      onSelectConversation: { selections.append($0) })
    roster.onFocusAgentPanel = { focused.append($0) }
    let actions = try XCTUnwrap(roster.agentPillMenu(for: "3").agentActions)
    XCTAssertTrue(actions.write.enabled)
    actions.write.perform()
    XCTAssertEqual(selections, [astra.id])
    XCTAssertTrue(focused.isEmpty, "writing to an already selected agent is still composing")
    XCTAssertTrue(toggles.isEmpty)
    XCTAssertEqual(state.captureGeneration, generation)
    XCTAssertTrue(roster.agentPillMenu(for: "0").members.isEmpty)
    XCTAssertNil(roster.agentPillMenu(for: "7").agentActions)
  }

  func testPillShowInAgentsDuringCaptureCannotBecomeTheHeaderStopAction() throws {
    let (state, _, _, astra) = try armedState()
    state.applyChannelRoster([
      CsChannelRosterState(
        channel: "3", audience: "astra", provider: "codex",
        providerSessionId: "astra-session", open: true, loud: false,
        autosealDeadlineUnixMs: nil, followerAlive: true, unreadSinceUnixMs: nil)
    ])
    state.chooseConversation(astra.id)
    state.setPresentationMode(.midi)
    let generation = state.captureGeneration
    let roster = DictationOverlayView(state: state).channelStatusView
    let show = try XCTUnwrap(roster.agentPillMenu(for: "3").agentActions?.showInAgents)
    XCTAssertTrue(show.enabled)
    show.perform()
    XCTAssertEqual(state.selectedConversationID, astra.id)
    XCTAssertEqual(state.presentationMode, .expanded)
    XCTAssertEqual(state.windowContentSurface, .agents)
    XCTAssertTrue(state.showsAgentMonitor)
    XCTAssertTrue(state.channelHudStates["3"]?.open == true)
    XCTAssertTrue(state.pendingChannelToggles.isEmpty)
    XCTAssertEqual(state.captureGeneration, generation)
  }

  func testPillAudioCommandsRefusePendingAndUnknownPlaybackWithoutOptimisticState() throws {
    let (state, _, _, _) = try armedState()
    var toggles: [UInt8] = []
    var playback: [String] = []
    var roster = OverlayChannelStatusView(
      channels: state.visibleChannelRows.filter { $0.channel != "0" }, unavailable: false,
      palette: .dark, animates: false, hudStates: state.channelHudStates,
      onToggleChannel: { toggles.append($0) }, conversations: state.conversations)
    roster.onTogglePlayback = { playback.append($0) }
    roster.pendingToggleChannels = ["3"]
    roster.pendingMuteChannels = ["3"]
    roster.mutedChannels = ["3": false]
    let pending = try XCTUnwrap(roster.agentPillMenu(for: "3").agentActions)
    XCTAssertFalse(try XCTUnwrap(pending.microphone).enabled)
    XCTAssertFalse(try XCTUnwrap(pending.playback).enabled)
    pending.microphone?.perform()
    pending.playback?.perform()
    XCTAssertTrue(toggles.isEmpty)
    XCTAssertTrue(playback.isEmpty)
    roster.pendingToggleChannels = []
    roster.pendingMuteChannels = []
    roster.mutedChannels = [:]
    let unknown = try XCTUnwrap(roster.agentPillMenu(for: "3").agentActions?.playback)
    XCTAssertFalse(unknown.enabled)
    unknown.perform()
    XCTAssertTrue(playback.isEmpty, "missing receipt is not permission to toggle")
    roster.mutedChannels = ["3": true]
    let ready = try XCTUnwrap(roster.agentPillMenu(for: "3").agentActions)
    XCTAssertTrue(try XCTUnwrap(ready.microphone).enabled)
    XCTAssertTrue(try XCTUnwrap(ready.playback).enabled)
    ready.microphone?.perform()
    ready.playback?.perform()
    XCTAssertEqual(toggles, [3])
    XCTAssertEqual(playback, ["3"])
    XCTAssertFalse(roster.hudStates["3"]?.open == true)
  }

  func testPillArchiveUsesExactOwnerAndRefusesPendingOrOpenCapture() throws {
    let (state, _, _, astra) = try armedState()
    let owner = try XCTUnwrap(astra.owner)
    var archived: [OverlayConversationOwner] = []
    func roster(open: Bool = false, pending: Bool = false) -> OverlayChannelStatusView {
      var view = OverlayChannelStatusView(
        channels: state.visibleChannelRows.filter { $0.channel != "0" }, unavailable: false,
        palette: .dark, animates: false,
        hudStates: ["3": hud(open: open, session: "astra-session")],
        conversations: state.conversations)
      view.archiveCandidates = ["3": owner]
      view.onArchiveAgent = { archived.append($0) }
      view.pendingArchives = pending ? [owner] : []
      return view
    }
    XCTAssertNil(
      roster().agentPillMenu(for: "3").agentActions?.focus,
      "unknown terminal must not inherit a navigation action")
    for view in [roster(pending: true), roster(open: true)] {
      let action = try XCTUnwrap(view.agentPillMenu(for: "3").agentActions?.archive)
      XCTAssertFalse(action.enabled)
      action.perform()
    }
    XCTAssertTrue(archived.isEmpty)
    let ready = try XCTUnwrap(roster().agentPillMenu(for: "3").agentActions?.archive)
    XCTAssertTrue(ready.enabled)
    ready.perform()
    XCTAssertEqual(archived, [owner])
  }

  func testMapChipsAndCountersShareOnePillView() throws {
    let mapView = try overlaySource("OverlayChannelMapView.swift")
    XCTAssertTrue(mapView.contains("form: .chip"))
    XCTAssertTrue(mapView.contains("form: .counter"))
    XCTAssertFalse(mapView.contains("Text(verbatim:"), "the map does not draw a digit itself")
    let pill = try overlaySource("OverlayChannelPill.swift")
    XCTAssertEqual(
      pill.components(separatedBy: "Text(verbatim: text)").count - 1, 1,
      "one drawing of the digit")
    // Spec §14: the open channel's digit carries one violet dot, drawn once,
    // and no symbol; the microphone and megaphone signs live on the Agents
    // button only.
    XCTAssertEqual(
      pill.components(separatedBy: "Circle()\n      .fill(palette.agentAccent.color)").count - 1, 1,
      "one drawing of the open-channel dot")
    let pillView = try section(
      of: pill, from: "struct OverlayChannelPill: View", to: "// MARK: Header metrics")
    // Symbols replace a figure: the stacked counter and the numberless human
    // mailbox. Neither adds a glyph to a numbered agent's digit.
    XCTAssertEqual(
      pillView.components(separatedBy: "Image(systemName:").count - 1, 2,
      "only the stacked counter and numberless mailbox carry symbols")
    XCTAssertTrue(pillView.contains("Image(systemName: Self.stackedCardsSymbol)"))
    XCTAssertFalse(pillView.contains("mic.fill"), "the microphone glyph left the digit")
    XCTAssertFalse(pillView.contains("megaphone"), "the megaphone stays on the Agents button")
    XCTAssertTrue(
      pillView.contains("if display.animates && !reduceMotion"),
      "the dot pulses only while the pill animates and Reduce Motion is off")
    for forbidden in [
      "OverlayState", "onToggleChannel", "toggleAgentChannel", "OverlayConversation",
      "onSelectConversation",
    ] {
      XCTAssertFalse(pill.contains(forbidden), "the pill takes display values only: \(forbidden)")
      XCTAssertFalse(
        mapView.contains(forbidden), "the map routes through the roster: \(forbidden)")
    }
    // The badge replaced the route text, and no popover came with it.
    for source in [pill, mapView] {
      XCTAssertFalse(source.contains("\\u{2192}"), "no MIC → arrow")
      XCTAssertFalse(source.contains("MIC"), "no MIC text")
      XCTAssertFalse(source.contains(".popover("), "the map popover is deferred")
      XCTAssertFalse(source.contains("NSPopover"), "the map popover is deferred")
    }
    let label = try overlaySource("OverlayAgentRosterLabel.swift")
    XCTAssertFalse(label.contains("OverlayState"))
  }

  func testMapSelectionStaysSeparateFromExplicitMicrophoneControls() throws {
    let overlay = try overlaySource("DictationOverlayView.swift")
    let header = try section(
      of: overlay, from: "private func justifiedHeader(density: OverlayHeaderDensity)",
      to: "private var transcriptionPreviewButton")
    XCTAssertTrue(
      header.contains("if density == .midi, state.channelMapInHeader, let map = channelMap {"),
      "only the horizontal strip carries the chips between the clock and the actions")
    XCTAssertTrue(
      header.contains("let leadingCounter = mini || state.channelMapCounterStacked"),
      "mini and every vertical form keep the counter beside the brand")
    XCTAssertTrue(
      header.contains("if leadingCounter, let map = channelMap {"),
      "the leading counter is the way to the Intercom group, not a microphone")
    XCTAssertTrue(header.contains("showsWord: !mini"), "mini drops cs")
    XCTAssertTrue(header.contains(".frame(height: OverlayHeaderDensity.rowHeight)"))
    let map = try section(
      of: overlay, from: "var channelMap: OverlayChannelMapView?",
      to: "var channelStatusView: OverlayChannelStatusView")
    XCTAssertTrue(map.contains("roster.selectChannelConversation($0)"))
    XCTAssertTrue(map.contains("state.activateChannelMapCounter()"))
    XCTAssertFalse(map.contains("showAgentMonitor"), "the W1 seam is gone")
    XCTAssertFalse(map.contains("toggleAgentChannel"))
    let selection = try section(of: map, from: "onSelectChannel:", to: "controls: controls")
    XCTAssertFalse(selection.contains("toggle"), "name and counter do not change capture")
    XCTAssertTrue(map.contains("onToggleMicrophone:"))
    XCTAssertTrue(
      map.contains("roster.toggle(channel)"), "only the dedicated microphone owns this action")
    // The intent rail stays as it is.
    XCTAssertFalse(header.contains("OverlayIntentRail("))
  }
}
