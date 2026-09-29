import SwiftUI

/// Always visible in both expanded and collapsed chrome. Open capture is a
/// full-width warning, separate from the newest utterance's delivery badge.
struct OverlayChannelStatusView: View {
  let channels: [OverlayChannelDelivery]
  let unavailable: Bool
  let palette: OverlayAppearancePalette

  var body: some View {
    VStack(alignment: .leading, spacing: 4) {
      ForEach(channels) { channel in
        if channel.isOpen {
          Label("CHANNEL \(channel.channel) OPEN · microphone active", systemImage: "mic.fill")
            .font(.system(size: 12, weight: .bold))
            .foregroundStyle(palette.listeningStatus.color)
            .fixedSize(horizontal: false, vertical: true)
            .accessibilityIdentifier("overlay-channel-open-\(channel.channel)")
        }
        HStack(spacing: 6) {
          Text("\(channel.channel) · \(channel.agent)")
            .lineLimit(1)
            .truncationMode(.middle)
          Spacer(minLength: 4)
          if unavailable {
            Text("status unavailable")
          } else if let stage = channel.stage {
            Label(stage.rawValue, systemImage: symbol(for: stage))
              .foregroundStyle(
                stage == .received ? palette.successStatus.color : palette.primaryText.color
              )
              .accessibilityIdentifier("overlay-channel-delivery-\(channel.channel)")
          } else {
            Text("no sealed utterance")
          }
        }
        .font(.system(size: 11, weight: .medium))
        .foregroundStyle(palette.mutedText.color)
        .accessibilityElement(children: .combine)
      }
      if unavailable {
        Text("Channel status unavailable — last open state retained")
          .font(.system(size: 11, weight: .medium))
          .foregroundStyle(palette.processingStatus.color)
          .accessibilityIdentifier("overlay-channel-status-unavailable")
      }
    }
    .frame(maxWidth: .infinity, alignment: .leading)
    .allowsHitTesting(false)
  }

  private func symbol(for stage: OverlayChannelDelivery.Stage) -> String {
    switch stage {
    case .sent: "paperplane"
    case .queued: "tray"
    case .received: "checkmark.circle.fill"
    }
  }
}
