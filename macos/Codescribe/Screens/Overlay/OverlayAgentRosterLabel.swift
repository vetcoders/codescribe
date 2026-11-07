import SwiftUI

/// The roster's replaceable visual content. Its caller resolves bus evidence,
/// conversation identity and commands; this view only renders supplied values.
/// See docs/OVERLAY_PRESENTATION.md before changing the widget's visual shell.
struct OverlayAgentRosterLabel: View {
  let title: String
  let channel: String?
  let conversationID: String?
  let descriptor: String?
  let preview: String?
  let timestamp: Date?
  let unreadCount: Int
  /// Nil for a mailbox: your inbox has no presence to show.
  let dot: OverlayAgentDotKind?
  /// A mailbox's glyph where an agent's digit and dot stand (the envelope).
  var symbol: String? = nil
  let statusLabel: String
  let statusDetail: String
  /// Rendered right after the name: the caller's own control, such as the
  /// client focus button that lives beside the agent's name.
  var titleAccessory: AnyView? = nil
  let palette: OverlayAppearancePalette
  let textScale: CGFloat
  var headerOnly = false
  var contentOnly = false
  /// The last sent message's receipt as a glyph after the name. It must tell
  /// the whole receipt story, not two states (Maciej, 2026-10-30 on build
  /// 2981: "ja bym zostawił te ptaszki, ale one naprawdę muszą dawać status.
  /// Teraz jest tylko (-) i 2 ptaszki").
  var receipt: OverlayRosterReceipt? = nil
  /// After the receipt: the caller's sync chain and panel button.
  var trailingMarks: AnyView? = nil

  var body: some View {
    HStack(alignment: .top, spacing: 6) {
      VStack(alignment: .leading, spacing: 1) {
        if !contentOnly {
          HStack(spacing: 6) {
            if let channel {
              Text(verbatim: channel)
                .font(.system(size: 11 * textScale, weight: .medium, design: .monospaced))
                .foregroundStyle(palette.roster.secondaryText.color)
                .fixedSize()
                .accessibilityIdentifier("overlay-agent-digit-\(channel)")
            }
            if let symbol {
              Image(systemName: symbol)
                .font(.system(size: 12 * textScale))
                .foregroundStyle(palette.mutedText.color)
                .frame(width: 16 * textScale)
                .accessibilityHidden(true)
            }
            if let dot {
              OverlayAgentStatusDot(kind: dot, palette: palette)
                .fixedSize()
                .accessibilityLabel(Text(verbatim: statusLabel))
                .accessibilityValue(Text(verbatim: statusDetail))
                .accessibilityIdentifier(
                  channel.map { "overlay-channel-delivery-\($0)" } ?? "overlay-channel-delivery"
                )
            }
            Text(verbatim: title)
              .font(.system(size: 14 * textScale, weight: .semibold))
              .lineLimit(1)
              .truncationMode(.tail)
              .layoutPriority(1)
              .help(Text(verbatim: title))
              .accessibilityIdentifier("overlay-agent-title-\(channel ?? "0")")
            if let titleAccessory {
              titleAccessory
                .fixedSize()
            }
            if let receipt {
              OverlayRosterReceiptMark(receipt: receipt, palette: palette, textScale: textScale)
                .fixedSize()
            }
            if let trailingMarks {
              trailingMarks
                .fixedSize()
            }
            Spacer(minLength: 4)
            if unreadCount > 0, let conversationID {
              OverlayAgentUnreadBadge(count: unreadCount, palette: palette, textScale: textScale)
                .layoutPriority(2)
                .accessibilityIdentifier("overlay-agent-unread-replies-\(conversationID)")
            }
            if let timestamp {
              MessageTimestampLabel(date: timestamp, compact: true)
                .font(.system(size: 11 * textScale))
                .foregroundStyle(palette.roster.secondaryText.color)
                .lineLimit(1)
                .fixedSize(horizontal: true, vertical: false)
                .accessibilityIdentifier(
                  "overlay-agent-timestamp-\(conversationID ?? channel ?? "0")")
            }
          }
          .frame(maxWidth: .infinity, alignment: .leading)
        }
        if !headerOnly, let descriptor, let channel {
          Text(verbatim: descriptor)
            .font(.system(size: 11 * textScale))
            .foregroundStyle(palette.roster.secondaryText.color)
            .lineLimit(1)
            .truncationMode(.middle)
            .help(Text(verbatim: descriptor))
            .accessibilityLabel(Text(verbatim: descriptor))
            .accessibilityIdentifier("overlay-agent-descriptor-\(channel)")
        }
        if !headerOnly, let preview {
          Text(verbatim: preview)
            .font(
              .system(size: 13 * textScale, weight: Self.previewWeight(unreadCount: unreadCount)))
            .foregroundStyle(
              unreadCount > 0 ? palette.bodyText.color : palette.roster.secondaryText.color
            )
            .lineLimit(2)
            .fixedSize(horizontal: false, vertical: true)
        }
      }
      .frame(maxWidth: .infinity, alignment: .leading)
    }
    .frame(maxWidth: .infinity, alignment: .leading)
    .contentShape(Rectangle())
  }

