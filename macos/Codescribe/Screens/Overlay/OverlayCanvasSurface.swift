import AppKit
import SwiftUI

/// Reading surfaces retain their quiet canvas; window chrome exposes the
/// native optical material without an additional opaque backing.
enum OverlayGlassRole {
  case readingCanvas
  case chrome
}

/// Appearance-aware physical sheet. Window interaction is owned by explicit
/// inert regions in `DictationOverlayView`, not by a full-sheet AppKit layer.
struct OverlayCanvasSurface<Content: View>: View {
  @Environment(\.accessibilityReduceTransparency) private var reduceTransparency
  @Environment(\.colorSchemeContrast) private var contrast
  @Environment(\.csReduceTransparencyOverride) private var reduceTransparencyOverride
  @Environment(\.csContrastOverride) private var contrastOverride
  let palette: OverlayAppearancePalette
  var glassRole: OverlayGlassRole = .readingCanvas
  @ViewBuilder let content: Content

  var body: some View {
    content
      .background {
        OverlayCanvasBackdrop(
          palette: palette,
          reduceTransparency: reduceTransparencyOverride ?? reduceTransparency,
          increasedContrast: (contrastOverride ?? contrast) == .increased,
          role: glassRole)
      }
      .clipShape(RoundedRectangle(cornerRadius: CSRadius.window, style: .continuous))
      .overlay {
        if glassRole == .readingCanvas {
          RoundedRectangle(cornerRadius: CSRadius.window, style: .continuous)
            .strokeBorder(palette.border.color, lineWidth: 1)
            .allowsHitTesting(false)
        }
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
  var role: OverlayGlassRole = .readingCanvas
  var tuningSurface: ChromeGlassSurface = .compactOverlay
  var glassPreferences = ChromeGlassPreferences()

  var body: some View {
    ZStack {
      if (reduceTransparencyOverride ?? reduceTransparency) || increasedContrast
        || (contrastOverride ?? contrast) == .increased
      {
        Rectangle().fill(palette.desktopBackground.color)
      } else {
        OverlayDesktopMaterial(
          palette: palette, cornerRadius: cornerRadius,
          tintColor: role == .chrome && glassPreferences.tint(for: tuningSurface) > 0
            ? palette.desktopBackground.nsColor.withAlphaComponent(
              glassPreferences.tint(for: tuningSurface))
            : nil)
        if role == .readingCanvas {
          Rectangle()
            .fill(Color(nsColor: Self.glassVeil(palette)))
            .allowsHitTesting(false)
            .accessibilityHidden(true)
        }
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

/// Sample the desktop behind the non-activating panel, not its empty content.
/// Keep the material active while the user types into another application.
private struct OverlayDesktopMaterial: NSViewRepresentable {
  let palette: OverlayAppearancePalette
  let cornerRadius: CGFloat
  var tintColor: NSColor? = nil

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
      glass.tintColor != tintColor
    {
      glass.tintColor = tintColor
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

extension EnvironmentValues {
  /// True while the agents divider is dragged, set by the fade both panes
  /// wear. System glass composites outside its parent's opacity and blur, so
  /// the composer, the conversation's navigation label and the pinned card
  /// would stay sharp over a faded pane; under this flag their control glass
  /// gives way to its own veil, which the fade reaches (Maciej, 2026-11-09
  /// 07:32Z: the whole view blurs and freezes while the divider moves).
  @Entry var overlayDividerDragging = false
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
  @Environment(\.colorScheme) private var colorScheme
  @Environment(\.overlayDividerDragging) private var dividerDragging
  var cornerRadius: CGFloat = CSRadius.window
  var isEnabled = true
  var role: OverlayGlassRole = .readingCanvas
  var showsBorder = true
  var glassPreferences = ChromeGlassPreferences()

  @ViewBuilder
  func body(content: Content) -> some View {
    let shape = RoundedRectangle(cornerRadius: cornerRadius, style: .continuous)
    let tint = glassPreferences.tint(for: .busHeader)
    // Surface glass stays still under the pointer; controls own interaction.
    let base = Glass.regular
    let material =
      role == .chrome && tint > 0
      ? base.tint(
        (colorScheme == .dark
          ? Color(red: 0.08, green: 0.08, blue: 0.09)
          : Color(red: 0.97, green: 0.96, blue: 0.94)).opacity(tint))
      : base
    let veilColor =
      colorScheme == .dark
      ? Color(red: 0.08, green: 0.08, blue: 0.09).opacity(0.76)
      : Color(red: 0.97, green: 0.96, blue: 0.94).opacity(0.82)
    // A divider drag freezes the glass into its veil for the drag only: two
    // flips per drag, and no backdrop is re-sampled while the panes resize.
    let frozen = isEnabled && dividerDragging
    let glass = isEnabled && !dividerDragging
    Group {
      if #available(macOS 26.0, *) {
        // Control glass does not dim with the window (Maciej, 2026-11-01).
        // A soft backing veil keeps scrolling rows legible as a hint rather
        // than colliding line-for-line with the foreground labels.
        content
          .background {
            if frozen || (glass && role == .readingCanvas) { shape.fill(veilColor) }
          }
          .glassEffect(glass ? material : .identity, in: shape)
          .environment(\.controlActiveState, .key)
      } else {
        content.background {
          if frozen {
            shape.fill(veilColor)
          } else if glass {
            shape.fill(.regularMaterial)
          }
        }
      }
    }
    .overlay {
      if isEnabled && role == .readingCanvas && showsBorder {
        // A short, soft contour separates glass from passing letters without
        // shadowing the panel's text or spreading across the clear gaps.
        shape.strokeBorder(
          Color.black.opacity(colorScheme == .dark ? 0.28 : 0.14),
          lineWidth: 1
        )
        .blur(radius: 1.2)
        .allowsHitTesting(false)
        .accessibilityHidden(true)
      }
    }
  }
}

/// The composer's surface for the bars that sit beside it: the same glass and
/// corner, and the same opaque sheet when Reduce Transparency is on. Maciej,
/// 2026-10-30: the composer is the reference and is not modified; the text
/// destination bar and the pinned All card adopt its surface, not the reverse.
/// A modifier, not a call site: `OverlayChannelStatusView` is never installed
/// as a view, so an environment read on it would see the default.
struct OverlayComposerSurface: ViewModifier {
  let palette: OverlayAppearancePalette
  var role: OverlayGlassRole = .readingCanvas
  var showsBorder = true
  @Environment(\.accessibilityReduceTransparency) private var reduceTransparency
  @Environment(\.csReduceTransparencyOverride) private var reduceTransparencyOverride

  func body(content: Content) -> some View {
    let opaque = reduceTransparencyOverride ?? reduceTransparency
    return
      content
      .background {
        if opaque {
          RoundedRectangle(cornerRadius: CSRadius.composer, style: .continuous)
            .fill(palette.desktopBackground.color)
        }
      }
      .modifier(
        OverlayControlGlass(
          cornerRadius: CSRadius.composer, isEnabled: !opaque, role: role,
          showsBorder: showsBorder))
  }
}
