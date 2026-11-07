import SwiftUI
import XCTest

@testable import Codescribe

/// Falsifiers for the bar chip's hover card (Founder, 2026-11-04): it states
/// presence, channel 0 reception and unread replies as separate lines, names
/// the agent and its provider, keeps the chord as its last line, and reads
/// every line to VoiceOver as one label. It no longer repeats the button
/// (`5 · Kuba · Ctrl+5`) and nothing else.
@MainActor
final class OverlayChannelChipHoverCardTests: XCTestCase {

  private func chip(
    _ digit: String = "5", _ name: String = "Kuba", status: OverlayChannelChipStatus? = nil
  ) -> OverlayChannelMapChip {
    OverlayChannelMapChip(
      digit: digit, name: name, disconnected: status?.presence == .inactive,
      microphoneOpen: false, unread: (status?.unread ?? 0) > 0, selectable: true,
      selected: false, status: status)
  }

  private func lines(
    _ status: OverlayChannelChipStatus, modifier: OverlayChannelChordModifier = .control
  ) -> [OverlayChannelChipHoverCard.Line] {
    OverlayChannelChipHoverCard.lines(
      chip: chip(status: status), status: status, modifier: modifier)
  }

  private func line(
    _ kind: OverlayChannelChipHoverCard.Line.Kind, in status: OverlayChannelChipStatus
  ) -> OverlayChannelChipHoverCard.Line? {
    lines(status).first { $0.kind == kind }
  }

  // MARK: - Presence

  func testPresenceReadsOnlineNotActiveAndNoData() {
    let online = OverlayChannelChipStatus(presence: .online, reception: .member)
    let inactive = OverlayChannelChipStatus(presence: .inactive, reception: .member)
    let unknown = OverlayChannelChipStatus(presence: .unknown, reception: .member)
    XCTAssertEqual(line(.presence, in: online)?.text, "Online")
    XCTAssertEqual(line(.presence, in: online)?.tone, .success)
    XCTAssertEqual(line(.presence, in: inactive)?.text, "Not active", "never a sure Offline")
    XCTAssertEqual(line(.presence, in: inactive)?.tone, .muted)
    XCTAssertEqual(line(.presence, in: unknown)?.text, "No data")
    XCTAssertEqual(line(.presence, in: unknown)?.tone, .muted)
    for status in [online, inactive, unknown] {
      XCTAssertFalse(
        line(.presence, in: status)?.text.contains("Offline") ?? true, "no Offline claim")
    }
  }

  /// The presence line wears the agents list's own dot (L2): olive online,
  /// violet and blinking while the reply plays, a muted ring for a gone
  /// follower, "?" without data, the stall's olive "!" (L6: not sleep).
  func testPresenceLineWearsTheRosterPresenceDot() {
    func dot(_ status: OverlayChannelChipStatus) -> OverlayAgentDotKind? {
      line(.presence, in: status)?.dot
    }
    XCTAssertEqual(dot(.init(presence: .online, reception: .member)), .present)
    XCTAssertEqual(dot(.init(presence: .online, reception: .member, speaking: true)), .speaking)
    XCTAssertEqual(dot(.init(presence: .inactive, reception: .member)), .sleeping)
    XCTAssertEqual(
      dot(.init(presence: .inactive, reception: .member, speaking: true)), .sleeping,
      "a gone follower is not speaking")
    XCTAssertEqual(dot(.init(presence: .unknown, reception: .member)), .idle)
    let stalled = OverlayChannelChipStatus(presence: .online, stalled: true, reception: .member)
    XCTAssertEqual(dot(stalled), .stalled, "a stall is the olive dot with its \"!\"")
    XCTAssertNil(line(.presence, in: stalled)?.symbol, "never the moon's zzz")
    XCTAssertEqual(
      dot(.init(presence: .online, stalled: true, reception: .member, speaking: true)), .speaking,
      "a reply playing aloud wins over the stall, as in the agents list")
    for status in [
      OverlayChannelChipStatus(presence: .online, reception: .member),
      OverlayChannelChipStatus(presence: .unknown, reception: .member),
      stalled,
    ] {
      XCTAssertNil(line(.presence, in: status)?.symbol, "the dot stands in the symbol's place")
    }
  }

