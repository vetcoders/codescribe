import SwiftUI

/// Appearance-aware physical sheet. The drag view is the last background
/// layer, behind controls and text selection but across every inert point of
/// the canvas.
struct OverlayCanvasSurface<Content: View>: View {
  let palette: OverlayAppearancePalette
  @ViewBuilder let content: Content

  var body: some View {
    content
      .background {
        ZStack {
          Rectangle().fill(.ultraThinMaterial)
          Rectangle().fill(palette.surfaceTint.color)
          OverlayDragHandle()
        }
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
