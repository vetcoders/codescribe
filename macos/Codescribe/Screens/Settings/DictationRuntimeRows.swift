import SwiftUI

/// Read-only STT truth rows (LLM truth lives on Agent › LLM lanes).
struct DictationRuntimeRows: View {
  @ObservedObject var model: SettingsViewModel

  var body: some View {
    VStack(spacing: 0) {
      RuntimeRow(
        key: "Active STT", value: model.activeSTT,
        tint: true, trailing: .dot(model.sttHealthy ? CSColor.oliveLight : CSColor.amber))
      divider
      RuntimeRow(
        key: "STT model (preference)", value: model.sttModelDescription,
        tint: false, mono: true, trailing: .none)
      divider
      RuntimeRow(
        key: "Whisper language", value: model.whisperLanguageCode,
        tint: true, mono: true, trailing: .none)
    }
    .clipShape(.rect(cornerRadius: CSRadius.composer))
    .overlay {
      RoundedRectangle(cornerRadius: CSRadius.composer)
        .strokeBorder(CSColor.hairline(0.07), lineWidth: 1)
    }
  }

  private var divider: some View {
    Rectangle().fill(CSColor.hairline(0.05)).frame(height: 1)
  }
}