  func testStallIsFlaggedBesidePresence() {
    let stalled = OverlayChannelChipStatus(presence: .online, stalled: true, reception: .member)
    XCTAssertEqual(line(.presence, in: stalled)?.text, "Online · Not reading")
    // No amber in the bus (Founder, 2026-11-04): a stall reads in the muted tone.
    XCTAssertEqual(line(.presence, in: stalled)?.tone, .muted)
    XCTAssertEqual(line(.presence, in: stalled)?.dot, .stalled, "the stall is not sleep")
  }

  // MARK: - Channel 0

  /// Channel 0 by its name (L3): the antenna and the name in the ink while
  /// the agent hears it, the crossed antenna and the name muted while it
  /// does not, the moon with Do not disturb. No "in channel", no violet,
  /// and a name of the user's own stays as written.
  func testReceptionNamesChannelZeroUnderTheAntenna() {
    func reception(
      _ value: OverlayChannelChipStatus.Reception, quiet: Bool = false, name: String = "Shared"
    ) -> OverlayChannelChipHoverCard.Line? {
      line(
        .reception,
        in: OverlayChannelChipStatus(
          presence: .online, reception: value, quiet: quiet, channelZeroName: name))
    }
    XCTAssertEqual(reception(.member)?.text, "Shared")
    XCTAssertEqual(reception(.member)?.tone, .primary, "the ink, not the agent violet")
    XCTAssertEqual(reception(.member)?.symbol, "antenna.radiowaves.left.and.right")
    XCTAssertEqual(reception(.off)?.text, "Shared")
    XCTAssertEqual(reception(.off)?.tone, .muted)
    XCTAssertEqual(reception(.off)?.symbol, "antenna.radiowaves.left.and.right.slash")
    XCTAssertEqual(reception(.member, quiet: true)?.text, "Shared · Do not disturb")
    XCTAssertEqual(reception(.member, quiet: true)?.symbol, "moon.fill")
    XCTAssertEqual(reception(.member, quiet: true)?.tone, .muted)
    XCTAssertEqual(reception(.off, quiet: true)?.text, "Shared · Do not disturb")
    // Off channel 0 under the moon: the crossed antenna and the moon beside
    // it, so the line shows what VoiceOver reads (Founder, 2026-11-06).
    XCTAssertEqual(
      reception(.off, quiet: true)?.symbol, "antenna.radiowaves.left.and.right.slash")
    XCTAssertEqual(reception(.off, quiet: true)?.secondSymbol, "moon.fill")
    XCTAssertNil(reception(.member, quiet: true)?.secondSymbol, "the moon alone while it hears")
    XCTAssertNil(reception(.off)?.secondSymbol)
    XCTAssertEqual(reception(.member, name: "Zespół Ekipa")?.text, "Zespół Ekipa")
    XCTAssertEqual(
      OverlayChannelChipStatus(presence: .online, reception: .member).channelZeroName,
      OverlaySharedChannelName.defaultName, "no stored name: the default")
  }

  /// The antenna's ink has one owner for the agents list, the roster's marks
  /// and the fiszki (L7): the regular ink the card's reception line wears,
  /// never the voice violet.
  func testAntennaWearsTheInkNotTheVoiceViolet() {
    for palette in [OverlayAppearancePalette.light, .dark] {
      XCTAssertEqual(OverlayReceptionMarks.antennaInk(palette), palette.primaryText)
      XCTAssertNotEqual(OverlayReceptionMarks.antennaInk(palette), palette.agentAccent)
    }
    let hears = OverlayChannelChipStatus(presence: .online, reception: .member)
    XCTAssertEqual(line(.reception, in: hears)?.tone, .primary, "the card's same ink")
  }

  /// The roster hands the card the name its own channel 0 chip shows.
  func testRosterCardsCarryChannelZerosDisplayName() throws {
    let owner = try XCTUnwrap(
      OverlayConversationOwner(row: [
        "provider": "codex", "provider_session_id": "s5",
        "lease_id": String(repeating: "5", count: 32), "channel": "5", "name": "Kuba",
      ]))
    let conversation = OverlayConversation(
      id: owner.id, channel: "5", name: "Kuba", owner: owner, messages: [])
    var roster = OverlayChannelStatusView(
      channels: [
        OverlayChannelDelivery(
          channel: "5", agent: "Kuba", deliveryID: nil, stage: nil, isOpen: false)
      ], unavailable: false, palette: .light, animates: false,
      hudStates: [
        "5": OverlayChannelHudProjection(
          open: false, loud: false, autosealDeadline: nil, followerAlive: true)
      ],
      conversations: [conversation], onSelectConversation: { _ in })
    roster.sharedChannelStoredName = "Ekipa"
    let chips = roster.channelMapModel.chips
    XCTAssertEqual(chips.first?.name, "Ekipa")
    XCTAssertEqual(chips.first { $0.digit == "5" }?.status?.channelZeroName, "Ekipa")
  }

