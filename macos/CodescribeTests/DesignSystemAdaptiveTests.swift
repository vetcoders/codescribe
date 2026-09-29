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
      ("amber", CSPalette.amber),
      ("modeProcessing", CSPalette.modeProcessing),
      ("dangerLight", CSPalette.dangerLight),
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
}
