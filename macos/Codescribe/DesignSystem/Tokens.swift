import AppKit
import SwiftUI

// codescribe design tokens — single source of truth for color.
// Adaptive native palette: surfaces, hairlines, and text follow the system
// appearance (light/dark) through dynamic NSColor providers, so every consumer
// of CSColor adapts without a call-site change. Brand and semantic hues
// (terracotta, assistive violet, olive, amber) are appearance-fixed — they
// carry app-owned mode, state, and brand meaning, not chrome meaning.
// Decorative controls use the operator's macOS accent instead.
// Assistive violet = voice routed to the agent. Olive/green = healthy status.
// Amber = reasoning/format meta.
// Never hardcode a replacement for the system accent: the operator owns it.

extension Color {
  init(hex: UInt32, alpha: Double = 1.0) {
    let r = Double((hex >> 16) & 0xFF) / 255.0
    let g = Double((hex >> 8) & 0xFF) / 255.0
    let b = Double(hex & 0xFF) / 255.0
    self.init(.sRGB, red: r, green: g, blue: b, opacity: alpha)
  }
}

extension NSColor {
  convenience init(hex: UInt32, alpha: Double = 1.0) {
    let r = CGFloat((hex >> 16) & 0xFF) / 255.0
    let g = CGFloat((hex >> 8) & 0xFF) / 255.0
    let b = CGFloat(hex & 0xFF) / 255.0
    self.init(srgbRed: r, green: g, blue: b, alpha: CGFloat(alpha))
  }
}

/// Resolved values behind `CSColor`. NSColor is the truth layer: dynamic
/// providers let the palette follow the system appearance, and tests can
/// resolve exact components under a forced light/dark `NSAppearance`.
/// `CSColor` is the SwiftUI projection of these values — screens consume that.
enum CSPalette {
  /// A color that resolves to `dark` under a dark appearance and to `light`
  /// otherwise (including high-contrast aqua variants, which stay on the
  /// light/dark axis via `bestMatch`).
  static func adaptive(light: NSColor, dark: NSColor) -> NSColor {
    NSColor(name: nil) { appearance in
      appearance.bestMatch(from: [.darkAqua, .aqua]) == .darkAqua ? dark : light
    }
  }

  // Brand + semantic hues — appearance-fixed by decision, they carry meaning.
  static let terracotta = NSColor(hex: 0xD97757)
  static let terracottaDeep = NSColor(hex: 0xC98A6E)
  static let terracottaTintBars = NSColor(hex: 0xE6A98F)
  static let assistive = NSColor(hex: 0x9B72F2)
  static let olive = NSColor(hex: 0x5F6B3E)
  static let indicatorRecording = NSColor(hex: 0xFF3B30)
  static let danger = NSColor(hex: 0xD84A4A)

  // Floating-canvas recipe — appearance-FIXED dark. A panel floating over
  // arbitrary desktop content (the dictation overlay) keeps one stable dark
  // glass in both system modes; adaptivity there would let a light wallpaper
  // flip the transcript canvas from under the reader.
  static let forestInk = NSColor(hex: 0x090A0D)

  // Surfaces — warm ink in dark, warm paper in light.
  static let ink = adaptive(light: NSColor(hex: 0xF7F5F0), dark: NSColor(hex: 0x090A0D))
  static let glassBase = adaptive(
    light: NSColor(hex: 0xFFFFFF, alpha: 0.78),
    dark: NSColor(hex: 0x12141A, alpha: 0.84)
  )
  static let glassUnder = adaptive(light: NSColor(hex: 0xEDEAE3), dark: NSColor(hex: 0x0B0C10))
  static let warmInk = adaptive(light: NSColor(hex: 0xF8F3EC), dark: NSColor(hex: 0x15110E))
  static let warmDeep = adaptive(light: NSColor(hex: 0xE9E5DD), dark: NSColor(hex: 0x0D1012))

  // Text — warm near-black on paper, warm off-white on ink. The "Alt" rung is
  // always the fainter one: closer to the page in whichever mode is active.
  static let textHigh = adaptive(light: NSColor(hex: 0x1D1B17), dark: NSColor(hex: 0xF4F2EC))
  static let textBody = adaptive(light: NSColor(hex: 0x2B2924), dark: NSColor(hex: 0xE9E7E0))
  static let textBodyAlt = adaptive(light: NSColor(hex: 0x363430), dark: NSColor(hex: 0xDFE2DB))
  static let textMuted = adaptive(light: NSColor(hex: 0x6E7168), dark: NSColor(hex: 0x9A9D97))
  static let textMutedAlt = adaptive(light: NSColor(hex: 0x84877E), dark: NSColor(hex: 0x82857F))
  static let textFaint = adaptive(light: NSColor(hex: 0x9DA094), dark: NSColor(hex: 0x6F7268))
  static let textFaintAlt = adaptive(light: NSColor(hex: 0xB2B5A9), dark: NSColor(hex: 0x5D6058))

  // "Light" label variants are drawn as text ON accent-tinted fills; on paper
  // they need the deepened rung of the same hue to stay legible.
  static let terracottaLight = adaptive(light: NSColor(hex: 0xA94E2C), dark: NSColor(hex: 0xE9B79F))
  static let assistiveLight = adaptive(light: NSColor(hex: 0x6F3FD8), dark: NSColor(hex: 0xC9B7FF))
  static let oliveLight = adaptive(light: NSColor(hex: 0x55663A), dark: NSColor(hex: 0x9DB178))
  static let eyebrowOlive = adaptive(light: NSColor(hex: 0x66744A), dark: NSColor(hex: 0x7F8C5E))
  static let amber = adaptive(light: NSColor(hex: 0x9A7B1E), dark: NSColor(hex: 0xD6B24E))
  static let modeProcessing = adaptive(light: NSColor(hex: 0xB96A24), dark: NSColor(hex: 0xF28C45))
  static let dangerLight = adaptive(light: NSColor(hex: 0xB3261E), dark: NSColor(hex: 0xFFAAA5))
}

