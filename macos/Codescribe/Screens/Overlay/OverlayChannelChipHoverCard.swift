import AppKit
import SwiftUI

/// The hover card of an agent's chip on the bar (Founder, 2026-11-04: the old
/// `7 · Nela · Ctrl+7` hint only repeated the button), variant D of the
/// mockup (Monika, 2026-11-07), four lines and nothing else:
///
///     ○ 4 · Tola · 5 min ago                 ((·)) ☾ (2)
///     Agy · Codescribe
///     ↗ 9:02 · Delivered
///     ↙ 8:51  Header cut checked: the bar holds…
///
/// 1. The header: the presence dot of the agents list under ✦
///    (`OverlayAgentStatusDot`), the digit and the name, then muted when
///    the agent was last there: "now" while its follower is alive, else the
///    time since its lease's last heartbeat (`MessageTimestamp.ago`). After
///    the name the antenna of the list (crossed off channel 0), the moon
///    under Do not disturb and the roster's unread badge.
/// 2. The runtime: provider, model and project, muted, whole.
/// 3. Your last message: its time and the roster's receipt reading for it
///    (`deliveryReceiptDisplay`), no text. No line without one.
/// 4. The agent's last reply: its time and one line of the roster's
///    preview, cut with an ellipsis (`OverlayChipHoverCardPreviewLine`).
///    No line without one.
///
/// The chord is not on the card (Monika, 2026-11-07, decision 4): it stays
/// in the pill's context menu. Values only: no state, no polling, no action.
/// The card is one accessibility element whose label reads every line.
struct OverlayChannelChipHoverCard: View {
  enum Tone: Equatable {
    case primary
    case muted

    func color(in palette: OverlayAppearancePalette) -> Color {
      switch self {
      case .primary: palette.primaryText.color
      case .muted: palette.mutedText.color
      }
    }
  }

  /// The header's glyphs after the name, as the list under ✦ draws them.
  struct Marks: Equatable {
    /// `AgentBroadcastPolicy.statusSymbol`: the antenna, crossed off channel 0.
    let antenna: String
    /// The agent hears channel 0: the antenna in the ink, else muted.
    let hears: Bool
    /// `AgentBroadcastPolicy.quietSymbol` under Do not disturb, else none.
    let moon: String?
    /// The unread replies, drawn as the roster's badge when there are any.
    let unread: Int
  }

  /// One line of the card as plain values, so tests read the card's text.
  struct Line: Equatable {
    enum Kind: Equatable {
      case header
      case runtime
      case sent
      case reply
    }

    let kind: Kind
    /// The header's `4 · Tola`, the runtime label, the sent line's
    /// `9:02 · Delivered` or the reply's time.
    let text: String
    let tone: Tone
    /// The header's activity (`now`, `5 min ago`), muted after the name;
    /// the reply's preview.
    var detail: String? = nil
    /// The arrow before your message and the agent's reply.
    var symbol: String? = nil
    /// The presence dot before the header's name.
    var dot: OverlayAgentDotKind? = nil
    var marks: Marks? = nil

    var size: CGFloat {
      kind == .header ? OverlayChannelChipHoverCard.titleSize : OverlayChannelChipHoverCard.lineSize
    }
  }

  let digit: String
  let lines: [Line]
  let accessibilityText: String
  let palette: OverlayAppearancePalette

  init(
    chip: OverlayChannelMapChip, status: OverlayChannelChipStatus,
    modifier: OverlayChannelChordModifier, palette: OverlayAppearancePalette, now: Date = Date()
  ) {
    self.init(
      digit: chip.digit, lines: Self.lines(chip: chip, status: status, now: now),
      accessibilityText: Self.accessibilityText(
        chip: chip, status: status, modifier: modifier, now: now),
      palette: palette)
  }

  /// The card from lines already worded, as the tests give the longer
  /// Polish lines.
  init(
    digit: String, lines: [Line], accessibilityText: String, palette: OverlayAppearancePalette
  ) {
    self.digit = digit
    self.lines = lines
    self.accessibilityText = accessibilityText
    self.palette = palette
  }

  static let padding: CGFloat = 10
  static let lineSpacing: CGFloat = 4
  static let replyPreviewLineLimit = 3
  static let symbolWidth: CGFloat = 14
  static let symbolGap: CGFloat = 5
  static let trailingGap: CGFloat = 8
  static let markGap: CGFloat = 2
  /// The roster's unread badge at this scale: 16 pt beside the 12 pt name.
  static let badgeScale: CGFloat = 0.8
  nonisolated static let titleSize: CGFloat = 12
  nonisolated static let lineSize: CGFloat = 11
  /// A tight card: wide enough for its longest line, within these bounds.
  /// No label is ever cut (Monika, 2026-11-07): a line wider than the
  /// ceiling wraps. Only the reply's preview ends in an ellipsis.
  static let minWidth: CGFloat = 200
  static let maxWidth: CGFloat = 320

