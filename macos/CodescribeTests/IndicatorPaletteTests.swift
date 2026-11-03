import AppKit
import XCTest

@testable import Codescribe

/// Founder smoke S5 (2026-10-26): the Shortcuts legend showed terracotta,
/// violet and amber while the pointer dot drew red and orange, and it left out
/// half of the dot's states. The legend and the recording red now come from the
/// core's one palette.
@MainActor
final class IndicatorPaletteTests: XCTestCase {
  private func srgb(_ color: NSColor) -> (r: CGFloat, g: CGFloat, b: CGFloat, a: CGFloat) {
    let c = color.usingColorSpace(.sRGB) ?? color
    return (c.redComponent, c.greenComponent, c.blueComponent, c.alphaComponent)
  }

  /// The legend lists every state the core paints, in the core's order.
  func testLegendListsEveryStateOfTheCorePalette() {
    XCTAssertEqual(IndicatorPalette.swatches, indicatorPalette())
    XCTAssertEqual(
      IndicatorPalette.swatches.map(\.state),
      [.recording, .wordsOwed, .processing, .agent, .agentQuiet, .refused])
    let names = IndicatorPalette.swatches.map(\.state.legendName)
    XCTAssertEqual(Set(names).count, names.count, "every state has its own caption")
    XCTAssertFalse(names.contains(where: \.isEmpty))
  }

  /// A legend dot is painted in the swatch's colour, not a brand token.
  func testLegendColoursAreTheDotsNotTheBrandTokens() {
    for swatch in IndicatorPalette.swatches {
      let c = srgb(swatch.nsColor)
      XCTAssertEqual(c.r, CGFloat(swatch.red), accuracy: 0.001, "\(swatch.state)")
      XCTAssertEqual(c.g, CGFloat(swatch.green), accuracy: 0.001, "\(swatch.state)")
      XCTAssertEqual(c.b, CGFloat(swatch.blue), accuracy: 0.001, "\(swatch.state)")
      XCTAssertEqual(c.a, CGFloat(swatch.alpha), accuracy: 0.001, "\(swatch.state)")
    }
    let recording = srgb(IndicatorPalette.swatch(.recording).nsColor)
    for brand in [CSPalette.terracotta, CSPalette.assistive, CSPalette.amber] {
      let b = srgb(brand)
      XCTAssertFalse(
        abs(recording.r - b.r) < 0.01 && abs(recording.g - b.g) < 0.01
          && abs(recording.b - b.b) < 0.01,
        "the recording dot is not a brand token")
    }
    XCTAssertEqual(IndicatorPalette.swatch(.agentQuiet).rim, .agent)
    XCTAssertNil(IndicatorPalette.swatch(.recording).rim)
  }

  /// One red for "recording": the lamp's token is the pointer dot's red, opaque.
  func testRecordingTokenIsThePointerDotsRed() {
    let dot = IndicatorPalette.swatch(.recording)
    let token = srgb(CSPalette.indicatorRecording)
    XCTAssertEqual(token.r, CGFloat(dot.red), accuracy: 0.001)
    XCTAssertEqual(token.g, CGFloat(dot.green), accuracy: 0.001)
    XCTAssertEqual(token.b, CGFloat(dot.blue), accuracy: 0.001)
    XCTAssertEqual(token.a, 1, accuracy: 0.001)
    XCTAssertEqual(token.g, CGFloat(0x3B) / 255.0, accuracy: 0.001, "#FF3B30")

    let quiet = IndicatorPalette.swatch(.agentQuiet)
    let quietToken = srgb(CSPalette.indicatorAgentQuiet)
    XCTAssertEqual(quietToken.r, CGFloat(quiet.red), accuracy: 0.001)
  }
}
