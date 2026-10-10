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
        "Applies to the next formatting pass. The on-device model runs first; any failure falls back to the cloud provider. A .env value overrides this toggle."
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

  /// Raw recognition timings. They used to be a Dictation tab; they are
  /// parameters, not product choices, so Lab is their only surface. The
  /// bindings and the promoted keys behind them are unchanged.
  private var recognitionParameters: some View {
    VStack(alignment: .leading, spacing: 10) {
      SettingsSectionLabel(
        String(
          localized: "Speech recognition parameters",
          comment: "Lab group: raw recognition timings"))

      VStack(alignment: .leading, spacing: 14) {
        LabSecondsSlider(
          title: String(localized: "Pause recognition after silence"),
          detail: String(
            localized:
              "After this much silence the Apple engine goes idle. Recognition resumes when you start speaking again."
          ),
          seconds: model.settings.toggleSilenceSec,
          value: $model.toggleSilenceSlider,
          range: 0.5...30,
          step: 0.5
        )

        LabSecondsSlider(
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

        LabSecondsSlider(
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

/// One labelled seconds slider: title, the one line that says what it changes,
/// and the live value. Shared shape for the Lab recognition parameters.
private struct LabSecondsSlider: View {
  let title: String
  let detail: String
  /// The persisted value, so the label shows what the core accepted.
  let seconds: Float
  @Binding var value: Double
  let range: ClosedRange<Double>
  let step: Double

  var body: some View {
    VStack(alignment: .leading, spacing: 6) {
      HStack(alignment: .firstTextBaseline, spacing: 12) {
        VStack(alignment: .leading, spacing: 2) {
          Text(title)
            .font(CSFont.ui(13, .semibold))
            .foregroundStyle(Color.primary)
          Text(detail)
            .font(CSFont.ui(11.5))
            .foregroundStyle(Color.secondary)
            .fixedSize(horizontal: false, vertical: true)
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        Text(valueLabel)
          .font(CSFont.mono(11, .semibold))
          .foregroundStyle(Color.primary)
      }
      Slider(value: $value, in: range, step: step)
        .tint(CSColor.chromeAccent)
        .accessibilityLabel(title)
        .accessibilityValue(valueLabel)
    }
  }

  /// Seconds without a trailing zero: `5 s`, `1.5 s` — never `5.0 s`.
  private var valueLabel: String {
    String(
      localized: "\(seconds, format: Self.seconds) s",
      comment: "Slider value in seconds. The placeholder is the number; keep the unit abbreviated")
  }

  private static let seconds = FloatingPointFormatStyle<Float>.number
    .precision(.fractionLength(0...1))
}
