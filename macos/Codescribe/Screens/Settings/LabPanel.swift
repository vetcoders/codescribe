import AppKit
import SwiftUI

/// Developer-only Lab desk. Hidden unless `CSDeveloperSurface` is baked.
struct LabPanel: View {
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
