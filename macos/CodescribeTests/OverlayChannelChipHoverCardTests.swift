import SwiftUI
import XCTest

@testable import Codescribe

/// Falsifiers for the bar chip's hover card, variant D of the mockup (Monika,
/// 2026-11-07): a header (presence dot, `4 · Tola`, when the agent was last
/// there, the antenna, the moon and the unread badge), the runtime label,
/// your last message with its receipt and the agent's last reply with one
/// line of its preview, and nothing else: no presence row, no channel 0 row,
/// no chord.
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

  private let now = MessageTimestamp.date(fromISO8601: "2026-11-07T08:10:00Z")!

  private func lines(_ status: OverlayChannelChipStatus) -> [OverlayChannelChipHoverCard.Line] {
    OverlayChannelChipHoverCard.lines(chip: chip(status: status), status: status, now: now)
  }

  private func line(
    _ kind: OverlayChannelChipHoverCard.Line.Kind, in status: OverlayChannelChipStatus
  ) -> OverlayChannelChipHoverCard.Line? {
    lines(status).first { $0.kind == kind }
  }

  private func header(_ status: OverlayChannelChipStatus) -> OverlayChannelChipHoverCard.Line? {
    line(.header, in: status)
  }

  // MARK: - Header

  /// The header wears the agents list's own dot (L2): olive online,
  /// violet and blinking while the reply plays, a muted ring for a gone
  /// follower, "?" without data, the stall's olive "!" (L6: not sleep).
  func testHeaderWearsTheRosterPresenceDot() {
    func dot(_ status: OverlayChannelChipStatus) -> OverlayAgentDotKind? { header(status)?.dot }
    XCTAssertEqual(dot(.init(presence: .online, reception: .member)), .present)
    XCTAssertEqual(dot(.init(presence: .online, reception: .member, speaking: true)), .speaking)
    XCTAssertEqual(dot(.init(presence: .inactive, reception: .member)), .sleeping)
    XCTAssertEqual(
      dot(.init(presence: .inactive, reception: .member, speaking: true)), .sleeping,
      "a gone follower is not speaking")
    XCTAssertEqual(dot(.init(presence: .unknown, reception: .member)), .idle)
    XCTAssertEqual(
      dot(.init(presence: .online, stalled: true, reception: .member)), .stalled,
      "a stall is the olive dot with its \"!\"")
    XCTAssertEqual(
      dot(.init(presence: .online, stalled: true, reception: .member, speaking: true)), .speaking,
      "a reply playing aloud wins over the stall, as in the agents list")
  }

  /// `5 · Kuba`, then when the agent was last there: now while its follower
  /// is alive, else the time since its lease's last heartbeat; nothing
  /// without a heartbeat.
  func testHeaderSaysWhenTheAgentWasLastThere() {
    let online = OverlayChannelChipStatus(presence: .online, reception: .member)
    XCTAssertEqual(header(online)?.text, "5 · Kuba")
    XCTAssertEqual(header(online)?.detail, MessageTimestamp.ago(now, now: now))
    var gone = OverlayChannelChipStatus(presence: .inactive, reception: .member)
    XCTAssertNil(header(gone)?.detail, "no heartbeat, no time")
    let heartbeat = now.addingTimeInterval(-5 * 60)
    gone.lastHeartbeat = heartbeat
    XCTAssertEqual(header(gone)?.detail, MessageTimestamp.ago(heartbeat, now: now))
    XCTAssertEqual(
      OverlayChannelChipHoverCard.activityText(gone, now: now),
      MessageTimestamp.ago(heartbeat, now: now))
    let unknown = OverlayChannelChipStatus(presence: .unknown, reception: .member)
    XCTAssertNil(header(unknown)?.detail, "no liveness claim, no time")
  }

  /// The antenna and the moon of the list under ✦ (`AgentBroadcastPolicy`'s
  /// own symbols), the moon only under Do not disturb, and the unread count.
  func testHeaderCarriesTheListsMarksAndTheUnreadCount() throws {
    func marks(
      _ reception: OverlayChannelChipStatus.Reception, quiet: Bool = false, unread: Int = 0
    ) throws -> OverlayChannelChipHoverCard.Marks {
      try XCTUnwrap(
        header(.init(presence: .online, reception: reception, quiet: quiet, unread: unread))?
          .marks)
    }
    XCTAssertEqual(try marks(.member).antenna, AgentBroadcastPolicy.on.statusSymbol)
    XCTAssertTrue(try marks(.member).hears)
    XCTAssertNil(try marks(.member).moon, "no moon without Do not disturb")
    XCTAssertEqual(try marks(.off).antenna, AgentBroadcastPolicy.off.statusSymbol)
    XCTAssertEqual(try marks(.off).antenna, "antenna.radiowaves.left.and.right.slash")
    XCTAssertFalse(try marks(.off).hears)
    XCTAssertEqual(try marks(.member, quiet: true).moon, AgentBroadcastPolicy.quiet.quietSymbol)
    XCTAssertEqual(try marks(.off, quiet: true).moon, "moon.fill")
    XCTAssertEqual(try marks(.member).unread, 0)
    XCTAssertEqual(try marks(.member, unread: 3).unread, 3)
  }

  /// The antenna's ink has one owner for the agents list, the roster's marks
  /// and the fiszki (L7): the regular ink, never the voice violet.
  func testAntennaWearsTheInkNotTheVoiceViolet() {
    for palette in [OverlayAppearancePalette.light, .dark] {
      XCTAssertEqual(OverlayReceptionMarks.antennaInk(palette), palette.primaryText)
      XCTAssertNotEqual(OverlayReceptionMarks.antennaInk(palette), palette.agentAccent)
    }
  }

  // MARK: - Lines

  /// Four kinds of line at most, in this order, and none of the rows the
  /// card had before: presence, channel 0, the chord.
  func testCardIsTheHeaderRuntimeSentAndReplyAndNothingElse() {
    var status = OverlayChannelChipStatus(
      presence: .online, reception: .member, unread: 2, provider: "codex")
    status.project = "Codescribe"
    status.lastSent = .init(at: now.addingTimeInterval(-600), status: "Read")
    status.lastReply = .init(at: now.addingTimeInterval(-300), preview: "Kuba → You: Done.")
    let card = lines(status)
    XCTAssertEqual(card.map(\.kind), [.header, .runtime, .sent, .reply])
    XCTAssertEqual(card[1].text, "Codex · Codescribe", "the provider and the project")
    XCTAssertEqual(card[1].tone, .muted)
    for text in card.map(\.text) + card.compactMap(\.detail) {
      XCTAssertFalse(text.contains("Ctrl+5"), "the chord left the card: \(text)")
      XCTAssertFalse(text.contains("Online"), "no presence row: \(text)")
      XCTAssertFalse(text.contains(status.channelZeroName), "no channel 0 row: \(text)")
    }
    let bare = OverlayChannelChipStatus(presence: .online, reception: .member)
    XCTAssertEqual(lines(bare).map(\.kind), [.header], "no runtime, no messages: the header")
  }

  /// Your last message: its time and its receipt, no text; no line
  /// without one.
  func testSentLineIsTheTimeAndTheReceipt() {
    var status = OverlayChannelChipStatus(presence: .online, reception: .member)
    let at = now.addingTimeInterval(-8 * 60)
    status.lastSent = .init(at: at, status: "Delivered")
    let sent = line(.sent, in: status)
    XCTAssertEqual(sent?.text, OverlayChannelChipHoverCard.time(at, now: now) + " · Delivered")
    XCTAssertEqual(sent?.symbol, "arrow.up.right")
    XCTAssertNil(sent?.detail, "no snippet on your line")
    status.lastSent = .init(at: at, status: nil)
    XCTAssertEqual(line(.sent, in: status)?.text, OverlayChannelChipHoverCard.time(at, now: now))
    status.lastSent = nil
    XCTAssertNil(line(.sent, in: status))
  }

  /// The agent's last reply: its time, then its preview as the roster row
  /// gives it; no line without one.
  func testReplyLineIsTheTimeAndThePreview() {
    var status = OverlayChannelChipStatus(presence: .online, reception: .member)
    let at = now.addingTimeInterval(-19 * 60)
    status.lastReply = .init(at: at, preview: "Kuba → You: Header cut checked.")
    let reply = line(.reply, in: status)
    XCTAssertEqual(reply?.text, OverlayChannelChipHoverCard.time(at, now: now))
    XCTAssertEqual(reply?.detail, "Kuba → You: Header cut checked.")
    XCTAssertEqual(reply?.symbol, "arrow.down.left")
    status.lastReply = nil
    XCTAssertNil(line(.reply, in: status))
  }

  // MARK: - The roster's facts

  private func owner() throws -> OverlayConversationOwner {
    try XCTUnwrap(
      OverlayConversationOwner(row: [
        "provider": "codex", "provider_session_id": "s5",
        "lease_id": String(repeating: "5", count: 32), "channel": "5", "name": "Kuba",
      ]))
  }

  private func roster(
    messages: [OverlayConversationMessage], alive: Bool?, heartbeat: Date? = nil
  ) throws -> OverlayChannelStatusView {
    let owner = try owner()
    let conversation = OverlayConversation(
      id: owner.id, channel: "5", name: "Kuba", owner: owner, messages: messages)
    var view = OverlayChannelStatusView(
      channels: [
        OverlayChannelDelivery(
          channel: "5", agent: "Kuba", deliveryID: nil, stage: nil, isOpen: false)
      ], unavailable: false, palette: .light, animates: false,
      hudStates: [
        "5": OverlayChannelHudProjection(
          open: false, loud: false, autosealDeadline: nil, followerAlive: alive,
          provider: "codex", providerSessionID: "s5", lastHeartbeat: heartbeat)
      ],
      conversations: [conversation], onSelectConversation: { _ in })
    view.agentProjects = ["5": "Codescribe"]
    return view
  }

  private func status(of view: OverlayChannelStatusView) throws -> OverlayChannelChipStatus {
    try XCTUnwrap(view.channelMapModel.chips.first { $0.digit == "5" }?.status)
  }

  private func reply(at: String, text: String = "Gotowe.") throws -> OverlayConversationMessage {
    OverlayConversationMessage(
      id: "reply:\(at)", kind: .reply, text: text, order: 2, emittedAt: at, owner: try owner(),
      recipients: [], deliveryID: nil, replyTo: nil, unsolicited: true, playback: nil,
      busPath: "/dev/null")
  }

  /// Your message to Kuba, delivered to his mailbox and not read yet.
  private func delivered(at: String) throws -> OverlayConversationMessage {
    OverlayConversationMessage(
      id: "typed:0123456789abcdef00000201", kind: .user, text: "Sprawdź belkę.", order: 3,
      emittedAt: at, owner: nil,
      recipients: [
        OverlayConversationRecipient(
          owner: try owner(), deliveryID: "d-kuba-1", queued: true, accepted: true,
          acknowledged: false)
      ], deliveryID: nil, replyTo: nil, unsolicited: false, playback: nil, busPath: "",
      sourceChannel: "5",
      receipt: OverlayMessageReceipt(
        messageID: "0123456789abcdef00000201",
        deliveries: [
          .init(
            deliveryID: "d-kuba-1", name: "kuba", channel: "5", state: .delivered, wake: "on",
            deliveredAt: at, at: at)
        ]))
  }

  /// A gone agent's header says when its lease last heartbeat, not when it
  /// last wrote: the reply is two hours old, the heartbeat five minutes.
  func testInactiveHeaderShowsTheLeaseHeartbeatNotTheLastMessage() throws {
    let at = Date()
    let replyAt = at.addingTimeInterval(-2 * 3_600)
    let heartbeat = at.addingTimeInterval(-5 * 60)
    let view = try roster(
      messages: [try reply(at: replyAt.ISO8601Format())], alive: false, heartbeat: heartbeat)
    let gone = try status(of: view)
    XCTAssertEqual(gone.lastHeartbeat, heartbeat, "the roster's lease heartbeat")
    let card = OverlayChannelChipHoverCard.lines(chip: chip(status: gone), status: gone, now: at)
    XCTAssertEqual(card.first?.detail, MessageTimestamp.ago(heartbeat, now: at))
    XCTAssertNotEqual(
      card.first?.detail, MessageTimestamp.ago(replyAt, now: at), "never the reply's time")
    let live = try status(of: try roster(messages: [], alive: true, heartbeat: nil))
    XCTAssertNil(live.lastHeartbeat, "a live follower is now, not a time")
    XCTAssertEqual(
      OverlayChannelChipHoverCard.lines(chip: chip(status: live), status: live, now: at)
        .first?.detail,
      MessageTimestamp.ago(at, now: at))
  }

  /// Your line reads the same receipt as the roster row for that message
  /// (`deliveryReceiptDisplay`), the one owner of the reading: delivered
  /// and not read is the row's "Delivered", word for word.
  func testSentStatusIsTheRosterReceiptReadingForThatMessage() throws {
    let sentAt = "2026-11-07T08:02:00Z"
    let view = try roster(
      messages: [try reply(at: "2026-11-07T07:51:00Z"), try delivered(at: sentAt)], alive: true)
    let conversation = try XCTUnwrap(view.currentConversations.first { $0.channel == "5" })
    let channel = try XCTUnwrap(view.channels.first)
    let receipt = try XCTUnwrap(
      view.deliveryReceiptDisplay(for: channel, conversation: conversation))
    let facts = try status(of: view)
    XCTAssertEqual(facts.lastSent?.status, receipt.text, "the row's own words")
    XCTAssertEqual(facts.lastSent?.status, String(localized: "Delivered"))
    XCTAssertEqual(facts.lastSent?.at, MessageTimestamp.date(fromISO8601: sentAt))
    let sent = OverlayChannelChipHoverCard.lines(chip: chip(status: facts), status: facts)
      .first { $0.kind == .sent }
    XCTAssertEqual(sent?.text.hasSuffix(" · " + receipt.text), true)
  }

  /// The reply's preview is the roster row's (`drawerPreview`), and the
  /// reply line comes from the agent's last reply, not your newer message.
  func testReplyComesFromTheLastReplyWithTheRowsPreview() throws {
    let answer = try reply(at: "2026-11-07T07:51:00Z", text: "Wycinek sprawdzony.\nBelka trzyma.")
    let view = try roster(
      messages: [answer, try delivered(at: "2026-11-07T08:02:00Z")], alive: true)
    let facts = try status(of: view)
    XCTAssertEqual(
      facts.lastReply?.preview, OverlayChannelStatusView.drawerPreview(message: answer))
    XCTAssertEqual(facts.lastReply?.at, MessageTimestamp.date(fromISO8601: answer.emittedAt))
    XCTAssertEqual(facts.project, "Codescribe")
  }

  /// The roster hands the card the name its own channel 0 chip shows.
  func testRosterCardsCarryChannelZerosDisplayName() throws {
    var view = try roster(messages: [], alive: true)
    view.sharedChannelStoredName = "Ekipa"
    let chips = view.channelMapModel.chips
    XCTAssertEqual(chips.first?.name, "Ekipa")
    XCTAssertEqual(chips.first { $0.digit == "5" }?.status?.channelZeroName, "Ekipa")
  }

  /// The card's text reads on the bar's sheet in both looks: the name and
  /// the messages in the ink at AA (4.5:1); the muted activity and runtime
  /// at least the 3:1 of the secondary rung.
  func testCardTextClearsItsFloorOnTheSheet() {
    for palette in [OverlayAppearancePalette.light, .dark] {
      for (tone, floor) in [(OverlayChannelChipHoverCard.Tone.primary, 4.5), (.muted, 3.0)] {
        let ink = tone == .muted ? palette.mutedText : palette.primaryText
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
    var status = OverlayChannelChipStatus(
      presence: .online, reception: .member, unread: 2, provider: "codex")
    let at = now.addingTimeInterval(-60)
    status.lastReply = .init(at: at, preview: "Kuba → You: Done.")
    let kuba = chip(status: status)
    let time = OverlayChannelChipHoverCard.time(at, now: now)
    XCTAssertEqual(
      OverlayChannelChipHoverCard.accessibilityText(
        chip: kuba, status: status, modifier: .control, now: now),
      "Channel 5, Kuba, shortcut Ctrl 5. Codex. Online. Hears: Shared (channel 0). "
        + "Last reply: \(time) · Kuba → You: Done.. 2 unread")
    XCTAssertEqual(
      OverlayChannelChipHoverCard.spokenStatus(status, now: now),
      ["Codex", "Online", "Hears: Shared (channel 0)", "Last reply: \(time)", "2 unread"],
      "the rows that show the preview themselves hear no preview")
    var inactive = OverlayChannelChipStatus(presence: .inactive, reception: .off)
    let heartbeat = now.addingTimeInterval(-5 * 60)
    inactive.lastHeartbeat = heartbeat
    XCTAssertEqual(
      OverlayChannelChipHoverCard.spokenStatus(inactive, now: now),
      [
        "Not active · " + MessageTimestamp.ago(heartbeat, now: now),
        "Does not hear: Shared (channel 0)",
      ])
    let unknown = OverlayChannelChipStatus(presence: .unknown, reception: .off, quiet: true)
    XCTAssertEqual(
      OverlayChannelChipHoverCard.spokenStatus(unknown, now: now),
      ["No data", "Does not hear: Shared (channel 0) · Do not disturb"])
    let stalled = OverlayChannelChipStatus(presence: .online, stalled: true, reception: .member)
    XCTAssertEqual(
      OverlayChannelChipHoverCard.spokenStatus(stalled, now: now).first, "Online · Not reading")
  }

  // MARK: - Nothing is cut (Monika, 2026-11-07)

  /// Variant D's three cards in their Polish length (fixture strings, not
  /// product copy): Tola gone five minutes under the moon, Bruno online
  /// with an unread reply, Test-A gone since yesterday with twelve unread.
  static let tolaLines: [OverlayChannelChipHoverCard.Line] = [
    .init(
      kind: .header, text: "4 · Tola", tone: .primary, detail: "5 min temu", dot: .sleeping,
      marks: .init(
        antenna: "antenna.radiowaves.left.and.right", hears: true, moon: "moon.fill", unread: 0)),
    .init(kind: .runtime, text: "Agy · Codescribe", tone: .muted),
    .init(kind: .sent, text: "9:02 · Doręczono", tone: .primary, symbol: "arrow.up.right"),
    .init(
      kind: .reply, text: "8:51", tone: .primary,
      detail: "Tola → Ty: Wycinek headerUI74d sprawdzony: belka trzyma rozmiar w obu motywach",
      symbol: "arrow.down.left"),
  ]

  static let brunoLines: [OverlayChannelChipHoverCard.Line] = [
    .init(
      kind: .header, text: "5 · Bruno", tone: .primary, detail: "teraz", dot: .present,
      marks: .init(
        antenna: "antenna.radiowaves.left.and.right", hears: true, moon: nil, unread: 1)),
    .init(kind: .runtime, text: "Claude Code · gpt-6.1-sol · Codescribe", tone: .muted),
    .init(kind: .sent, text: "8:40 · Przeczytano", tone: .primary, symbol: "arrow.up.right"),
    .init(
      kind: .reply, text: "8:58", tone: .primary, detail: "Bruno → Ty: Gotowe.",
      symbol: "arrow.down.left"),
  ]

  static let testALines: [OverlayChannelChipHoverCard.Line] = [
    .init(
      kind: .header, text: "7 · Test-A", tone: .primary, detail: "wczoraj 23:19", dot: .sleeping,
      marks: .init(
        antenna: "antenna.radiowaves.left.and.right.slash", hears: false, moon: nil, unread: 12)),
    .init(kind: .runtime, text: "Codex · Vibecrafted", tone: .muted),
    .init(
      kind: .sent, text: "Wczoraj 22:47 · W kolejce", tone: .primary, symbol: "arrow.up.right"),
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

  /// The card is as wide as its widest label drawn whole, up to the
  /// ceiling, and as tall as its lines wrapped at that width: the hint
  /// panel draws it at exactly this size, so a narrower or shorter card
  /// cuts a line. The reply's preview is a preview, measured apart.
  private func assertNothingCut(
    _ lines: [OverlayChannelChipHoverCard.Line], _ name: String,
    file: StaticString = #filePath, line: UInt = #line
  ) {
    let size = OverlayChannelChipHoverCard.size(lines: lines)
    let labels = lines.filter { $0.kind != .reply }
    let needs = swiftUINeeds(labels, width: size.width)
    if needs.unwrapped <= ceiling {
      XCTAssertGreaterThanOrEqual(
        size.width, needs.unwrapped,
        "\(name): every label fits whole on one line, so the card is that wide", file: file,
        line: line)
    } else {
      XCTAssertEqual(
        size.width, ceiling, "\(name): a line over the ceiling grows the card to it", file: file,
        line: line)
    }
    XCTAssertLessThanOrEqual(
      size.width, ceiling, "\(name): within the ceiling", file: file, line: line)
    XCTAssertGreaterThanOrEqual(
      size.height, swiftUINeeds(lines, width: size.width).height,
      "\(name): room for every wrapped line", file: file, line: line)
  }

  func testTolasCardIsNeverCut() {
    assertNothingCut(Self.tolaLines, "Tola")
  }

  func testBrunosLongRuntimeLabelStaysWhole() {
    assertNothingCut(Self.brunoLines, "Bruno")
  }

  func testTheUnreadBadgeAndAYesterdayHeaderStayWhole() {
    assertNothingCut(Self.testALines, "Test-A")
  }

  /// Labels wrap without being cut. Reply previews use up to three lines and
  /// never widen the card beyond its ceiling.
  func testLabelsAndReplyPreviewsWrapWithinTheirSeparateBudgets() {
    var lines = Self.tolaLines
    lines[1] = .init(
      kind: .runtime,
      text: "Claude Code · gpt-6.1-sol · Codescribe-wt · bar-polish-clean-20261106 · worktree",
      tone: .muted)
    assertNothingCut(lines, "over the ceiling")
    let oneLine = OverlayChannelChipHoverCard.size(lines: Self.tolaLines)
    let wrapped = OverlayChannelChipHoverCard.size(lines: lines)
    XCTAssertGreaterThanOrEqual(
      wrapped.height,
      oneLine.height
        + OverlayChannelChipHoverCard.lineHeight(size: OverlayChannelChipHoverCard.lineSize),
      "the runtime label wraps to a second line")
    var long = Self.brunoLines
    long[3] = .init(
      kind: .reply, text: "8:58", tone: .primary,
      detail: String(repeating: "Bardzo długa odpowiedź agenta, ", count: 12),
      symbol: "arrow.down.left")
    let size = OverlayChannelChipHoverCard.size(lines: long)
    XCTAssertEqual(size.width, ceiling, "a long preview takes the card to the ceiling, no further")
    let oneLineHeight = OverlayChannelChipHoverCard.oneLine(
      OverlayChannelChipHoverCard.font(long[3]))
    XCTAssertEqual(
      OverlayChannelChipHoverCard.height(long[3], within: size.width - 20),
      CGFloat(OverlayChannelChipHoverCard.replyPreviewLineLimit) * oneLineHeight,
      "the preview has room for three measured lines")
    XCTAssertGreaterThan(
      size.height, OverlayChannelChipHoverCard.size(lines: Self.brunoLines).height,
      "a multiline reply grows the card instead of hiding the rest of its preview")
  }

  func testSizeStaysWithinTheCardBounds() {
    let short = OverlayChannelChipStatus(presence: .online, reception: .member)
    var full = OverlayChannelChipStatus(
      presence: .inactive, reception: .off, quiet: true, unread: 12, provider: "claude")
    full.lastHeartbeat = now.addingTimeInterval(-3 * 86_400)
    full.lastSent = .init(at: now.addingTimeInterval(-3 * 86_400), status: "Queued")
    let small = OverlayChannelChipHoverCard.size(
      chip: chip(status: short), status: short, now: now)
    let large = OverlayChannelChipHoverCard.size(
      chip: chip("7", "Maksymilian", status: full), status: full, now: now)
    for size in [small, large] {
      XCTAssertGreaterThanOrEqual(size.width, OverlayChannelChipHoverCard.minWidth)
      XCTAssertLessThanOrEqual(size.width, OverlayChannelChipHoverCard.maxWidth)
    }
    XCTAssertGreaterThan(large.height, small.height, "the runtime and sent lines add height")
    XCTAssertGreaterThanOrEqual(large.width, small.width)
  }
}
