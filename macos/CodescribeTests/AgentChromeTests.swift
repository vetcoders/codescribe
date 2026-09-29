import AppKit
import SwiftUI
import XCTest

@testable import Codescribe

final class AgentChromeTests: XCTestCase {
  func testWindowFollowsSystemAppearance() {
    XCTAssertTrue(AgentChrome.followsSystemAppearance)
    XCTAssertNil(AgentChrome.forcedColorScheme)
    XCTAssertFalse(AgentChrome.paintsFixedDarkWindow)
    XCTAssertFalse(AgentChrome.Sidebar.paintsCustomWash)
  }

  func testSidebarAndEmptyStateStayRestrained() {
    XCTAssertEqual(AgentChrome.Sidebar.title, "codescribe")
    XCTAssertLessThanOrEqual(AgentChrome.Sidebar.titleSize, 13)
    XCTAssertEqual(AgentChrome.Sidebar.emptyTitle, "New thread")
    XCTAssertFalse(AgentChrome.Sidebar.emptyDetail.isEmpty)
    XCTAssertLessThan(AgentChrome.Sidebar.emptyDetail.count, 140)
    XCTAssertGreaterThanOrEqual(AgentChrome.Sidebar.rowVerticalPadding, 4)
    XCTAssertLessThanOrEqual(AgentChrome.Sidebar.rowVerticalPadding, 8)
  }

  func testComposerAndTranscriptInkAreSemanticLabels() {
    XCTAssertTrue(AgentChrome.composerTextColor().isEqual(.labelColor))
    XCTAssertTrue(AgentChrome.composerPlaceholderColor().isEqual(.placeholderTextColor))
    XCTAssertTrue(AgentChrome.transcriptTextColor().isEqual(.labelColor))
    let fixedCream = NSColor(srgbRed: 0xE9 / 255, green: 0xE7 / 255, blue: 0xE0 / 255, alpha: 1)
    XCTAssertFalse(AgentChrome.composerTextColor().isEqual(fixedCream))
    XCTAssertFalse(AgentChrome.transcriptTextColor().isEqual(fixedCream))
  }

  func testCaretAndBrandLabelStayTerracotta() {
    let caret = srgb(AgentChrome.composerCaretColor())
    let brand = srgb(NSColor(AgentChrome.brandLabel))
    let terracotta = srgb(NSColor(CSColor.terracotta))
    XCTAssertEqual(caret.r, terracotta.r, accuracy: 0.02)
    XCTAssertEqual(caret.g, terracotta.g, accuracy: 0.02)
    XCTAssertEqual(caret.b, terracotta.b, accuracy: 0.02)
    XCTAssertEqual(brand.r, terracotta.r, accuracy: 0.02)
    XCTAssertEqual(brand.g, terracotta.g, accuracy: 0.02)
    XCTAssertEqual(brand.b, terracotta.b, accuracy: 0.02)
    XCTAssertFalse(AgentChrome.composerTextColor().isEqual(AgentChrome.composerCaretColor()))
  }

  func testCodePaletteTracksLightAndDark() {
    XCTAssertEqual(AgentChrome.CodePalette.resolve(.light), .light)
    XCTAssertEqual(AgentChrome.CodePalette.resolve(.dark), .dark)
    XCTAssertFalse(AgentChrome.CodePalette.light.usesDarkCSS)
    XCTAssertTrue(AgentChrome.CodePalette.dark.usesDarkCSS)
    XCTAssertEqual(
      AgentChrome.codeCSS(dark: AgentChrome.CodePalette.light.usesDarkCSS),
      CodeTheme.lightCSS
    )
    XCTAssertEqual(
      AgentChrome.codeCSS(dark: AgentChrome.CodePalette.dark.usesDarkCSS),
      CodeTheme.darkCSS
    )
    XCTAssertTrue(CodeTheme.lightCSS.contains("#090A0D"))
    XCTAssertTrue(CodeTheme.darkCSS.contains("#D97757"))
    XCTAssertNotEqual(CodeTheme.lightCSS, CodeTheme.darkCSS)
  }

  private func srgb(_ color: NSColor) -> (r: CGFloat, g: CGFloat, b: CGFloat) {
    let resolved = color.usingColorSpace(.sRGB) ?? color
    return (resolved.redComponent, resolved.greenComponent, resolved.blueComponent)
  }
}
