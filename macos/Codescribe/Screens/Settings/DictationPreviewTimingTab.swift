import SwiftUI

/// Dictation › Preview: how fast transcribed text appears in the preview
/// window while recording. The preset picker answers the everyday question and
/// says in words what the choice means; the four numeric knobs behind it are
/// the Custom editor and stay folded away until Custom is the choice.
struct DictationPreviewTimingTab: View {
  @ObservedObject var model: SettingsViewModel
  @State private var detailsExpanded = false

  var body: some View {
    let configuration = model.previewTimingConfiguration
    let values = configuration.values
    let selectedPreset = model.previewPresetPicker
    let previewEnabled = configuration.overlayEnabled

    VStack(alignment: .leading, spacing: 10) {
      Picker("Transcript display pace", selection: $model.previewPresetPicker) {
        ForEach(PreviewTimingPreset.allCases) { preset in
          Text(preset.displayName).tag(preset)
        }
      }
      .pickerStyle(.segmented)
      .labelsHidden()

      Text(selectedPreset.summary)
        .font(CSFont.ui(11.5))
        .foregroundStyle(Color.secondary)
        .fixedSize(horizontal: false, vertical: true)

      // Collapsed by default: the preset above is the everyday answer. Opened
      // when Custom is the choice, because then these four are the setting.
      DisclosureGroup(isExpanded: $detailsExpanded) {
        VStack(alignment: .leading, spacing: 8) {
          // The note stays outside the disabled scope so it is still readable
          // while the controls it explains are not interactive.
          if !previewEnabled {
            Text("These values take effect again once the preview is back on.")
              .font(CSFont.ui(11))
              .foregroundStyle(Color.secondary)
              .fixedSize(horizontal: false, vertical: true)
          }
          slidersGroup(values: values, enabled: previewEnabled)
        }
        .padding(.top, CSSpace.xs)
      } label: {
        SettingsSectionLabel(String(localized: "Detailed settings"))
      }
      .padding(.top, CSSpace.xs)
      .accessibilityIdentifier("preview-timing-details")
    }
    .settingsGroupedInset()
    .onAppear {
      if selectedPreset == .custom { detailsExpanded = true }
    }
    .onChange(of: model.previewPresetPicker) { _, selected in
      if selected == .custom { detailsExpanded = true }
    }
  }

  /// The Custom editor. Disabled as a group while the preview is off: the four
  /// values stay on disk, but nothing on screen would react to a change.
  private func slidersGroup(values: PreviewTimingValues, enabled: Bool) -> some View {
    VStack(alignment: .leading, spacing: 8) {
      PreviewTimingSlider(
        title: String(
          localized: "Update delay",
          comment: "Preview timing: how long the preview waits before showing new text"),
        value: $model.bufferDelaySlider,
        range: 0...1500,
        step: 1,
        valueLabel: Self.milliseconds(values.bufferDelayMs)
      )
      PreviewTimingSlider(
        title: String(
          localized: "Character pace",
          comment: "Preview timing: how fast characters appear, in characters per second"),
        value: $model.typingCpsSlider,
        range: 5...180,
        step: 0.1,
        valueLabel: Self.charactersPerSecond(values.typingCps)
      )
      PreviewTimingSlider(
        title: String(
          localized: "Max words per update",
          comment: "Preview timing: most words the preview adds in one update"),
        value: $model.emitWordsSlider,
        range: 1...10,
        step: 1,
        valueLabel: Self.words(values.emitWordsMax)
      )
      PreviewTimingSlider(
        title: String(
          localized: "Interim result interval",
          comment: "Preview timing: how often speech recognition produces a provisional result"),
        value: $model.interimSecondsSlider,
        range: 1...30,
        step: 0.1,
        valueLabel: Self.seconds(values.interimSeconds)
      )
    }
    .disabled(!enabled)
  }

  private static func milliseconds(_ value: UInt64) -> String {
    String(
      localized: "\(Int(value)) ms",
      comment: "Preview timing value: a delay in milliseconds")
  }

  private static func charactersPerSecond(_ value: Float) -> String {
    String(
      localized: "\(value.formatted(Self.upToOneDecimal)) cps",
      comment: "Preview timing value: characters per second. The placeholder is a formatted number")
  }

  private static func words(_ value: UInt64) -> String {
    String(
      localized: "\(Int(value)) words",
      comment: "Preview timing value: a number of words. Needs plural variations")
  }

  private static func seconds(_ value: Float) -> String {
    String(
      localized: "\(value.formatted(Self.upToOneDecimal)) s",
      comment: "Preview timing value: an interval in seconds. The placeholder is a formatted number"
    )
  }

  /// One decimal where it matters, none on a whole value, and the locale's own
  /// decimal separator — the sliders step by 0.1, so the label has to be able
  /// to show it.
  private static let upToOneDecimal = FloatingPointFormatStyle<Float>.number
    .precision(.fractionLength(0...1))
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
        localized: "settings.previewTiming.preset.off", defaultValue: "No preview",
        comment: "Preview timing preset: no preview window appears while recording")
    case .custom: String(localized: "Custom")
    }
  }

  /// One line under the picker: what this choice means, in place of the raw
  /// numbers. The numbers stay available under Detailed settings.
  var summary: String {
    switch self {
    case .smooth:
      String(localized: "An even pace, close to comfortable reading speed.")
    case .snappy:
      String(localized: "Text appears almost as soon as it is recognized.")
    case .relaxed:
      String(localized: "A calmer pace, with longer pauses between updates.")
    case .off:
      String(localized: "No preview window appears while recording.")
    case .custom:
      String(localized: "Your own values — edit them under Detailed settings.")
    }
  }
}
