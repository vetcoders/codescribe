import SwiftUI

/// Quiet corner mark for an org `make install-app` bake.
/// Hidden on production DMGs (`CSDeveloperSurface` off).
struct DeveloperPowerMark: View {
  var body: some View {
    if DeveloperSurface.isEnabled() {
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
