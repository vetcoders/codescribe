import SwiftUI
import XCTest

@testable import Codescribe

final class OverlayAppearanceTests: XCTestCase {
  func testColorSchemeSelectsTheMatchingPalette() {
    XCTAssertEqual(OverlayAppearancePalette.resolve(ColorScheme.light), .light)
    XCTAssertEqual(OverlayAppearancePalette.resolve(ColorScheme.dark), .dark)
  }

  func testSheetShadowStaysSubtleInBothAppearances() {
    for palette in [OverlayAppearancePalette.light, .dark] {
      XCTAssertLessThanOrEqual(palette.shadowOpacity, 0.20)
    }
  }

  // Glass contrast needs a live compositor check against actual desktop content.
  func testTextAndPhaseTokensMeetReduceTransparencyContrastContract() {
    for palette in [OverlayAppearancePalette.light, .dark] {
      assertContrast(palette.primaryText, on: palette, minimum: 4.5, role: "primary")
      assertContrast(palette.bodyText, on: palette, minimum: 4.5, role: "body")
      assertContrast(palette.listeningStatus, on: palette, minimum: 4.5, role: "listening")
    }
  }

  /// Resting header glyphs are muted grey; a status a step away from that grey
  /// makes a resting control read as "healthy" (the overlay's success once took
  /// the app's label olive, 0.145 from the muted grey on ink, 2026-10-18).
  func testStatusRolesStayClearOfTheMutedGreyOnBothAppearances() {
    func distance(_ lhs: OverlayColorToken, _ rhs: OverlayColorToken) -> Double {
      let a = lhs.components
      let b = rhs.components
      let red = a.red - b.red
      let green = a.green - b.green
      let blue = a.blue - b.blue
      return (red * red + green * green + blue * blue).squareRoot()
    }
    for palette in [OverlayAppearancePalette.light, .dark] {
      for (role, token) in [
        ("success", palette.successStatus),
        ("processing", palette.processingStatus),
        ("agentAccent", palette.agentAccent),
      ] {
        XCTAssertGreaterThan(
          distance(token, palette.mutedText), 0.2, "\(role) sits too near the muted grey")
      }
    }
  }

  func testUncertainWordSharesAttentionOrangeAndStaysDistinctFromErrorAndLexicon() {
    for palette in [OverlayAppearancePalette.light, .dark] {
      XCTAssertEqual(palette.uncertainWord, palette.processingStatus)
      XCTAssertNotEqual(palette.uncertainWord, palette.errorStatus)
      XCTAssertNotEqual(palette.uncertainWord, palette.lexiconMarker)
    }
  }

  /// The coverage pill's microphone keeps the warning's orange hue and clears
  /// the 3:1 non-text floor on every sheet shade of both looks (Bruno,
  /// 2026-11-07); the raw orange stays under it on the light sheets.
  func testCoverageMicrophoneGlyphClearsThreeToOneInTheSameOrange() {
    func hue(_ token: OverlayColorToken) -> CGFloat {
      var hue: CGFloat = 0
      token.nsColor.getHue(&hue, saturation: nil, brightness: nil, alpha: nil)
      return hue
    }
    for palette in [OverlayAppearancePalette.light, .dark] {
      let shades = palette.appearance == .dark ? SheetShade.darkChoices : SheetShade.lightChoices
      let glyph = palette.microphoneWarning
      XCTAssertEqual(hue(glyph), hue(palette.processingStatus), accuracy: 0.01)
      XCTAssertEqual(glyph.alpha, palette.processingStatus.alpha)
      for shade in shades {
        let sheet = OverlayColorToken(shade.hex)
        let before = OverlayColorToken.contrastRatio(
          foreground: palette.processingStatus, surface: sheet, background: sheet)
        let after = OverlayColorToken.contrastRatio(
          foreground: glyph, surface: sheet, background: sheet)
        XCTAssertGreaterThanOrEqual(
          after, 3.0, "\(palette.appearance) \(shade): \(before):1 -> \(after):1")
      }
    }
    let dark = OverlayAppearancePalette.dark
    let light = OverlayAppearancePalette.light
    XCTAssertEqual(dark.microphoneWarning, dark.processingStatus)
    XCTAssertNotEqual(light.microphoneWarning, light.processingStatus)
  }

  private func assertContrast(
    _ foreground: OverlayColorToken,
    on palette: OverlayAppearancePalette,
    minimum: Double,
    role: String,
    file: StaticString = #filePath,
    line: UInt = #line
  ) {
    let ratio = OverlayColorToken.contrastRatio(
      foreground: foreground,
      surface: palette.desktopBackground,
      background: palette.desktopBackground
    )
    XCTAssertGreaterThanOrEqual(
      ratio,
      minimum,
      "\(palette.appearance) \(role) contrast was \(ratio):1",
      file: file,
      line: line
    )
  }
}
