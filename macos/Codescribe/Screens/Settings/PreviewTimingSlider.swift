import SwiftUI

/// One labelled preview-timing slider: title and live value above the track.
struct PreviewTimingSlider: View {
  let title: String
  @Binding var value: Double
  let range: ClosedRange<Double>
  let step: Double
  let valueLabel: String

  var body: some View {
    VStack(alignment: .leading, spacing: 6) {
      HStack {
        Text(title)
          .font(CSFont.ui(12, .medium))
          .foregroundStyle(CSColor.textMutedAlt)
        Spacer(minLength: 0)
        Text(valueLabel)
          .font(CSFont.mono(10.5, .semibold))
          .foregroundStyle(CSColor.textBody)
      }
      Slider(value: $value, in: range, step: step)
        .tint(CSColor.chromeAccent)
        .accessibilityLabel(title)
        .accessibilityValue(valueLabel)
    }
  }
}