  // MARK: - Lines

  func testCardOrdersTitlePresenceReceptionUnreadAndTheChordLast() {
    let status = OverlayChannelChipStatus(
      presence: .online, reception: .member, unread: 2, provider: "codex")
    let card = lines(status)
    XCTAssertEqual(card.map(\.kind), [.title, .presence, .reception, .unread, .shortcut])
    XCTAssertEqual(card[0].text, "5 · Kuba")
    XCTAssertEqual(card[0].trailing, "Codex")
    XCTAssertEqual(card[3].text, "2 unread")
    // New replies in the text tone: not the warning amber, not the voice violet.
    XCTAssertEqual(card[3].tone, .primary)
    XCTAssertEqual(card.last?.text, "Ctrl+5")
    XCTAssertEqual(card.last?.tone, .muted)
    XCTAssertLessThan(
      OverlayChannelChipHoverCard.shortcutSize, OverlayChannelChipHoverCard.lineSize,
      "the chord is the small last line")
  }

  func testNoUnreadLineWithoutRepliesAndNoChordLineWithoutAChord() {
    let quiet = OverlayChannelChipStatus(presence: .online, reception: .member)
    XCTAssertEqual(lines(quiet).map(\.kind), [.title, .presence, .reception, .shortcut])
    XCTAssertEqual(lines(quiet, modifier: .function).last?.text, "Fn+5")
    XCTAssertEqual(lines(quiet, modifier: .unbound).map(\.kind), [.title, .presence, .reception])
    XCTAssertNil(lines(quiet).first?.trailing, "no provider, no trailing name")
  }

  /// The old hint was the button again; the card carries the facts and
  /// keeps the chord only as one of its lines.
  func testHintIsNoLongerTheButtonRepeated() {
    let status = OverlayChannelChipStatus(presence: .online, reception: .member)
    let kuba = chip(status: status)
    let texts = lines(status).map(\.text)
    XCTAssertFalse(texts.contains(kuba.hint(modifier: .control)), "not `5 · Kuba · Ctrl+5`")
    XCTAssertGreaterThanOrEqual(texts.count, 3)
    XCTAssertEqual(texts.filter { $0.contains("Ctrl+5") }, ["Ctrl+5"])
  }

  // MARK: - The full state (L16)

  /// Since when (L16): a stalled reader's minutes, a gone agent's last-heard
  /// time; an online agent's line stays short.
  func testPresenceSaysSinceWhen() throws {
    var stalled = OverlayChannelChipStatus(presence: .online, stalled: true, reception: .member)
    stalled.stalledMinutes = 12
    XCTAssertEqual(line(.presence, in: stalled)?.text, "Online · Not reading for 12 min")
    let heard = try XCTUnwrap(MessageTimestamp.date(fromISO8601: "2026-11-06T19:36:00Z"))
    var gone = OverlayChannelChipStatus(presence: .inactive, reception: .member)
    gone.lastHeard = heard
    XCTAssertEqual(
      line(.presence, in: gone)?.text,
      "Not active · last heard " + OverlayChannelChipHoverCard.clock(heard))
    var online = OverlayChannelChipStatus(presence: .online, reception: .member)
    online.lastHeard = heard
    XCTAssertEqual(line(.presence, in: online)?.text, "Online")
  }

