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
}
