import AppKit
import SwiftUI

/// One deterministic sRGB color role. Surfaces may be translucent; text is
/// intentionally opaque so desktop content below the material cannot decide
/// whether the transcript remains readable.
struct OverlayColorToken: Equatable, Sendable {
  let rgb: UInt32
  let alpha: Double

  init(_ rgb: UInt32, alpha: Double = 1) {
    self.rgb = rgb
    self.alpha = alpha
  }

  /// One `CSPalette` role sampled for a fixed appearance. The overlay reasons
  /// about exact sRGB components — contrast over glass, composited veils — and
  /// a dynamic `NSColor` cannot answer that outside a drawing appearance, so
  /// the role is resolved once instead of being restated as a second value.
  init(_ role: NSColor, in appearance: OverlayAppearance) {
    let resolved = OverlayColorToken.resolved(role, in: appearance)
    let red = UInt32((resolved.redComponent * 255).rounded())
    let green = UInt32((resolved.greenComponent * 255).rounded())
    let blue = UInt32((resolved.blueComponent * 255).rounded())
    self.init(red << 16 | green << 8 | blue, alpha: Double(resolved.alphaComponent))
  }

  private static func resolved(_ color: NSColor, in appearance: OverlayAppearance) -> NSColor {
    var resolved = color
    NSAppearance(named: appearance == .dark ? .darkAqua : .aqua)?
      .performAsCurrentDrawingAppearance {
        resolved = NSColor(cgColor: color.cgColor) ?? color
      }
    return resolved.usingColorSpace(.sRGB) ?? resolved
  }

  var components: (red: Double, green: Double, blue: Double) {
    (
      Double((rgb >> 16) & 0xFF) / 255,
      Double((rgb >> 8) & 0xFF) / 255,
      Double(rgb & 0xFF) / 255
    )
  }

  var color: Color {
    let value = components
    return Color(
      .sRGB,
      red: value.red,
      green: value.green,
      blue: value.blue,
      opacity: alpha
    )
  }

  var nsColor: NSColor {
    let value = components
    return NSColor(
      srgbRed: value.red,
      green: value.green,
      blue: value.blue,
      alpha: alpha
    )
  }

  func composited(over background: OverlayColorToken) -> (
    red: Double, green: Double, blue: Double
  ) {
    let foreground = components
    let backdrop = background.components
    return (
      foreground.red * alpha + backdrop.red * (1 - alpha),
      foreground.green * alpha + backdrop.green * (1 - alpha),
      foreground.blue * alpha + backdrop.blue * (1 - alpha)
    )
  }

  static func contrastRatio(
    foreground: OverlayColorToken,
    surface: OverlayColorToken,
    background: OverlayColorToken
  ) -> Double {
    let foregroundRGB = foreground.composited(over: surface)
    let surfaceRGB = surface.composited(over: background)
    let foregroundLuminance = relativeLuminance(foregroundRGB)
    let surfaceLuminance = relativeLuminance(surfaceRGB)
    let lighter = max(foregroundLuminance, surfaceLuminance)
    let darker = min(foregroundLuminance, surfaceLuminance)
    return (lighter + 0.05) / (darker + 0.05)
  }

  private static func relativeLuminance(
    _ rgb: (red: Double, green: Double, blue: Double)
  ) -> Double {
    0.2126 * linearize(rgb.red)
      + 0.7152 * linearize(rgb.green)
      + 0.0722 * linearize(rgb.blue)
  }

  private static func linearize(_ component: Double) -> Double {
    if component <= 0.03928 { return component / 12.92 }
    return pow((component + 0.055) / 1.055, 2.4)
  }
}
