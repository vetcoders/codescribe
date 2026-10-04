import SwiftUI

/// Read-only STT truth rows (LLM truth lives on Agent › LLM lanes).
struct DictationRuntimeRows: View {
  @ObservedObject var model: SettingsViewModel

  var body: some View {
    VStack(spacing: 0) {
      RuntimeRow(
        key: String(localized: "Active STT"), value: model.activeSTT,
        tint: true, trailing: .dot(model.sttHealthy ? CSColor.oliveLight : CSColor.amber))
      divider
      RuntimeRow(
        key: String(localized: "STT model (preference)"), value: model.sttModelDescription,
        tint: false, mono: true, trailing: .none)
      divider
      RuntimeRow(
        key: String(localized: "Whisper language"), value: model.whisperLanguageCode,
        tint: true, mono: true, trailing: .none)
    }
    .clipShape(.rect(cornerRadius: CSRadius.composer))
    .overlay {
      RoundedRectangle(cornerRadius: CSRadius.composer)
        .strokeBorder(Color.primary.opacity(0.12), lineWidth: 1)
    }
  }

  private var divider: some View {
    Rectangle().fill(Color.primary.opacity(0.12)).frame(height: 1)
  }
}
