import AppKit
import SwiftUI

/// Native macOS sidebar material behind every window's sidebar column.
///
/// The column keeps the system's glass so it follows the transparency the
/// user chose in macOS. A flat palette tone replaced it for one candidate and
/// lost that effect; its tint and the seam beside the content are separate
/// questions from whether the glass is there (Monika, 2026-10-18).
///
/// `CSSidebarColumn` and the transcript actions column share this material.
/// A window never reaches for it on its own.
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
private struct CSReduceTransparencyOverrideKey: EnvironmentKey {
  static let defaultValue: Bool? = nil
}

private struct CSContrastOverrideKey: EnvironmentKey {
  static let defaultValue: ColorSchemeContrast? = nil
}

extension EnvironmentValues {
  var csReduceTransparencyOverride: Bool? {
    get { self[CSReduceTransparencyOverrideKey.self] }
    set { self[CSReduceTransparencyOverrideKey.self] = newValue }
  }

  var csContrastOverride: ColorSchemeContrast? {
    get { self[CSContrastOverrideKey.self] }
    set { self[CSContrastOverrideKey.self] = newValue }
  }
}

struct CSSidebarMaterial: View {
  @Environment(\.accessibilityReduceTransparency) private var reduceTransparency
  @Environment(\.colorSchemeContrast) private var contrast
  @Environment(\.csReduceTransparencyOverride) private var envReduceTransparencyOverride
  @Environment(\.csContrastOverride) private var envContrastOverride

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
    overrideReduceTransparency ?? envReduceTransparencyOverride ?? reduceTransparency
  }

  var isIncreasedContrast: Bool {
    (overrideContrast ?? envContrastOverride ?? contrast) == .increased
  }

  var body: some View {
    if isReducedTransparency || isIncreasedContrast {
      Color(nsColor: .windowBackgroundColor)
        .accessibilityHidden(true)
    } else {
      CSSidebarVisualEffectView()
        .accessibilityHidden(true)
    }
  }
}

/// AppKit hosting for the semantic sidebar visual effect view.
struct CSSidebarVisualEffectView: NSViewRepresentable {
  func makeNSView(context: Context) -> CSSidebarVisualEffectNSView {
    let view = CSSidebarVisualEffectNSView()
    view.material = .sidebar
    view.blendingMode = .behindWindow
    view.state = .followsWindowActiveState
    return view
  }

  func updateNSView(_ view: CSSidebarVisualEffectNSView, context: Context) {
    view.material = .sidebar
    view.blendingMode = .behindWindow
    view.state = .followsWindowActiveState
  }
}

/// Backing `NSVisualEffectView` subclass that drops mouse hits to preserve
/// full hit-testing reachability for the SwiftUI list and footer controls.
final class CSSidebarVisualEffectNSView: NSVisualEffectView {
  override func hitTest(_ point: NSPoint) -> NSView? {
    nil
  }
}
