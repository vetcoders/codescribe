import AppKit
import SwiftUI

// codescribe design tokens — single source of truth for color.
// System palette (Founder, 2026-11-05): surfaces, edges and text are the
// system semantic NSColors, so every consumer follows light, dark and
// increased contrast without a call-site change. Interactive chrome follows
// the user's macOS accent through one role, `chromeAccent`. A meaning that
// needs a colour takes one system colour: red = error, orange = attention
// (warning, work in progress, uncertain word), indigo = voice routed to the
// agent, yellow = silence, green = success (a connected agent's presence
// dot, a recognized gesture test). Any other correct state wears no colour.
// The brand terracotta is the dictation / listening state only. Never
// hardcode a replacement for the system accent: the user owns it.

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

  // Founder, 2026-11-05: every custom colour goes; what stays is what must,
  // where it must. The brand terracotta stays for the dictation / listening
  // state only (the lamp, the wave, the tray icon while dictating); the
  // Wordmark is the word alone. Every other meaning takes a named
  // system colour (one meaning per colour), and a correct state wears no
  // colour at all: label ink and a glyph. Canon:
  // AI_notes/10_projects/codescribe/specs/2026-11-05_lista-rol-kolorow.md.
  static let terracotta = NSColor(hex: 0xD97757)
  /// Voice routed to an agent: the system indigo, one meaning (Founder,
  /// 2026-11-05: the system purple sits too close to a pink accent).
  static let assistive = NSColor.systemIndigo
  /// The recording red is the pointer dot's, from the core palette
  /// (`IndicatorPalette`), opaque; the lamp and the menu-bar dot never keep a
  /// red of their own.
  static let indicatorRecording = IndicatorPalette.opaqueColor(.recording)
  /// Capture is live but nothing reaches speaking level. Founder intent
  /// s04-027 (2026-09-25): the indicator is yellow while you are not speaking.
  static let indicatorSilence = NSColor.systemYellow
  /// Agent mode between phrases: the pointer dot's white, from the core palette.
  static let indicatorAgentQuiet = IndicatorPalette.opaqueColor(.agentQuiet)
  static let danger = NSColor.systemRed
  /// Success: the system green. A connected agent's presence dot and a
  /// recognized gesture test; every other correct state is ink and a glyph.
  static let success = NSColor.systemGreen

  // Surfaces — the system window and control backgrounds.
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

  // Text — the system label rungs: they carry light, dark and increased
  // contrast variants and follow the user's settings, which no warm hex did.
  static let textHigh = NSColor.labelColor
  static let textBody = NSColor.labelColor
  static let textMuted = NSColor.secondaryLabelColor
  /// Decoration only, never text: rules, separators and hairlines.
  static let textFaint = NSColor.tertiaryLabelColor

  // "Light" label variants are drawn as text ON accent-tinted fills; on paper
  // they need the deepened rung of the same hue to stay legible.
  // The paper rungs are sized for the darkest light surface a label meets: a
  // chip on a travertine card, the glass sheet measured at #DFDDD9, or the
  // label's own 12 % tint on any selectable card or glass.
  // The terracotta rungs are the brand hue with only its lightness moved in
  // OKLCH (C 0.13, H 39°): brick, never the brown that blending with black
  // used to give (Founder, 2026-10-26).
  /// The listening state as a label on a fill: the brick rung of the brand
  /// hue, the one custom text colour that stays (dictation is the brand's).
  static let terracottaLight = adaptive(light: NSColor(hex: 0x862C08), dark: NSColor(hex: 0xE9B79F))
  /// Voice routed to an agent at label strength: the same system indigo.
  static let assistiveLight = NSColor.systemIndigo
  /// Attention: a warning that needs an action, work in progress, an
  /// uncertain word. One system orange for the whole family.
  static let amber = NSColor.systemOrange
  static let modeProcessing = NSColor.systemOrange
  static let dangerLight = NSColor.systemRed
  /// Secondary text: the system rung, with its increased-contrast variant.
  static let secondaryInk = NSColor.secondaryLabelColor
  static let amberLight = NSColor.systemOrange
  static let modeProcessingLight = NSColor.systemOrange
  /// A floating panel's edge and every hairline: the system separator.
  static let panelEdge = NSColor.separatorColor

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
  /// Content area of every window outside its sidebar; see `CSPalette.sheet`.
  static var sheet: Color { Color(nsColor: CSPalette.sheet) }

  /// Lifted surface: the system control background (opaque; light in light
  /// mode, a darker well in dark mode). To be judged on the render (Founder,
  /// 2026-11-05): cards over glass lose the white veil they had. The strength
  /// argument is kept for the call sites and ignored.
  static func surfaceRaised(_ a: Double = 0.03) -> Color {
    Color(nsColor: .controlBackgroundColor)
  }

  /// One-point separator: the system separator colour. The strength argument
  /// is kept for its call sites and ignored; the system owns the veil.
  static func hairline(_ a: Double = 0.07) -> Color {
    _ = a
    return Color(nsColor: .separatorColor)
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

  // App semantics (Founder, 2026-11-05): the brand terracotta for the
  // dictation / listening state only; every other meaning is a
  // system colour. Roles marked retired in `CSPalette` point at their
  // replacement and go once their use sites have moved.
  static let terracotta = Color(nsColor: CSPalette.terracotta)  // dictation / listening
  // The listening state as a label on a fill.
  static let terracottaLight = Color(nsColor: CSPalette.terracottaLight)

  // Agent-routed voice — the system indigo
  static let assistive = Color(nsColor: CSPalette.assistive)
  static let assistiveLight = Color(nsColor: CSPalette.assistiveLight)

  static let modeDictation = terracotta
  static let modeAgent = assistive
  static let modeRecording = modeDictation
  static let modeProcessing = Color(nsColor: CSPalette.modeProcessing)
  static let modeReady = secondaryInk
  static let indicatorRecording = Color(nsColor: CSPalette.indicatorRecording)
  static let indicatorAgentQuiet = Color(nsColor: CSPalette.indicatorAgentQuiet)

  // UI chrome — focus and interactive controls follow macOS.
  static let chromeAccent = Color(nsColor: CSPalette.chromeAccent)
  /// The one primary action's fill tint (apply the wash alpha at the call site).
  static let accentWash = Color(nsColor: CSPalette.accentWash)

  // Attention — the system orange: an actionable warning, work in progress,
  // never reassurance, an informational tag or a normal first-run state.
  static let amber = Color(nsColor: CSPalette.amber)
  static let amberLight = Color(nsColor: CSPalette.amberLight)

  // Destructive actions — the system red
  static let danger = Color(nsColor: CSPalette.danger)
  static let dangerLight = Color(nsColor: CSPalette.dangerLight)

  // Success — the system green (see `CSPalette.success`)
  static let success = Color(nsColor: CSPalette.success)

  // Text — the system label rungs
  static let textHigh = Color(nsColor: CSPalette.textHigh)  // headlines
  static let textBody = Color(nsColor: CSPalette.textBody)
  static let textMuted = Color(nsColor: CSPalette.textMuted)
  static let textFaint = Color(nsColor: CSPalette.textFaint)  // decoration only, never text
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
  /// A selected choice row: its wash and its one-point edge, both in the
  /// user's accent (`CSColor.chromeAccent`; 12–13 % plus about 45 %).
  static let selectionWash: Double = 0.13
  static let selectionStroke: Double = 0.45
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