enum CSColor {
  // Surfaces
  static let ink = Color(nsColor: CSPalette.ink)  // page base
  static let glassBase = Color(nsColor: CSPalette.glassBase)  // app window material tint
  static let glassUnder = Color(nsColor: CSPalette.glassUnder)

  /// Lifted surface wash. Dark mode lifts with a white veil; on paper the same
  /// lift is a white card over the warm base, so the veil maps to a stronger
  /// white wash (a small alpha is nearly invisible on light backgrounds).
  static func surfaceRaised(_ a: Double = 0.03) -> Color {
    Color(
      nsColor: CSPalette.adaptive(
        light: NSColor.white.withAlphaComponent(CGFloat(min(0.85, a * 16))),
        dark: NSColor.white.withAlphaComponent(CGFloat(a))
      ))
  }

  /// One-point separator. Dark mode draws hairlines as a white veil; on paper
  /// the same stroke is a faint ink veil, slightly stronger to stay visible.
  static func hairline(_ a: Double = 0.07) -> Color {
    Color(
      nsColor: CSPalette.adaptive(
        light: NSColor.black.withAlphaComponent(CGFloat(min(0.22, a + 0.04))),
        dark: NSColor.white.withAlphaComponent(CGFloat(a))
      ))
  }

  /// Hairline for the floating canvas: always the dark-mode white veil, because
  /// the forest glass itself never adapts (see `CSPalette.forestInk`).
  static func forestHairline(_ a: Double = 0.07) -> Color {
    Color.white.opacity(a)
  }

  // App semantics — these colors carry information and do not follow macOS accent.
  static let terracotta = Color(nsColor: CSPalette.terracotta)  // dictation / processing / brand
  // Active labels on accent fills.
  static let terracottaLight = Color(nsColor: CSPalette.terracottaLight)
  static let terracottaDeep = Color(nsColor: CSPalette.terracottaDeep)  // secondary voice accent
  // Every-5th waveform bar.
  static let terracottaTintBars = Color(nsColor: CSPalette.terracottaTintBars)

  // Assistive accent — agent-routed voice
  static let assistive = Color(nsColor: CSPalette.assistive)
  static let assistiveLight = Color(nsColor: CSPalette.assistiveLight)

  static let modeDictation = terracotta
  static let modeAgent = assistive
  static let modeRecording = modeDictation
  static let modeProcessing = Color(nsColor: CSPalette.modeProcessing)
  static let modeReady = oliveLight
  static let indicatorRecording = Color(nsColor: CSPalette.indicatorRecording)

  // UI chrome — selection, focus, and interactive controls follow macOS.
  static var chromeAccent: Color { Color(nsColor: .controlAccentColor) }

  // Status — olive / green
  static let olive = Color(nsColor: CSPalette.olive)  // healthy base
  static let oliveLight = Color(nsColor: CSPalette.oliveLight)  // idle / granted / success dot
  static let eyebrowOlive = Color(nsColor: CSPalette.eyebrowOlive)  // section eyebrows

  // Reasoning — amber
  static let amber = Color(nsColor: CSPalette.amber)

  // Destructive actions — reserved for explicit danger-zone controls
  static let danger = Color(nsColor: CSPalette.danger)
  static let dangerLight = Color(nsColor: CSPalette.dangerLight)

  // Text
  static let textHigh = Color(nsColor: CSPalette.textHigh)  // headlines
  static let textBody = Color(nsColor: CSPalette.textBody)
  static let textBodyAlt = Color(nsColor: CSPalette.textBodyAlt)
  static let textMuted = Color(nsColor: CSPalette.textMuted)
  static let textMutedAlt = Color(nsColor: CSPalette.textMutedAlt)
  static let textFaint = Color(nsColor: CSPalette.textFaint)  // mono meta
  static let textFaintAlt = Color(nsColor: CSPalette.textFaintAlt)  // timestamps

  /// Warm wash gradient shared by Settings detail and overlay/tray previews so
  /// those surfaces cannot drift apart. Follows the system appearance.
  static let windowWash = LinearGradient(
    stops: [
      .init(color: Color(nsColor: CSPalette.warmInk), location: 0.0),
      .init(color: Color(nsColor: CSPalette.glassUnder), location: 0.55),
      .init(color: Color(nsColor: CSPalette.warmDeep), location: 1.0),
    ],
    startPoint: .topLeading,
    endPoint: .bottomTrailing
  )
}

enum CSRadius {
  static let chip: CGFloat = 8
  static let input: CGFloat = 9
  static let card: CGFloat = 12
  static let composer: CGFloat = 13
  static let tray: CGFloat = 14
  static let pill: CGFloat = 20
  static let window: CGFloat = 22
}

/// Layout rhythm. Screens were inventing 6/7/8/11/12/14/15/20/28/44 independently.
/// Cluster onto this scale; overlay/tray preview inset stays 44 — that is hit-geometry, not page padding.
enum CSSpace {
  static let xxs: CGFloat = 4
  static let xs: CGFloat = 6
  static let sm: CGFloat = 8
  static let control: CGFloat = 9
  static let md: CGFloat = 12
  static let card: CGFloat = 14
  static let lg: CGFloat = 20
  static let section: CGFloat = 24
  static let xl: CGFloat = 28
  static let page: CGFloat = 32
  static let previewInset: CGFloat = 44
}