  var body: some View {
    VStack(alignment: .leading, spacing: Self.lineSpacing) {
      ForEach(Array(lines.enumerated()), id: \.offset) { _, line in
        row(line)
      }
    }
    .padding(Self.padding)
    .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
    .background(palette.desktopBackground.color.opacity(0.9))
    .accessibilityElement(children: .ignore)
    .accessibilityLabel(Text(verbatim: accessibilityText))
    .accessibilityIdentifier("overlay-channel-chip-card-\(digit)")
  }

  @ViewBuilder
  private func row(_ line: Line) -> some View {
    switch line.kind {
    case .header: header(line)
    case .runtime:
      Text(verbatim: line.text)
        .font(.system(size: line.size))
        .monospacedDigit()
        .foregroundStyle(line.tone.color(in: palette))
        .lineLimit(nil)
        .fixedSize(horizontal: false, vertical: true)
    case .sent:
      HStack(alignment: .firstTextBaseline, spacing: Self.symbolGap) {
        if let symbol = line.symbol {
          Image(systemName: symbol)
            .font(.system(size: line.size))
            .frame(width: Self.symbolWidth)
        }
        Text(verbatim: line.text)
          .font(.system(size: line.size))
          .monospacedDigit()
          .lineLimit(nil)
          .fixedSize(horizontal: false, vertical: true)
      }
      .foregroundStyle(line.tone.color(in: palette))
    case .reply: OverlayChipHoverCardPreviewLine(line: line, palette: palette)
    }
  }

  private func header(_ line: Line) -> some View {
    HStack(alignment: .center, spacing: Self.symbolGap) {
      if let dot = line.dot {
        OverlayAgentStatusDot(kind: dot, palette: palette)
          .frame(width: Self.symbolWidth)
      }
      headerText(line)
        .lineLimit(nil)
        .fixedSize(horizontal: false, vertical: true)
      if let marks = line.marks {
        Spacer(minLength: Self.trailingGap)
        HStack(spacing: Self.markGap) {
          Image(systemName: marks.antenna)
            .font(.system(size: Self.lineSize))
            .foregroundStyle(
              marks.hears
                ? OverlayReceptionMarks.antennaInk(palette).color : palette.mutedText.color
            )
            .frame(width: Self.symbolWidth)
          if let moon = marks.moon {
            Image(systemName: moon)
              .font(.system(size: Self.lineSize))
              .foregroundStyle(palette.primaryText.color)
              .frame(width: Self.symbolWidth)
          }
          if marks.unread > 0 {
            OverlayAgentUnreadBadge(
              count: marks.unread, palette: palette, textScale: Self.badgeScale)
          }
        }
      }
    }
  }

  /// The name semibold in the line's tone, the activity regular and muted.
  private func headerText(_ line: Line) -> Text {
    typealias Ink = AttributeScopes.SwiftUIAttributes.ForegroundColorAttribute
    typealias Face = AttributeScopes.SwiftUIAttributes.FontAttribute
    var name = AttributedString(line.text)
    name[Face.self] = Font.system(size: line.size, weight: .semibold).monospacedDigit()
    name[Ink.self] = line.tone.color(in: palette)
    guard let detail = line.detail else { return Text(name) }
    var activity = AttributedString(" · " + detail)
    activity[Face.self] = Font.system(size: line.size).monospacedDigit()
    activity[Ink.self] = Tone.muted.color(in: palette)
    return Text(name + activity)
  }

  // MARK: Text

  static func lines(
    chip: OverlayChannelMapChip, status: OverlayChannelChipStatus, now: Date = Date()
  ) -> [Line] {
    let policy = AgentBroadcastPolicy(member: status.reception == .member, moon: status.quiet)
    var lines = [
      Line(
        kind: .header, text: chip.name.isEmpty ? chip.digit : chip.digit + " · " + chip.name,
        tone: .primary, detail: activityText(status, now: now), dot: presenceDot(status),
        marks: Marks(
          antenna: policy.statusSymbol, hears: policy.member,
          moon: policy.moon ? policy.quietSymbol : nil, unread: status.unread))
    ]
    if let runtime = runtimeLabel(status) {
      lines.append(Line(kind: .runtime, text: runtime, tone: .muted))
    }
    if let sent = status.lastSent {
      lines.append(
        Line(kind: .sent, text: sentText(sent, now: now), tone: .primary, symbol: "arrow.up.right"))
    }
    if let reply = status.lastReply {
      lines.append(
        Line(
          kind: .reply, text: reply.at.map { time($0, now: now) } ?? "", tone: .primary,
          detail: reply.preview, symbol: "arrow.down.left"))
    }
    return lines
  }

