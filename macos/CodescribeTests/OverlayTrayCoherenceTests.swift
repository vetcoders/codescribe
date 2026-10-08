import SwiftUI
import XCTest

@testable import Codescribe

/// Coherence pins for the overlay + tray visual-language cut.
///
/// The suite follows the `OverlayChromeV3Tests` convention: it reads the
/// surface sources and asserts the invariants that keep Overlay and Tray on
/// the shared design language — palette-owned caution, the CSFont type ramp
/// for text, token-only tray colors, and token corner radii. These are the
/// regression tripwires for drift that a compiler cannot see.
final class OverlayTrayCoherenceTests: XCTestCase {

  // MARK: - Source access (same convention as OverlayChromeV3Tests)

  private var screensRoot: URL {
    URL(fileURLWithPath: #filePath)
      .deletingLastPathComponent()  // CodescribeTests
      .deletingLastPathComponent()  // macos
      .appendingPathComponent("Codescribe/Screens")
  }

  private func source(_ relativePath: String) throws -> String {
    try String(
      contentsOf: screensRoot.appendingPathComponent(relativePath), encoding: .utf8)
  }

  private func sources(in directory: String) throws -> [(name: String, text: String)] {
    let dir = screensRoot.appendingPathComponent(directory)
    return try FileManager.default.contentsOfDirectory(atPath: dir.path)
      .filter { $0.hasSuffix(".swift") }
      .map { (name: $0, text: try String(contentsOf: dir.appendingPathComponent($0), encoding: .utf8)) }
  }

  // MARK: - Caution is a palette role, never a raw system colour

  /// The overlay palette owns an appearance-aware caution token
  /// (`processingStatus`, held to a measured contrast floor by
  /// `OverlayAppearanceTests`). Raw `.orange` bypasses both appearances and
  /// the floor — it is the one hue the overlay must never improvise.
  func testOverlayAndTrayNeverUseRawSystemOrange() throws {
    for directory in ["Overlay", "Tray"] {
      for file in try sources(in: directory) {
        XCTAssertFalse(
          file.text.contains(".foregroundStyle(.orange)"),
          "\(directory)/\(file.name) paints caution with raw .orange; use palette.processingStatus")
      }
    }
  }

  /// Both header diagnostic icons (acoustic warning, preference-save error)
  /// tint from the same appearance-aware caution token.
  func testHeaderDiagnosticsTintFromPaletteCaution() throws {
    let view = try source("Overlay/DictationOverlayView.swift")
    let acoustic = try XCTUnwrap(view.range(of: "waveform.badge.magnifyingglass"))
    let preference = try XCTUnwrap(view.range(of: "exclamationmark.triangle.fill"))
    for anchor in [acoustic, preference] {
      let window = String(view[anchor.lowerBound...].prefix(200))
      XCTAssertTrue(
        window.contains("palette.processingStatus.color"),
        "diagnostic icon must use the palette caution token")
    }
  }

  // MARK: - Popovers and hints speak the brand type ramp

  /// Transient overlay text (tooltips, popover bodies, popover buttons) uses
  /// CSFont — the surface voice. `.font(.system(size:))` on text is drift;
  /// SF Symbol glyphs keep `.system` sizing and are not covered by this pin.
  func testOverlayTransientTextUsesBrandTypeRamp() throws {
    for file in [
      "Overlay/OverlayHoverControl.swift",
      "Overlay/OverlayIntentRail.swift",
      "Overlay/OverlayCoverageStatus.swift",
    ] {
      let text = try source(file)
      XCTAssertFalse(
        text.contains(".font(.system(size:"),
        "\(file) styles text with the system font instead of CSFont")
    }
  }

  /// The evidence chip's words moved onto the brand ramp; only its waveform
  /// glyph keeps system sizing.
  func testEvidenceChipTextUsesBrandTypeRamp() throws {
    let text = try source("Overlay/OverlayEvidenceList.swift")
    let label = try XCTUnwrap(text.range(of: "private func label(count: Int"))
    let body = String(text[label.lowerBound...].prefix(400))
    XCTAssertTrue(body.contains(".font(CSFont.ui(11, .medium))"))
  }

  // MARK: - Tray consumes tokens, never local hex

  /// `TrayLocal.subnote` (#C7CABF) was a mock-only fixed-dark tint competing
  /// with the locked `CSColor` ramp; secondary tray text now uses
  /// `CSColor.textMuted`. No tray file may reintroduce a local hex.
  func testTrayContainsNoLocalHexColors() throws {
    for file in try sources(in: "Tray") {
      XCTAssertFalse(
        file.text.contains("Color(hex:"),
        "Tray/\(file.name) hardcodes a colour; consume CSColor tokens")
    }
  }

  /// Radius literals drift silently; the tray speaks `CSRadius`.
  func testTrayUsesTokenCornerRadii() throws {
    for file in try sources(in: "Tray") {
      XCTAssertFalse(
        file.text.contains("cornerRadius: 8"),
        "Tray/\(file.name) hardcodes a corner radius; use CSRadius.chip")
    }
  }

  /// Secondary tray text sits on the shared muted ramp — one mute colour for
  /// child rows and the Quit label, owned by the design system.
  func testTraySecondaryTextUsesSharedMutedToken() throws {
    XCTAssertTrue(try source("Tray/TrayRow.swift").contains("CSColor.textMuted"))
    XCTAssertFalse(try source("Tray/TrayMenuView.swift").contains("subnoteColor"))
  }

  // MARK: - Behavioural sanity (unchanged grammar, kept under test)

  /// The disclosure idiom the pin file references: right collapsed, down
  /// expanded — the native macOS disclosure gesture.
  func testTrayDisclosureChevronKeepsNativeGesture() {
    XCTAssertEqual(TrayDisclosureChevron.rotationDegrees(expanded: false), 0)
    XCTAssertEqual(TrayDisclosureChevron.rotationDegrees(expanded: true), 90)
  }
}