  /// The newest message, one line with its direction, time and fate; the
  /// microphone and the muted voice, one line when either is so; at most
  /// seven lines in the order title, presence, reception, message, audio,
  /// unread, chord.
  func testCardCarriesTheNewestMessageAndTheAudioState() throws {
    let at = try XCTUnwrap(MessageTimestamp.date(fromISO8601: "2026-11-06T19:36:00Z"))
    var status = OverlayChannelChipStatus(
      presence: .online, reception: .member, unread: 2, provider: "codex")
    status.lastMessage = .init(outgoing: true, at: at, fate: "Read")
    status.microphoneOpen = true
    status.voiceMuted = true
    status.project = "Codescribe"
    let card = lines(status)
    XCTAssertEqual(
      card.map(\.kind),
      [.title, .presence, .reception, .lastMessage, .audio, .unread, .shortcut])
    XCTAssertEqual(card[0].trailing, "Codex · Codescribe", "the provider and the project")
    let message = try XCTUnwrap(card.first { $0.kind == .lastMessage })
    XCTAssertEqual(message.text, OverlayChannelChipHoverCard.clock(at) + " · Read")
    XCTAssertEqual(message.symbol, "arrow.up.right")
    XCTAssertEqual(card.first { $0.kind == .audio }?.text, "Microphone open · Voice muted")

    status.lastMessage = .init(outgoing: false, at: at, fate: nil)
    status.microphoneOpen = false
    XCTAssertEqual(line(.lastMessage, in: status)?.symbol, "arrow.down.left")
    XCTAssertEqual(line(.lastMessage, in: status)?.text, OverlayChannelChipHoverCard.clock(at))
    XCTAssertEqual(line(.audio, in: status)?.symbol, "speaker.slash.fill")
    status.voiceMuted = false
    XCTAssertNil(line(.audio, in: status), "no audio line when nothing is so")
    XCTAssertTrue(
      OverlayChannelChipHoverCard.spokenStatus(status).contains(
        "Last reply: " + OverlayChannelChipHoverCard.clock(at)))
  }

  /// The roster fills the facts from its own projections: the newest
  /// reply's time and its spoken playback, the HUD's open microphone and
  /// the speaker's mute.
  func testRosterFillsTheFullStateFromItsProjections() throws {
    let owner = try XCTUnwrap(
      OverlayConversationOwner(row: [
        "provider": "codex", "provider_session_id": "s5",
        "lease_id": String(repeating: "5", count: 32), "channel": "5", "name": "Kuba",
      ]))
    let reply = OverlayConversationMessage(
      id: "reply:1", kind: .reply, text: "Gotowe.", order: 1,
      emittedAt: "2026-11-06T19:36:00Z", owner: owner, recipients: [], deliveryID: nil,
      replyTo: nil, unsolicited: true,
      playback: OverlayReplyPlayback(
        replyID: "1", ticket: "t-1", state: "spoken", reason: nil, spoken: true,
        emittedAt: "2026-11-06T19:36:01Z"),
      busPath: "/dev/null")
    let conversation = OverlayConversation(
      id: owner.id, channel: "5", name: "Kuba", owner: owner, messages: [reply])
    func roster(alive: Bool, open: Bool, muted: Bool) -> OverlayChannelStatusView {
      var view = OverlayChannelStatusView(
        channels: [
          OverlayChannelDelivery(
            channel: "5", agent: "Kuba", deliveryID: nil, stage: nil, isOpen: open)
        ], unavailable: false, palette: .light, animates: false,
        hudStates: [
          "5": OverlayChannelHudProjection(
            open: open, loud: false, autosealDeadline: nil, followerAlive: alive,
            provider: "codex", providerSessionID: "s5")
        ],
        conversations: [conversation], onSelectConversation: { _ in })
      view.mutedChannels = ["5": muted]
      view.agentProjects = ["5": "Codescribe"]
      return view
    }
    let heard = try XCTUnwrap(MessageTimestamp.date(fromISO8601: "2026-11-06T19:36:00Z"))
    let gone = try XCTUnwrap(
      roster(alive: false, open: false, muted: true).channelMapModel.chips
        .first { $0.digit == "5" }?.status)
    XCTAssertEqual(gone.lastHeard, heard)
    XCTAssertEqual(gone.lastMessage, .init(outgoing: false, at: heard, fate: "Spoken"))
    XCTAssertTrue(gone.voiceMuted)
    XCTAssertEqual(gone.project, "Codescribe")
    XCTAssertFalse(gone.microphoneOpen)
    let live = try XCTUnwrap(
      roster(alive: true, open: true, muted: false).channelMapModel.chips
        .first { $0.digit == "5" }?.status)
    XCTAssertTrue(live.microphoneOpen)
    XCTAssertFalse(live.voiceMuted)
  }

