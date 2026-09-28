import SwiftUI

/// A projected uncertainty, not a verdict about the words or their delivery.
struct OverlayCoverageStatus: View {
  let palette: OverlayAppearancePalette
  let canRetranscribe: Bool
  let cloudConfigured: Bool
  var diagnosticNotice: String? = nil
  var diagnosticDetail: String? = nil
  let onRetranscribe: (OverlayRetranscribePass) -> Void
  @State private var presented: String?

  var body: some View {
    OverlayHoverControl(
      id: "overlay-coverage-status", title: "Review transcript", palette: palette,
      presented: $presented
    ) {
      Label("Review transcript", systemImage: "info.circle")
        .csMono(10, .medium)
        .foregroundStyle(palette.processingStatus.color)
    } detail: { close in
      VStack(alignment: .leading, spacing: 10) {
        Text("The text was kept, but we could not confirm that the transcription is complete.")
          .fixedSize(horizontal: false, vertical: true)
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
        }
      }
      .frame(width: 250)
    }
  }
}