  /// The chip's own label (channel, name, chord), then each fact, the
  /// reply's preview included.
  static func accessibilityText(
    chip: OverlayChannelMapChip, status: OverlayChannelChipStatus,
    modifier: OverlayChannelChordModifier, now: Date = Date()
  ) -> String {
    ([chip.accessibilityLabel(modifier: modifier)]
      + spokenStatus(status, now: now, includesPreview: true))
      .joined(separator: ". ")
  }

  /// The facts alone, for the pill's accessibility value, the agents list
  /// and the roster's presence dot. The reply's preview only on the card,
  /// since those rows show the preview themselves.
  static func spokenStatus(
    _ status: OverlayChannelChipStatus, now: Date = Date(), includesPreview: Bool = false
  ) -> [String] {
    var parts: [String] = []
    if let runtime = runtimeLabel(status) { parts.append(runtime) }
    parts.append(spokenPresence(status, now: now))
    parts.append(spokenReception(status))
    if let sent = status.lastSent {
      parts.append(
        String(
          localized: "Last sent message",
          comment: "Chip hover card, VoiceOver: the newest message is yours")
          + ": " + sentText(sent, now: now))
    }
    if let reply = status.lastReply {
      let facts = [reply.at.map { time($0, now: now) }, includesPreview ? reply.preview : nil]
        .compactMap { $0 }.filter { !$0.isEmpty }
      if !facts.isEmpty {
        parts.append(
          String(
            localized: "Last reply",
            comment: "Chip hover card, VoiceOver: the newest message is the agent's")
            + ": " + facts.joined(separator: " · "))
      }
    }
    if status.unread > 0 { parts.append(unreadText(status.unread)) }
    return parts
  }

  /// The runtime line: the provider and the project the agent works in
  /// (L16: "Agy · Codescribe"), either alone when the other is unknown.
  static func runtimeLabel(_ status: OverlayChannelChipStatus) -> String? {
    let parts = [status.provider.map(providerName), status.project].compactMap { $0 }
    return parts.isEmpty ? nil : parts.joined(separator: " · ")
  }

  /// `codex` → `Codex`: the provider as the card names it.
  static func providerName(_ provider: String) -> String {
    OverlayChannelMapChip.displayName(provider)
  }

  /// When the agent was last there: "now" while its follower is alive,
  /// else the time since its lease's last heartbeat, never its last
  /// message. Nil without a heartbeat (no lease, or no liveness claim).
  static func activityText(_ status: OverlayChannelChipStatus, now: Date = Date()) -> String? {
    if status.presence == .online { return MessageTimestamp.ago(now, now: now) }
    return status.lastHeartbeat.map { MessageTimestamp.ago($0, now: now) }
  }

  /// Presence for VoiceOver, which sees no dot: Online, Not active or No
  /// data, "Not reading" for a stall, and since when a gone agent was there.
  static func spokenPresence(_ status: OverlayChannelChipStatus, now: Date = Date()) -> String {
    var parts: [String]
    switch status.presence {
    case .online:
      parts = [
        String(
          localized: "Online",
          comment: "Channel chip hover card: the agent's follower is alive in the roster sample")
      ]
      if status.stalled { parts.append(String(localized: "Not reading")) }
    case .inactive:
      parts = [
        String(
          localized: "Not active",
          comment: "Channel chip hover card: no live follower reported; cautious, not offline")
      ]
      if let activity = activityText(status, now: now) { parts.append(activity) }
    case .unknown:
      parts = [
        String(
          localized: "No data",
          comment: "Channel chip hover card: the roster makes no liveness claim for the agent")
      ]
    }
    return parts.joined(separator: " · ")
  }

  /// A message's time as the conversation's bubbles give it: the clock
  /// today, yesterday's word and the clock, else the date and the clock.
  static func time(_ date: Date, now: Date = Date()) -> String {
    MessageTimestamp.label(for: date, now: now)
  }

