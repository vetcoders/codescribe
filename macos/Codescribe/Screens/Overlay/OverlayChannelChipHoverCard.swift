import AppKit
import SwiftUI

/// The hover card of an agent's chip on the bar (Founder, 2026-11-04: the old
/// `7 · Nela · Ctrl+7` hint only repeated the button). It answers whether
/// the agent is there, whether a message to it makes sense and whether it
/// hears channel 0, from `OverlayChannelChipStatus`, three facts kept apart:
///
/// 1. `5 · Kuba` and the provider;
/// 2. presence: Online, Not active or No data, with "Not reading" when the
///    live follower stalls, behind the presence dot of the agents list
///    under ✦ and the Bus window's roster (`OverlayAgentStatusDot`, L2
///    2026-11-06): olive online, violet and blinking while the agent's
///    reply plays aloud, a muted ring when gone or "?" without data;
/// 3. channel 0 by its display name (L3, 2026-11-06): the antenna and the
///    name in the ink while the agent hears it, the crossed antenna and the
///    name muted while it does not, the moon with Do not disturb;
/// 4. the newest message (L16, 2026-11-06): an arrow for its direction, its
///    time and its fate in the roster's words (Read, Delivered, Spoken);
/// 5. the microphone open on the channel and the agent's voice muted, when
///    either is so (L16);
/// 6. the unread replies, when there are any;
/// 7. the chord, last and small, the one line the button already said.
///
/// The presence line says since when (L16): a stalled reader's minutes, a
/// gone agent's last-heard time (its newest reply), since the roster has
/// no time of a presence change.
///
/// Values only: no state, no polling, no action. The card is one
/// accessibility element whose label reads every line.
struct OverlayChannelChipHoverCard: View {
  enum Tone: Equatable {
    case primary
    case success
    case muted

    func color(in palette: OverlayAppearancePalette) -> Color {
      switch self {
      case .primary: palette.primaryText.color
      case .success: palette.primaryText.color
      case .muted: palette.mutedText.color
      }
    }
  }

  /// One line of the card as plain values, so tests read the card's text.
  struct Line: Equatable {
    enum Kind: Equatable {
      case title
      case presence
      case reception
      case lastMessage
      case audio
      case unread
      case shortcut
    }

    let kind: Kind
    let text: String
    let tone: Tone
    var symbol: String? = nil
    /// A second symbol beside the first: the moon beside the crossed antenna
    /// of an agent off channel 0 under Do not disturb.
    var secondSymbol: String? = nil
    /// The presence dot, drawn by the roster's own dot in place of a symbol.
    var dot: OverlayAgentDotKind? = nil
    /// The title's trailing provider.
    var trailing: String? = nil

    var size: CGFloat {
      switch kind {
      case .title: OverlayChannelChipHoverCard.titleSize
      case .shortcut: OverlayChannelChipHoverCard.shortcutSize
      case .presence, .reception, .lastMessage, .audio, .unread:
        OverlayChannelChipHoverCard.lineSize
      }
    }
  }

  let chip: OverlayChannelMapChip
  let status: OverlayChannelChipStatus
  let modifier: OverlayChannelChordModifier
  let palette: OverlayAppearancePalette