  /// The card's text reads on the bar's sheet in both looks: its title
  /// and its facts in the ink at AA (4.5:1); the muted lines (channel 0 not
  /// heard, Do not disturb), whose tone is the state (L3, 2026-11-06), at
  /// least the 3:1 of the secondary rung.
  func testCardTextClearsItsFloorOnTheSheet() {
    for palette in [OverlayAppearancePalette.light, .dark] {
      for (tone, floor) in [
        (OverlayChannelChipHoverCard.Tone.primary, 4.5), (.success, 4.5), (.muted, 3.0),
      ] {
        let ink =
          tone == .muted ? palette.mutedText : palette.primaryText
        XCTAssertEqual(tone.color(in: palette), ink.color, "\(tone)")
        let ratio = OverlayColorToken.contrastRatio(
          foreground: ink, surface: palette.desktopBackground,
          background: palette.desktopBackground)
        XCTAssertGreaterThanOrEqual(ratio, floor, "\(palette.appearance) \(tone): \(ratio):1")
      }
    }
  }

  // MARK: - VoiceOver

  func testCardIsOneAccessibilityLabelReadingEveryFact() {
    let status = OverlayChannelChipStatus(
      presence: .online, reception: .member, unread: 2, provider: "codex")
    let kuba = chip(status: status)
    XCTAssertEqual(
      OverlayChannelChipHoverCard.accessibilityText(chip: kuba, status: status, modifier: .control),
      "Channel 5, Kuba, shortcut Ctrl 5. Codex. Online. Hears: Shared (channel 0). 2 unread")
    let inactive = OverlayChannelChipStatus(presence: .inactive, reception: .off)
    XCTAssertEqual(
      OverlayChannelChipHoverCard.spokenStatus(inactive),
      ["Not active", "Does not hear: Shared (channel 0)"])
    let unknown = OverlayChannelChipStatus(presence: .unknown, reception: .off, quiet: true)
    XCTAssertEqual(
      OverlayChannelChipHoverCard.spokenStatus(unknown),
      ["No data", "Does not hear: Shared (channel 0) · Do not disturb"])
  }

  // MARK: - Size

  func testSizeIsMeasuredFromTheLinesWithinTheCardBounds() {
    let short = OverlayChannelChipStatus(presence: .online, reception: .member)
    let full = OverlayChannelChipStatus(
      presence: .online, stalled: true, reception: .off, quiet: true, unread: 12,
      provider: "claude")
    let small = OverlayChannelChipHoverCard.size(
      chip: chip(status: short), status: short, modifier: .control)
    let large = OverlayChannelChipHoverCard.size(
      chip: chip("7", "Maksymilian", status: full), status: full, modifier: .control)
    for size in [small, large] {
      XCTAssertGreaterThanOrEqual(size.width, OverlayChannelChipHoverCard.minWidth)
      XCTAssertLessThanOrEqual(size.width, OverlayChannelChipHoverCard.maxWidth)
    }
    XCTAssertGreaterThan(large.height, small.height, "the unread line adds its height")
    XCTAssertGreaterThanOrEqual(large.width, small.width)
    let fiveLines =
      5 * OverlayChannelChipHoverCard.lineHeight(size: OverlayChannelChipHoverCard.shortcutSize)
      + 2 * OverlayChannelChipHoverCard.padding
    XCTAssertGreaterThan(large.height, fiveLines, "every line has room")
  }

  // MARK: - Nothing is cut (Monika, 2026-11-07)

  /// The two cards of Monika's screenshots in their Polish length: Tola,
  /// not active with the time she was last heard, and Bruno with a long
  /// runtime label beside his name. Fixture strings, not product copy.
  static let tolaLines: [OverlayChannelChipHoverCard.Line] = [
    .init(kind: .title, text: "4 · Tola", tone: .primary, trailing: "Agy · Codescribe"),
    .init(
      kind: .presence, text: "Nieaktywny · ostatnio słyszany 8:51 PM", tone: .muted,
      dot: .sleeping),
    .init(
      kind: .reception, text: "All", tone: .primary,
      symbol: "antenna.radiowaves.left.and.right"),
    .init(kind: .lastMessage, text: "8:51 PM", tone: .primary, symbol: "arrow.down.left"),
    .init(kind: .shortcut, text: "Ctrl+4", tone: .muted, symbol: "keyboard"),
  ]

