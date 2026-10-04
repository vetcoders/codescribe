import AppKit
import XCTest

@testable import Codescribe

/// Locks the Agent chat window's appearance contract: the window uses semantic
/// system colors directly, so these tests resolve the dynamic `NSColor` providers
/// under a forced light/dark appearance and compare exact sRGB components — no
/// rendering, no screenshots.
final class AgentWindowAppearanceTests: XCTestCase {

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

  // MARK: - Semantic ink really follows the system appearance

  func testAgentWindowSemanticColorsAdaptAcrossAppearances() {
    let semanticColors: [(String, NSColor)] = [
      ("labelColor", .labelColor),
      ("secondaryLabelColor", .secondaryLabelColor),
      ("placeholderTextColor", .placeholderTextColor),
      ("windowBackgroundColor", .windowBackgroundColor),
      ("textBackgroundColor", .textBackgroundColor),
      ("controlBackgroundColor", .controlBackgroundColor),
    ]
    for (name, color) in semanticColors {
      assertDiffers(
        color,
        "Agent window semantic color \(name) must resolve differently in .aqua vs .darkAqua"
      )
    }
  }

  // MARK: - Brand stays appearance-fixed

  func testBrandTerracottaIsAppearanceFixed() {
    let color = NSColor(CSColor.terracotta)
    let dark = components(color, dark: true)
    let light = components(color, dark: false)

    XCTAssertEqual(dark.r, CGFloat(0xD9) / 255.0, accuracy: 0.001)
    XCTAssertEqual(dark.g, CGFloat(0x77) / 255.0, accuracy: 0.001)
    XCTAssertEqual(dark.b, CGFloat(0x57) / 255.0, accuracy: 0.001)

    XCTAssertEqual(light.r, dark.r, accuracy: 0.0001)
    XCTAssertEqual(light.g, dark.g, accuracy: 0.0001)
    XCTAssertEqual(light.b, dark.b, accuracy: 0.0001)
  }

  // MARK: - Code-block themes guard legibility

  func testCodeThemesDifferAndCarryKeyTokens() {
    XCTAssertNotEqual(CodeTheme.lightCSS, CodeTheme.darkCSS)
    XCTAssertTrue(CodeTheme.lightCSS.contains("#090A0D"))
    XCTAssertTrue(CodeTheme.darkCSS.contains("#D97757"))
  }

  // MARK: - Composer caret stays distinct from text ink

  func testComposerCaretDoesNotMatchSemanticTextInk() {
    let caret = NSColor(CSColor.terracotta)
    for dark in [false, true] {
      let mode = dark ? "darkAqua" : "aqua"
      XCTAssertFalse(
        caret.isEqual(resolve(.labelColor, dark: dark)),
        "composer caret must not equal labelColor under \(mode)"
      )
    }
  }
}
