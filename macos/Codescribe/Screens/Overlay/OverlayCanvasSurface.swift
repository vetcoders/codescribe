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
        OverlayCanvasBackdrop(
          palette: palette, reduceTransparency: reduceTransparency)
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
      } else {
        OverlayDesktopMaterial()
      }
    }
  }
}

/// Sample the desktop behind the non-activating panel, not its empty content.
/// Keep the material active while the user types into another application.
private struct OverlayDesktopMaterial: NSViewRepresentable {
  func makeNSView(context: Context) -> NSView {
    if #available(macOS 26, *) {
      let view = OverlayDesktopGlassView()
      view.style = .regular
      view.cornerRadius = CSRadius.window
      return view
    }
    let view = OverlayDesktopEffectView()
    view.material = .hudWindow
    view.blendingMode = .behindWindow
    view.state = .active
    return view
  }

  func updateNSView(_ nsView: NSView, context: Context) {}
}

/// One stable background, independent of recording, editing, and toolbelt state.
@available(macOS 26, *)
final class OverlayDesktopGlassView: NSGlassEffectView {
  override func hitTest(_ point: NSPoint) -> NSView? { nil }
}

final class OverlayDesktopEffectView: NSVisualEffectView {
  override func hitTest(_ point: NSPoint) -> NSView? { nil }
}

/// Native SwiftUI glass samples the scroll content in its own composition.
struct OverlayScrollMaterial: View {
  let top: Bool

  var body: some View {
    material
      .mask(OverlayScrollFade(top: top))
      .allowsHitTesting(false)
      .accessibilityHidden(true)
  }

  @ViewBuilder
  private var material: some View {
    if #available(macOS 26.0, *) {
      Color.clear.glassEffect(.regular, in: Rectangle())
    } else {
      Rectangle().fill(.regularMaterial)
    }
  }
}

/// Transparency changes continuously across the entire header or input region.
struct OverlayScrollFade: View {
  let top: Bool

  var body: some View {
    LinearGradient(
      colors: top ? [.black, .clear] : [.clear, .black],
      startPoint: .top, endPoint: .bottom)
  }
}
