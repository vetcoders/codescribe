import SwiftUI

/// Read-only rows about the last transcription (LLM truth lives on Agent › LLM
/// lanes). No readiness dot: the engine row reports what served the last take,
/// not whether the configuration looks healthy.
struct DictationRuntimeRows: View {
  @ObservedObject var model: SettingsViewModel

  var body: some View {
    VStack(spacing: 0) {
      RuntimeRow(
        key: String(
          localized: "Last transcription engine",
          comment:
            "Engine tab row: the engine that served the last transcription of this app session"
        ),
        value: model.activeSTT,
        tint: true, trailing: .none)
      if let row = model.sttModelRow {
        divider
        RuntimeRow(key: row.label, value: row.value, tint: false, mono: true, trailing: .none)
      }
      divider
      RuntimeRow(
        key: String(
          localized: "Spoken language",
          comment:
            "Engine tab row: the language setting handed to Apple, local Whisper and the cloud engine"
        ),
        value: model.whisperLanguageDisplay,
        tint: true, trailing: .none)
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
