import AppKit
import XCTest

@testable import Codescribe

/// Locks the adaptive-native palette contract: surfaces, hairlines, and text
/// follow the system appearance; brand and semantic hues stay fixed. These
/// tests resolve the dynamic `NSColor` providers under a forced light/dark
/// appearance and compare exact sRGB components — no rendering, no screenshots.
final class DesignSystemAdaptiveTests: XCTestCase {

  // MARK: - Resolution helpers

  private func resolve(_ color: NSColor, dark: Bool) -> NSColor {
    let appearance = NSAppearance(named: dark ? .darkAqua : .aqua)!
    var resolved = color
    appearance.performAsCurrentDrawingAppearance {
      resolved = NSColor(cgColor: color.cgColor) ?? color
    }
    return resolved.usingColorSpace(.sRGB) ?? resolved
  }

  private func components(
    _ color: NSColor, dark: Bool
  ) -> (r: CGFloat, g: CGFloat, b: CGFloat, a: CGFloat) {
    let c = resolve(color, dark: dark)
    return (c.redComponent, c.greenComponent, c.blueComponent, c.alphaComponent)
  }

  private func luminance(_ color: NSColor, dark: Bool) -> CGFloat {
    let c = components(color, dark: dark)
    func linear(_ v: CGFloat) -> CGFloat {
      v <= 0.03928 ? v / 12.92 : pow((v + 0.055) / 1.055, 2.4)
    }
    return 0.2126 * linear(c.r) + 0.7152 * linear(c.g) + 0.0722 * linear(c.b)
  }

  private func contrast(_ fg: NSColor, _ bg: NSColor, dark: Bool) -> CGFloat {
    let l1 = luminance(fg, dark: dark)
    let l2 = luminance(bg, dark: dark)
    return (max(l1, l2) + 0.05) / (min(l1, l2) + 0.05)
  }

  private func assertDiffers(
    _ color: NSColor, _ message: String, file: StaticString = #filePath, line: UInt = #line
  ) {
    let light = components(color, dark: false)
    let dark = components(color, dark: true)
    let delta =
      abs(light.r - dark.r) + abs(light.g - dark.g) + abs(light.b - dark.b)
      + abs(light.a - dark.a)
    XCTAssertGreaterThan(delta, 0.01, message, file: file, line: line)
  }

  private func assertFixed(
    _ color: NSColor, _ message: String, file: StaticString = #filePath, line: UInt = #line
  ) {
    let light = components(color, dark: false)
    let dark = components(color, dark: true)
    XCTAssertEqual(light.r, dark.r, accuracy: 0.0001, message, file: file, line: line)
    XCTAssertEqual(light.g, dark.g, accuracy: 0.0001, message, file: file, line: line)
    XCTAssertEqual(light.b, dark.b, accuracy: 0.0001, message, file: file, line: line)
    XCTAssertEqual(light.a, dark.a, accuracy: 0.0001, message, file: file, line: line)
  }

  // MARK: - Adaptivity contract

  func testSurfacesAndTextAdaptAcrossAppearances() {
    let adaptiveTokens: [(String, NSColor)] = [
      ("ink", CSPalette.ink),
      ("glassBase", CSPalette.glassBase),
      ("glassUnder", CSPalette.glassUnder),
      ("warmInk", CSPalette.warmInk),
      ("warmDeep", CSPalette.warmDeep),
      ("textHigh", CSPalette.textHigh),
      ("textBody", CSPalette.textBody),
      ("textMuted", CSPalette.textMuted),
      ("textFaint", CSPalette.textFaint),
      ("terracottaLight", CSPalette.terracottaLight),
      ("assistiveLight", CSPalette.assistiveLight),
      ("oliveLight", CSPalette.oliveLight),
      ("oliveStatus", CSPalette.oliveStatus),
      ("amber", CSPalette.amber),
      ("modeProcessing", CSPalette.modeProcessing),
      ("dangerLight", CSPalette.dangerLight),
      ("accentWash", CSPalette.accentWash),
    ]
    for (name, token) in adaptiveTokens {
      assertDiffers(token, "\(name) must resolve differently in light and dark appearances")
    }
  }

  func testBrandAndSemanticHuesAreAppearanceFixed() {
    let fixedTokens: [(String, NSColor)] = [
      ("terracotta", CSPalette.terracotta),
      ("terracottaDeep", CSPalette.terracottaDeep),
      ("terracottaTintBars", CSPalette.terracottaTintBars),
      ("assistive", CSPalette.assistive),
      ("olive", CSPalette.olive),
      ("indicatorRecording", CSPalette.indicatorRecording),
      ("indicatorSilence", CSPalette.indicatorSilence),
      ("danger", CSPalette.danger),
    ]
    for (name, token) in fixedTokens {
      assertFixed(
        token, "\(name) carries brand/semantic meaning and must not follow the appearance")
    }
  }

