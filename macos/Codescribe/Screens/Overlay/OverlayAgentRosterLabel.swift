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
  let dot: OverlayAgentDotKind
  let statusLabel: String
  let statusDetail: String
  /// Rendered right after the name: the caller's own control, such as the
  /// client focus button that lives beside the agent's name.
  var titleAccessory: AnyView? = nil
  let palette: OverlayAppearancePalette
  let textScale: CGFloat
  var headerOnly = false
  var contentOnly = false
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
                .foregroundStyle(palette.mutedText.color)
                .accessibilityIdentifier("overlay-agent-digit-\(channel)")
            }
            OverlayAgentStatusDot(kind: dot, palette: palette)
              .accessibilityLabel(Text(verbatim: statusLabel))
              .accessibilityValue(Text(verbatim: statusDetail))
              .accessibilityIdentifier(
                channel.map { "overlay-channel-delivery-\($0)" } ?? "overlay-channel-delivery"
              )
            Text(verbatim: title)
              .font(.system(size: 14 * textScale, weight: .semibold))
              .lineLimit(1)
              .truncationMode(.tail)
              .layoutPriority(1)
              .help(Text(verbatim: title))
              .accessibilityIdentifier("overlay-agent-title-\(channel ?? "0")")
            if let titleAccessory {
              titleAccessory
            }
            if let receipt {
              OverlayRosterReceiptMark(receipt: receipt, palette: palette, textScale: textScale)
            }
            if let trailingMarks {
              trailingMarks
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
                .foregroundStyle(palette.mutedText.color)
                .lineLimit(1)
                .accessibilityIdentifier(
                  "overlay-agent-timestamp-\(conversationID ?? channel ?? "0")")
            }
          }
          .frame(maxWidth: .infinity, alignment: .leading)
        }
        if !headerOnly, let descriptor, let channel {
          Text(verbatim: descriptor)
            .font(.system(size: 11 * textScale))
            .foregroundStyle(palette.mutedText.color)
            .lineLimit(1)
            .truncationMode(.middle)
            .help(Text(verbatim: descriptor))
            .accessibilityLabel(Text(verbatim: descriptor))
            .accessibilityIdentifier("overlay-agent-descriptor-\(channel)")
        }
        if !headerOnly, let preview {
          Text(verbatim: preview)
            .font(.system(size: 13 * textScale))
            .foregroundStyle(unreadCount > 0 ? palette.bodyText.color : palette.mutedText.color)
            .lineLimit(2)
            .fixedSize(horizontal: false, vertical: true)
        }
      }
      .frame(maxWidth: .infinity, alignment: .leading)
    }
    .frame(maxWidth: .infinity, alignment: .leading)
    .contentShape(Rectangle())
  }
}

/// One badge for both the current roster and saved conversations.
struct OverlayAgentUnreadBadge: View {
  let count: Int
  let palette: OverlayAppearancePalette
  let textScale: CGFloat

  private var badgeDiameter: CGFloat {
    max(20, CGFloat(String(count).count) * 8 + 8) * textScale
  }

  var body: some View {
    Text(verbatim: String(count))
      .font(.system(size: 11 * textScale, weight: .semibold))
      .monospacedDigit()
      .foregroundStyle(palette.processingStatus.color)
      .frame(width: badgeDiameter, height: badgeDiameter)
      .background(palette.processingStatus.color.opacity(0.12), in: Circle())
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

/// Compact projection of a frozen recipient's delivery evidence.
struct OverlayRosterReceipt {
  let symbol: String
  let text: String
  let helpText: String
  var isRead = false
}

private struct OverlayRosterReceiptMark: View {
  let receipt: OverlayRosterReceipt
  let palette: OverlayAppearancePalette
  let textScale: CGFloat

  var body: some View {
    Group {
      if receipt.symbol == "double-check" || receipt.symbol == "checkmark" {
        OverlayRosterDoubleCheckmark()
          .stroke(style: StrokeStyle(lineWidth: 1.5, lineCap: .round, lineJoin: .round))
          .frame(width: 16 * textScale, height: 10 * textScale)
      } else {
        Image(systemName: receipt.symbol)
          .font(.system(size: 11 * textScale, weight: .medium))
      }
    }
    .foregroundStyle(receipt.isRead ? palette.bodyText.color : palette.mutedText.color)
    .help(Text(verbatim: receipt.helpText))
    .accessibilityLabel(Text(verbatim: receipt.helpText))
  }
}

private struct OverlayRosterDoubleCheckmark: Shape {
  func path(in rect: CGRect) -> Path {
    var check = Path()
    check.move(to: CGPoint(x: rect.minX + rect.width * 0.05, y: rect.minY + rect.height * 0.53))
    check.addLine(to: CGPoint(x: rect.minX + rect.width * 0.28, y: rect.minY + rect.height * 0.82))
    check.addLine(to: CGPoint(x: rect.minX + rect.width * 0.68, y: rect.minY + rect.height * 0.18))
    var path = check
    path.addPath(check, transform: CGAffineTransform(translationX: rect.width * 0.27, y: 0))
    return path
  }
}
