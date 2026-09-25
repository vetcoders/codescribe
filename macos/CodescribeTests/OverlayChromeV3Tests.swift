import AppKit
import SwiftUI
import XCTest

@testable import Codescribe

@MainActor
final class OverlayChromeV3Tests: XCTestCase {
  func testCloseControlShowsCrossOnlyOnHoverAndKeepsCloseNameAtBothTextScales() throws {
    let source = try String(
      contentsOf: URL(fileURLWithPath: #filePath)
        .deletingLastPathComponent()
        .deletingLastPathComponent()
        .appendingPathComponent("Codescribe/Screens/Overlay/DictationOverlayView.swift"),
      encoding: .utf8)
    let header = try XCTUnwrap(source.range(of: "private func justifiedHeader(compact: Bool)"))
    let tail = String(source[header.lowerBound...])
    let button = try XCTUnwrap(tail.range(of: "Button {\n          state.relayIntent(.close)"))
    let wordmark = try XCTUnwrap(tail.range(of: "Text(\"codescribe\")"))
    XCTAssertLessThan(button.lowerBound, wordmark.lowerBound)
    let close = String(tail[button.lowerBound..<wordmark.lowerBound])
    XCTAssertTrue(close.contains("ModeDot("))
    XCTAssertTrue(close.contains("size: closeDotHovered ? (compact ? 7.5 : 10) : (compact ? 5.25 : 7)"))
    XCTAssertTrue(close.contains("if closeDotHovered {"))
    XCTAssertTrue(close.contains("OverlayCloseCross()"))
    XCTAssertTrue(close.contains(".onHover { closeDotHovered = $0 }"))
    XCTAssertFalse(close.contains(".frame("), "A frame would move the dot")
    XCTAssertFalse(tail.contains("Text(\"×\")"))
    XCTAssertTrue(tail.contains(".accessibilityLabel(OverlayIntent.close.accessibilityLabel)"))
    for scale in [TextScaleController.minScale, CGFloat(1)] {
      XCTAssertEqual(OverlayIntent.close.accessibilityLabel, "Close overlay", "scale \(scale)")
      XCTAssertEqual(TextScaleController.clamp(scale), scale)
    }
  }

  func testPointerEntryAndExitRevealWithoutAReservedDock() {
    XCTAssertFalse(
      OverlayChromeVisibility.actionsVisible(
        pointerInside: false, keyboardFocus: false, voiceOver: false))
    XCTAssertTrue(
      OverlayChromeVisibility.actionsVisible(
        pointerInside: true, keyboardFocus: false, voiceOver: false))
    XCTAssertFalse(
      OverlayChromeVisibility.actionsVisible(
        pointerInside: false, keyboardFocus: false, voiceOver: false))
  }

  func testKeyboardAndVoiceOverKeepActionsVisibleWithoutPointer() {
    XCTAssertTrue(
      OverlayChromeVisibility.actionsVisible(
        pointerInside: false, keyboardFocus: true, voiceOver: false))
    XCTAssertTrue(
      OverlayChromeVisibility.actionsVisible(
        pointerInside: false, keyboardFocus: false, voiceOver: true))
  }

  func testCloseUsesProductionIntentRoute() {
    let state = OverlayState.previewFormatted()
    var closed = false
    state.onClose = { closed = true }
    XCTAssertTrue(OverlayIntentRail.projectedIntents(for: state).contains(.close))
    state.relayIntent(.close)
    XCTAssertTrue(closed)
  }

  func testRealPanelKeepsTranscriptWithinBothSupportedWidths() throws {
    for width in [320.0, 900.0] {
      let panel = DictationOverlayWindow.make(
        state: .previewListening(),
        textScale: TextScaleController(key: "OverlayChromeV3Tests.\(width)"))
      defer {
        panel.orderOut(nil)
        (panel as? FloatingOverlayPanel)?.invalidatePresence()
      }
      panel.setContentSize(NSSize(width: width, height: 280))
      panel.orderFrontRegardless()
      let root = try XCTUnwrap(panel.contentView)
      root.layoutSubtreeIfNeeded()
      RunLoop.main.run(until: Date().addingTimeInterval(0.05))
      root.layoutSubtreeIfNeeded()
      let text = try XCTUnwrap(descendant(LiveTranscriptNativeTextView.self, in: root))
      let scroll = try XCTUnwrap(text.enclosingScrollView)
      let rect = root.convert(scroll.bounds, from: scroll)
      XCTAssertGreaterThan(rect.width, width - 65)
      XCTAssertGreaterThan(rect.height, 170)
      XCTAssertLessThan(rect.minY, 24)
      XCTAssertLessThanOrEqual(rect.maxX, root.bounds.maxX)
    }
  }

  private func descendant<T: NSView>(_ type: T.Type, in root: NSView) -> T? {
    if let view = root as? T { return view }
    return root.subviews.lazy.compactMap { self.descendant(type, in: $0) }.first
  }
}
