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
      assertContrast(palette.mutedText, on: palette, minimum: 3.0, role: "muted")

      for (role, token) in [
        ("listening", palette.listeningStatus),
        ("processing", palette.processingStatus),
        ("success", palette.successStatus),
        ("error", palette.errorStatus),
      ] {
        assertContrast(token, on: palette, minimum: 4.5, role: role)
      }
    }
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
