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
          Text(preset.displayName).tag(preset)
        }
      }
      .pickerStyle(.segmented)
      .labelsHidden()

      Text(previewSummary(values))
        .font(CSFont.mono(10.5, .medium))
        .foregroundStyle(Color.secondary)

      Text("Advanced")
        .font(CSFont.ui(12.5, .semibold))
        .foregroundStyle(Color.primary)
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
    .settingsGroupedInset()
  }

  private func previewSummary(_ values: PreviewTimingValues) -> String {
    guard model.previewTimingConfiguration.overlayEnabled else {
      return String(localized: "Preview off · committed transcripts are unchanged")
    }
    return "\(values.bufferDelayMs) ms · \(values.typingCps.formatted(Self.oneDecimal)) cps · "
      + "\(values.emitWordsMax) words · \(values.interimSeconds.formatted(Self.oneDecimal)) s interim"
  }

  private static let oneDecimal = FloatingPointFormatStyle<Float>.number
    .precision(.fractionLength(1))
}

extension PreviewTimingPreset {
  /// Display name for the preset picker; `rawValue` stays the persisted identity.
  var displayName: String {
    switch self {
    case .smooth: String(localized: "Smooth")
    case .snappy: String(localized: "Snappy")
    case .relaxed: String(localized: "Relaxed")
    case .off:
      String(
        localized: "settings.previewTiming.preset.off", defaultValue: "Off",
        comment: "Preview timing preset: pacing is turned off")
    case .custom: String(localized: "Custom")
    }
  }
}
