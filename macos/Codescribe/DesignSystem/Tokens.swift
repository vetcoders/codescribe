import AppKit
import SwiftUI

// codescribe design tokens — single source of truth for color.
// Adaptive native palette: surfaces, hairlines, and text follow the system
// appearance (light/dark) through dynamic NSColor providers, so every consumer
// of CSColor adapts without a call-site change. Brand and semantic hues
// (terracotta, assistive violet, olive, amber) are appearance-fixed — they
// carry app-owned mode, state, and brand meaning, not chrome meaning.
// Interactive chrome follows the user's macOS accent through one role,
// `chromeAccent`; terracotta stays the dictation and recording colour
// (Founder, 2026-10-24: actions are not terracotta).
// Window chrome (canvas, code wells, control fills) is the system semantic
// NSColor directly — one owner, no per-surface appearance wrappers.
// Assistive violet = voice routed to the agent. Olive/green = healthy status.
// Amber = reasoning/format meta.
// Never hardcode a replacement for the system accent: the user owns it.

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
  /// Capture is live but nothing reaches speaking level. Founder intent
  /// s04-027 (2026-09-25): the indicator is yellow while you are not speaking.
  static let indicatorSilence = NSColor(hex: 0xFFCC00)
  /// Agent mode between phrases: the pointer dot's white.
  static let indicatorAgentQuiet = NSColor(white: 0.97, alpha: 1)
  static let danger = NSColor(hex: 0xD84A4A)

  // Surfaces — warm ink in dark, warm paper in light.
  static let ink = adaptive(light: NSColor(hex: 0xF7F5F0), dark: NSColor(hex: 0x090A0D))
  /// The app sheet: the overlay canvas colour, shared by every window's content
  /// area outside its sidebar so Settings, the Agent window, the wizard, the
  /// tray menu and the overlay read as one surface (Maciej, ch1 2026-10-15
  /// 16:26Z: "obszarów poza sidebarami … takich samych"). The hex values are
  /// the single source for `OverlayAppearancePalette.desktopBackground`; the
  /// dark sheet keeps the hue of the Vibecrafted terminal's navy (#0B0B12,
  /// `~/.config/vibecrafted/terminal-theme.toml`) lifted one step above the
  /// 6 % bubble lift.
  /// Read the observable preference while a view evaluates its tokens. The
  /// palette and its SwiftUI projection must not freeze a launch-time colour.
  static var sheetLightHex: UInt32 { SheetShadePreference.shared.current(dark: false).hex }
  static var sheetDarkHex: UInt32 { SheetShadePreference.shared.current(dark: true).hex }
  static var sheet: NSColor { sheetColor(preference: .shared) }

  static func sheetColor(preference: SheetShadePreference) -> NSColor {
    adaptive(
      light: NSColor(hex: preference.current(dark: false).hex),
      dark: NSColor(hex: preference.current(dark: true).hex))
  }

  // Text — warm near-black on paper, warm off-white on ink. The "Alt" rung is
  // always the fainter one: closer to the page in whichever mode is active.
  static let textHigh = adaptive(light: NSColor(hex: 0x1D1B17), dark: NSColor(hex: 0xF4F2EC))
  static let textBody = adaptive(light: NSColor(hex: 0x2B2924), dark: NSColor(hex: 0xE9E7E0))
  /// The paper rung clears 4.5:1 on the real light surfaces, not only on
  /// `ink`: the sheet, a card or chip on it, a bubble, and the measured glass
  /// sheet (#DFDDD9, 2026-10-20), including every selectable sheet shade.
  /// Travertine's chip is the darkest light surface.
  static let textMuted = adaptive(light: NSColor(hex: 0x4B4D46), dark: NSColor(hex: 0xA0A39D))
  /// The faint rungs are decoration only, never text: rules, separators and
  /// hairlines that clear 3:1. Text uses the three warm rungs above —
  /// textHigh, textBody, textMuted (Founder, 2026-10-26).
  static let textFaint = adaptive(light: NSColor(hex: 0x9DA094), dark: NSColor(hex: 0x6F7268))
  static let textFaintAlt = adaptive(light: NSColor(hex: 0xB2B5A9), dark: NSColor(hex: 0x5D6058))

  // "Light" label variants are drawn as text ON accent-tinted fills; on paper
  // they need the deepened rung of the same hue to stay legible.
  // The paper rungs are sized for the darkest light surface a label meets: a
  // chip on a travertine card, the glass sheet measured at #DFDDD9, or the
  // label's own 12 % tint on any selectable card or glass.
  static let terracottaLight = adaptive(light: NSColor(hex: 0x75351E), dark: NSColor(hex: 0xE9B79F))
  static let assistiveLight = adaptive(light: NSColor(hex: 0x522EA2), dark: NSColor(hex: 0xC9B7FF))
  static let oliveLight = adaptive(light: NSColor(hex: 0x404D2C), dark: NSColor(hex: 0x9DB178))
  /// Section eyebrows sit on sheets, cards, chips and glass, never on their
  /// own tint; both rungs clear 4.5:1 on the darkest of those surfaces (a chip
  /// on a travertine card, a chip on a warm-carbon card).
  static let eyebrowOlive = adaptive(light: NSColor(hex: 0x464F33), dark: NSColor(hex: 0x9BA77C))
  static let amber = adaptive(light: NSColor(hex: 0x9A7B1E), dark: NSColor(hex: 0xD6B24E))
  static let modeProcessing = adaptive(light: NSColor(hex: 0xB96A24), dark: NSColor(hex: 0xF28C45))
  static let dangerLight = adaptive(light: NSColor(hex: 0x8C1D17), dark: NSColor(hex: 0xFFAAA5))
  /// Terracotta drawn as a word or an essential glyph (an error line, a
  /// destructive icon, a failed status). The brand hue reads 2.57:1 on a light
  /// card, so paper takes the label rung; ink keeps a saturated brand hue,
  /// lifted to clear 4.5:1 on its own 12 % tint over warm carbon. This is not
  /// `terracottaLight` (pale #E9B79F on ink).
  static let terracottaText = adaptive(light: NSColor(hex: 0x75351E), dark: NSColor(hex: 0xE58D6F))
  /// Secondary text: the system label ink at a strength that clears 4.5:1 on
  /// the app sheet, its cards, chips and bubbles. `secondaryLabelColor` (50 %)
  /// measured 3.82:1 for Settings helper text on a card (2026-10-24); the app
  /// sheet is darker than the window background that rung was tuned for.
  static let secondaryInk = adaptive(
    light: NSColor.black.withAlphaComponent(0.6), dark: NSColor.white.withAlphaComponent(0.6))
  /// The amber and the processing orange at label strength. The floating sheet
  /// proves 4.5:1 for status words against the opaque sheet, which the base
  /// rungs of both hues miss on paper; these are the same hues, deepened.
  static let amberLight = adaptive(light: NSColor(hex: 0x644200), dark: NSColor(hex: 0xE2BE5B))
  static let modeProcessingLight = adaptive(
    light: NSColor(hex: 0x763607), dark: NSColor(hex: 0xF09A4E))
  /// The healthy olive at status strength. On ink the label rung sits within
  /// 0.15 of `textMuted`, so a resting grey glyph beside a status read as
  /// "healthy"; this rung keeps the status clear of the muted grey on both
  /// appearances.
  /// Paper keeps a saturated olive distinct from the muted grey.
  static let oliveStatus = adaptive(light: NSColor(hex: 0x3C4D1B), dark: NSColor(hex: 0xB5C98D))
  /// Steel blue: the dictionary owns the words it rewrote, so a rewritten
  /// surface never wears the uncertainty orange.
  static let lexicon = adaptive(light: NSColor(hex: 0x2A4C69), dark: NSColor(hex: 0x9FC2E8))
  /// A floating panel's edge. Warmer and stronger than a `hairline` divider,
  /// and the paper rung is the fainter of the two: the edge separates the sheet
  /// from an arbitrary desktop, not two rows inside one window.
  static let panelEdge = adaptive(
    light: NSColor(hex: 0x5F5A52, alpha: 0.20), dark: NSColor(hex: 0xF3F0EA, alpha: 0.16))

  /// The one chrome accent shared by selection, focus and interactive
  /// controls: the user's macOS accent. Actions read as clickable through
  /// their shape (small bordered buttons), not through a brand hue; with the
  /// Graphite accent they stay grey by the user's choice.
  static let chromeAccent = NSColor.controlAccentColor

  /// Primary-action wash: the chrome accent on paper; on ink the same accent
  /// lifted halfway to white. A translucent accent wash alone paints the same
  /// mid-tone on both appearances, which reads as a highlight on paper but
  /// sinks into dark glass. Callers own the wash alpha.
  static let accentWash = NSColor(name: nil) { appearance in
    var accent = chromeAccent
    appearance.performAsCurrentDrawingAppearance {
      accent = chromeAccent.usingColorSpace(.sRGB) ?? accent
    }
    guard appearance.bestMatch(from: [.darkAqua, .aqua]) == .darkAqua else { return accent }
    return accent.blended(withFraction: 0.5, of: .white) ?? accent
  }
}

