import AppKit
import SwiftUI
import XCTest

@testable import Codescribe

/// Falsifiers for the overlay's channel map (bus intercom spec §5.1, §5.5, §13,
/// decision D1; bus brief 2026-11-04 §4, §5, §11C): one pill per bound agent,
/// led by channel 0's nameless chip (lit while it broadcasts), each agent's
/// hover card from the roster's projections, names measured in the pill's font
/// (whole up to six letters, else five and an ellipsis, four at the floor) and
/// cut longest first, stacked pills in the bar's single row for 1–5 agents and
/// chips in its second row for 6–9, the violet dot on an open channel's digit,
/// capitalised names, hints that follow the Agent channel modifier, a counter
/// whose click changes the form or the width and nothing else, and one pill
/// view shared with the Agents rail.
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

  /// The first `count` agents of the nine-agent roster, on digits 1…count.
  private func agents(_ count: Int) -> OverlayChannelMapModel {
    let names = ["Miksa", "Nora", "Igor", "Hania", "Kuba", "Konfig", "Klara", "Leon", "Zofia"]
    return named(Array(names.prefix(count)))
  }

  /// Agents with the given (already capitalised) names on digits 1, 2, ….
  private func named(_ names: [String]) -> OverlayChannelMapModel {
    var hudStates: [String: OverlayChannelHudProjection] = ["0": hud(alive: nil)]
    var channels: [OverlayChannelDelivery] = []
    for (index, name) in names.enumerated() {
      let digit = String(index + 1)
      hudStates[digit] = hud(session: "s\(digit)")
      channels.append(delivery(digit, name))
    }
    return model(channels: channels, hudStates: hudStates)
  }

  /// The agents' name widths, without channel 0's nameless chip.
  private func nameWidths(_ names: [String]) -> OverlayChannelMapNameWidths {
    OverlayChannelMapNameWidths(chips: named(names).agents, scale: 1)
  }

  /// A row of the map's pills at the given name widths, as the window and the
  /// map both measure it.
  private func row(_ nameWidths: [CGFloat], _ form: OverlayChannelPill.Form) -> CGFloat {
    OverlayChannelMapModel.rowWidth(
      nameWidths: nameWidths, form: form, scale: 1, spacing: OverlayChannelMapView.chipSpacing)
  }

  // MARK: - Projection (§13.2)

  func testChannelZeroLeadsTheBoundAgentDigitsInOrder() {
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
    XCTAssertEqual(map.chips.map(\.digit), ["0", "3", "5"], "(0) leads at rest")
    XCTAssertEqual(
      map.chips.map(\.name), [OverlaySharedChannelName.defaultName, "Igor", "Kuba"],
      "channel 0 carries the shared channel's default name")
    XCTAssertTrue(map.chips[0].isBroadcast)
    XCTAssertFalse(map.chips[0].microphoneOpen, "at rest channel 0 carries no open dot")
    XCTAssertNil(map.chips[0].status, "channel 0's hover is its carousel, not a card")
    XCTAssertEqual(map.agents.map(\.digit), ["3", "5"])
    XCTAssertEqual(map.count, 2, "the counter counts agents")
    XCTAssertEqual(map.counterFigure, "2")
  }

  func testNoRosterMeansNoMap() {
    let map = model(channels: [delivery("2", "Miksa")], hudStates: [:])
    XCTAssertTrue(map.chips.isEmpty)
    XCTAssertEqual(
      DictationOverlayWindow.midiBarGeometry(map: map, textScale: 1), .classic,
      "no map: the classic 410 × 46 bar")
  }

  func testChannelZeroAloneIsNoMap() {
    let map = model(channels: [], hudStates: ["0": hud(alive: nil)])
    XCTAssertTrue(map.chips.isEmpty, "channel 0 is not an agent")
  }

  func testBroadcastLightsChannelZeroWithoutResizingTheMap() {
    let map = scenario(open: ["0"])
    XCTAssertEqual(map.chips.map(\.digit), ["0", "2", "3", "5"])
    let zero = map.chips[0]
    XCTAssertTrue(zero.isBroadcast)
    XCTAssertTrue(zero.microphoneOpen)
    XCTAssertEqual(zero.name, OverlaySharedChannelName.defaultName)
    // Channel 0's name is measured by the agents' rules, at most its whole
    // width and never below its shortest cut.
    let names = OverlayChannelMapNameWidths(chips: map.chips, scale: 1)
    XCTAssertEqual(
      names.full[0], OverlayChannelPill.nameWidth(OverlaySharedChannelName.defaultName, scale: 1))
    XCTAssertLessThanOrEqual(names.preferred[0], names.full[0])
    XCTAssertLessThanOrEqual(names.shortest[0], names.preferred[0])
    XCTAssertGreaterThan(names.shortest[0], 0)
    XCTAssertEqual(map.count, 3, "channel 0 is not counted")
    XCTAssertEqual(map.agents.map(\.digit), ["2", "3", "5"])
    XCTAssertEqual(map.openDigits, ["0"])
    XCTAssertEqual(map.counterFigure, "0", "the counter shows 0 with the badge")
    XCTAssertEqual(map.counterHint, "2 Miksa · 3 Igor · 5 Kuba", "the hint lists agents only")
    XCTAssertEqual(
      zero.hint(modifier: .control), "0 · " + OverlaySharedChannelName.defaultName + " · Ctrl+0")
    XCTAssertEqual(
      zero.accessibilityLabel(modifier: .control), "Channel 0, shortcut Ctrl 0, microphone open")
    // At rest the same chip leads, without the dot, and the bar keeps its
    // width: (0) is always there, so a broadcast only lights it.
    let rest = scenario()
    XCTAssertEqual(rest.chips.map(\.digit), ["0", "2", "3", "5"])
    XCTAssertFalse(rest.chips[0].microphoneOpen)
    XCTAssertEqual(
      rest.chips[0].accessibilityLabel(modifier: .control), "Channel 0, shortcut Ctrl 0")
    XCTAssertEqual(rest.chips[0].accessibilityLabel(modifier: .unbound), "Channel 0")
    XCTAssertEqual(
      DictationOverlayWindow.midiBarGeometry(map: map, textScale: 1),
      DictationOverlayWindow.midiBarGeometry(map: rest, textScale: 1),
      "a broadcast resizes nothing")
    // Broadcasting with no agent bound still shows the open channel.
    let alone = model(channels: [], hudStates: ["0": hud(open: true, alive: nil)])
    XCTAssertEqual(alone.chips.map(\.digit), ["0"])
    XCTAssertEqual(alone.count, 0)
    XCTAssertEqual(alone.counterAccessibilityValue, "Microphone open on channel 0")
  }

  func testDeadFollowerDimsItsChipAndKeepsItsDigit() {
    let map = scenario(dead: ["3"])
    XCTAssertEqual(map.chips.map(\.digit), ["0", "2", "3", "5"])
    XCTAssertEqual(map.chips.map(\.disconnected), [false, false, true, false])
  }

  func testUnreadSelectableAndSelectedComeFromTheRoster() {
    let map = model(
      channels: [delivery("2", "Miksa"), delivery("3", "Igor")],
      hudStates: ["0": hud(alive: nil), "2": hud(session: "s2"), "3": hud(session: "s3")],
      selected: "3", selectable: ["0", "2"], unread: ["3"])
    XCTAssertEqual(map.chips.map(\.digit), ["0", "2", "3"])
    XCTAssertEqual(map.chips.map(\.selectable), [true, true, false])
    XCTAssertEqual(map.chips.map(\.unread), [false, false, true])
    XCTAssertEqual(map.chips.map(\.selected), [false, false, true])
    XCTAssertTrue(map.anyUnread)
  }

  // MARK: - Hover card status (Founder, 2026-11-04)

  /// Three separate facts: presence from `followerAlive` (false is only
  /// "not active", nil no data), the stall as its own flag, reception and
  /// the moon from the channel 0 policy as painted (two states, no
  /// unconfirmed third: Founder, 2026-11-04).
  func testChipStatusKeepsPresenceReceptionAndUnreadApart() {
    func status(
      alive: Bool?, stalled: Bool = false, policy: AgentBroadcastPolicy = .on,
      unread: Int = 0, provider: String? = "codex"
    ) -> OverlayChannelChipStatus {
      OverlayChannelChipStatus(
        followerAlive: alive, stalled: stalled, policy: policy, unread: unread,
        provider: provider)
    }
    XCTAssertEqual(status(alive: true).presence, .online)
    XCTAssertEqual(status(alive: false).presence, .inactive, "false is not a sure offline")
    XCTAssertEqual(status(alive: nil).presence, .unknown)
    XCTAssertTrue(status(alive: true, stalled: true).stalled)
    XCTAssertEqual(status(alive: true, stalled: true).presence, .online, "a stall is separate")
    XCTAssertEqual(status(alive: true, policy: .on).reception, .member)
    XCTAssertEqual(status(alive: true, policy: .off).reception, .off)
    XCTAssertTrue(status(alive: true, policy: .quiet).quiet)
    XCTAssertTrue(status(alive: true, policy: .offQuiet).quiet)
    XCTAssertEqual(status(alive: true, policy: .offQuiet).reception, .off)
    XCTAssertEqual(
      status(alive: nil, policy: .on).reception, .member,
      "reception never decides presence, presence never decides reception")
    XCTAssertEqual(status(alive: true, unread: 3).unread, 3)
    XCTAssertEqual(status(alive: true, unread: -1).unread, 0)
    XCTAssertNil(status(alive: true, provider: "").provider)
  }

  /// The model hands each agent its status by digit; channel 0 has none.
  func testModelCarriesEachAgentsStatus() {
    let online = OverlayChannelChipStatus(presence: .online, reception: .member, unread: 1)
    let map = OverlayChannelMapModel(
      channels: [delivery("2", "Miksa"), delivery("3", "Igor")],
      hudStates: ["2": hud(session: "s2"), "3": hud(session: "s3")],
      selectableDigits: [], unreadDigits: [], statuses: ["2": online])
    XCTAssertEqual(map.chips.map(\.status), [nil, online, nil])
    XCTAssertEqual(map.count, 2, "statuses add no agent")
  }

  /// The roster fills the card from the projections its rows read: a green
  /// delivery dot beside a dead follower stays "not active", a policy without
  /// a receipt stays unconfirmed, a stalled follower is flagged, and unread
  /// Founder, 2026-11-04: the dot before the name sleeps ("zzz") under the
  /// moon as the bus paints it, receipt or not (the same two states as the
  /// roster's moon and the context menu). The microphone wins over the moon;
  /// a gone follower is a ring either way; without a liveness claim the moon
  /// proves nothing (Olek's QC).
  func testPresenceDotSleepsUnderAReceiptedMoon() throws {
    func identity(_ digit: String) -> AgentPlaybackIdentity {
      AgentPlaybackIdentity(provider: "codex", session: "dot-moon-\(digit)", bus: "/dev/null")
    }
    var channels: [OverlayChannelDelivery] = []
    for (digit, open) in [("2", false), ("3", false), ("4", true), ("5", false), ("6", false)] {
      var channel = OverlayChannelDelivery(
        channel: digit, agent: "agent-\(digit)", deliveryID: "moon-\(digit)",
        stage: .sent, isOpen: open)
      channel.broadcastIdentity = identity(digit)
      channel.broadcastPolicy = .quiet
      channels.append(channel)
    }
    let ledger = OverlayBroadcastPolicyLedger.shared
    let previousReceipts = ledger.receipted
    defer { ledger.noteReceipts(previousReceipts) }
    // 3 has no receipt: the painted moon counts all the same.
    ledger.noteReceipts([identity("2"), identity("4"), identity("5"), identity("6")])
    let roster = OverlayChannelStatusView(
      channels: channels, unavailable: false, palette: .dark, animates: false,
      hudStates: [
        "2": hud(alive: true, session: "s2"), "3": hud(alive: true, session: "s3"),
        "4": hud(open: true, alive: true, session: "s4"), "5": hud(alive: false, session: "s5"),
        // 6: a receipted moon with no liveness claim is no sleeper.
        "6": hud(alive: nil, session: "s6"),
      ])
    let dots = Dictionary(
      uniqueKeysWithValues: channels.map { ($0.channel, roster.presenceDotKind(for: $0)) })
    XCTAssertEqual(dots["2"], .asleep, "a receipted moon: zzz")
    XCTAssertEqual(dots["3"], .asleep, "a polled moon without a receipt sleeps too")
    XCTAssertEqual(dots["4"], .listening, "the open microphone wins over the moon")
    XCTAssertEqual(dots["5"], .sleeping, "a gone follower is a ring under the moon too")
    XCTAssertEqual(dots["6"], .idle, "no liveness claim: a faint ring, not zzz")
  }

  /// is the conversation's count.
  func testRosterFillsTheCardFromItsRowProjections() throws {
    func identity(_ digit: String) -> AgentPlaybackIdentity {
      AgentPlaybackIdentity(provider: "codex", session: "chip-status-\(digit)", bus: "/dev/null")
    }
    let names = ["2": "miksa", "3": "igor", "4": "hania", "5": "kuba"]
    let policies: [String: AgentBroadcastPolicy] = [
      "2": .on, "3": .off, "4": .off, "5": .quiet,
    ]
    let digits = ["2", "3", "4", "5"]
    var channels: [OverlayChannelDelivery] = []
    for digit in digits {
      var channel = OverlayChannelDelivery(
        channel: digit, agent: names[digit] ?? "", deliveryID: "chip-\(digit)",
        stage: .received, isOpen: false)
      channel.broadcastIdentity = identity(digit)
      channel.broadcastPolicy = policies[digit] ?? .on
      channels.append(channel)
    }
    var stalled = hud(alive: true, session: "s5")
    stalled.unreadSince = .distantPast
    let hudStates: [String: OverlayChannelHudProjection] = [
      "2": hud(alive: true, session: "s2"), "3": hud(alive: false, session: "s3"),
      "4": hud(alive: nil, session: "s4"), "5": stalled,
    ]
    let leases = ["a", "b", "c", "d"]
    let conversations = try digits.enumerated().map { index, digit in
      try ownerConversation(
        names[digit] ?? "", channel: digit, session: "s\(digit)", lease: leases[index])
    }
    let ledger = OverlayBroadcastPolicyLedger.shared
    let previousReceipts = ledger.receipted
    defer { ledger.noteReceipts(previousReceipts) }
    ledger.noteReceipts([identity("2"), identity("3"), identity("5")])

    let roster = OverlayChannelStatusView(
      channels: channels, unavailable: false, palette: .dark, animates: false,
      hudStates: hudStates, conversations: conversations,
      unreadCounts: [conversations[0].id: 3], onSelectConversation: { _ in })
    let statuses = Dictionary(
      uniqueKeysWithValues: roster.channelMapModel.agents.map { ($0.digit, $0.status) })
    XCTAssertEqual(statuses["2"]??.presence, .online)
    XCTAssertEqual(statuses["2"]??.reception, .member)
    XCTAssertEqual(statuses["2"]??.unread, 3)
    XCTAssertEqual(statuses["2"]??.provider, "codex")
    XCTAssertEqual(
      statuses["3"]??.presence, .inactive, "the received dot is a delivery, not presence")
    XCTAssertEqual(statuses["3"]??.reception, .off)
    XCTAssertEqual(statuses["4"]??.presence, .unknown)
    XCTAssertEqual(statuses["4"]??.reception, .off, "the painted policy, receipt or not")
    XCTAssertEqual(statuses["5"]??.presence, .online)
    XCTAssertEqual(statuses["5"]??.stalled, true)
    XCTAssertEqual(statuses["5"]??.quiet, true)
    XCTAssertEqual(statuses["5"]??.unread, 0)
    XCTAssertNil(roster.channelMapModel.chips.first?.status, "(0) has no card")
  }

  // MARK: - Names (§13.5)

  func testLeaseNamesPreserveEnteredSpellingOnTheMap() {
    let map = model(
      channels: [
        delivery("3", "div0"), delivery("4", "iOS"), delivery("6", "Łucja"),
        delivery("7", "McKay"),
      ],
      hudStates: [
        "0": hud(alive: nil), "3": hud(session: "s3"), "4": hud(session: "s4"),
        "6": hud(session: "s6"), "7": hud(session: "s7"),
      ])
    XCTAssertEqual(map.agents.map(\.name), ["div0", "iOS", "Łucja", "McKay"])
    XCTAssertEqual(map.agents.map(\.digit), ["3", "4", "6", "7"], "identity stays the digit")
    XCTAssertEqual(map.counterHint, "3 div0 · 4 iOS · 6 Łucja · 7 McKay")
    XCTAssertEqual(map.agents[1].hint(modifier: .control), "4 · iOS · Ctrl+4")
    XCTAssertEqual(OverlayChannelMapChip.displayName("codex"), "Codex", "provider label formatting")
    XCTAssertEqual(OverlayChannelMapChip.displayName("ąbc"), "Ąbc", "the first grapheme only")
    XCTAssertEqual(OverlayChannelMapChip.displayName("Kuba"), "Kuba")
    XCTAssertEqual(OverlayChannelMapChip.displayName(""), "")
  }

  func testMapAndRosterKeepTheSuppliedLeaseNameSpelling() {
    let state = OverlayState.previewFormatted()
    state.applyChannelRoster([
      CsChannelRosterState(
        channel: "3", audience: "div0", provider: "codex", providerSessionId: "div0-session",
        open: false, loud: false, autosealDeadlineUnixMs: nil, followerAlive: true,
        unreadSinceUnixMs: nil)
    ])
    XCTAssertEqual(state.channelMapLayoutModel.agents.map(\.name), ["div0"])
    XCTAssertEqual(state.channelRosterNames["3"], "div0", "the lease keeps its name")
  }

  // MARK: - Open-channel dot (§13.3, §14)

  func testOpenChannelCarriesTheDotInPlace() {
    let map = scenario(open: ["5"])
    XCTAssertEqual(map.chips.map(\.digit), ["0", "2", "3", "5"], "no second row, no reorder")
    XCTAssertEqual(map.chips.map(\.microphoneOpen), [false, false, false, true])
    XCTAssertEqual(map.openDigits, ["5"])
    XCTAssertEqual(map.counterFigure, "5", "the collapsed counter shows the open digit")
    XCTAssertEqual(map.count, 3, "the agent count returns when the channel closes")
    XCTAssertEqual(
      map.agents[2].accessibilityLabel(modifier: .control),
      "Channel 5, Kuba, shortcut Ctrl 5, microphone open")
    XCTAssertTrue(map.counterAccessibilityValue.hasPrefix("Microphone open on channel 5"))
    for form in [OverlayChannelPill.Form.stacked, .chip] {
      XCTAssertEqual(
        row(OverlayChannelMapNameWidths(chips: map.chips, scale: 1).full, form),
        row(OverlayChannelMapNameWidths(chips: scenario().chips, scale: 1).full, form),
        "\(form): the dot overlaps the circle and adds no width")
    }
    XCTAssertEqual(
      DictationOverlayWindow.midiBarGeometry(map: map, textScale: 1),
      DictationOverlayWindow.midiBarGeometry(map: scenario(), textScale: 1),
      "an open channel resizes nothing")
  }

  func testClosedChannelReturnsThePlainChipAndCounter() {
    let map = scenario()
    XCTAssertFalse(map.microphoneOpen)
    XCTAssertEqual(map.counterFigure, "3", "the counter counts agents")
    XCTAssertEqual(
      map.agents[2].accessibilityLabel(modifier: .control), "Channel 5, Kuba, shortcut Ctrl 5")
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

  // MARK: - Mini width (bus brief 2026-11-04 §3)

  /// Mini is as wide and as tall as its content and carries no channel
  /// counter: 7 pt insets, a 9 pt close dot, the closed centre and four 22 pt
  /// controls 4 pt apart. With the map enabled that is 127 × 36; without it
  /// the brand word adds its gap and its measured width.
  func testMiniIsItsControlsWithoutACounter() {
    let floor = DictationOverlayWindow.miniFloorWidth
    XCTAssertEqual(DictationOverlayWindow.miniInset, 7)
    XCTAssertEqual(DictationOverlayWindow.miniControl, 22)
    XCTAssertEqual(DictationOverlayWindow.miniControlSpacing, 4)
    XCTAssertEqual(DictationOverlayWindow.miniOuterPadding, 2 * DictationOverlayWindow.miniInset)
    XCTAssertEqual(
      DictationOverlayWindow.miniTrailingControls, 100, "4 × 22 controls + 3 × 4 gaps")
    XCTAssertEqual(
      2 * DictationOverlayWindow.miniRowSpacing, DictationOverlayWindow.miniControlSpacing,
      "across the closed centre, the gap the controls keep")
    XCTAssertEqual(
      floor,
      DictationOverlayWindow.miniOuterPadding + DictationOverlayWindow.miniBrandDot
        + 2 * DictationOverlayWindow.miniRowSpacing + DictationOverlayWindow.miniTrailingControls)
    XCTAssertEqual(floor, 127, "14 padding + 9 dot + 2 × 2 gaps + 100 controls")
    XCTAssertEqual(DictationOverlayWindow.miniHeight, 36, "22 pt control + 2 × 7 pt insets")
    XCTAssertEqual(
      DictationOverlayWindow.miniBrandWidth,
      ceil(floor + DictationOverlayWindow.miniBrandGap + DictationOverlayWindow.brandWordWidth),
      "without the map mini keeps the word after its 6 pt gap")
    XCTAssertEqual(
      DictationOverlayWindow.minimumSize(for: .mini), NSSize(width: floor, height: 36),
      "mini's floor is its controls")
  }

  /// A live take grows mini (bus brief 2026-11-04 §6, §11D): zero at rest;
  /// a dictation adds the 82 pt waveform and the clock with their 4 pt gaps;
  /// an agent take adds one 19 pt circled digit per open channel, 4 pt apart;
  /// channel 0 adds no row: the Agents button already wears its megaphone.
  func testMiniTakeWidthIsWaveformClockAndAddressees() {
    typealias W = DictationOverlayWindow
    XCTAssertEqual(W.miniTakeWidth(live: false, openDigits: []), 0, "at rest mini adds nothing")
    XCTAssertEqual(W.miniTakeWidth(live: false, openDigits: ["3"]), 0)
    let dictation = W.miniTakeWidth(live: true, openDigits: [])
    XCTAssertEqual(
      dictation,
      ceil(OverlayBarMetrics.miniWaveformWidth + 2 * W.miniControlSpacing + W.midiTimerWidth),
      "a dictation alone: waveform and clock, no addressees")
    XCTAssertEqual(OverlayBarMetrics.miniWaveformWidth, 82, "mini keeps its 24-bar strip")
    XCTAssertEqual(W.miniAddresseesWidth(openDigits: []), 0)

    let circle = OverlayChannelPill.counterWidth(figure: "", openDigitCount: 1, scale: 1)
    XCTAssertEqual(circle, 19, "15 pt circle with its 2 pt padding")
    XCTAssertEqual(W.miniAddresseesWidth(openDigits: ["3"]), circle)
    XCTAssertEqual(
      W.miniAddresseesWidth(openDigits: ["2", "3", "5"]), 3 * circle + 2 * W.miniControlSpacing)
    XCTAssertEqual(
      W.miniTakeWidth(live: true, openDigits: ["2", "5"]),
      ceil(
        OverlayBarMetrics.miniWaveformWidth + 2 * W.miniControlSpacing + W.midiTimerWidth
          + W.miniControlSpacing + 2 * circle + W.miniControlSpacing))
    XCTAssertGreaterThan(
      W.miniTakeWidth(live: true, openDigits: ["2", "5"]),
      W.miniTakeWidth(live: true, openDigits: ["5"]), "each open channel adds its digit")

    XCTAssertEqual(
      W.miniAddresseesWidth(openDigits: ["0"]), 0,
      "channel 0 adds nothing: the Agents button already wears the megaphone")
    XCTAssertEqual(
      W.miniAddresseesWidth(openDigits: ["0", "3"]), 0,
      "a broadcast shows no addressee row at all")
    XCTAssertEqual(W.miniTakeWidth(live: true, openDigits: ["0"]), dictation)
  }

  /// `miniTakeLive` is a dictation or any open channel, channel 0 included.
  func testMiniTakeLiveFollowsDictationAndOpenChannels() throws {
    let (state, broadcast, lena, astra) = try armedState()
    XCTAssertFalse(state.miniTakeLive, "at rest")
    let conversations = [broadcast, lena, astra]
    state.applyChannelRoster(chordRoster(conversations, open: ["3"]))
    XCTAssertTrue(state.miniTakeLive, "an open agent channel")
    state.applyChannelRoster(chordRoster(conversations, open: ["0"]))
    XCTAssertTrue(state.miniTakeLive, "a broadcast on 0")
    state.applyChannelRoster(chordRoster(conversations, open: []))
    XCTAssertFalse(state.miniTakeLive, "the take ended")
    state.handleRecordingStarted()
    XCTAssertTrue(state.recording)
    XCTAssertTrue(state.miniTakeLive, "a dictation")
    state.finishControllerRecording()
    XCTAssertFalse(state.miniTakeLive, "the controller's stop ends it")
  }

  // MARK: - Names (bus brief 2026-11-04 §5, §11C)

  /// Every width is the string measured in the pill's own font: whole up to
  /// six letters by default, else five letters and an ellipsis; whole when
  /// widened; four letters and an ellipsis at the floor.
  func testNameWidthsAreMeasuredStringsInThePillFont() {
    let names = ["Igor", "Konfig", "Konstanty"]
    let widths = nameWidths(names)
    let full = names.map { OverlayChannelPill.nameWidth($0, scale: 1) }
    func measured(_ text: String) -> CGFloat {
      OverlayChannelPill.textWidth(text, size: OverlayChannelPill.nameSize)
    }
    XCTAssertEqual(OverlayChannelPill.preferredNameLength, 6)
    XCTAssertEqual(OverlayChannelPill.preferredNamePrefix, 5)
    XCTAssertEqual(OverlayChannelPill.shortestNamePrefix, 4)
    XCTAssertEqual(widths.full, full, "widened: every name whole")
    XCTAssertEqual(widths.preferred[0], full[0], "four letters stay whole")
    XCTAssertEqual(widths.preferred[1], full[1], "six letters stay whole by default")
    XCTAssertEqual(
      widths.preferred[2], min(full[2], measured("Konst" + OverlayChannelPill.ellipsis)),
      "seven or more letters: five and an ellipsis")
    XCTAssertLessThan(widths.preferred[2], full[2])
    XCTAssertEqual(
      widths.shortest[2], min(full[2], measured("Kons" + OverlayChannelPill.ellipsis)),
      "the floor: four letters and an ellipsis")
    XCTAssertLessThan(widths.shortest[2], widths.preferred[2])
    XCTAssertEqual(widths.shortest[0], full[0], "four letters never lose one")
    for index in names.indices {
      XCTAssertLessThanOrEqual(widths.shortest[index], widths.preferred[index], names[index])
      XCTAssertLessThanOrEqual(widths.preferred[index], widths.full[index], names[index])
    }
    // Larger text measures wider names; a runaway name stops at the cap.
    XCTAssertGreaterThan(
      OverlayChannelMapNameWidths(chips: named(names).agents, scale: 1.2).full[0], full[0])
    XCTAssertEqual(
      OverlayChannelPill.nameWidth(String(repeating: "W", count: 80), scale: 1),
      OverlayChannelPill.maxNameWidth)
  }

  /// `Lena` and `Lena2` must still read differently at the floor: the four
  /// letters stay whole, the five become `Lena…`.
  func testLenaAndLena2StayDistinctAtTheFloor() {
    let widths = nameWidths(["Lena", "Lena2"])
    XCTAssertEqual(widths.shortest[0], widths.full[0], "Lena keeps every letter")
    XCTAssertNotEqual(widths.shortest[0], widths.shortest[1], "Lena and Lena… differ")
    XCTAssertGreaterThan(widths.shortest[1], widths.shortest[0])
    XCTAssertEqual(widths.preferred, widths.full, "both are whole by default")
  }

  /// One common cap cuts the longest names first; short names keep every
  /// letter; nothing goes below its shortest width; channel 0 stays nameless.
  func testFittedCutsTheLongestNamesFirstAndNeverBelowTheFloor() {
    let widths = OverlayChannelMapNameWidths(
      full: [0, 20, 80, 40], preferred: [0, 20, 40, 40], shortest: [0, 10, 30, 20])
    let sum: ([CGFloat]) -> CGFloat = { $0.reduce(0, +) }
    XCTAssertEqual(widths.capped(at: 25), [0, 20, 30, 25])
    XCTAssertEqual(
      widths.fitted(available: nil, rowWidth: sum), widths.full,
      "the ideal-size probe is every name whole")
    XCTAssertEqual(widths.fitted(available: .infinity, rowWidth: sum), widths.full)
    XCTAssertEqual(widths.fitted(available: 140, rowWidth: sum), widths.full)
    XCTAssertEqual(
      widths.fitted(available: 139.6, rowWidth: sum), widths.full,
      "fractional rounding does not cut a name")

    let cut = widths.fitted(available: 100, rowWidth: sum)
    XCTAssertEqual(cut[0], 0, "channel 0 never gains a name")
    XCTAssertEqual(cut[1], 20, "a short name keeps every letter")
    XCTAssertEqual(cut[2], cut[3], "one cap for every name")
    XCTAssertGreaterThan(cut[2], 39)
    XCTAssertLessThanOrEqual(cut[2], 40)
    XCTAssertLessThanOrEqual(sum(cut), 100.5, "the row fits what it was given")
    XCTAssertGreaterThan(
      widths.full[2] - cut[2], widths.full[3] - cut[3], "the longest gives way first")

    XCTAssertEqual(
      widths.fitted(available: 50, rowWidth: sum), widths.shortest,
      "below the floor the shortest names stay; the window never proposes less")
  }

  // MARK: - Rows (bus brief 2026-11-04 §4)

  /// The pill's form follows the height the bar proposes: a stacked pill's
  /// height is the single row, a chip's the second row. The width stays
  /// between the shortest names and every name whole.
  func testSlotChoosesTheFormFromTheProposedHeight() {
    let stackedHeight = OverlayChannelPill.stackedHeight(scale: 1)
    let chipHeight = OverlayChannelPill.headerHeight
    XCTAssertGreaterThan(stackedHeight, chipHeight, "the caption line is under the circle")
    XCTAssertGreaterThanOrEqual(
      stackedHeight,
      OverlayChannelPill.digitCircle + OverlayChannelPill.captionLineHeight(scale: 1))
    let slot = OverlayChannelMapSlot(
      stackedHeight: stackedHeight, chipHeight: chipHeight, stackedWidths: 80...120,
      chipWidths: 100...160)
    XCTAssertTrue(slot.stacks(.unspecified), "the ideal probe is the single row")
    XCTAssertTrue(slot.stacks(ProposedViewSize(width: 90, height: stackedHeight)))
    XCTAssertTrue(slot.stacks(ProposedViewSize(width: 90, height: stackedHeight - 0.4)))
    XCTAssertFalse(slot.stacks(ProposedViewSize(width: 90, height: chipHeight)))
    XCTAssertEqual(slot.width(proposed: nil, range: 80...120), 120)
    XCTAssertEqual(slot.width(proposed: 500, range: 80...120), 120, "no hole beside the pills")
    XCTAssertEqual(slot.width(proposed: 100, range: 80...120), 100)
    XCTAssertEqual(slot.width(proposed: 10, range: 80...120), 80, "never below the floor")
  }

  /// 1–5 agents: one row of stacked pills, two rows only when the user
  /// narrows it below its shortest names. 6–9 agents: always two rows.
  func testAgentCountAndWidthChooseOneOrTwoRows() {
    XCTAssertEqual(OverlayChannelMapModel.singleRowAgentLimit, 5)
    for count in 1...5 {
      let bar = DictationOverlayWindow.midiBarGeometry(map: agents(count), textScale: 1)
      XCTAssertTrue(bar.singleRowMinimumWidth.isFinite, "\(count)")
      XCTAssertEqual(bar.height(forWidth: bar.naturalWidth), bar.singleRowHeight, "\(count)")
      XCTAssertEqual(bar.height(forWidth: bar.maximumWidth), bar.singleRowHeight, "\(count)")
      XCTAssertLessThanOrEqual(bar.minimumWidth, bar.singleRowMinimumWidth, "\(count)")
      XCTAssertEqual(
        bar.height(forWidth: bar.singleRowMinimumWidth - 1), bar.twoRowHeight,
        "\(count): narrowed below its shortest names, one row becomes two")
    }
    for count in 6...9 {
      let bar = DictationOverlayWindow.midiBarGeometry(map: agents(count), textScale: 1)
      XCTAssertEqual(bar.singleRowMinimumWidth, .infinity, "\(count)")
      for width in [bar.minimumWidth, bar.naturalWidth, bar.maximumWidth] {
        XCTAssertEqual(bar.height(forWidth: width), bar.twoRowHeight, "\(count) at \(width)")
      }
    }
    let one = DictationOverlayWindow.midiBarGeometry(map: agents(1), textScale: 1)
    XCTAssertGreaterThan(one.twoRowHeight, one.singleRowHeight)
    XCTAssertGreaterThanOrEqual(
      one.singleRowHeight, ceil(OverlayChannelPill.stackedHeight(scale: 1)),
      "the single row holds a stacked pill")
  }

  /// The bar is as wide as its content: no 410 pt floor, no 820 pt budget.
  /// Its fixed parts are one constant and the map's measured row is the rest,
  /// so a new agent adds exactly its pill.
  func testBarWidthIsItsMeasuredContent() {
    typealias W = DictationOverlayWindow
    let fixed = ceil(
      W.midiOuterPadding + W.midiBrandWidth + 2 * OverlayBarMetrics.itemSpacing
        + OverlayBarMetrics.waveformWidth + W.midiTimerWidth + OverlayChannelMapView.leadingGap
        + OverlayBarMetrics.itemSpacing + W.midiTrailingControls)
    XCTAssertEqual(OverlayBarMetrics.itemSpacing, 10)
    XCTAssertEqual(OverlayBarMetrics.waveformWidth, 110, "32 bars of 1.5 pt with 31 gaps of 2 pt")
    XCTAssertEqual(OverlayBarMetrics.waveformMaximumWidth, 170)
    XCTAssertEqual(OverlayBarMetrics.rowGap, 2)
    for names in [["Miksa", "Igor", "Kuba"], ["Miksa", "Igor", "Kuba", "Konstanty"], ["Lena"]] {
      let map = named(names)
      let bar = W.midiBarGeometry(map: map, textScale: 1)
      let widths = OverlayChannelMapNameWidths(chips: map.chips, scale: 1)
      XCTAssertEqual(
        bar.naturalWidth, fixed + row(widths.preferred, .stacked), accuracy: 1,
        "\(names): the default bar is its parts with the preferred names")
      XCTAssertEqual(
        bar.maximumWidth, fixed + row(widths.full, .stacked), accuracy: 1,
        "\(names): widening stops at every name whole")
      XCTAssertEqual(
        bar.singleRowMinimumWidth, fixed + row(widths.shortest, .stacked), accuracy: 1,
        "\(names): one row's floor is the shortest names")
      XCTAssertLessThanOrEqual(bar.minimumWidth, bar.naturalWidth)
      XCTAssertLessThanOrEqual(bar.naturalWidth, bar.maximumWidth)
    }
    let short = W.midiBarGeometry(map: named(["Miksa", "Igor", "Kuba"]), textScale: 1)
    XCTAssertEqual(
      short.naturalWidth, short.maximumWidth,
      "names up to six letters are whole by default, so widening opens no hole")
    let long = W.midiBarGeometry(map: named(["Konstanty"]), textScale: 1)
    XCTAssertLessThan(long.naturalWidth, long.maximumWidth, "a long name widens when asked")
    // Nine agents: two rows, the wider of the first row and the chips.
    let nine = agents(9)
    let nineBar = W.midiBarGeometry(map: nine, textScale: 1)
    let nineNames = OverlayChannelMapNameWidths(chips: nine.chips, scale: 1)
    XCTAssertEqual(
      nineBar.naturalWidth,
      ceil(
        max(
          fixed - OverlayChannelMapView.leadingGap,
          W.midiOuterPadding + row(nineNames.preferred, .chip))), accuracy: 1)
    // The screen bounds every width.
    let bounded = W.midiBarGeometry(map: nine, textScale: 1, visibleWidth: 300)
    XCTAssertLessThanOrEqual(bounded.naturalWidth, 300)
    XCTAssertLessThanOrEqual(bounded.maximumWidth, 300)
    XCTAssertLessThanOrEqual(bounded.minimumWidth, bounded.maximumWidth)
  }

  /// Pills keep the map's spacing and their circles never overlap, in the
  /// stacked form and in the chip form, at every width the names can take.
  func testPillCirclesNeverOverlapInEitherForm() {
    let widths = nameWidths(["Miksa", "Konstanty", "Igor", "Lena2", "Bo"])
    let spacing = OverlayChannelMapView.chipSpacing
    let circle = OverlayChannelPill.digitCircle
    XCTAssertGreaterThan(spacing, 0)
    for form in [OverlayChannelPill.Form.stacked, .chip] {
      for names in [widths.full, widths.preferred, widths.shortest] {
        var x: CGFloat = 0
        var circles: [ClosedRange<CGFloat>] = []
        for name in names {
          let width = OverlayChannelPill.chipWidth(nameWidth: name, form: form, scale: 1)
          switch form {
          case .stacked:
            // The circle and the caption under it share the pill's width.
            XCTAssertGreaterThanOrEqual(width, circle + 2 * OverlayChannelPill.stackedPadding)
            XCTAssertGreaterThanOrEqual(width, name + 2 * OverlayChannelPill.stackedPadding)
            circles.append((x + (width - circle) / 2)...(x + (width + circle) / 2))
          default:
            XCTAssertGreaterThanOrEqual(width, 2 * OverlayChannelPill.chipPadding + circle + name)
            let start = x + OverlayChannelPill.chipPadding
            circles.append(start...(start + circle))
          }
          x += width + spacing
        }
        XCTAssertEqual(x - spacing, row(names, form), accuracy: 0.01, "\(form): one row width")
        for (left, right) in zip(circles, circles.dropFirst()) {
          XCTAssertGreaterThanOrEqual(
            right.lowerBound - left.upperBound, spacing, "\(form): circles overlap")
        }
      }
    }
    XCTAssertGreaterThan(
      OverlayChannelPill.chipWidth(nameWidth: 30, form: .chip, scale: 1.2),
      OverlayChannelPill.chipWidth(nameWidth: 30, form: .chip, scale: 1))
  }

  /// The folded rail's unread count is its own capsule outside the pill, at
  /// its top-right shoulder: no point of it lies inside the pill's capsule,
  /// so the channel digit stays whole; it grows with its digits; and the
  /// rail keeps room for two digits beside its widest pill.
  func testRailUnreadCapsuleSitsBesideThePillAndGrowsWithItsDigits() throws {
    func inCapsule(_ rect: CGRect, _ point: CGPoint) -> Bool {
      let radius = min(rect.width, rect.height) / 2
      let x = min(max(point.x, rect.minX + radius), rect.maxX - radius)
      let y = min(max(point.y, rect.minY + radius), rect.maxY - radius)
      return hypot(point.x - x, point.y - y) < radius
    }
    for scale: CGFloat in [1, 1.2, 1.5, 2] {
      let widest = OverlayAgentDrawerLayout.railPillWidth(textScale: scale)
      // The antenna's pill and an agent's (dot 8 instead of glyph 12).
      for pillWidth in [widest, widest - 4 * scale] {
        let pill = CGRect(x: 0, y: 0, width: pillWidth, height: 24 * scale)
        for count in [1, 9, 10, 99, 100, 250] {
          let badge = OverlayChannelPill.railBadgeFrame(
            count: count, pillSize: pill.size, scale: scale)
          XCTAssertEqual(badge.height, OverlayChannelPill.railBadgeHeight * scale)
          XCTAssertGreaterThan(badge.maxX, pill.maxX, "\(count) @\(scale): beside the pill")
          XCTAssertLessThan(badge.minY, pill.minY, "\(count) @\(scale): at its top shoulder")
          XCTAssertLessThan(badge.midY, pill.midY, "\(count) @\(scale): not below its middle")
          var y = badge.minY
          while y <= badge.maxY {
            var x = badge.minX
            while x <= badge.maxX {
              let point = CGPoint(x: x, y: y)
              if inCapsule(badge, point) {
                XCTAssertFalse(
                  inCapsule(pill, point), "\(count) @\(scale): the capsule overlaps the pill")
                if inCapsule(pill, point) { return }
              }
              x += 0.25
            }
            y += 0.25
          }
        }
      }
      let width = { OverlayChannelPill.railBadgeWidth(count: $0, scale: scale) }
      XCTAssertEqual(width(1), OverlayChannelPill.railBadgeHeight * scale, accuracy: 1)
      XCTAssertEqual(width(9), width(1), "one digit is a circle")
      XCTAssertGreaterThan(width(10), width(9), "10 is wider than 9")
      XCTAssertEqual(width(99), width(10))
      XCTAssertEqual(width(100), width(99), "99+ keeps the two digits' width")
      XCTAssertEqual(width(250), width(99))
      XCTAssertEqual(OverlayChannelPill.railBadgeLabel(count: 99), "99")
      XCTAssertEqual(OverlayChannelPill.railBadgeLabel(count: 100), "99+")
      XCTAssertEqual(OverlayChannelPill.railBadgeLabel(count: 250), "99+")
      // `99+` (8 pt digits, 6 pt plus, each measured with a point of
      // slack) fits the two digits' width.
      let figure =
        OverlayChannelPill.textWidth(
          "99", size: OverlayChannelPill.railBadgeCappedDigitSize * scale, monospacedDigits: true)
        + OverlayChannelPill.textWidth(
          "+", size: OverlayChannelPill.railBadgeCappedPlusSize * scale)
      XCTAssertLessThanOrEqual(figure, width(250), "@\(scale): 99+ is not clipped")
      // Centred in the column before the reserved room, the widest pill's
      // two-digit capsule ends inside the rail's content, before the band.
      let content =
        OverlayAgentDrawerLayout.railWidth(textScale: scale) - 2 * OverlayResizeHit.band
      let reserved = OverlayChannelPill.railBadgeFrame(
        count: 99, pillSize: CGSize(width: widest, height: 24 * scale), scale: scale)
      XCTAssertLessThanOrEqual(2 + reserved.maxX, content + 0.001, "@\(scale): 99 is not clipped")
    }
    let pillSource = try overlaySource("OverlayChannelPill.swift")
    let badge = try section(
      of: pillSource, from: "private var railUnreadBadge: some View {",
      to: "private var spokenValue: String {")
    XCTAssertTrue(
      badge.contains(
        "Self.railBadgeFrame(count: display.unreadCount, pillSize: .zero, scale: textScale)"),
      "the capsule is drawn from the frame these assertions measure")
    XCTAssertTrue(badge.contains(".offset(x: frame.maxX, y: frame.minY)"))
    XCTAssertFalse(pillSource.contains("OverlayAgentUnreadBadge("), "the rail draws its own count")
  }

  /// The bar's pills keep their hint on: an agent's chip shows its hover
  /// card, built from the chip's status, with the plain `5 · Kuba · Ctrl+5`
  /// as the fallback; only channel 0 gives its hover to the carousel.
  func testBarPillsShowTheirHint() throws {
    let kuba = scenario().agents[2]
    let pill = OverlayChannelPill(
      form: .chip, display: OverlayChannelPill.Display(figure: kuba.digit, name: kuba.name),
      hint: kuba.hint(modifier: .control), accessibilityText: "", identifier: "hint-probe",
      palette: .dark, textScale: 1
    ) {}
    XCTAssertTrue(pill.showsHint)
    XCTAssertEqual(pill.hint, "5 · Kuba · Ctrl+5")
    let mapView = try overlaySource("OverlayChannelMapView.swift")
    XCTAssertTrue(
      mapView.contains("showsHint: !(chip.isBroadcast && channelZeroHint != nil)"),
      "only channel 0 with its carousel turns the pill's own hint off")
    XCTAssertTrue(mapView.contains("hint: chip.hint(modifier: modifier)"))
    XCTAssertTrue(mapView.contains("hintCard: hintCard(chip)"), "agents show their card")
  }

  /// Unread, dimming and the badge stay on every pill whatever its name width.
  func testUnreadDimmingAndBadgeComeFromTheRoster() {
    let marked = model(
      channels: [delivery("2", "Miksa"), delivery("3", "Igor")],
      hudStates: [
        "2": hud(open: true, session: "s2"), "3": hud(alive: false, session: "s3"),
      ], unread: ["3"])
    XCTAssertEqual(marked.agents.map(\.microphoneOpen), [true, false])
    XCTAssertEqual(marked.agents.map(\.disconnected), [false, true])
    XCTAssertEqual(marked.agents.map(\.unread), [false, true])
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
    let kuba = scenario().agents[2]
    XCTAssertEqual(kuba.hint(modifier: .control), "5 · Kuba · Ctrl+5")
    XCTAssertEqual(kuba.hint(modifier: .function), "5 · Kuba · Fn+5")
    XCTAssertEqual(
      kuba.accessibilityLabel(modifier: .function), "Channel 5, Kuba, shortcut Fn 5")
  }

  /// Both keys record dictation: the core resolves the modifier to `off`,
  /// and no hint names a chord that does not exist.
  func testNoChordWhenBothKeysRecord() {
    XCTAssertEqual(OverlayChannelChordModifier(setting: "off"), .unbound)
    let kuba = scenario().agents[2]
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
    XCTAssertEqual(map.chips.map(\.digit), ["0", "2", "3"])
    XCTAssertEqual(map.chips.map(\.name), [OverlaySharedChannelName.defaultName, "lena", "astra"])
    XCTAssertEqual(
      map.chips.map(\.selectable), [true, true, true], "(0) selects the All conversation")
    XCTAssertEqual(map.chips.map(\.selected), [false, false, false])

    roster.selectChannelConversation("3")
    XCTAssertEqual(state.selectedConversationID, astra.id, "the exact owner's conversation")
    XCTAssertEqual(state.windowContentSurface, .agents)
    XCTAssertEqual(state.presentationMode, .expanded, "the Agents canvas unfolds")
    XCTAssertEqual(state.captureGeneration, generation)
    XCTAssertTrue(state.pendingChannelToggles.isEmpty, "a chip is not a microphone")
    XCTAssertFalse(state.channelHudStates.values.contains(where: \.open))
    XCTAssertEqual(
      DictationOverlayView(state: state).channelStatusView.channelMapModel.chips.map(\.selected),
      [false, false, true], "the selected conversation's pill wears the selection fill")

    // (0)'s click is the same path: the All conversation, no microphone.
    roster.selectChannelConversation("0")
    XCTAssertEqual(state.selectedConversationID, broadcast.id)
    XCTAssertEqual(state.captureGeneration, generation)
    XCTAssertTrue(state.pendingChannelToggles.isEmpty, "(0) is not a microphone")
    XCTAssertFalse(state.channelHudStates.values.contains(where: \.open))
    XCTAssertEqual(
      DictationOverlayView(state: state).channelStatusView.channelMapModel.chips.map(\.selected),
      [true, false, false])
    // A digit with no current conversation is not an identity.
    roster.selectChannelConversation("7")
    XCTAssertEqual(state.selectedConversationID, broadcast.id)
    XCTAssertTrue(state.pendingChannelToggles.isEmpty)
  }

  /// The horizontal mini carries no channel map (Founder, 2026-11-04: the
  /// Agents list replaces its counter), and a stray counter call there
  /// changes nothing.
  func testHorizontalMiniHasNoChannelMapAndTheCounterCallIsANoOp() throws {
    let (state, _, _, _) = try armedState()
    state.channelMapPreference.selectLayout(.horizontal)
    state.setPresentationMode(.mini)
    XCTAssertTrue(state.channelMapInMini, "the map setting still shapes mini")
    let surface = state.windowContentSurface
    let generation = state.captureGeneration
    var restored = 0
    state.onRestoreMidiNaturalWidth = { restored += 1 }
    let view = DictationOverlayView(state: state)
    XCTAssertNil(view.channelMap(roster: view.channelStatusView), "mini shows no counter")
    state.activateChannelMapCounter()
    XCTAssertEqual(state.presentationMode, .mini, "mini stays mini")
    XCTAssertEqual(restored, 0)
    XCTAssertNil(state.selectedConversationID, "no conversation is chosen")
    XCTAssertEqual(state.windowContentSurface, surface)
    XCTAssertEqual(state.captureGeneration, generation)
    XCTAssertTrue(state.pendingChannelToggles.isEmpty, "nothing touches the microphone")
  }

  /// The vertical layout keeps its stacked counter in mini and in the bar
  /// (Maciej, 2026-10-23: the way to the Intercom group stays in every
  /// form); the counter leads to the Intercom group and keeps the form.
  func testVerticalCounterLivesInMiniAndTheBarAndOpensTheIntercomAndNothingElse() throws {
    let (state, _, _, _) = try armedState()
    state.channelMapPreference.selectLayout(.vertical)
    state.setPresentationMode(.mini)
    let surface = state.windowContentSurface
    let generation = state.captureGeneration
    var revealed = 0
    var restored = 0
    state.onRevealIntercom = { revealed += 1 }
    state.onRestoreMidiNaturalWidth = { restored += 1 }
    let view = DictationOverlayView(state: state)
    let miniCounter = try XCTUnwrap(
      view.channelMap(roster: view.channelStatusView), "the vertical mini keeps its counter")
    XCTAssertEqual(miniCounter.placement, .counter)
    XCTAssertTrue(miniCounter.stacked)
    miniCounter.onCounter()
    XCTAssertEqual(revealed, 1, "mini's counter leads to the Intercom group")
    XCTAssertEqual(state.presentationMode, .mini, "mini stays mini")
    state.setPresentationMode(.midi)
    let map = try XCTUnwrap(view.channelMap(roster: view.channelStatusView))
    XCTAssertEqual(map.placement, .counter)
    XCTAssertTrue(map.stacked)
    XCTAssertEqual(map.model.counterFigure, "2", "the badge carries the same agent count")
    map.onCounter()
    XCTAssertEqual(revealed, 2, "the counter leads to the Intercom group")
    XCTAssertEqual(state.presentationMode, .midi, "the bar stays the bar")
    XCTAssertEqual(restored, 0)
    XCTAssertNil(state.selectedConversationID)
    XCTAssertEqual(state.windowContentSurface, surface)
    XCTAssertEqual(state.captureGeneration, generation)
    XCTAssertTrue(state.pendingChannelToggles.isEmpty)
  }

  /// With the map enabled the horizontal mini is the 127 × 36 capsule of
  /// its controls; the vertical mini is wider by its stacked counter and the
  /// 6 pt gap after the dot, as the counter is drawn.
  func testVerticalMiniIsWiderByItsStackedCounter() throws {
    let (state, _, _, _) = try armedState()
    let panel = FloatingOverlayPanel(
      contentRect: NSRect(x: 100, y: 100, width: 200, height: 46),
      styleMask: [.borderless, .nonactivatingPanel], backing: .buffered, defer: false)
    func fit() -> CGFloat {
      let counter = DictationOverlayWindow.miniStackedCounterWidth(
        map: state.channelMapLayoutModel, stacked: state.channelMapCounterStacked, textScale: 1)
      panel.setChannelMapPresentation(
        enabled: state.channelMapInMini, stackedCounterWidth: counter)
      return counter
    }
    state.channelMapPreference.selectLayout(.horizontal)
    XCTAssertTrue(state.channelMapInMini)
    XCTAssertEqual(fit(), 0, "the horizontal mini has no counter")
    XCTAssertEqual(panel.miniSize, NSSize(width: 127, height: 36))

    state.channelMapPreference.selectLayout(.vertical)
    let map = state.channelMapLayoutModel
    let expected = ceil(
      DictationOverlayWindow.miniBrandGap
        + OverlayChannelPill.counterWidth(
          figure: String(map.count), openDigitCount: map.openDigits.count,
          scale: OverlayChannelPill.headerScale(1), stacked: true))
    XCTAssertEqual(fit(), expected)
    XCTAssertGreaterThan(expected, 0)
    XCTAssertEqual(panel.miniSize, NSSize(width: 127 + expected, height: 36))

    panel.setChannelMapPresentation(enabled: false, stackedCounterWidth: expected)
    XCTAssertEqual(
      panel.miniSize,
      NSSize(width: DictationOverlayWindow.miniBrandWidth, height: 36),
      "without the map mini keeps the brand word")
  }

  func testCompactCounterRestoresTheNaturalWidthAndNothingElse() throws {
    let (state, _, _, _) = try armedState()
    state.setPresentationMode(.midi)
    let generation = state.captureGeneration
    var restored = 0
    state.onRestoreMidiNaturalWidth = { restored += 1 }
    let view = DictationOverlayView(state: state)
    let map = try XCTUnwrap(view.channelMap(roster: view.channelStatusView))
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
    let view = DictationOverlayView(state: state)
    XCTAssertNil(view.channelMap(roster: view.channelStatusView))
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

  /// With the map on, a chord in mini or compact keeps the form: the bar's
  /// chip shows the open digit and mini's Agents button the capture, so the
  /// chord is readable without unfolding. The selection follows the open channel as before.
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

  // MARK: - Hosted expanded bar

  /// Two agents after `(0)`: one row of stacked pills, at the bar's measured
  /// size.
  func testExpandedBarShowsStackedPillsAndAPillClickSelects() throws {
    let (state, _, _, astra) = try armedState()
    state.setPresentationMode(.midi)
    let bar = DictationOverlayWindow.midiBarGeometry(
      map: state.channelMapLayoutModel, textScale: 1)
    let host = NSHostingView(
      rootView: DictationOverlayView(state: state)
        .defaultAppStorage(try XCTUnwrap(UserDefaults(suiteName: UUID().uuidString))))
    host.sizingOptions = []
    let panel = FloatingOverlayPanel(
      contentRect: NSRect(
        x: 100, y: 100, width: bar.naturalWidth,
        height: bar.height(forWidth: bar.naturalWidth)),
      styleMask: [.borderless, .nonactivatingPanel], backing: .buffered, defer: false)
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
      XCTAssertLessThanOrEqual(chip.height, OverlayChannelPill.stackedHeight(scale: 1) + 0.5)
      XCTAssertGreaterThan(
        chip.height, OverlayChannelPill.headerHeight, "the name sits under the circle")
    }
    let zero = try NativeAccessibilityProbe.waitForElements("overlay-channel-chip-0").elements
    XCTAssertEqual(zero.first?.role, "AXButton", "(0) leads the row while channel 0 is closed")
    XCTAssertTrue(
      try NativeAccessibilityProbe.query("overlay-channel-map-toggle").elements.isEmpty,
      "the bar shows its pills, not the counter")
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
    XCTAssertEqual(state.agentsLayout, .split)
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

  func testRailPillsAndMapChipsShareOnePillView() throws {
    let status = try overlaySource("OverlayChannelStatusView.swift")
    let rail = try section(
      of: status, from: "private func railPill(", to: "var channelMapModel")
    XCTAssertTrue(rail.contains("OverlayChannelPill("))
    XCTAssertTrue(rail.contains("form: .rail"))
    XCTAssertFalse(rail.contains("Text(verbatim: digit)"), "the rail does not draw its own digit")
    let mapView = try overlaySource("OverlayChannelMapView.swift")
    XCTAssertTrue(
      mapView.contains("proxy.size.height + 0.5 >= stackedHeight ? .stacked : .chip"),
      "the bar's row picks stacked pills or chips by the height it gives the map")
    XCTAssertTrue(mapView.contains("form: form"))
    XCTAssertTrue(mapView.contains("form: .counter"))
    XCTAssertFalse(mapView.contains("Text(verbatim:"), "the map does not draw a digit itself")
    let pill = try overlaySource("OverlayChannelPill.swift")
    XCTAssertEqual(
      pill.components(separatedBy: "Text(verbatim: text)").count - 1, 1,
      "one drawing of the digit")
    // Founder, 2026-11-04 (decision 3): the open channel's violet circle and
    // waveform are enough; no pulsing open-channel dot on the chip's circle.
    let pillView = try section(
      of: pill, from: "struct OverlayChannelPill: View", to: "// MARK: Header metrics")
    XCTAssertFalse(
      pillView.contains("openChannelDot"),
      "the pulsing open-channel dot on the chip circle is gone")
    // Symbols replace a figure: the stacked counter and the numberless human
    // mailbox. Neither adds a glyph to a numbered agent's digit.
    XCTAssertEqual(
      pillView.components(separatedBy: "Image(systemName:").count - 1, 1,
      "the numberless mailbox carries its symbol")
    XCTAssertEqual(
      pillView.components(separatedBy: "symbol: Self.stackedCardsSymbol").count - 1, 1,
      "the stacked counter uses the shared header glyph exactly once")
    XCTAssertFalse(pillView.contains("mic.fill"), "the microphone glyph left the digit")
    XCTAssertFalse(pillView.contains("megaphone"), "the megaphone stays on the Agents button")
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

  func testMapSelectionStaysSeparateFromExplicitMicrophoneControls() async throws {
    for stacked in [false, true] {
      let (state, broadcast, lena, astra) = try armedState()
      state.channelMapPreference.selectLayout(stacked ? .vertical : .horizontal)
      state.setPresentationMode(.midi)
      let generation = state.captureGeneration
      let hudBefore = state.channelHudStates
      let view = DictationOverlayView(state: state)
      let map = try XCTUnwrap(view.channelMap(roster: view.channelStatusView))
      XCTAssertEqual(map.stacked, stacked)

      for conversation in [lena, astra, broadcast] {
        map.onSelectChannel(conversation.channel)
        await Task.yield()
        XCTAssertEqual(state.selectedConversationID, conversation.id)
        XCTAssertEqual(state.captureGeneration, generation)
        XCTAssertEqual(state.channelHudStates, hudBefore)
        XCTAssertTrue(state.pendingChannelToggles.isEmpty)
      }
    }
  }

  // MARK: - An agent speaking (L1)

  private func owner(_ name: String, channel: String) throws -> OverlayConversationOwner {
    try XCTUnwrap(
      OverlayConversationOwner(row: [
        "provider": "codex", "provider_session_id": "s\(channel)",
        "lease_id": String(repeating: channel, count: 32), "channel": channel, "name": name,
      ]))
  }

  private func reply(_ state: String, owner: OverlayConversationOwner) -> OverlayConversationMessage {
    OverlayConversationMessage(
      id: "reply:\(state)", kind: .reply, text: "Gotowe.", order: 1,
      emittedAt: "2026-11-06T03:00:00Z", owner: owner, recipients: [], deliveryID: nil,
      replyTo: nil, unsolicited: true,
      playback: OverlayReplyPlayback(
        replyID: state, ticket: "t-\(state)", state: state, reason: nil, spoken: false,
        emittedAt: "2026-11-06T03:00:01Z"),
      busPath: "/dev/null")
  }

  /// While an agent's reply plays aloud its presence dot is the violet
  /// speaking dot, in the agents list and on its bar chip; synthesis still
  /// waiting and a finished playback leave the presence dot as it was.
  func testAgentSpeakingTurnsItsDotVioletOnlyWhilePlaying() throws {
    let kuba = try owner("Kuba", channel: "5")
    func roster(playback: String?) -> OverlayChannelStatusView {
      let messages = playback.map { [reply($0, owner: kuba)] } ?? []
      let conversation = OverlayConversation(
        id: kuba.id, channel: "5", name: "Kuba", owner: kuba, messages: messages)
      return OverlayChannelStatusView(
        channels: [delivery("5", "Kuba")], unavailable: false, palette: .light,
        animates: false, hudStates: ["5": hud(session: "s5")],
        conversations: [conversation], unreadCounts: [:], onSelectConversation: { _ in })
    }
    for (playback, speaking) in [
      ("playing", true), ("waiting", false), ("finished", false), (nil, false),
    ] as [(String?, Bool)] {
      let view = roster(playback: playback)
      let channel = try XCTUnwrap(view.channels.first)
      XCTAssertEqual(
        view.presenceDotKind(for: channel), speaking ? .speaking : .present, "\(playback ?? "none")")
      let chip = view.channelMapModel.chips.first { $0.digit == "5" }
      XCTAssertEqual(chip?.speaking, speaking, "\(playback ?? "none")")
      // The chip's hover card hears it from the same source (L2).
      let status = try XCTUnwrap(chip?.status)
      XCTAssertEqual(status.speaking, speaking, "\(playback ?? "none"): the card's status")
      XCTAssertEqual(
        OverlayChannelChipHoverCard.presenceDot(status), speaking ? .speaking : .present,
        "\(playback ?? "none"): the card's presence dot")
    }
  }

  /// The speaking dot blinks on the listening caret's beat; with Reduce
  /// Motion it stays full.
  func testSpeakingDotBlinksUnlessMotionIsReduced() {
    let beat = OverlayAgentStatusDot.speakingBeat
    XCTAssertEqual(OverlayAgentStatusDot.speakingOpacity(at: beat * 0.5, reduceMotion: false), 1)
    XCTAssertEqual(
      OverlayAgentStatusDot.speakingOpacity(at: beat * 1.5, reduceMotion: false),
      OverlayAgentStatusDot.speakingFaintOpacity)
    for step in 0..<8 {
      XCTAssertEqual(
        OverlayAgentStatusDot.speakingOpacity(at: beat * Double(step) + 0.1, reduceMotion: true), 1)
    }
  }

  // MARK: - Channel 0's name (shared channel)

  /// Channel 0 carries its display name like an agent: the stored name, or
  /// the default when none is stored, on the bar's chip, its hint, the row
  /// in the agents list, the roster's header and the overlay's map that sizes
  /// the bar. Only the name changes; the digit stays 0.
  func testChannelZeroCarriesItsDisplayNameEverywhere() throws {
    let state = OverlayState(micAccessProvider: { true })
    var stored = ""
    state.sharedChannelNameSetting = { stored }
    state.applyConversationSnapshot(
      .init(
        deliveries: [delivery("3", "Lena")],
        conversations: [
          OverlayConversation(id: "0", channel: "0", name: "All", owner: nil, messages: [])
        ]))
    state.applyChannelRoster([
      CsChannelRosterState(
        channel: "3", audience: "Lena", provider: "codex", providerSessionId: "s3", open: false,
        loud: false, autosealDeadlineUnixMs: nil, followerAlive: true, unreadSinceUnixMs: nil)
    ])
    for (value, shown) in [
      ("", OverlaySharedChannelName.defaultName), ("Ekipa", "Ekipa"),
      ("  ", OverlaySharedChannelName.defaultName),
    ] {
      stored = value
      state.refreshSharedChannelName()
      XCTAssertEqual(state.sharedChannelName, shown)
      let zero = try XCTUnwrap(state.channelMapLayoutModel.chips.first)
      XCTAssertEqual(zero.digit, "0")
      XCTAssertEqual(zero.name, shown)
      let roster = DictationOverlayView(state: state).channelStatusView
      XCTAssertEqual(roster.channelMapModel.chips.first?.name, shown)
      XCTAssertEqual(roster.hoverListRows.first { $0.digit == "0" }?.name, shown)
      XCTAssertTrue(roster.broadcastHeader.hasPrefix("0 · " + shown), roster.broadcastHeader)
      XCTAssertTrue(
        zero.hint(modifier: .control).hasPrefix("0 · " + shown), zero.hint(modifier: .control))
    }
  }

  /// A cut name shows its short form when it has one and it fits (channel
  /// 0's default name), else the name with an ellipsis; a name the width
  /// holds is shown whole.
  func testChannelZeroDefaultNameShowsItsShortFormWhenCut() {
    func pill(_ name: String, short: String?, width: CGFloat) -> OverlayChannelPill {
      OverlayChannelPill(
        form: .stacked,
        display: OverlayChannelPill.Display(
          figure: "0", name: name, nameWidth: width, shortName: short),
        hint: "", accessibilityText: "", identifier: "t", palette: .light, textScale: 1
      ) {}
    }
    let whole = OverlayChannelPill.nameWidth("Wspólny", scale: 1)
    let short = OverlayChannelPill.nameWidth("Wsp.", scale: 1)
    XCTAssertEqual(pill("Wspólny", short: "Wsp.", width: whole).shownName("Wspólny"), "Wspólny")
    XCTAssertEqual(pill("Wspólny", short: "Wsp.", width: short).shownName("Wspólny"), "Wsp.")
    XCTAssertEqual(pill("Wspólny", short: "Wsp.", width: short - 4).shownName("Wspólny"), "Wspólny")
    XCTAssertEqual(pill("Wspólny", short: nil, width: short).shownName("Wspólny"), "Wspólny")
    let widths = OverlayChannelMapNameWidths(
      chips: [
        OverlayChannelMapChip(
          digit: "0", name: "Wspólny", disconnected: false, microphoneOpen: false, unread: false,
          selectable: true, selected: false, shortName: "Wsp.")
      ], scale: 1)
    XCTAssertEqual(widths.preferred, [short], "the default bar gives the short form's width")
    XCTAssertEqual(widths.full, [whole])
  }
}