  /// Your last message's line: its time, then the roster's receipt reading.
  static func sentText(_ sent: OverlayChannelChipStatus.Sent, now: Date = Date()) -> String {
    [sent.at.map { time($0, now: now) }, sent.status].compactMap { $0 }
      .joined(separator: " · ")
  }

  /// The reception as VoiceOver reads it, which hears no antenna: whether
  /// the agent hears channel 0, named after a colon so the name is never
  /// declined, then Do not disturb under the moon.
  static func spokenReception(_ status: OverlayChannelChipStatus) -> String {
    let name = status.channelZeroName
    let reception: String
    switch status.reception {
    case .member:
      reception = String(
        localized: "Hears: \(name) (channel 0)",
        comment:
          "Channel chip hover card, VoiceOver: broadcasts reach this agent; the argument is channel 0's display name, kept in the nominative"
      )
    case .off:
      reception = String(
        localized: "Does not hear: \(name) (channel 0)",
        comment:
          "Channel chip hover card, VoiceOver: the agent left channel 0; the argument is channel 0's display name, kept in the nominative"
      )
    }
    guard status.quiet else { return reception }
    return reception + " · " + String(localized: "Do not disturb")
  }

  static func unreadText(_ count: Int) -> String {
    String(
      localized: "\(count) unread",
      comment: "Channel chip hover card: new replies from the agent not read yet")
  }

  /// The header's dot, as the agents list draws its row's: a gone
  /// follower's ring first, then the speaking dot while the reply plays,
  /// the stall's olive dot with its bold "!" (L6, 2026-11-06: a stall is
  /// not sleep; "zzz" belongs to the moon alone), olive online and "?"
  /// without a liveness claim.
  static func presenceDot(_ status: OverlayChannelChipStatus) -> OverlayAgentDotKind {
    if status.presence == .inactive { return .sleeping }
    if status.speaking { return .speaking }
    if status.stalled { return .stalled }
    switch status.presence {
    case .online: return .present
    case .inactive: return .sleeping
    case .unknown: return .idle
    }
  }

  // MARK: Size

  /// The card's size, measured from its lines in the fonts it draws them
  /// in: the widest line between `minWidth` and `maxWidth`, every line's
  /// height as wrapped at the card's width and the gaps between them. The
  /// hint panel takes this size as given, so nothing is cut.
  @MainActor
  static func size(
    chip: OverlayChannelMapChip, status: OverlayChannelChipStatus, now: Date = Date()
  ) -> CGSize {
    size(lines: lines(chip: chip, status: status, now: now))
  }

  /// The size of a card drawing these lines.
  @MainActor
  static func size(lines: [Line]) -> CGSize {
    let widest = lines.map(naturalWidth).max() ?? 0
    let width = ceil(min(maxWidth, max(minWidth, widest + 2 * padding)))
    let heights = lines.map { height($0, within: width - 2 * padding) }
    let height =
      heights.reduce(0, +) + lineSpacing * CGFloat(max(0, lines.count - 1)) + 2 * padding
    return CGSize(width: width, height: ceil(height))
  }

  /// A line's own font: the 12 pt header, 11 pt below, monospaced digits.
  static func font(_ line: Line, weight: NSFont.Weight = .regular) -> NSFont {
    NSFont.monospacedDigitSystemFont(ofSize: line.size, weight: weight)
  }

  /// A line's words as drawn: the header's name semibold and its activity
  /// regular; every other line's text in the line's font.
  static func words(_ line: Line) -> NSAttributedString {
    guard line.kind == .header else {
      return NSAttributedString(string: line.text, attributes: [.font: font(line)])
    }
    let words = NSMutableAttributedString(
      string: line.text, attributes: [.font: font(line, weight: .semibold)])
    if let detail = line.detail {
      words.append(NSAttributedString(string: " · " + detail, attributes: [.font: font(line)]))
    }
    return words
  }

  /// The symbol column before a line's words: the dot or the arrow.
  static func symbolsWidth(_ line: Line) -> CGFloat {
    line.symbol != nil || line.dot != nil ? symbolWidth + symbolGap : 0
  }

  /// The roster's badge as the card draws it.
  @MainActor
  static func badgeDiameter(_ count: Int) -> CGFloat {
    OverlayAgentUnreadBadge.diameter(count: count, textScale: badgeScale)
  }

  /// The header's glyphs after the name, and the room before them: the
  /// spacer's minimum and the row's spacing on both of its sides.
  @MainActor
  static func marksWidth(_ line: Line) -> CGFloat {
    guard let marks = line.marks else { return 0 }
    var width = trailingGap + 2 * symbolGap + symbolWidth
    if marks.moon != nil { width += markGap + symbolWidth }
    if marks.unread > 0 { width += markGap + badgeDiameter(marks.unread) }
    return width
  }

