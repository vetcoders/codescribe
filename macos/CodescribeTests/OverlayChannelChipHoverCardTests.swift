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

  func testStallIsFlaggedBesidePresence() {
    let stalled = OverlayChannelChipStatus(presence: .online, stalled: true, reception: .member)
    XCTAssertEqual(line(.presence, in: stalled)?.text, "Online · Not reading")
    // No amber in the bus (Founder, 2026-11-04): a stall reads in the muted tone.
    XCTAssertEqual(line(.presence, in: stalled)?.tone, .muted)
    XCTAssertEqual(line(.presence, in: stalled)?.symbol, "zzz")
  }

  // MARK: - Channel 0

  func testReceptionReadsMemberOffAndTheMoon() {
    func reception(_ value: OverlayChannelChipStatus.Reception, quiet: Bool = false) -> String? {
      line(
        .reception,
        in: OverlayChannelChipStatus(presence: .online, reception: value, quiet: quiet))?.text
    }
    XCTAssertEqual(reception(.member), "In channel 0")
    XCTAssertEqual(reception(.off), "Not in channel 0")
    XCTAssertEqual(reception(.member, quiet: true), "In channel 0 · Do not disturb")
    XCTAssertEqual(reception(.off, quiet: true), "Not in channel 0 · Do not disturb")
    XCTAssertEqual(
      line(.reception, in: OverlayChannelChipStatus(presence: .online, reception: .member))?.tone,
      .agent)
    XCTAssertEqual(
      line(
        .reception,
        in: OverlayChannelChipStatus(presence: .online, reception: .member, quiet: true))?.symbol,
      "moon.fill")
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
      "Channel 5, Kuba, shortcut Ctrl 5. Codex. Online. In channel 0. 2 unread")
    let inactive = OverlayChannelChipStatus(presence: .inactive, reception: .off)
    XCTAssertEqual(
      OverlayChannelChipHoverCard.spokenStatus(inactive), ["Not active", "Not in channel 0"])
    let unknown = OverlayChannelChipStatus(presence: .unknown, reception: .off, quiet: true)
    XCTAssertEqual(
      OverlayChannelChipHoverCard.spokenStatus(unknown),
      ["No data", "Not in channel 0 · Do not disturb"])
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