enum CSColor {
  // Surfaces
  static let ink = Color(nsColor: CSPalette.ink)  // page base
  /// Content area of every window outside its sidebar; see `CSPalette.sheet`.
  static var sheet: Color { Color(nsColor: CSPalette.sheet) }

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

  // Native window chrome — the system semantic surfaces, shared so no screen
  // needs its own appearance wrapper.
  static let windowCanvas = Color(nsColor: .windowBackgroundColor)
  static let codeWell = Color(nsColor: .textBackgroundColor)
  static let controlFill = Color(nsColor: .controlBackgroundColor)
  // SwiftUI ships no tertiary `Color` rung; the AppKit semantic lives here.
  static let textTertiary = Color(nsColor: .tertiaryLabelColor)
  /// Secondary text that must stay readable; see `CSPalette.secondaryInk`.
  static let secondaryInk = Color(nsColor: CSPalette.secondaryInk)

  // App semantics — these colors carry information and do not follow macOS accent.
  static let terracotta = Color(nsColor: CSPalette.terracotta)  // dictation / processing / brand
  // Active labels on accent fills.
  static let terracottaLight = Color(nsColor: CSPalette.terracottaLight)
  static let terracottaDeep = Color(nsColor: CSPalette.terracottaDeep)  // secondary voice accent
  /// Terracotta as a word or essential glyph; see `CSPalette.terracottaText`.
  static let terracottaText = Color(nsColor: CSPalette.terracottaText)
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
  static let indicatorAgentQuiet = Color(nsColor: CSPalette.indicatorAgentQuiet)

