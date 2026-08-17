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
          ? "Lab mode is on. Daily overlay stays off so you test against the bus and the PWA tape."
          : "Open the loopback Voice Lab. Production builds never show this panel."
      )
      .font(CSFont.ui(12.5))
      .foregroundStyle(CSColor.textMutedAlt)

      Toggle("Lab mode (overlay off)", isOn: $labMode)
        .toggleStyle(.switch)
        .font(CSFont.ui(13, .medium))
        .onChange(of: labMode) { _, on in
          if on {
            AppModel.shared.overlay.hide()
          }
        }

      Button("Open Voice Lab") {
        guard let url = URL(string: "http://127.0.0.1:8765/lab") else { return }
        NSWorkspace.shared.open(url)
      }
      .font(CSFont.mono(11, .semibold))
      .foregroundStyle(CSColor.chromeAccent)
    }
    .padding(28)
    .frame(maxWidth: .infinity, alignment: .leading)
  }
}
