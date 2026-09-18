import SwiftUI
import XCTest

@testable import Codescribe

final class OverlayAppearanceTests: XCTestCase {
  func testColorSchemeSelectsTheMatchingPalette() {
    XCTAssertEqual(OverlayAppearancePalette.resolve(ColorScheme.light), .light)
    XCTAssertEqual(OverlayAppearancePalette.resolve(ColorScheme.dark), .dark)
  }

  func testSheetTintStaysLightAndTransparentInBothAppearances() {
    for palette in [OverlayAppearancePalette.light, .dark] {
      XCTAssertGreaterThan(palette.surfaceTint.alpha, 0)
      XCTAssertLessThanOrEqual(
        palette.surfaceTint.alpha,
        0.18,
        "the stabilizing tint must not turn the material into an opaque panel"
      )
      XCTAssertLessThanOrEqual(palette.shadowOpacity, 0.20)
    }
  }

  func testTextAndPhaseTokensMeetTheContrastContract() {
    for palette in [OverlayAppearancePalette.light, .dark] {
      assertContrast(palette.primaryText, on: palette, minimum: 4.5, role: "primary")
      assertContrast(palette.bodyText, on: palette, minimum: 4.5, role: "body")
      assertContrast(palette.mutedText, on: palette, minimum: 3.0, role: "muted")

      for (role, token) in [
        ("listening", palette.listeningStatus),
        ("processing", palette.processingStatus),
        ("success", palette.successStatus),
        ("neutral", palette.neutralStatus),
        ("error", palette.errorStatus),
      ] {
        assertContrast(token, on: palette, minimum: 4.5, role: role)
      }
    }
  }

  // MARK: Refusal recovery (rc-w2-refusal-ui) — UNRUN under W2

  /// The negative control is the point of this test. A refused take reuses a
  /// token, so the risk is not that the colour is wrong — it is that some
  /// later tidy-up folds `coverageRefused` in with `formatted` and the dot
  /// turns green over a seal that was refused.
  func testRefusedCoverageNeverBorrowsTheSuccessToken() {
    for palette in [OverlayAppearancePalette.light, .dark] {
      XCTAssertNotEqual(
        palette.statusToken(for: .coverageRefused),
        palette.successStatus,
        "\(palette.appearance) painted refused coverage with the success token"
      )
      XCTAssertEqual(
        palette.statusToken(for: .coverageRefused),
        palette.processingStatus,
        "refused coverage is the caution amber, already contrast-verified above"
      )
      XCTAssertEqual(palette.statusToken(for: .formatted), palette.successStatus)
    }
  }

  /// Every phase resolves to a token, and no two terminal outcomes that mean
  /// different things share one. Listening/finalizing/refused deliberately do
  /// share amber; the assertion below fences the three that must stay apart.
  func testEveryPhaseResolvesADistinctTerminalToken() {
    for palette in [OverlayAppearancePalette.light, .dark] {
      let terminals: [OverlayMode] = [.formatted, .coverageRefused, .noSpeech, .error]
      let tokens = terminals.map { palette.statusToken(for: $0) }
      XCTAssertEqual(
        Set(tokens.map(\.rgb)).count,
        terminals.count,
        "\(palette.appearance) collapsed two different terminal outcomes onto one token"
      )
      XCTAssertEqual(palette.statusToken(for: .listening), palette.listeningStatus)
      XCTAssertEqual(palette.statusToken(for: .finalizing), palette.processingStatus)
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
      surface: palette.surfaceTint,
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