  func testBrandTerracottaKeepsItsLockedValue() {
    let c = components(CSPalette.terracotta, dark: true)
    XCTAssertEqual(c.r, CGFloat(0xD9) / 255.0, accuracy: 0.001)
    XCTAssertEqual(c.g, CGFloat(0x77) / 255.0, accuracy: 0.001)
    XCTAssertEqual(c.b, CGFloat(0x57) / 255.0, accuracy: 0.001)
  }

  // MARK: - Legibility contract

  func testHeadlineAndBodyTextKeepWcagAaContrastInBothModes() {
    for dark in [false, true] {
      let mode = dark ? "dark" : "light"
      XCTAssertGreaterThanOrEqual(
        contrast(CSPalette.textHigh, CSPalette.ink, dark: dark), 7.0,
        "textHigh on ink must clear AAA in \(mode) mode"
      )
      XCTAssertGreaterThanOrEqual(
        contrast(CSPalette.textBody, CSPalette.ink, dark: dark), 4.5,
        "textBody on ink must clear AA in \(mode) mode"
      )
      XCTAssertGreaterThanOrEqual(
        contrast(CSPalette.textMuted, CSPalette.ink, dark: dark), 4.5,
        "textMuted on ink must clear AA in \(mode) mode"
      )
    }
  }

  func testFaintRungStaysCloserToThePageThanBodyInBothModes() {
    for dark in [false, true] {
      let mode = dark ? "dark" : "light"
      let bodyContrast = contrast(CSPalette.textBody, CSPalette.ink, dark: dark)
      let faintContrast = contrast(CSPalette.textFaint, CSPalette.ink, dark: dark)
      XCTAssertLessThan(
        faintContrast, bodyContrast,
        "the faint rung must read quieter than body text in \(mode) mode"
      )
    }
  }

  // MARK: - Contrast on the surfaces text really meets (audit 2026-10-24)

  /// The surfaces a label meets in the app, composited exactly: the sheet,
  /// a Settings card (`primary` at 5 %), a chip on that card (6 %), the glass
  /// sheet measured on screen (2026-10-20: #DFDDD9 paper, #18181C ink; the
  /// overlay, tray and setup wizard), and a bubble on that glass. A token
  /// that clears 4.5:1 on `ink` alone missed most of these.
  private func realSurfaces(_ name: NSAppearance.Name) -> [(String, RGB)] {
    let dark = isDark(name)
    let sheet = RGB(hex: dark ? SheetShade.todayDark.hex : SheetShade.todayLight.hex)
    let label = resolvedRGBA(.labelColor, name)
    let card = label.over(sheet, alpha: label.alpha * 0.05)
    let chip = label.over(card, alpha: label.alpha * 0.06)
    let glass = RGB(hex: dark ? 0x18181C : 0xDFDDD9)
    let high = resolvedRGBA(CSPalette.textHigh, name)
    return [
      ("sheet", sheet), ("card", card), ("chip", chip), ("glass", glass),
      ("glass bubble", high.over(glass, alpha: 0.06)),
    ]
  }

  private static let auditAppearances: [NSAppearance.Name] = [
    .aqua, .darkAqua, .accessibilityHighContrastAqua, .accessibilityHighContrastDarkAqua,
  ]

  func testSecondaryInkClearsAaOnEveryRealSurface() {
    for name in Self.auditAppearances {
      let ink = resolvedRGBA(CSPalette.secondaryInk, name)
      let system = resolvedRGBA(.secondaryLabelColor, name)
      for (surface, background) in realSurfaces(name) {
        let ratio = ink.over(background, alpha: ink.alpha).contrast(background)
        XCTAssertGreaterThanOrEqual(
          ratio, 4.5, "secondaryInk on \(surface) in \(name.rawValue): \(ratio):1")
        let systemRatio = system.over(background, alpha: system.alpha).contrast(background)
        XCTAssertGreaterThan(
          ratio, systemRatio, "secondaryInk must read stronger than the system rung on \(surface)")
      }
    }
    let plain = resolvedRGBA(CSPalette.secondaryInk, .aqua)
    let raised = resolvedRGBA(CSPalette.secondaryInk, .accessibilityHighContrastAqua)
    XCTAssertGreaterThan(raised.alpha, plain.alpha, "Increase Contrast must raise secondary ink")
  }

