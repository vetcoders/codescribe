import SwiftUI

/// A quiet header affordance. Delivery details belong to its popover, not the transcript.
struct OverlayChannelStatusView: View {
  let channels: [OverlayChannelDelivery]
  let unavailable: Bool
  let palette: OverlayAppearancePalette

  @State private var showsDetails = false

  private var hasOpenChannel: Bool { channels.contains(where: \.isOpen) }
  private var summary: String {
    if unavailable { return "Channel status unavailable" }
    return hasOpenChannel ? "Microphone active · channel open" : "Agent channels connected"
  }

  var body: some View {
    Button {
      showsDetails.toggle()
    } label: {
      Image(
        systemName: unavailable
          ? "questionmark.circle"
          : hasOpenChannel ? "mic.fill" : "antenna.radiowaves.left.and.right"
      )
      .font(.system(size: 11, weight: .medium))
      .foregroundStyle(hasOpenChannel ? palette.listeningStatus.color : palette.mutedText.color)
      .frame(width: 22, height: 22)
      .contentShape(RoundedRectangle(cornerRadius: 6))
    }
    .buttonStyle(.plain)
    .help(summary + " — show details")
    .accessibilityLabel("Agent channels")
    .accessibilityValue(summary)
    .accessibilityIdentifier("overlay-channel-details")
    .popover(isPresented: $showsDetails, arrowEdge: .bottom) {
      details
        .padding(16)
        .frame(width: 300)
    }
  }

  private var details: some View {
    VStack(alignment: .leading, spacing: 4) {
      ForEach(channels) { channel in
        if channel.isOpen {
          Label("Microphone active · channel \(channel.channel)", systemImage: "mic.fill")
            .font(.system(size: 11, weight: .medium))
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
  }

  private func symbol(for stage: OverlayChannelDelivery.Stage) -> String {
    switch stage {
    case .sent: "paperplane"
    case .queued: "tray"
    case .received: "checkmark.circle.fill"
    }
  }
}
