import SwiftUI

/// Quiet corner mark for active Voice Lab, never merely for a developer build.
struct DeveloperPowerMark: View {
  @AppStorage(DictationOverlayGate.labModeDefaultsKey) private var labMode = false

  var body: some View {
    if DeveloperSurface.isPowerModeEnabled(labMode: labMode) {
      Text(DeveloperSurface.powerModeCaption)
        .font(CSFont.mono(10, .medium))
        .tracking(0.2)
        .foregroundStyle(CSColor.terracottaLight)
        .opacity(0.72)
        .allowsHitTesting(false)
        .accessibilityIdentifier("developer-power-mark")
    }
  }
}

extension View {
  func developerPowerCorner(padding: CGFloat = 10) -> some View {
    overlay(alignment: .bottomTrailing) {
      DeveloperPowerMark()
        .padding(.trailing, padding)
        .padding(.bottom, padding)
    }
  }
}