  static let brunoLines: [OverlayChannelChipHoverCard.Line] = [
    .init(
      kind: .title, text: "5 · Bruno", tone: .primary,
      trailing: "Claude Code · gpt-6.1-sol · Codescribe"),
    .init(kind: .presence, text: "Aktywny", tone: .success, dot: .present),
    .init(
      kind: .reception, text: "All", tone: .primary,
      symbol: "antenna.radiowaves.left.and.right"),
    .init(
      kind: .lastMessage, text: "8:51 PM · Przeczytane", tone: .primary,
      symbol: "arrow.up.right"),
    .init(kind: .shortcut, text: "Ctrl+5", tone: .muted, symbol: "keyboard"),
  ]

  /// The card's width the requirement allows at text scale 1.
  private let ceiling: CGFloat = 320

  private func card(_ lines: [OverlayChannelChipHoverCard.Line]) -> OverlayChannelChipHoverCard {
    OverlayChannelChipHoverCard(
      digit: "4", lines: lines, accessibilityText: "", palette: .light)
  }

  /// What SwiftUI itself needs to draw every line whole: the card's ideal
  /// width with nothing wrapped, and its height wrapped at `width`.
  private func swiftUINeeds(
    _ lines: [OverlayChannelChipHoverCard.Line], width: CGFloat
  ) -> (unwrapped: CGFloat, height: CGFloat) {
    let unwrapped = NSHostingView(rootView: card(lines).fixedSize()).fittingSize.width
    let wrapped = NSHostingView(
      rootView: card(lines).frame(width: width).fixedSize(horizontal: false, vertical: true)
    ).fittingSize.height
    return (unwrapped, wrapped)
  }

  /// The card is as wide as its widest line drawn whole, up to the ceiling,
  /// and as tall as its lines wrapped at that width: the hint panel draws
  /// it at exactly this size, so a narrower or shorter card cuts a line.
  private func assertNothingCut(
    _ lines: [OverlayChannelChipHoverCard.Line], _ name: String,
    file: StaticString = #filePath, line: UInt = #line
  ) {
    let size = OverlayChannelChipHoverCard.size(lines: lines)
    let needs = swiftUINeeds(lines, width: size.width)
    if needs.unwrapped <= ceiling {
      XCTAssertGreaterThanOrEqual(
        size.width, needs.unwrapped,
        "\(name): every line fits whole on one line, so the card is that wide", file: file,
        line: line)
    } else {
      XCTAssertEqual(
        size.width, ceiling, "\(name): a line over the ceiling grows the card to it", file: file,
        line: line)
    }
    XCTAssertLessThanOrEqual(
      size.width, ceiling, "\(name): within the ceiling", file: file, line: line)
    XCTAssertGreaterThanOrEqual(
      size.height, needs.height, "\(name): room for every wrapped line", file: file, line: line)
  }

  func testTheInactivePresenceLineIsNeverCut() {
    assertNothingCut(Self.tolaLines, "Tola")
  }

  func testALongRuntimeLabelStaysWholeBesideTheName() {
    assertNothingCut(Self.brunoLines, "Bruno")
  }

  /// A line longer than the ceiling wraps to a second line instead of an
  /// ellipsis, and a runtime label that cannot stand beside the name goes
  /// under it, whole.
  func testLinesOverTheCeilingWrapInsteadOfBeingCut() {
    var lines = Self.tolaLines
    lines[0] = .init(
      kind: .title, text: "4 · Tola", tone: .primary,
      trailing: "Claude Code · gpt-6.1-sol · Codescribe-wt · bar-polish-clean-20261106")
    lines[1] = .init(
      kind: .presence,
      text: "Nieaktywny · ostatnio słyszany 8:51 PM · nie czyta od 125 min · głos wyciszony",
      tone: .muted, dot: .sleeping)
    assertNothingCut(lines, "over the ceiling")
    let oneLine = OverlayChannelChipHoverCard.size(lines: Self.tolaLines)
    let wrapped = OverlayChannelChipHoverCard.size(lines: lines)
    XCTAssertGreaterThanOrEqual(
      wrapped.height,
      oneLine.height + 2
        * OverlayChannelChipHoverCard.lineHeight(size: OverlayChannelChipHoverCard.lineSize),
      "the presence line wraps to two lines and the label stands under the name")
  }
}
