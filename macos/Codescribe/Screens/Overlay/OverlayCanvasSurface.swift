import AppKit
import SwiftUI

/// Reading surfaces retain their quiet canvas; window chrome exposes the
/// native optical material without an additional opaque backing.
enum OverlayGlassRole: Equatable {
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
  var readingTopInset: CGFloat = 0
  @ViewBuilder let content: Content

  var body: some View {
    content
      .background {
        OverlayCanvasBackdrop(
          palette: palette,
          reduceTransparency: reduceTransparencyOverride ?? reduceTransparency,
          increasedContrast: (contrastOverride ?? contrast) == .increased,
          role: glassRole, showsBorder: glassRole == .readingCanvas,
          topInset: glassRole == .readingCanvas ? readingTopInset : 0)
      }
      .clipShape(RoundedRectangle(cornerRadius: CSRadius.window, style: .continuous))
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
  var showsBorder = false
  var topInset: CGFloat = 0
  var tuningSurface: ChromeGlassSurface = .compactOverlay
  var glassPreferences = ChromeGlassPreferences()

  var body: some View {
    OverlayDesktopMaterial(
      configuration: .init(
        palette: palette, cornerRadius: cornerRadius, role: role,
        opaque: (reduceTransparencyOverride ?? reduceTransparency) || increasedContrast
          || (contrastOverride ?? contrast) == .increased,
        tintColor: role == .chrome && glassPreferences.tint(for: tuningSurface) > 0
          ? palette.desktopBackground.nsColor.withAlphaComponent(
            glassPreferences.tint(for: tuningSurface))
          : nil, showsBorder: showsBorder, topInset: topInset))
  }

  /// The shared sheet colour over the native glass: near-black on ink,
  /// paper on paper, with the desktop left as a faint blurred hint (Maciej,
  /// 2026-10-15: "bardziej czarno", "dark almost black"). The 22 % veil of
  /// 2026-10-14 still took the page's brightness; the macOS Liquid Glass slider
  /// does not darken it.
  ///
  /// The veil is a separate layer owned with the glass, not its `tintColor`:
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
  let configuration: OverlayCanvasBackdropView.Configuration

  func makeNSView(context: Context) -> OverlayCanvasBackdropView {
    OverlayCanvasBackdropView(configuration: configuration)
  }

  func updateNSView(_ view: OverlayCanvasBackdropView, context: Context) {
    view.configure(configuration)
  }
}

/// The canvas material and its reading veil have one native owner. A resize
/// capture temporarily excludes this owner, so a frozen foreground image never
/// carries an old-sized sheet. The preview reuses the same material policy.
final class OverlayCanvasBackdropView: NSView {
  struct Configuration: Equatable {
    let palette: OverlayAppearancePalette
    let cornerRadius: CGFloat
    let role: OverlayGlassRole
    let opaque: Bool
    let tintColor: NSColor?
    var showsBorder = false
    var topInset: CGFloat = 0
  }

  private(set) var configuration: Configuration
  private var material: NSView?
  private let veil = NSView()

  init(configuration: Configuration) {
    self.configuration = configuration
    super.init(frame: .zero)
    identifier = NSUserInterfaceItemIdentifier("overlay-canvas-backdrop")
    wantsLayer = true
    veil.wantsLayer = true
    addSubview(veil)
    applyConfiguration()
  }

  required init?(coder: NSCoder) { nil }
  override func hitTest(_ point: NSPoint) -> NSView? { nil }

  func configure(_ value: Configuration) {
    guard configuration != value else { return }
    configuration = value
    applyConfiguration()
  }

  private func applyConfiguration() {
    let value = configuration
    appearance = NSAppearance(named: value.palette.appearance == .dark ? .darkAqua : .aqua)
    layer?.cornerRadius = value.cornerRadius
    layer?.cornerCurve = .continuous
    layer?.masksToBounds = true
    // Keep the existing window perimeter, without drawing a new border along
    // the reading/header boundary when the reading layers start below it.
    layer?.borderWidth = value.showsBorder ? 1 : 0
    layer?.borderColor = value.palette.border.nsColor.cgColor
    if value.opaque {
      // Opaque accessibility must remove desktop sampling, not just hide it.
      material?.removeFromSuperview()
      material = nil
    } else if material == nil {
      let view: NSView
      if #available(macOS 26, *) {
        view = OverlayDesktopGlassView()
      } else {
        view = OverlayDesktopEffectView()
      }
      material = view
      addSubview(view, positioned: .below, relativeTo: veil)
    }
    material?.appearance = appearance
    veil.isHidden = !value.opaque && value.role == .chrome
    veil.layer?.backgroundColor =
      (value.opaque
      ? value.palette.desktopBackground.nsColor
      : OverlayCanvasBackdrop.glassVeil(value.palette)).cgColor
    if #available(macOS 26, *), let glass = material as? OverlayDesktopGlassView {
      glass.style = .regular
      glass.tintColor = value.tintColor
    } else if let effect = material as? OverlayDesktopEffectView {
      effect.material = .hudWindow
      effect.blendingMode = .behindWindow
      effect.state = .active
    }
    sizeLayers()
  }

  override func setFrameSize(_ newSize: NSSize) {
    super.setFrameSize(newSize)
    sizeLayers()
  }

  override func layout() {
    super.layout()
    sizeLayers()
  }

  private func sizeLayers() {
    let inset = min(bounds.height, max(0, configuration.topInset))
    let readingBounds = NSRect(
      x: bounds.minX, y: bounds.minY + (isFlipped ? inset : 0),
      width: bounds.width, height: bounds.height - inset)
    material?.frame = readingBounds
    veil.frame = readingBounds
    // The window clip owns the lower corners. Rounding the new top edge
    // would cut notches into the join beneath the independently owned header.
    let radius = inset > 0 ? 0 : configuration.cornerRadius
    veil.layer?.cornerCurve = .continuous
    veil.layer?.cornerRadius = radius
    if #available(macOS 26, *), let glass = material as? OverlayDesktopGlassView {
      glass.cornerRadius = radius
    }
  }

  static func all(in view: NSView) -> [OverlayCanvasBackdropView] {
    if let backdrop = view as? OverlayCanvasBackdropView { return [backdrop] }
    return view.subviews.flatMap { all(in: $0) }
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
  @Environment(\.colorScheme) private var colorScheme
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
    Group {
      if #available(macOS 26.0, *) {
        // Control glass does not dim with the window (Maciej, 2026-11-01).
        // A soft backing veil keeps scrolling rows legible as a hint rather
        // than colliding line-for-line with the foreground labels.
        content
          .background {
            if isEnabled && role == .readingCanvas { shape.fill(veilColor) }
          }
          .glassEffect(isEnabled ? material : .identity, in: shape)
          .environment(\.controlActiveState, .key)
      } else {
        content.background {
          if isEnabled { shape.fill(.regularMaterial) }
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
