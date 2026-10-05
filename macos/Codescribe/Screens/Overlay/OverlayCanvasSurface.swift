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

/// The desktop glass samples outside the window. Scroll chrome must instead
/// sample the messages behind it inside the same window, including an inactive panel.
struct OverlayScrollMaterial: NSViewRepresentable {
  let top: Bool
  let fade: CGFloat

  func makeNSView(context: Context) -> OverlayScrollEffectView {
    let view = OverlayScrollEffectView()
    view.material = .headerView
    view.blendingMode = .withinWindow
    view.state = .active
    view.setAccessibilityElement(false)
    return view
  }

  func updateNSView(_ view: OverlayScrollEffectView, context: Context) {
    view.top = top
    view.fade = fade
    view.needsLayout = true
  }
}

final class OverlayScrollEffectView: NSVisualEffectView {
  var top = true
  var fade: CGFloat = 20

  override func hitTest(_ point: NSPoint) -> NSView? { nil }

  override func layout() {
    super.layout()
    guard bounds.width > 0, bounds.height > 0 else { return }
    let length = min(max(0, fade), bounds.height)
    let image = NSImage(size: bounds.size)
    image.lockFocus()
    NSColor.black.setFill()
    NSRect(origin: .zero, size: bounds.size).fill()
    if length > 0 {
      let edge = NSRect(
        x: 0, y: top ? 0 : bounds.height - length,
        width: bounds.width, height: length)
      NSColor.clear.setFill()
      edge.fill(using: .copy)
      NSGradient(starting: .black, ending: .clear)?.draw(in: edge, angle: top ? -90 : 90)
    }
    image.unlockFocus()
    maskImage = image
  }
}
