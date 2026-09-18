import AppKit
import SwiftUI
import XCTest

@testable import Codescribe

@MainActor
final class OverlayChromeV3Tests: XCTestCase {
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
