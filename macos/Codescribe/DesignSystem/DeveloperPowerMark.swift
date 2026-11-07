import SwiftUI

/// Quiet mark for active Voice Lab, never merely for a developer build. It sits
/// in the Agent conversation header, beside the Context entry, so it stays
/// visible whether the context panel is open or closed and never competes with
/// the composer field.
struct DeveloperPowerMark: View {
  @AppStorage(DictationOverlayGate.labModeDefaultsKey) private var labMode = false

  var body: some View {
    if DeveloperSurface.isPowerModeEnabled(labMode: labMode) {
      Text(DeveloperSurface.powerModeCaption)
        .font(CSFont.mono(10, .medium))
        .tracking(0.2)
        // A mode label, not a listening signal: secondary ink, no extra veil
        // (the system rung is already translucent).
        .foregroundStyle(CSColor.secondaryInk)
        .lineLimit(1)
        .allowsHitTesting(false)
        .accessibilityIdentifier("developer-power-mark")
    }
  }
}
