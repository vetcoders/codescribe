import SwiftUI

/// Dictation › Hands-free: the Apple engine silence epoch (TOGGLE_SILENCE_SEC).
struct DictationHandsFreeTab: View {
  @ObservedObject var model: SettingsViewModel

  var body: some View {
    VStack(alignment: .leading, spacing: 10) {
      HStack {
        VStack(alignment: .leading, spacing: 4) {
          Text("Hands-free silence")
            .font(CSFont.ui(13, .semibold))
            .foregroundStyle(CSColor.textBody)
          Text(
            "Rest the Apple engine after this much silence; the next speech edge wakes a fresh epoch so Whisper can patch the sealed span"
          )
          .font(CSFont.ui(11.5))
          .foregroundStyle(CSColor.textMutedAlt)
        }
        Spacer(minLength: 12)
        Text("\(model.settings.toggleSilenceSec, format: Self.oneDecimal) s")
          .font(CSFont.mono(11, .semibold))
          .foregroundStyle(CSColor.textBody)
      }
      Slider(value: $model.toggleSilenceSlider, in: 0.5...30, step: 0.5)
        .tint(CSColor.chromeAccent)
        .accessibilityLabel("Hands-free silence duration")
        .accessibilityValue(
          Text("\(model.settings.toggleSilenceSec, format: Self.oneDecimal) seconds"))
    }
    .csSettingsCard()
  }

  private static let oneDecimal = FloatingPointFormatStyle<Float>.number
    .precision(.fractionLength(1))
}
