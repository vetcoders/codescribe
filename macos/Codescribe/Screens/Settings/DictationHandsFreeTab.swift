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
            .foregroundStyle(Color.primary)
          Text(
            "Rest the Apple engine after this much silence; the next speech edge wakes a fresh epoch so Whisper can patch the sealed span"
          )
          .font(CSFont.ui(11.5))
          .foregroundStyle(Color.secondary)
        }
        Spacer(minLength: 12)
        Text(verbatim: "\(model.settings.toggleSilenceSec.formatted(Self.oneDecimal)) s")
          .font(CSFont.mono(11, .semibold))
          .foregroundStyle(Color.primary)
      }
      Slider(value: $model.toggleSilenceSlider, in: 0.5...30, step: 0.5)
        .tint(CSColor.chromeAccent)
        .accessibilityLabel("Hands-free silence duration")
        .accessibilityValue(
          Text("\(model.settings.toggleSilenceSec, format: Self.oneDecimal) seconds"))

      HStack {
        VStack(alignment: .leading, spacing: 4) {
          Text("Whisper context")
            .font(CSFont.ui(13, .semibold))
            .foregroundStyle(Color.primary)
          Text(
            "How many seconds of audio Whisper hears with each fragment. A shorter window is faster, but a short ending can be lost."
          )
          .font(CSFont.ui(11.5))
          .foregroundStyle(Color.secondary)
        }
        Spacer(minLength: 12)
        Text(
          verbatim:
            "\(model.settings.whisperContextWindowSec.formatted(Self.oneDecimal)) s"
        )
        .font(CSFont.mono(11, .semibold))
        .foregroundStyle(Color.primary)
      }
      Slider(value: $model.whisperContextWindowSlider, in: 0.5...10, step: 0.5)
        .tint(CSColor.chromeAccent)
        .accessibilityLabel("Whisper context window")
        .accessibilityValue(
          Text("\(model.settings.whisperContextWindowSec, format: Self.oneDecimal) seconds"))

      HStack {
        VStack(alignment: .leading, spacing: 4) {
          Text("Light+ sentence pause")
            .font(CSFont.ui(13, .semibold))
            .foregroundStyle(Color.primary)
          Text("A longer gap in speech opens a new sentence in pasted dictation.")
            .font(CSFont.ui(11.5))
            .foregroundStyle(Color.secondary)
        }
        Spacer(minLength: 12)
        Text(
          verbatim:
            "\(model.settings.lightPlusSentencePauseSec.formatted(Self.oneDecimal)) s"
        )
        .font(CSFont.mono(11, .semibold))
        .foregroundStyle(Color.primary)
      }
      Slider(value: $model.lightPlusSentencePauseSlider, in: 0.3...2.0, step: 0.1)
        .tint(CSColor.chromeAccent)
        .accessibilityLabel("Light+ sentence pause")
        .accessibilityValue(
          Text("\(model.settings.lightPlusSentencePauseSec, format: Self.oneDecimal) seconds"))
    }
    .settingsGroupedInset()
  }

  private static let oneDecimal = FloatingPointFormatStyle<Float>.number
    .precision(.fractionLength(1))
}
