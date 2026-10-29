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
            Spacer(minLength: 4)
            if unreadCount > 0, let conversationID {
              OverlayAgentUnreadBadge(count: unreadCount, palette: palette, textScale: textScale)
                .accessibilityIdentifier("overlay-agent-unread-replies-\(conversationID)")
            }
            if let receipt {
              OverlayRosterReceiptMark(receipt: receipt, palette: palette, textScale: textScale)
            }
            if let timestamp {
              MessageTimestampLabel(date: timestamp, compact: true)
                .font(.system(size: 11 * textScale))
                .foregroundStyle(palette.mutedText.color)
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
      .accessibilityLabel("\(count) unread replies")
  }
}

/// Compact projection of a frozen recipient's delivery evidence.
struct OverlayRosterReceipt {
  let symbol: String
  let text: String
}

private struct OverlayRosterReceiptMark: View {
  let receipt: OverlayRosterReceipt
  let palette: OverlayAppearancePalette
  let textScale: CGFloat

  var body: some View {
    Group {
      if receipt.symbol == "double-check" {
        OverlayRosterDoubleCheckmark()
          .stroke(style: StrokeStyle(lineWidth: 1.5, lineCap: .round, lineJoin: .round))
          .frame(width: 14 * textScale, height: 10 * textScale)
      } else {
        Image(systemName: receipt.symbol)
          .font(.system(size: 11 * textScale, weight: .medium))
      }
    }
    .foregroundStyle(palette.mutedText.color)
    .help(Text(verbatim: receipt.text))
    .accessibilityLabel(Text(verbatim: receipt.text))
  }
}

private struct OverlayRosterDoubleCheckmark: Shape {
  func path(in rect: CGRect) -> Path {
    var path = Path()
    path.move(to: CGPoint(x: rect.width * 0.03, y: rect.height * 0.53))
    path.addLine(to: CGPoint(x: rect.width * 0.28, y: rect.height * 0.82))
    path.addLine(to: CGPoint(x: rect.width * 0.72, y: rect.height * 0.18))
    path.move(to: CGPoint(x: rect.width * 0.40, y: rect.height * 0.82))
    path.addLine(to: CGPoint(x: rect.width * 0.97, y: rect.height * 0.18))
    return path
  }
}