  // UI chrome — selection, focus and interactive controls follow macOS.
  static let chromeAccent = Color(nsColor: CSPalette.chromeAccent)
  /// The one primary action's fill tint (apply the wash alpha at the call site).
  static let accentWash = Color(nsColor: CSPalette.accentWash)

  // Status — olive / green
  static let olive = Color(nsColor: CSPalette.olive)  // healthy base
  static let oliveLight = Color(nsColor: CSPalette.oliveLight)  // idle / granted / success dot
  static let oliveStatus = Color(nsColor: CSPalette.oliveStatus)  // healthy status words and fills
  static let eyebrowOlive = Color(nsColor: CSPalette.eyebrowOlive)  // section eyebrows

  // Reasoning — amber
  static let amber = Color(nsColor: CSPalette.amber)
  /// Amber as a word (a warning, a pending state): the label rung that clears
  /// 4.5:1 on paper, where the base amber reads 3.31:1 on a card.
  static let amberLight = Color(nsColor: CSPalette.amberLight)

  // Destructive actions — reserved for explicit danger-zone controls
  static let danger = Color(nsColor: CSPalette.danger)
  static let dangerLight = Color(nsColor: CSPalette.dangerLight)

  // Text
  static let textHigh = Color(nsColor: CSPalette.textHigh)  // headlines
  static let textBody = Color(nsColor: CSPalette.textBody)
  static let textMuted = Color(nsColor: CSPalette.textMuted)
  static let textFaint = Color(nsColor: CSPalette.textFaint)  // decoration only, never text
  static let textFaintAlt = Color(nsColor: CSPalette.textFaintAlt)  // decoration only, never text
}

enum CSRadius {
  static let chip: CGFloat = 8
  static let input: CGFloat = 8
  static let card: CGFloat = 12
  static let composer: CGFloat = 12
  static let window: CGFloat = 22
}

/// Ink veils: `Color.primary` at one of four strengths, from the faintest
/// surface tint to a control's border (Founder, 2026-10-26). Surfaces pick a
/// level; they do not invent an alpha.
enum CSAlpha {
  /// A barely raised surface: a bubble or a quiet well.
  static let subtle: Double = 0.03
  /// A filled control or chip at rest.
  static let fill: Double = 0.05
  /// A hairline rule or a hovered fill.
  static let line: Double = 0.08
  /// A control's border.
  static let stroke: Double = 0.12
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