  static let padding: CGFloat = 10
  static let lineSpacing: CGFloat = 4
  static let symbolWidth: CGFloat = 14
  static let symbolGap: CGFloat = 5
  static let trailingGap: CGFloat = 8
  nonisolated static let titleSize: CGFloat = 12
  nonisolated static let lineSize: CGFloat = 11
  nonisolated static let shortcutSize: CGFloat = 10
  /// A tight card: wide enough for its longest line, within these bounds.
  static let minWidth: CGFloat = 200
  static let maxWidth: CGFloat = 240

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
    .accessibilityIdentifier("overlay-channel-chip-card-\(chip.digit)")
  }

  private func row(_ line: Line) -> some View {
    HStack(alignment: .firstTextBaseline, spacing: Self.symbolGap) {
      if let dot = line.dot {
        OverlayAgentStatusDot(kind: dot, palette: palette)
          .frame(width: Self.symbolWidth)
      } else if let symbol = line.symbol {
        Image(systemName: symbol)
          .font(.system(size: line.size))
          .frame(width: Self.symbolWidth)
      }
      if let second = line.secondSymbol {
        Image(systemName: second)
          .font(.system(size: line.size))
          .frame(width: Self.symbolWidth)
      }
      Text(verbatim: line.text)
        .font(.system(size: line.size, weight: line.kind == .title ? .semibold : .regular))
        .monospacedDigit()
        .lineLimit(1)
        .truncationMode(.tail)
      if let trailing = line.trailing {
        Spacer(minLength: Self.trailingGap)
        Text(verbatim: trailing)
          .font(.system(size: Self.lineSize))
          .foregroundStyle(Tone.muted.color(in: palette))
          .lineLimit(1)
      }
    }
    .foregroundStyle(line.tone.color(in: palette))
  }

  var lines: [Line] {
    Self.lines(chip: chip, status: status, modifier: modifier)
  }

  var accessibilityText: String {
    Self.accessibilityText(chip: chip, status: status, modifier: modifier)
  }

  // MARK: Text

  static func lines(
    chip: OverlayChannelMapChip, status: OverlayChannelChipStatus,
    modifier: OverlayChannelChordModifier
  ) -> [Line] {
    var lines = [
      Line(
        kind: .title, text: chip.name.isEmpty ? chip.digit : chip.digit + " · " + chip.name,
        tone: .primary, trailing: status.provider.map(providerName)),
      Line(
        kind: .presence, text: presenceText(status),
        tone: status.stalled ? .muted : (status.presence == .online ? .success : .muted),
        dot: presenceDot(status)),
      Line(
        kind: .reception, text: receptionLine(status),
        tone: status.reception == .member && !status.quiet ? .primary : .muted,
        symbol: receptionSymbol(status), secondSymbol: receptionMoonBeside(status)),
    ]
    if let message = status.lastMessage, let text = lastMessageText(message) {
      lines.append(
        Line(
          kind: .lastMessage, text: text, tone: .primary,
          symbol: message.outgoing ? "arrow.up.right" : "arrow.down.left"))
    }
    if let audio = audioText(status) {
      lines.append(
        Line(
          kind: .audio, text: audio, tone: .primary,
          symbol: status.microphoneOpen ? "mic.fill" : "speaker.slash.fill"))
    }
    if status.unread > 0 {
      lines.append(
        Line(
          kind: .unread, text: unreadText(status.unread), tone: .primary,
          symbol: "circle.fill"))
    }
    if let shortcut = modifier.shortcut(digit: chip.digit) {
      lines.append(Line(kind: .shortcut, text: shortcut, tone: .muted, symbol: "keyboard"))
    }
    return lines
  }

  /// The chip's own label (channel, name, chord), then each fact.
  static func accessibilityText(
    chip: OverlayChannelMapChip, status: OverlayChannelChipStatus,
    modifier: OverlayChannelChordModifier
  ) -> String {
    ([chip.accessibilityLabel(modifier: modifier)] + spokenStatus(status))
      .joined(separator: ". ")
  }

  /// The facts alone, for the pill's accessibility value.
  static func spokenStatus(_ status: OverlayChannelChipStatus) -> [String] {
    var parts: [String] = []
    if let provider = status.provider { parts.append(providerName(provider)) }
    parts.append(presenceText(status))
    parts.append(spokenReception(status))
    if let message = status.lastMessage, let text = lastMessageText(message) {
      let direction =
        message.outgoing
        ? String(
          localized: "Last sent message",
          comment: "Chip hover card, VoiceOver: the newest message is yours")
        : String(
          localized: "Last reply",
          comment: "Chip hover card, VoiceOver: the newest message is the agent's")
      parts.append(direction + ": " + text)
    }
    if let audio = audioText(status) { parts.append(audio) }
    if status.unread > 0 { parts.append(unreadText(status.unread)) }
    return parts
  }

  /// `codex` → `Codex`: the provider as the card names it.
  static func providerName(_ provider: String) -> String {
    OverlayChannelMapChip.displayName(provider)
  }

  static func presenceText(_ status: OverlayChannelChipStatus) -> String {
    let presence: String
    switch status.presence {
    case .online:
      presence = String(
        localized: "Online",
        comment: "Channel chip hover card: the agent's follower is alive in the roster sample")
    case .inactive:
      presence = String(
        localized: "Not active",
        comment: "Channel chip hover card: no live follower reported; cautious, not offline")
    case .unknown:
      presence = String(
        localized: "No data",
        comment: "Channel chip hover card: the roster makes no liveness claim for the agent")
    }
    if status.stalled {
      guard let minutes = status.stalledMinutes else {
        return presence + " · " + String(localized: "Not reading")
      }
      return presence + " · "
        + String(
          localized: "Not reading for \(minutes) min",
          comment:
            "Channel chip hover card: the live follower has not read its backlog for this many minutes"
        )
    }
    guard status.presence == .inactive, let heard = status.lastHeard else { return presence }
    return presence + " · "
      + String(
        localized: "last heard \(clock(heard))",
        comment: "Channel chip hover card: the time of the gone agent's newest reply")
  }

  /// A time on the card: hours and minutes in the user's locale.
  static func clock(_ date: Date) -> String {
    date.formatted(date: .omitted, time: .shortened)
  }

  /// The newest message's line: its time, then its fate when the roster
  /// knows one. Nil without a time or a fate.
  static func lastMessageText(_ message: OverlayChannelChipStatus.LastMessage) -> String? {
    let parts = [message.at.map(clock), message.fate].compactMap { $0 }
    return parts.isEmpty ? nil : parts.joined(separator: " · ")
  }

  /// The microphone open on the channel and the voice muted, when either is.
  static func audioText(_ status: OverlayChannelChipStatus) -> String? {
    var parts: [String] = []
    if status.microphoneOpen { parts.append(String(localized: "Microphone open")) }
    if status.voiceMuted {
      parts.append(
        String(
          localized: "Voice muted",
          comment: "Channel chip hover card: the agent's replies are not played aloud"))
    }
    return parts.isEmpty ? nil : parts.joined(separator: " · ")
  }

  /// The reception line as drawn: channel 0's name alone, its symbol (the
  /// antenna, crossed or not) saying whether the agent hears it; the moon
  /// adds Do not disturb. No "in channel" wording and no declension: the
  /// name is the user's own and stays in the nominative.
  static func receptionLine(_ status: OverlayChannelChipStatus) -> String {
    guard status.quiet else { return status.channelZeroName }
    return status.channelZeroName + " · " + String(localized: "Do not disturb")
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

  /// The presence line's dot, as the agents list draws its row's: a gone
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

  /// The reception line's symbol: the moon under Do not disturb while the
  /// agent hears channel 0, else the antenna, crossed off channel 0. Off
  /// channel 0 under the moon keeps the crossed antenna and adds the moon
  /// beside it (`receptionMoonBeside`), so the line shows what VoiceOver
  /// reads (Founder, 2026-11-06).
  static func receptionSymbol(_ status: OverlayChannelChipStatus) -> String {
    switch status.reception {
    case .member:
      return status.quiet ? "moon.fill" : "antenna.radiowaves.left.and.right"
    case .off: return "antenna.radiowaves.left.and.right.slash"
    }
  }

  /// The moon beside the crossed antenna, off channel 0 under Do not disturb.
  static func receptionMoonBeside(_ status: OverlayChannelChipStatus) -> String? {
    status.quiet && status.reception == .off ? "moon.fill" : nil
  }

  // MARK: Size

  /// The card's size, measured from its lines in their own fonts: the
  /// widest line between `minWidth` and `maxWidth`, every line's height and
  /// the gaps between them. The hint panel takes this size as given.
  @MainActor
  static func size(
    chip: OverlayChannelMapChip, status: OverlayChannelChipStatus,
    modifier: OverlayChannelChordModifier
  ) -> CGSize {
    let lines = lines(chip: chip, status: status, modifier: modifier)
    let widest =
      lines.map { line -> CGFloat in
        var width = OverlayChannelPill.textWidth(line.text, size: line.size)
        if line.symbol != nil || line.dot != nil { width += symbolWidth + symbolGap }
        if line.secondSymbol != nil { width += symbolWidth + symbolGap }
        if let trailing = line.trailing {
          width += trailingGap + OverlayChannelPill.textWidth(trailing, size: lineSize)
        }
        return width
      }.max() ?? 0
    let heights = lines.map { lineHeight(size: $0.size) }
    let height =
      heights.reduce(0, +) + lineSpacing * CGFloat(max(0, lines.count - 1)) + 2 * padding
    // A point of slack per line for the symbols' own line height.
    return CGSize(
      width: ceil(min(maxWidth, max(minWidth, widest + 2 * padding))),
      height: ceil(height + CGFloat(lines.count)))
  }

  @MainActor
  static func lineHeight(size: CGFloat) -> CGFloat {
    let font = NSFont.systemFont(ofSize: size, weight: .semibold)
    return ceil(font.ascender - font.descender + font.leading)
  }
}