  func testTextRolesClearAaOnEveryRealSurfaceAndTheirOwnTint() {
    let roles: [(String, NSColor)] = [
      ("textMuted", CSPalette.textMuted),
      ("terracottaText", CSPalette.terracottaText),
      ("terracottaLight", CSPalette.terracottaLight),
      ("amberLight", CSPalette.amberLight),
      ("dangerLight", CSPalette.dangerLight),
      ("oliveLight", CSPalette.oliveLight),
      ("oliveStatus", CSPalette.oliveStatus),
      ("assistiveLight", CSPalette.assistiveLight),
      ("modeProcessingLight", CSPalette.modeProcessingLight),
      ("lexicon", CSPalette.lexicon),
    ]
    for name in [NSAppearance.Name.aqua, .darkAqua] {
      let surfaces = realSurfaces(name)
      for (role, color) in roles {
        let ink = resolvedRGBA(color, name)
        var backgrounds = surfaces
        // A status word on its own 12 % tint: pills, probe chips, the tray row.
        for (surface, background) in surfaces where surface == "card" || surface == "glass" {
          backgrounds.append(("12 % \(role) tint on \(surface)", ink.over(background, alpha: 0.12)))
        }
        for (surface, background) in backgrounds {
          let ratio = ink.contrast(background)
          XCTAssertGreaterThanOrEqual(
            ratio, 4.5, "\(role) on \(surface) in \(name.rawValue): \(ratio):1")
        }
      }
    }
  }

  // MARK: - Veil polarity

  func testHairlineAndRaisedVeilsFlipPolarityWithTheAppearance() {
    // Dark mode paints veils with white; light mode paints them with ink.
    // Resolve through the same adaptive provider the CSColor helpers use.
    let hairline = CSPalette.adaptive(
      light: NSColor.black.withAlphaComponent(0.11),
      dark: NSColor.white.withAlphaComponent(0.07)
    )
    XCTAssertGreaterThan(luminance(hairline, dark: true), 0.9, "dark-mode veil must be white-based")
    XCTAssertLessThan(luminance(hairline, dark: false), 0.1, "light-mode veil must be ink-based")
  }

  // MARK: - Native window chrome

  func testWindowChromeRungsAreSystemSemantic() {
    // The shared chrome rungs must be the dynamic system colors themselves —
    // a fixed hex here would reintroduce the per-surface appearance wrappers.
    for (name, color) in [
      ("windowCanvas", NSColor.windowBackgroundColor),
      ("codeWell", NSColor.textBackgroundColor),
      ("controlFill", NSColor.controlBackgroundColor),
    ] {
      assertDiffers(color, "\(name) must follow the system appearance")
    }
  }

  // MARK: - One sheet for every content area

  func testSheetIsTheOverlayCanvasInBothAppearances() {
    assertDiffers(CSPalette.sheet, "sheet must follow the system appearance")
    XCTAssertEqual(CSPalette.sheetDarkHex, 0x16161C)
    XCTAssertEqual(
      OverlayAppearancePalette.dark.desktopBackground,
      OverlayColorToken(CSPalette.sheetDarkHex),
      "the overlay canvas and the window sheet must be one colour")
    XCTAssertEqual(
      OverlayAppearancePalette.light.desktopBackground,
      OverlayColorToken(CSPalette.sheetLightHex))
  }
}

/// Exact sRGB composition for the contrast checks above.
private struct RGB {
  var red: Double
  var green: Double
  var blue: Double
  var alpha: Double = 1

  init(red: Double, green: Double, blue: Double, alpha: Double = 1) {
    self.red = red
    self.green = green
    self.blue = blue
    self.alpha = alpha
  }

  init(hex: UInt32) {
    self.init(
      red: Double((hex >> 16) & 0xFF) / 255, green: Double((hex >> 8) & 0xFF) / 255,
      blue: Double(hex & 0xFF) / 255)
  }

  /// This colour at `alpha` over an opaque `background`.
  func over(_ background: RGB, alpha: Double) -> RGB {
    RGB(
      red: red * alpha + background.red * (1 - alpha),
      green: green * alpha + background.green * (1 - alpha),
      blue: blue * alpha + background.blue * (1 - alpha))
  }

  var luminance: Double {
    func linear(_ value: Double) -> Double {
      value <= 0.04045 ? value / 12.92 : pow((value + 0.055) / 1.055, 2.4)
    }
    return 0.2126 * linear(red) + 0.7152 * linear(green) + 0.0722 * linear(blue)
  }

  func contrast(_ other: RGB) -> Double {
    let lighter = max(luminance, other.luminance)
    let darker = min(luminance, other.luminance)
    return (lighter + 0.05) / (darker + 0.05)
  }
}

private func isDark(_ name: NSAppearance.Name) -> Bool {
  NSAppearance(named: name)?.bestMatch(from: [.darkAqua, .aqua]) == .darkAqua
}

private func resolvedRGBA(_ color: NSColor, _ name: NSAppearance.Name) -> RGB {
  var resolved = color
  NSAppearance(named: name)?.performAsCurrentDrawingAppearance {
    resolved = NSColor(cgColor: color.cgColor) ?? color
  }
  let srgb = resolved.usingColorSpace(.sRGB) ?? resolved
  return RGB(
    red: Double(srgb.redComponent), green: Double(srgb.greenComponent),
    blue: Double(srgb.blueComponent), alpha: Double(srgb.alphaComponent))
}
