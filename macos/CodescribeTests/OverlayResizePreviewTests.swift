import AppKit
import SwiftUI
import XCTest

@testable import Codescribe

@MainActor
final class OverlayResizePreviewTests: XCTestCase {
  func testSplitBusResizeStopsAtBothPaneFloorsWithoutReflowOrFolding() throws {
    let state = OverlayState.previewFormatted()
    state.setAgentsLayout(.split)
    state.showAgentMonitor()
    let panel = try XCTUnwrap(
      DictationOverlayWindow.make(
        state: state, textScale: TextScaleController(key: "ResizePreview.splitFloor"))
        as? FloatingOverlayPanel)
    defer {
      panel.invalidatePresence()
      panel.close()
    }
    panel.settleFrameTransition()
    let floor = OverlayAgentDrawerLayout.minimumSplitWindowWidth(textScale: 1)
    XCTAssertGreaterThanOrEqual(panel.presentationMinimumSize.width, floor)
    let initial = panel.frame
    XCTAssertTrue(panel.beginUserResize(edge: .right, at: .zero))
    XCTAssertTrue(panel.updateUserResize(to: NSPoint(x: -2_000, y: 0)))
    XCTAssertGreaterThanOrEqual(panel.resizeGestureFrame.width, floor)
    XCTAssertEqual(panel.frame, initial, "the renderer's geometry remains frozen")
    XCTAssertEqual(state.agentsLayout, .split, "resizing never folds either pane")
    panel.endUserResize()
    panel.settleFrameTransition()
    XCTAssertGreaterThanOrEqual(panel.frame.width, floor)
    XCTAssertEqual(state.agentsLayout, .split)
    XCTAssertEqual(
      OverlayAgentDrawerLayout.minimumWindowWidth(layout: .split, textScale: 1.5), floor * 1.5)
    XCTAssertLessThan(
      OverlayAgentDrawerLayout.minimumWindowWidth(layout: .agents, textScale: 1), floor)
    XCTAssertLessThan(
      OverlayAgentDrawerLayout.minimumWindowWidth(layout: .rail, textScale: 1), floor)
  }

  private final class SourceView: NSView {
    var paints = 0
    override func draw(_ dirtyRect: NSRect) {
      paints += 1
      NSColor.systemBlue.setFill()
      bounds.fill()
    }
  }

  func testFrozenContentDoesNotRecaptureOrResizeTheSourceDuringDrag() throws {
    let window = NSPanel(
      contentRect: NSRect(x: 80, y: 80, width: 400, height: 300),
      styleMask: [.borderless, .nonactivatingPanel], backing: .buffered, defer: false)
    window.isReleasedWhenClosed = false
    let source = SourceView(frame: NSRect(x: 0, y: 0, width: 400, height: 300))
    window.contentView = source
    window.alphaValue = 0.8
    let initialFrame = window.frame
    var restorations = 0
    let preview = OverlayResizePreview(
      windows: [window], group: false, headerHeight: 40,
      palette: .resolve(ColorScheme.dark), capturesContent: true,
      restore: { _ in restorations += 1 })
    defer {
      preview.finish()
      window.close()
    }
    let capturedPaints = source.paints
    XCTAssertGreaterThan(capturedPaints, 0)
    XCTAssertEqual(window.alphaValue, 0)
    XCTAssertTrue(window.ignoresMouseEvents)
    for step in 0..<12 {
      preview.update(
        frame: NSRect(
          x: 80, y: 80, width: 400 + CGFloat(step) * 10, height: 300 + CGFloat(step) * 5))
      XCTAssertEqual(window.frame, initialFrame, "Markdown keeps its original layout size")
      XCTAssertEqual(source.paints, capturedPaints, "pointer updates must reuse the capture")
    }
    let drawing = try XCTUnwrap(
      NSApp.windows.filter(\.isVisible).compactMap { $0.contentView as? OverlayResizePreviewView }
        .first { $0.headers.contains { $0?.content != nil } })
    XCTAssertTrue(
      drawing.headers.contains { $0?.content != nil }, "the body must not become a blank plate")
    preview.finish()
    preview.finish()
    XCTAssertEqual(restorations, 1, "finish/cancel restoration is idempotent")
    XCTAssertEqual(window.alphaValue, 0.8)
    XCTAssertFalse(window.ignoresMouseEvents)
  }

  func testFrozenBodyKeepsPixelSizeAndClipsBelowTheHeader() throws {
    let header = NSImage(size: NSSize(width: 200, height: 40), flipped: false) { rect in
      NSColor.green.setFill()
      rect.fill()
      return true
    }
    let body = NSImage(size: NSSize(width: 200, height: 100), flipped: false) { rect in
      NSColor.red.setFill()
      rect.fill()
      NSColor.blue.setFill()
      NSRect(x: 20, y: 0, width: 10, height: 100).fill()
      return true
    }
    let view = OverlayResizePreviewView(
      headers: [.init(image: header, splitX: 100, content: body)], group: false,
      headerHeight: 40, palette: .resolve(ColorScheme.dark), cornerRadius: 0)
    for width in [160 as CGFloat, 400] {
      view.frame = NSRect(x: 0, y: 0, width: width, height: 180)
      let bitmap = try XCTUnwrap(view.bitmapImageRepForCachingDisplay(in: view.bounds))
      view.cacheDisplay(in: view.bounds, to: bitmap)
      let sx = CGFloat(bitmap.pixelsWide) / view.bounds.width
      let sy = CGFloat(bitmap.pixelsHigh) / view.bounds.height
      func colour(_ x: CGFloat, _ yFromTop: CGFloat) throws -> NSColor {
        try XCTUnwrap(
          bitmap.colorAt(x: Int(x * sx), y: Int(yFromTop * sy))?.usingColorSpace(.deviceRGB))
      }
      let stripe = try colour(25, 70)
      XCTAssertGreaterThan(stripe.blueComponent, 0.8, "body pixels must not stretch with width")
      let adjacent = try colour(40, 70)
      XCTAssertGreaterThan(adjacent.redComponent, 0.8)
      let top = try colour(25, 20)
      XCTAssertGreaterThan(top.greenComponent, 0.8, "body cannot paint over header controls")
    }
  }
}
