import AppKit
import SwiftUI

/// Appearance-aware physical sheet. Window interaction is owned by explicit
/// inert regions in `DictationOverlayView`, not by a full-sheet AppKit layer.
struct OverlayCanvasSurface<Content: View>: View {
  @Environment(\.accessibilityReduceTransparency) private var reduceTransparency
  @Environment(\.colorSchemeContrast) private var contrast
  let palette: OverlayAppearancePalette
  @ViewBuilder let content: Content

  var body: some View {
    content
      .background {
        OverlayCanvasBackdrop(
          palette: palette, reduceTransparency: reduceTransparency,
          increasedContrast: contrast == .increased)
      }
      .clipShape(RoundedRectangle(cornerRadius: CSRadius.window, style: .continuous))
      .overlay {
        RoundedRectangle(cornerRadius: CSRadius.window, style: .continuous)
          .strokeBorder(palette.border.color, lineWidth: 1)
          .allowsHitTesting(false)
      }
      .shadow(
        color: .black.opacity(palette.shadowOpacity),
        radius: 20,
        x: 0,
        y: 9
      )
  }

}

struct OverlayCanvasBackdrop: View {
  let palette: OverlayAppearancePalette
  let reduceTransparency: Bool
  var increasedContrast = false

  var body: some View {
    ZStack {
      if reduceTransparency || increasedContrast {
        Rectangle().fill(palette.desktopBackground.color)
      } else {
        OverlayDesktopMaterial(palette: palette)
      }
    }
  }
}

/// Sample the desktop behind the non-activating panel, not its empty content.
/// Keep the material active while the user types into another application.
private struct OverlayDesktopMaterial: NSViewRepresentable {
  let palette: OverlayAppearancePalette

  func makeNSView(context: Context) -> NSView {
    if #available(macOS 26, *) {
      let view = OverlayDesktopGlassView()
      view.style = .regular
      view.cornerRadius = CSRadius.window
      applyAppearance(to: view)
      return view
    }
    let view = OverlayDesktopEffectView()
    view.material = .hudWindow
    view.blendingMode = .behindWindow
    view.state = .active
    applyAppearance(to: view)
    return view
  }

  func updateNSView(_ nsView: NSView, context: Context) {
    applyAppearance(to: nsView)
  }

  private func applyAppearance(to view: NSView) {
    let name: NSAppearance.Name = palette.appearance == .dark ? .darkAqua : .aqua
    if view.appearance?.name != name {
      view.appearance = NSAppearance(named: name)
    }
    if #available(macOS 26, *), let glass = view as? OverlayDesktopGlassView {
      let tint = palette.desktopBackground.nsColor.withAlphaComponent(Self.glassTint)
      if glass.tintColor != tint { glass.tintColor = tint }
    }
  }

  /// A light veil of the palette's own sheet colour. 72 % (removed in
  /// 469b701b8, 2026-10-12) turned the glass into a solid sheet; bare glass
  /// read a little too transparent (Maciej, 2026-10-14).
  static let glassTint: CGFloat = 0.22
}

/// One stable background, independent of recording, editing, and toolbelt state.
@available(macOS 26, *)
final class OverlayDesktopGlassView: NSGlassEffectView {
  override func hitTest(_ point: NSPoint) -> NSView? { nil }
}

final class OverlayDesktopEffectView: NSVisualEffectView {
  override func hitTest(_ point: NSPoint) -> NSView? { nil }
}

/// Header and composer use the same system-owned control surface.
struct OverlayControlGlass: ViewModifier {
  @ViewBuilder
  func body(content: Content) -> some View {
    if #available(macOS 26.0, *) {
      content.glassEffect(.regular.interactive(), in: RoundedRectangle(cornerRadius: 20))
    } else {
      content.background(.regularMaterial, in: RoundedRectangle(cornerRadius: 20))
    }
  }
}
