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
  /// The row whose antenna and moon are drawn, clickable, before the status dot;
  /// nil for the All row, which has no reception of its own.
  let reception: OverlayChannelDelivery?
  var receptionEnabled = true
  let palette: OverlayAppearancePalette
  let textScale: CGFloat

  var body: some View {
    HStack(alignment: .top, spacing: 6) {
      VStack(alignment: .leading, spacing: 1) {
        HStack(spacing: 6) {
          if let channel {
            Text(verbatim: channel)
              .font(.system(size: 11 * textScale, weight: .medium, design: .monospaced))
              .foregroundStyle(palette.mutedText.color)
              .accessibilityIdentifier("overlay-agent-digit-\(channel)")
          }
          if let reception {
            OverlayReceptionMarks(
              channel: reception, palette: palette, textScale: textScale,
              enabled: receptionEnabled)
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
            .accessibilityIdentifier("overlay-agent-title-\(channel ?? "0")")
          Spacer(minLength: 4)
          if unreadCount > 0, let conversationID {
            OverlayAgentUnreadBadge(count: unreadCount, palette: palette, textScale: textScale)
              .accessibilityIdentifier("overlay-agent-unread-replies-\(conversationID)")
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
        if let descriptor, let channel {
          Text(verbatim: descriptor)
            .font(.system(size: 11 * textScale))
            .foregroundStyle(palette.mutedText.color)
            .lineLimit(1)
            .truncationMode(.middle)
            .accessibilityLabel(Text(verbatim: descriptor))
            .accessibilityIdentifier("overlay-agent-descriptor-\(channel)")
        }
        if let preview {
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

  var body: some View {
    Text(verbatim: String(count))
      .font(.system(size: 11 * textScale, weight: .semibold))
      .monospacedDigit()
      .foregroundStyle(palette.processingStatus.color)
      .padding(.horizontal, 6).padding(.vertical, 2)
      .background(palette.processingStatus.color.opacity(0.12), in: Capsule())
      .accessibilityLabel("\(count) unread replies")
  }
}