  /// A line's width unwrapped: what the card grows to, up to the ceiling.
  /// The reply's time and preview share the available width; the preview may
  /// wrap up to `replyPreviewLineLimit` lines without widening the card.
  @MainActor
  static func naturalWidth(_ line: Line) -> CGFloat {
    let text = textWidth(words(line))
    guard line.kind == .reply else { return symbolsWidth(line) + text + marksWidth(line) }
    let preview = line.detail.map { textWidth($0, font(line)) + symbolGap } ?? 0
    return symbolsWidth(line) + text + preview
  }

  /// A line's height with its words wrapped within the card's content
  /// width; the reply's preview wraps within its line budget; the header at
  /// least its badge.
  @MainActor
  static func height(_ line: Line, within content: CGFloat) -> CGFloat {
    switch line.kind {
    case .reply:
      let one = oneLine(font(line))
      guard let preview = line.detail, !preview.isEmpty else { return one }
      let available = content - symbolsWidth(line) - textWidth(words(line)) - symbolGap
      let previewWords = NSAttributedString(string: preview, attributes: [.font: font(line)])
      return min(
        wrappedHeight(previewWords, font(line), within: available),
        CGFloat(replyPreviewLineLimit) * one)
    case .header:
      let column = content - symbolsWidth(line) - marksWidth(line)
      let text = wrappedHeight(words(line), font(line, weight: .semibold), within: column)
      guard let marks = line.marks, marks.unread > 0 else { return text }
      return max(text, badgeDiameter(marks.unread))
    case .runtime, .sent:
      return wrappedHeight(words(line), font(line), within: content - symbolsWidth(line))
    }
  }

  /// Words measured in their font, with two points of slack against
  /// SwiftUI's own rounding, so a line measured to fit is never cut.
  static func textWidth(_ text: String, _ font: NSFont) -> CGFloat {
    textWidth(NSAttributedString(string: text, attributes: [.font: font]))
  }

  static func textWidth(_ words: NSAttributedString) -> CGFloat {
    ceil(words.size().width) + 2
  }

  /// One line of the font, a point taller, as SwiftUI lays out the card's
  /// 11 and 12 pt lines.
  static func oneLine(_ font: NSFont) -> CGFloat {
    ceil(font.ascender - font.descender + font.leading) + 1
  }

  /// The height of words wrapped at `width`: whole lines of the font.
  static func wrappedHeight(
    _ words: NSAttributedString, _ font: NSFont, within width: CGFloat
  ) -> CGFloat {
    let one = oneLine(font)
    let bounds = words.boundingRect(
      with: CGSize(width: max(1, width - 2), height: .greatestFiniteMagnitude),
      options: [.usesLineFragmentOrigin, .usesFontLeading])
    let count = max(1, (bounds.height / one - 0.25).rounded(.up))
    return count * one
  }

  @MainActor
  static func lineHeight(size: CGFloat) -> CGFloat {
    let font = NSFont.systemFont(ofSize: size, weight: .semibold)
    return ceil(font.ascender - font.descender + font.leading)
  }
}

/// The agent's last reply on the hover card: the arrow, its time and up to
/// three lines of the roster's preview (`OverlayChannelStatusView.drawerPreview`).
/// Longer replies end with an ellipsis at the card's measured height.
struct OverlayChipHoverCardPreviewLine: View {
  let line: OverlayChannelChipHoverCard.Line
  let palette: OverlayAppearancePalette

  var body: some View {
    HStack(alignment: .firstTextBaseline, spacing: OverlayChannelChipHoverCard.symbolGap) {
      if let symbol = line.symbol {
        Image(systemName: symbol)
          .font(.system(size: line.size))
          .frame(width: OverlayChannelChipHoverCard.symbolWidth)
      }
      if !line.text.isEmpty {
        Text(verbatim: line.text)
          .font(.system(size: line.size))
          .monospacedDigit()
          .lineLimit(1)
          .fixedSize()
      }
      if let preview = line.detail {
        Text(verbatim: preview)
          .font(.system(size: line.size))
          .lineLimit(OverlayChannelChipHoverCard.replyPreviewLineLimit)
          .truncationMode(.tail)
          .fixedSize(horizontal: false, vertical: true)
      }
    }
    .foregroundStyle(line.tone.color(in: palette))
  }
}
