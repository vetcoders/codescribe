import SwiftUI

/// Sidebar canvas for the Settings and Agent windows.
///
/// One flat palette tone, `CSColor.sidebar`, a single step away from the
/// detail sheet. A `.sidebar` `NSVisualEffectView` blending `.behindWindow`
/// took its colour from the desktop instead, so the column read grey-blue over
/// one wallpaper and grey over another, and the vertical seam beside the
/// near-black content belonged to neither window (Maciej, 2026-10-18). The
/// flat tone is adaptive, so it steps the same way in light and in dark, and
/// it needs no `reduceTransparency` or `increaseContrast` fallback: there is
/// no translucency left to reduce.
///
/// Passively backs navigation: it ignores hit tests so all pointer and
/// keyboard events reach sidebar list rows, search fields and footer controls.
struct SettingsSidebarMaterial: View {
  var body: some View {
    CSColor.sidebar
      .allowsHitTesting(false)
      .accessibilityHidden(true)
  }
}
