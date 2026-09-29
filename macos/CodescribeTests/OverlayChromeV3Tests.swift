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
    XCTAssertTrue(close.contains("size: 7"))
    XCTAssertFalse(close.contains("compact ?"), "Close size must not depend on header width")
    XCTAssertFalse(close.contains("state.mode"), "Close must not signal engine state")
    XCTAssertTrue(close.contains(".scaleEffect(closeDotHovered ? 1.15 : 1)"))
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

  func testPointerEntryDoesNotRevealActions() {
    var actions = OverlayActionsPresentation()
    actions.pointerChanged(true)
    XCTAssertEqual(actions.phase, .idle)
    actions.pointerChanged(false)
    XCTAssertNil(actions.hideDeadline)
    actions.toggle()
    XCTAssertEqual(actions.phase, .open)
  }

  func testKeyboardActivationOpensActionsWithoutPointer() {
    var actions = OverlayActionsPresentation()
    actions.toggle()
    XCTAssertEqual(actions.phase, .open)
    XCTAssertNotNil(actions.hideDeadline)
    actions.dismiss()
    XCTAssertEqual(actions.phase, .idle)
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
      XCTAssertGreaterThan(rect.height, 100)
      XCTAssertEqual(rect.minY, root.bounds.minY, accuracy: 2)
      XCTAssertEqual(rect.maxY, root.bounds.maxY, accuracy: 2)
      XCTAssertGreaterThan(scroll.contentView.contentInsets.top, 30)
      XCTAssertGreaterThan(scroll.contentView.contentInsets.bottom, 30)
      XCTAssertFalse(scroll.drawsBackground)
      XCTAssertFalse(text.drawsBackground)
      XCTAssertLessThanOrEqual(rect.maxX, root.bounds.maxX)
    }
  }

  func testLongFooterMessageDoesNotStealTranscriptHeight() throws {
    for width in [320.0, 900.0] {
      let state = OverlayState.previewFormatted()
      let panel = DictationOverlayWindow.make(
        state: state,
        textScale: TextScaleController(key: "OverlayFooterBudgetTests.\(width)"))
      defer {
        panel.orderOut(nil)
        (panel as? FloatingOverlayPanel)?.invalidatePresence()
      }
      panel.setContentSize(NSSize(width: width, height: 350))
      panel.orderFrontRegardless()
      let root = try XCTUnwrap(panel.contentView)
      root.layoutSubtreeIfNeeded()
      RunLoop.main.run(until: Date().addingTimeInterval(0.05))
      root.layoutSubtreeIfNeeded()
      let text = try XCTUnwrap(descendant(LiveTranscriptNativeTextView.self, in: root))
      let scroll = try XCTUnwrap(text.enclosingScrollView)
      let initialInset = scroll.contentView.contentInsets.bottom
      let originalText = text.string
      state.showToast("Copied")
      RunLoop.main.run(until: Date().addingTimeInterval(0.05))
      root.layoutSubtreeIfNeeded()
      let messageInset = scroll.contentView.contentInsets.bottom
      XCTAssertGreaterThan(messageInset, initialInset, "No notice must leave no reserved row")
      XCTAssertLessThanOrEqual(messageInset - initialInset, 27)
      state.showToast(String(repeating: "A long message that must stay in one row. ", count: 20))
      RunLoop.main.run(until: Date().addingTimeInterval(0.05))
      root.layoutSubtreeIfNeeded()
      XCTAssertEqual(scroll.contentView.contentInsets.bottom, messageInset, accuracy: 1)
      XCTAssertEqual(text.string, originalText)
    }
  }

  private func descendant<T: NSView>(_ type: T.Type, in root: NSView) -> T? {
    if let view = root as? T { return view }
    return root.subviews.lazy.compactMap { self.descendant(type, in: $0) }.first
  }

  func testCanvasSamplesDesktopWithoutInterceptingInput() throws {
    for scheme in [ColorScheme.light, .dark] {
      let root = NSHostingView(
        rootView: OverlayCanvasBackdrop(palette: .resolve(scheme), reduceTransparency: false)
          .environment(\.colorScheme, scheme)
      )
      root.frame = NSRect(x: 0, y: 0, width: 320, height: 200)
      root.layoutSubtreeIfNeeded()
      if #available(macOS 26, *) {
        let glass = try XCTUnwrap(descendant(OverlayDesktopGlassView.self, in: root))
        XCTAssertEqual(glass.style, .regular)
        XCTAssertNil(glass.tintColor)
        XCTAssertNil(glass.hitTest(NSPoint(x: 100, y: 100)))
        XCTAssertNil(descendant(OverlayDesktopEffectView.self, in: root))
      } else {
        let effect = try XCTUnwrap(descendant(OverlayDesktopEffectView.self, in: root))
        XCTAssertEqual(effect.blendingMode, .behindWindow)
        XCTAssertEqual(effect.state, .active)
        XCTAssertNil(effect.hitTest(NSPoint(x: 100, y: 100)))
      }
    }
  }

  func testReduceTransparencyRemovesDesktopSampling() {
    let root = NSHostingView(
      rootView: OverlayCanvasBackdrop(palette: .dark, reduceTransparency: true)
    )
    root.frame = NSRect(x: 0, y: 0, width: 320, height: 200)
    root.layoutSubtreeIfNeeded()
    XCTAssertNil(descendant(OverlayDesktopEffectView.self, in: root))
    if #available(macOS 26, *) {
      XCTAssertNil(descendant(OverlayDesktopGlassView.self, in: root))
    }
  }
}