  /// The preview of an unread conversation is semibold until it is opened
  /// (round 5 decision (b), 2026-11-07), from the same unread count as the
  /// badge beside it; opening clears the count and the weight returns.
  static func previewWeight(unreadCount: Int) -> Font.Weight {
    unreadCount > 0 ? .semibold : .regular
  }
}

/// One badge for both the current roster and saved conversations.
struct OverlayAgentUnreadBadge: View {
  let count: Int
  let palette: OverlayAppearancePalette
  let textScale: CGFloat

  private var badgeDiameter: CGFloat { Self.diameter(count: count, textScale: textScale) }

  /// The badge's side for `count` at `textScale`; the chip's hover card
  /// measures its header with it.
  static func diameter(count: Int, textScale: CGFloat) -> CGFloat {
    max(20, CGFloat(String(count).count) * 8 + 8) * textScale
  }

  var body: some View {
    Text(verbatim: String(count))
      .font(.system(size: 11 * textScale, weight: .semibold))
      .monospacedDigit()
      // New replies in the text tone: amber means warning, violet your voice.
      .foregroundStyle(palette.primaryText.color)
      .frame(width: badgeDiameter, height: badgeDiameter)
      .background(palette.primaryText.color.opacity(0.12), in: Circle())
      .fixedSize()
      .accessibilityLabel("\(count) unread replies")
  }
}

/// The draft-sync chain in its canonical form: the glyph the fiszki render.
/// Lit in the agent accent while this conversation's draft is linked.
struct OverlayRosterSyncMark: View {
  let linked: Bool
  let palette: OverlayAppearancePalette
  let textScale: CGFloat

  var body: some View {
    Image(systemName: "link")
      .font(.system(size: 10 * textScale, weight: .semibold))
      .foregroundStyle(linked ? palette.agentAccent.color : palette.mutedText.color)
      .frame(width: 16 * textScale, height: 16 * textScale)
      .contentShape(Rectangle())
  }
}

/// Compact projection of a frozen recipient's delivery evidence: the words
/// the conversation bubble uses for the same receipt, and the glyph that
/// stands for them in the name row: `clock` queued or addressed with no
/// receipt yet, `checkmark` accepted (`✓` grey), `double-check` delivered
/// (`✓✓` muted, thin) or read (`✓✓` in label ink, heavier, via `isRead`;
/// Founder, 2026-11-05: read and delivered differ by weight and ink, not by
/// a colour), `circle.dotted` no receipt. An empty symbol draws no mark: a message still held for
/// withdrawal has words under the preview and nothing beside the name
/// (Founder, 2026-11-04).
struct OverlayRosterReceipt {
  let symbol: String
  let text: String
  let helpText: String
  var isRead = false
  /// The message this receipt reads, so the chip's hover card puts the
  /// reading beside that message's time and no other.
  var messageID: String? = nil
}

private struct OverlayRosterReceiptMark: View {
  let receipt: OverlayRosterReceipt
  let palette: OverlayAppearancePalette
  let textScale: CGFloat

  var body: some View {
    Group {
      switch receipt.symbol {
      case "":
        EmptyView()
      case "double-check":
        OverlayRosterCheckmarks(count: 2)
          .stroke(
            style: StrokeStyle(
              lineWidth: receipt.isRead ? 2.25 : 1.5, lineCap: .round, lineJoin: .round)
          )
          .frame(width: 16 * textScale, height: 10 * textScale)
      case "checkmark":
        OverlayRosterCheckmarks(count: 1)
          .stroke(style: StrokeStyle(lineWidth: 1.5, lineCap: .round, lineJoin: .round))
          .frame(width: 16 * textScale, height: 10 * textScale)
      default:
        Image(systemName: receipt.symbol)
          .font(.system(size: 11 * textScale, weight: .medium))
      }
    }
    // Every receipt is read like text (L8): the label ink; the heavier stroke says read.
    .foregroundStyle(palette.roster.receipt.color)
    .help(Text(verbatim: receipt.helpText))
    .accessibilityLabel(Text(verbatim: receipt.helpText))
    .accessibilityIdentifier("overlay-agent-receipt-\(receipt.symbol)")
  }
}

/// One check for queue acceptance, two for delivery or explicit read/ACK.
/// Delivered and Read share the shape; their stroke and ink are set by the
/// receipt mark.
/// The second check sits a third of the width behind the first.
private struct OverlayRosterCheckmarks: Shape {
  let count: Int

  func path(in rect: CGRect) -> Path {
    var check = Path()
    check.move(to: CGPoint(x: rect.minX + rect.width * 0.05, y: rect.minY + rect.height * 0.53))
    check.addLine(to: CGPoint(x: rect.minX + rect.width * 0.28, y: rect.minY + rect.height * 0.82))
    check.addLine(to: CGPoint(x: rect.minX + rect.width * 0.68, y: rect.minY + rect.height * 0.18))
    var path = check
    if count > 1 {
      path.addPath(check, transform: CGAffineTransform(translationX: rect.width * 0.27, y: 0))
    }
    return path
  }
}
