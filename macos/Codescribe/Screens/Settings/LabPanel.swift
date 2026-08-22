import AppKit
import SwiftUI

/// Developer-only Lab desk. Hidden unless `CSDeveloperSurface` is baked.
struct LabPanel: View {
  @AppStorage("codescribe.lab_mode") private var labMode = false

  var body: some View {
    VStack(alignment: .leading, spacing: 16) {
      EyebrowLabel(text: "Settings · \(SettingsSection.lab.title)")
      Text("Voice Lab")
        .font(CSFont.ui(26, .bold))
        .foregroundStyle(CSColor.textHigh)
      Text(
        labMode
          ? "Lab mode is on. Overlay follows the tray toggle — Lab does not steal it."
          : "Open the loopback Voice Lab. Production builds never show this panel."
      )
      .font(CSFont.ui(12.5))
      .foregroundStyle(CSColor.textMutedAlt)

      Toggle("Lab mode", isOn: $labMode)
        .toggleStyle(.switch)
        .font(CSFont.ui(13, .medium))

      Button("Open Voice Lab") {
        VoiceLabRuntime.openConsole()
      }
      .font(CSFont.mono(11, .semibold))
      .foregroundStyle(CSColor.chromeAccent)
    }
    .padding(28)
    .frame(maxWidth: .infinity, alignment: .leading)
  }
}
