import AppKit
import SwiftUI

/// Developer-only Lab desk. Hidden unless `CSDeveloperSurface` is baked.
struct LabPanel: View {
  @ObservedObject var model: SettingsViewModel
  @AppStorage("codescribe.lab_mode") private var labMode = false

  var body: some View {
    VStack(alignment: .leading, spacing: 16) {
      SettingsPageHeader(
        "Voice Lab",
        blurb: labMode
          ? "Lab mode is on. Overlay follows the tray toggle — Lab does not steal it."
          : "Open the loopback Voice Lab. Production builds never show this panel."
      )

      Toggle("Lab mode", isOn: $labMode)
        .toggleStyle(.switch)
        .font(.body)

      Picker(
        "Whisper buffering",
        selection: Binding(
          get: { model.settings.whisperAdaptiveBuffer },
          set: { model.setWhisperAdaptiveBuffer($0) }
        )
      ) {
        Text("Fixed windows").tag(false)
        Text("Adaptive buffer (experimental)").tag(true)
      }
      Text(
        "Applies to the next recording. Adaptive buffering preserves phrase boundaries within bounded audio and wait limits."
      )
      .font(.caption)
      .foregroundStyle(.secondary)

      Picker(
        "Text formatting",
        selection: Binding(
          get: { model.settings.formatOnDevice },
          set: { model.setFormatOnDevice($0) }
        )
      ) {
        Text("Cloud provider").tag(false)
        Text("Apple on-device (experimental)").tag(true)
      }
      Text(
        "Applies to the next formatting pass. The on-device model runs first; any failure falls back to the cloud provider. A .env value overrides this toggle."
      )
      .font(.caption)
      .foregroundStyle(.secondary)

      Button("Open Voice Lab") {
        Task { await VoiceLabRuntime.shared.openConsole() }
      }
      .font(CSFont.mono(11, .semibold))
      .foregroundStyle(CSColor.chromeAccent)
    }
    .padding(CSSpace.xl)
    .frame(maxWidth: .infinity, alignment: .leading)
  }
}
