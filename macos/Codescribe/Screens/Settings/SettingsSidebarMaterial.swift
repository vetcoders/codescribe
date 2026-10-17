import AppKit
import SwiftUI

/// Native macOS sidebar material surface for the Settings window.
///
/// Follows the native navigation material model:
/// - In standard mode: an `NSVisualEffectView` with `.sidebar` material,
///   `.behindWindow` blending mode and `.followsWindowActiveState` state.
/// - Under `reduceTransparency` accessibility preference: renders an opaque
///   system-consistent sidebar canvas (`NSColor.windowBackgroundColor`).
/// - Under `increaseContrast` (`colorSchemeContrast == .increased`): renders an
///   opaque high-contrast canvas.
/// - Passively backs navigation: ignores hit tests so all pointer and keyboard
///   events reach sidebar list rows, search fields, and footer controls.
struct SettingsSidebarMaterial: View {
  @Environment(\.accessibilityReduceTransparency) private var reduceTransparency
  @Environment(\.colorSchemeContrast) private var contrast

  var overrideReduceTransparency: Bool?
  var overrideContrast: ColorSchemeContrast?

  init(
    reduceTransparency: Bool? = nil,
    contrast: ColorSchemeContrast? = nil
  ) {
    self.overrideReduceTransparency = reduceTransparency
    self.overrideContrast = contrast
  }

  static func shouldReduceTransparency(
    reduceTransparency: Bool,
    contrast: ColorSchemeContrast
  ) -> Bool {
    reduceTransparency || contrast == .increased
  }

  var isReducedTransparency: Bool {
    overrideReduceTransparency ?? reduceTransparency
  }

  var isIncreasedContrast: Bool {
    (overrideContrast ?? contrast) == .increased
  }

  var body: some View {
    if isReducedTransparency || isIncreasedContrast {
      Color(nsColor: .windowBackgroundColor)
        .accessibilityHidden(true)
    } else {
      SettingsSidebarVisualEffectView()
        .accessibilityHidden(true)
    }
  }
}

/// AppKit hosting for the semantic sidebar visual effect view.
struct SettingsSidebarVisualEffectView: NSViewRepresentable {
  func makeNSView(context: Context) -> SettingsSidebarVisualEffectNSView {
    let view = SettingsSidebarVisualEffectNSView()
    view.material = .sidebar
    view.blendingMode = .behindWindow
    view.state = .followsWindowActiveState
    return view
  }

  func updateNSView(_ view: SettingsSidebarVisualEffectNSView, context: Context) {
    view.material = .sidebar
    view.blendingMode = .behindWindow
    view.state = .followsWindowActiveState
  }
}

/// Backing `NSVisualEffectView` subclass that drops mouse hits to preserve
/// full hit-testing reachability for the SwiftUI list and footer controls.
final class SettingsSidebarVisualEffectNSView: NSVisualEffectView {
  override func hitTest(_ point: NSPoint) -> NSView? {
    nil
  }
}
