import AppKit
import SwiftUI

/// Developer-only Lab desk. Hidden unless `CSDeveloperSurface` is baked.
struct LabPanel: View {
  @ObservedObject var model: SettingsViewModel
  @AppStorage("codescribe.lab_mode") private var labMode = false

  var body: some View {
    VStack(alignment: .leading, spacing: 16) {
      SettingsPageHeader(
        String(localized: "Voice Lab"),
        blurb: labMode
          ? String(
            localized: "Lab mode is on. Overlay follows the tray toggle — Lab does not steal it.")
          : String(
            localized: "Open the loopback Voice Lab. Production builds never show this panel.")
      )

      Toggle("Lab mode", isOn: $labMode)
        .toggleStyle(.switch)
        .font(.body)

      sealLaneRow

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
        "Applies to Smart and Corrections from the next formatting pass. The on-device model runs first without a cloud credential; on failure the formatting provider is used when one is configured. Max always uses the Agent model. A .env value overrides this toggle."
      )
      .font(.caption)
      .foregroundStyle(.secondary)

      Button("Open Voice Lab") {
        Task { await VoiceLabRuntime.shared.openConsole() }
      }
      .font(CSFont.mono(11, .semibold))
      .foregroundStyle(CSColor.chromeAccent)

      recognitionParameters
    }
    .padding(CSSpace.xl)
    .frame(maxWidth: .infinity, alignment: .leading)
    .task { await model.refreshAdmission() }
  }

  /// The committing lane. Settings › Audio states the effective value and
  /// nothing else: turning the recorder's own precondition off is a power-user
  /// act, so the one switch that can do it lives on this desk. An env override
  /// still wins, and the switch then says so instead of lying about authority.
  @ViewBuilder
  private var sealLaneRow: some View {
    let sealLane = sealLaneControlState(model.admission)
    Toggle("Committing transcript fragments", isOn: sealLaneBinding)
      .toggleStyle(.switch)
      .font(.body)
      .tint(CSColor.chromeAccent)
      .disabled(!sealLane.isEnabled)
      .accessibilityValue(sealLaneAccessibilityValue(sealLane))
      .accessibilityIdentifier("lab-committing-transcript-fragments")
    Text("Required before a recording can start.")
      .font(.caption)
      .foregroundStyle(.secondary)
    if !sealLane.isEnabled {
      Text(sealLane.detail)
        .font(.caption)
        .foregroundStyle(.secondary)
        .fixedSize(horizontal: false, vertical: true)
    }
  }

  private var sealLaneBinding: Binding<Bool> {
    Binding(
      get: { sealLaneControlState(model.admission).isOn },
      set: { armed in
        model.setSealLaneArmed(armed)
        Task { await model.refreshAdmission() }
      }
    )
  }

  private func sealLaneAccessibilityValue(_ state: SealLaneControlState) -> String {
    let value =
      state.isOn
      ? String(localized: "On", comment: "Toggle state")
      : String(localized: "Off", comment: "Toggle state")
    return state.isEnabled
      ? value
      : String(
        localized: "\(value), overridden",
        comment: "VoiceOver value: a toggle state frozen by an override")
  }

  /// Experimental recognition parameters. The speech-pause limit is a
  /// product control in Agent settings; these engine timings stay in Lab.
  private var recognitionParameters: some View {
    VStack(alignment: .leading, spacing: 10) {
      SettingsSectionLabel(
        String(
          localized: "Speech recognition parameters",
          comment: "Lab group: raw recognition timings"))

      VStack(alignment: .leading, spacing: 14) {
        SettingsSecondsSlider(
          title: String(localized: "Whisper context length"),
          detail: String(
            localized:
              "How much of the recording the local Whisper model sees when it refines the transcript. Refinement runs in Local power and Cloud, never in Apple only."
          ),
          seconds: model.settings.whisperContextWindowSec,
          value: $model.whisperContextWindowSlider,
          range: 0.5...10,
          step: 0.5
        )

        SettingsSecondsSlider(
          title: String(localized: "Sentence pause"),
          detail: String(
            localized:
              "A longer gap in speech may be treated as a sentence boundary during automatic text formatting."
          ),
          seconds: model.settings.lightPlusSentencePauseSec,
          value: $model.lightPlusSentencePauseSlider,
          range: 0.3...2.0,
          step: 0.1
        )

        Text(
          "This build decodes Whisper refinement on a fixed capture-clock window, so the context length is stored but not read."
        )
        .font(CSFont.ui(11))
        .foregroundStyle(Color.secondary)
        .fixedSize(horizontal: false, vertical: true)
      }
      .settingsGroupedInset()
    }
  }
}
