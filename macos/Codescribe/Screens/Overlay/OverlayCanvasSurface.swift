import AppKit
import SwiftUI

/// Appearance-aware physical sheet. Window interaction is owned by explicit
/// inert regions in `DictationOverlayView`, not by a full-sheet AppKit layer.
struct OverlayCanvasSurface<Content: View>: View {
  @Environment(\.accessibilityReduceTransparency) private var reduceTransparency
  let palette: OverlayAppearancePalette
  var isEditing = false
  @ViewBuilder let content: Content

  var body: some View {
    content
      .background {
        OverlayCanvasBackdrop(
          palette: palette, reduceTransparency: reduceTransparency, isEditing: isEditing)
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
  var isEditing = false

  var body: some View {
    ZStack {
      if reduceTransparency || isEditing {
        Rectangle().fill(palette.desktopBackground.color)
      } else {
        OverlayDesktopMaterial()
        // The document needs a stable contrast floor, regardless of the glass slider.
        Rectangle().fill(palette.desktopBackground.color.opacity(0.85))
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
