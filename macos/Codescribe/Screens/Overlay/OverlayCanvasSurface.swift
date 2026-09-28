import AppKit
import SwiftUI

/// Appearance-aware physical sheet. Window interaction is owned by explicit
/// inert regions in `DictationOverlayView`, not by a full-sheet AppKit layer.
struct OverlayCanvasSurface<Content: View>: View {
  @Environment(\.accessibilityReduceTransparency) private var reduceTransparency
  let palette: OverlayAppearancePalette
  @ViewBuilder let content: Content

  var body: some View {
    content
      .background {
        OverlayCanvasBackdrop(palette: palette, reduceTransparency: reduceTransparency)
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

  var body: some View {
    ZStack {
      if reduceTransparency {
        Rectangle().fill(palette.desktopBackground.color)
      } else if #available(macOS 26.0, *) {
        OverlayClearGlass()
      } else {
        OverlayDesktopMaterial()
      }
    }
  }
}

/// Sample the desktop behind the non-activating panel, not its empty content.
/// Keep the material active while the user types into another application.
private struct OverlayDesktopMaterial: NSViewRepresentable {
  func makeNSView(context: Context) -> OverlayDesktopEffectView {
    let view = OverlayDesktopEffectView()
    view.material = .hudWindow
    view.blendingMode = .behindWindow
    view.state = .active
    return view
  }

  func updateNSView(_ nsView: OverlayDesktopEffectView, context: Context) {}
}

final class OverlayDesktopEffectView: NSVisualEffectView {
  override func hitTest(_ point: NSPoint) -> NSView? { nil }
}

/// Native clear glass samples the desktop without an extra tint or HUD scrim.
@available(macOS 26.0, *)
private struct OverlayClearGlass: NSViewRepresentable {
  func makeNSView(context: Context) -> OverlayClearGlassView {
    let view = OverlayClearGlassView()
    view.style = .clear
    view.cornerRadius = CSRadius.window
    return view
  }

  func updateNSView(_ nsView: OverlayClearGlassView, context: Context) {}
}

@available(macOS 26.0, *)
final class OverlayClearGlassView: NSGlassEffectView {
  override func hitTest(_ point: NSPoint) -> NSView? { nil }
}
