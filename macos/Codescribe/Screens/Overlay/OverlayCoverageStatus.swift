import SwiftUI

/// A projected uncertainty, not a verdict about the words or their delivery.
struct OverlayCoverageStatus: View {
  @Environment(\.openSettings) private var openSettings
  static let message =
    "Recording quality low. Run mic calibration and check surroundings."
  let palette: OverlayAppearancePalette
  let canRetranscribe: Bool
  let cloudConfigured: Bool
  var diagnosticNotice: String? = nil
  var diagnosticDetail: String? = nil
  let onRetranscribe: (OverlayRetranscribePass) -> Void
  @State private var presented: String?

  var body: some View {
    OverlayHoverControl(
      id: "overlay-coverage-status", title: "Recording quality details", palette: palette,
      presented: $presented
    ) {
      Label("Recording quality low — review", systemImage: "info.circle")
        .font(.system(size: 11))
        .lineLimit(1)
        .truncationMode(.tail)
        .foregroundStyle(palette.processingStatus.color)
    } detail: { close in
      VStack(alignment: .leading, spacing: 10) {
        Text(Self.message)
          .fixedSize(horizontal: false, vertical: true)
        Button("Mic calibration in Settings…") {
          close()
          SettingsDeepLink.present(.audio, anchor: .audioReadiness)
          openSettings()
          NSApp.activate(ignoringOtherApps: true)
        }
        .controlSize(.small)
        .accessibilityIdentifier("overlay-open-mic-calibration-settings")
        if let diagnosticNotice, let diagnosticDetail {
          Divider()
          Text(diagnosticNotice)
          Text(diagnosticDetail).font(.system(size: 10, design: .monospaced))
        }
        if canRetranscribe {
          Text("Transcribe again")
          HStack {
            Button("Local") {
              close()
              onRetranscribe(.fullHq)
            }
            if cloudConfigured {
              Button("Cloud") {
                close()
                onRetranscribe(.cloud)
              }
            }
          }
          .buttonStyle(.borderless)
          .controlSize(.small)
          .font(.system(size: 11, weight: .medium))
        }
      }
      .frame(width: 250)
    }
  }
}
