import SwiftUI

/// One labelled preview-timing slider: title and live value above the track.
///
/// `title` and `valueLabel` arrive already localized — these four labels each
/// need a translator comment, which a `LocalizedStringKey` parameter cannot
/// carry — so both render verbatim here.
struct PreviewTimingSlider: View {
  let title: String
  @Binding var value: Double
  let range: ClosedRange<Double>
  let step: Double
  let valueLabel: String

  var body: some View {
    VStack(alignment: .leading, spacing: 6) {
      Text(title)
        .font(CSFont.ui(12, .medium))
        .foregroundStyle(CSColor.secondaryInk)
      // The track stops at `sliderWidth`; the value reads at its end.
      HStack(spacing: CSSpace.md) {
        Slider(value: $value, in: range, step: step)
          .tint(CSColor.chromeAccent)
          .frame(maxWidth: SettingsScale.sliderWidth)
          .accessibilityLabel(title)
          .accessibilityValue(valueLabel)
        Text(valueLabel)
          .font(CSFont.mono(10.5, .semibold))
          .foregroundStyle(Color.primary)
      }
    }
  }
}
