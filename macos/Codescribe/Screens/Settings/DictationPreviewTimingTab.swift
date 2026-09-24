import SwiftUI

/// Dictation › Preview timing: overlay pacing, written to the existing promoted
/// keys. The preset picker and the fine-tune sliders share one card — the tab
/// has the room, so the sliders no longer hide behind an "Advanced" disclosure.
struct DictationPreviewTimingTab: View {
  @ObservedObject var model: SettingsViewModel

  var body: some View {
    let values = model.previewTimingConfiguration.values
    VStack(alignment: .leading, spacing: 10) {
      Picker("Preview timing preset", selection: $model.previewPresetPicker) {
        ForEach(PreviewTimingPreset.allCases) { preset in
          Text(preset.rawValue).tag(preset)
        }
      }
      .pickerStyle(.segmented)
      .labelsHidden()

      Text(previewSummary(values))
        .font(CSFont.mono(10.5, .medium))
        .foregroundStyle(CSColor.textFaint)

      Text("Advanced")
        .font(CSFont.ui(12.5, .semibold))
        .foregroundStyle(CSColor.textBody)
        .padding(.top, CSSpace.xs)

      VStack(spacing: 8) {
        PreviewTimingSlider(
          title: "Buffer delay",
          value: $model.bufferDelaySlider,
          range: 0...1500,
          step: 1,
          valueLabel: "\(values.bufferDelayMs) ms"
        )
        PreviewTimingSlider(
          title: "Typing speed",
          value: $model.typingCpsSlider,
          range: 5...180,
          step: 0.1,
          valueLabel: "\(values.typingCps.formatted(Self.oneDecimal)) cps"
        )
        PreviewTimingSlider(
          title: "Words per tick",
          value: $model.emitWordsSlider,
          range: 1...10,
          step: 1,
          valueLabel: "\(values.emitWordsMax)"
        )
        PreviewTimingSlider(
          title: "Interim cadence",
          value: $model.interimSecondsSlider,
          range: 1...30,
          step: 0.1,
          valueLabel: "\(values.interimSeconds.formatted(Self.oneDecimal)) s"
        )
      }
    }
    .csSettingsCard()
  }

  private func previewSummary(_ values: PreviewTimingValues) -> String {
    guard model.previewTimingConfiguration.overlayEnabled else {
      return "Preview off · committed transcripts are unchanged"
    }
    return "\(values.bufferDelayMs) ms · \(values.typingCps.formatted(Self.oneDecimal)) cps · "
      + "\(values.emitWordsMax) words · \(values.interimSeconds.formatted(Self.oneDecimal)) s interim"
  }

  private static let oneDecimal = FloatingPointFormatStyle<Float>.number
    .precision(.fractionLength(1))
}
