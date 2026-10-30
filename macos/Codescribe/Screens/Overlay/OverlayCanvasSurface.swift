import AppKit
import SwiftUI

/// Which glass a canvas wears. The open window keeps its near-black sheet;
/// the bar (mini and the expanded bar) wears the hover preview's lighter
/// glass, so the app speaks one glass language (Monika, 2026-10-30).
enum OverlayCanvasGlass: Equatable {
  case sheet
  case popover
}

/// Appearance-aware physical sheet. Window interaction is owned by explicit
/// inert regions in `DictationOverlayView`, not by a full-sheet AppKit layer.
struct OverlayCanvasSurface<Content: View>: View {
  @Environment(\.accessibilityReduceTransparency) private var reduceTransparency
  @Environment(\.colorSchemeContrast) private var contrast
  @Environment(\.csReduceTransparencyOverride) private var reduceTransparencyOverride
  @Environment(\.csContrastOverride) private var contrastOverride
  let palette: OverlayAppearancePalette
  var glass: OverlayCanvasGlass = .sheet
  @ViewBuilder let content: Content

  var body: some View {
    content
      .background {
        OverlayCanvasBackdrop(
          palette: palette,
          reduceTransparency: reduceTransparencyOverride ?? reduceTransparency,
          increasedContrast: (contrastOverride ?? contrast) == .increased,
          glass: glass)
      }
      .clipShape(RoundedRectangle(cornerRadius: CSRadius.window, style: .continuous))
      .overlay {
        RoundedRectangle(cornerRadius: CSRadius.window, style: .continuous)
          .strokeBorder(
            glass == .popover ? OverlayPopoverGlass.edge(palette) : palette.border.color,
            lineWidth: OverlayPopoverGlass.edgeWidth
          )
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
  @Environment(\.colorSchemeContrast) private var contrast
  @Environment(\.csContrastOverride) private var contrastOverride
  @Environment(\.csReduceTransparencyOverride) private var reduceTransparencyOverride
  let palette: OverlayAppearancePalette
  let reduceTransparency: Bool
  var increasedContrast = false
  var cornerRadius: CGFloat = CSRadius.window
  var glass: OverlayCanvasGlass = .sheet

  var body: some View {
    ZStack {
      if (reduceTransparencyOverride ?? reduceTransparency) || increasedContrast
        || (contrastOverride ?? contrast) == .increased
      {
        Rectangle().fill(palette.desktopBackground.color)
      } else if glass == .popover {
        OverlayPopoverMaterial(palette: palette, cornerRadius: cornerRadius)
        Rectangle()
          .fill(OverlayPopoverGlass.veil(palette, opacity: OverlayPopoverGlass.barVeilOpacity))
          .allowsHitTesting(false)
          .accessibilityHidden(true)
      } else {
        OverlayDesktopMaterial(palette: palette, cornerRadius: cornerRadius)
        Rectangle()
          .fill(Color(nsColor: Self.glassVeil(palette)))
          .allowsHitTesting(false)
          .accessibilityHidden(true)
      }
    }
  }

  /// The shared sheet colour over the native glass: near-black on ink,
  /// paper on paper, with the desktop left as a faint blurred hint (Maciej,
  /// 2026-10-15: "bardziej czarno", "dark almost black"). The 22 % veil of
  /// 2026-10-14 still took the page's brightness; the macOS Liquid Glass slider
  /// does not darken it.
  ///
  /// The veil is a SwiftUI layer above the glass, not the glass's `tintColor`:
  /// Liquid Glass drops its tint and brightens when the window is not key, so a
  /// tinted sheet read #09 active and #50 inactive on the same wizard (Maciej's
  /// screenshots, 2026-10-15 17:36). A layer above the glass keeps 84 % of the
  /// sheet constant in both states; only the 16 % of desktop hint still follows
  /// the window.
  static func glassVeil(_ palette: OverlayAppearancePalette) -> NSColor {
    let alpha: CGFloat = palette.appearance == .dark && BuildChannel.isNightly() ? 0.92 : 0.84
    return palette.desktopBackground.nsColor.withAlphaComponent(alpha)
  }
}

/// The hover preview's glass (Nina, candidate 16), shared with the bar so the
/// app has one glass language: the native popover material behind the
/// window, a veil in the palette's sheet colour above it, a 1 pt edge in the
/// text colour and a shadow. `OverlayHintPanel` and the bar's canvas both
/// read it; neither keeps its own copy.
@MainActor
enum OverlayPopoverGlass {
  static let material: NSVisualEffectView.Material = .popover
  /// The hover preview's veil: a hint over the desktop.
  static let previewVeilOpacity: Double = 0.28
  /// The bar's veil. Digits and names must read over any desktop, so the
  /// bar takes a stronger veil than the preview; still lighter than the open
  /// window's 84 % sheet (`OverlayCanvasBackdrop.glassVeil`).
  static let barVeilOpacity: Double = 0.56
  static let edgeWidth: CGFloat = 1

  static func veil(_ palette: OverlayAppearancePalette, opacity: Double) -> Color {
    palette.desktopBackground.color.opacity(opacity)
  }

  static func edge(_ palette: OverlayAppearancePalette) -> Color {
    palette.primaryText.color.opacity(palette.appearance == .dark ? 0.20 : 0.14)
  }

  /// Native effect views blur behind the window outside SwiftUI's clip; a
  /// mask gives the blur the same rounded corner as its shape.
  static func roundedMask(radius: CGFloat) -> NSImage {
    let edge = radius * 2 + 1
    let image = NSImage(size: NSSize(width: edge, height: edge), flipped: false) { rect in
      NSColor.black.setFill()
      NSBezierPath(roundedRect: rect, xRadius: radius, yRadius: radius).fill()
      return true
    }
    image.capInsets = NSEdgeInsets(top: radius, left: radius, bottom: radius, right: radius)
    image.resizingMode = .stretch
    return image
  }

  /// Configures an effect view as the popover glass.
  static func apply(to view: NSVisualEffectView, cornerRadius: CGFloat) {
    view.material = material
    view.blendingMode = .behindWindow
    view.state = .active
    view.maskImage = roundedMask(radius: cornerRadius)
  }
}

/// The bar's backdrop: the popover glass of `OverlayPopoverGlass`.
private struct OverlayPopoverMaterial: NSViewRepresentable {
  let palette: OverlayAppearancePalette
  let cornerRadius: CGFloat

  func makeNSView(context: Context) -> OverlayDesktopEffectView {
    let view = OverlayDesktopEffectView()
    OverlayPopoverGlass.apply(to: view, cornerRadius: cornerRadius)
    view.appearance = NSAppearance(named: palette.appearance == .dark ? .darkAqua : .aqua)
    return view
  }

  func updateNSView(_ view: OverlayDesktopEffectView, context: Context) {
    let name: NSAppearance.Name = palette.appearance == .dark ? .darkAqua : .aqua
    if view.appearance?.name != name { view.appearance = NSAppearance(named: name) }
  }
}

/// Sample the desktop behind the non-activating panel, not its empty content.
/// Keep the material active while the user types into another application.
private struct OverlayDesktopMaterial: NSViewRepresentable {
  let palette: OverlayAppearancePalette
  let cornerRadius: CGFloat

  func makeNSView(context: Context) -> NSView {
    if #available(macOS 26, *) {
      let view = OverlayDesktopGlassView()
      view.style = .regular
      view.cornerRadius = cornerRadius
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
    if #available(macOS 26, *), let glass = nsView as? OverlayDesktopGlassView,
      glass.cornerRadius != cornerRadius
    {
      glass.cornerRadius = cornerRadius
    }
    applyAppearance(to: nsView)
  }

  private func applyAppearance(to view: NSView) {
    let name: NSAppearance.Name = palette.appearance == .dark ? .darkAqua : .aqua
    if view.appearance?.name != name {
      view.appearance = NSAppearance(named: name)
    }
    if #available(macOS 26, *), let glass = view as? OverlayDesktopGlassView,
      glass.tintColor != nil
    {
      // The sheet colour lives in `OverlayCanvasBackdrop.glassVeil` above the
      // glass; a tint here would vanish whenever the window is not key.
      glass.tintColor = nil
    }
  }
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
///
/// A surface flush with the window's own edges must carry the window's corner,
/// not a pill's: the header spans the full width at the very top, so a
/// circular pill corner sat inside the canvas's `CSRadius.window` continuous
/// clip and the two curves read as a cut-off right end (Founder, 2026-10-18).
/// Floating composers pass their dedicated 12 pt corner, so their curve stays
/// the same as the editor grows. Both surfaces use the canvas's continuous style.
struct OverlayControlGlass: ViewModifier {
  var cornerRadius: CGFloat = CSRadius.window
  var isEnabled = true

  @ViewBuilder
  func body(content: Content) -> some View {
    let shape = RoundedRectangle(cornerRadius: cornerRadius, style: .continuous)
    if #available(macOS 26.0, *) {
      content.glassEffect(isEnabled ? .regular.interactive() : .identity, in: shape)
    } else {
      content.background {
        if isEnabled { shape.fill(.regularMaterial) }
      }
    }
  }
}
