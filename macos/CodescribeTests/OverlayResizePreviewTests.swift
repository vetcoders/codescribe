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
    let drawing = try previewDrawing()
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

  func testRepeatedPreviewPaintDoesNotAccumulateEarlierFrames() throws {
    let view = OverlayResizePreviewView(
      headers: [nil], group: false, headerHeight: 40,
      palette: .resolve(ColorScheme.light), cornerRadius: 10)
    view.frame = NSRect(x: 0, y: 0, width: 120, height: 100)
    let bitmap = try XCTUnwrap(
      NSBitmapImageRep(
        bitmapDataPlanes: nil, pixelsWide: 120, pixelsHigh: 100,
        bitsPerSample: 8, samplesPerPixel: 4, hasAlpha: true, isPlanar: false,
        colorSpaceName: .deviceRGB, bytesPerRow: 0, bitsPerPixel: 0))
    let context = try XCTUnwrap(NSGraphicsContext(bitmapImageRep: bitmap))
    NSGraphicsContext.saveGraphicsState()
    defer { NSGraphicsContext.restoreGraphicsState() }
    NSGraphicsContext.current = context
    NSColor.clear.setFill()
    view.bounds.fill(using: .copy)
    view.draw(view.bounds)
    let firstAlpha = try XCTUnwrap(bitmap.colorAt(x: 30, y: 50)).alphaComponent
    for _ in 0..<5 { view.draw(view.bounds) }
    let repeatedAlpha = try XCTUnwrap(bitmap.colorAt(x: 30, y: 50)).alphaComponent
    XCTAssertEqual(
      repeatedAlpha, firstAlpha, accuracy: 0.01,
      "A buffered preview must replace the previous frame, not composite into it")
  }

  func testFrozenCaptureContainsForegroundWithoutTheOldCanvas() throws {
    let source = NSHostingView(
      rootView:
        OverlayCanvasSurface(palette: .resolve(ColorScheme.light)) {
          Rectangle().fill(Color.red).frame(width: 12)
            .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .leading)
        }
        .environment(\.csReduceTransparencyOverride, false)
        .environment(\.csContrastOverride, .standard))
    source.sizingOptions = []
    let window = NSPanel(
      contentRect: NSRect(x: 80, y: 80, width: 240, height: 200),
      styleMask: [.borderless, .nonactivatingPanel], backing: .buffered, defer: false)
    window.isReleasedWhenClosed = false
    window.isOpaque = false
    window.backgroundColor = .clear
    window.contentView = source
    source.layoutSubtreeIfNeeded()
    let preview = OverlayResizePreview(
      windows: [window], group: false, headerHeight: 40,
      palette: .resolve(ColorScheme.light), capturesContent: true)
    defer {
      preview.finish()
      window.close()
    }
    preview.update(frame: window.frame)
    let drawing = try previewDrawing()
    let header = try XCTUnwrap(drawing.headers.compactMap { $0 }.first)
    let content = try XCTUnwrap(header.content)
    let bitmap = try XCTUnwrap(content.tiffRepresentation.flatMap(NSBitmapImageRep.init(data:)))
    let xScale = CGFloat(bitmap.pixelsWide) / content.size.width
    let y = bitmap.pixelsHigh / 2
    let foreground = try XCTUnwrap(bitmap.colorAt(x: Int(6 * xScale), y: y))
    let canvas = try XCTUnwrap(bitmap.colorAt(x: bitmap.pixelsWide / 2, y: y))
    XCTAssertGreaterThan(foreground.alphaComponent, 0.9, "The frozen text/control layer stays")
    XCTAssertLessThan(
      canvas.alphaComponent, 0.05,
      "Old glass and its veil cannot become a smaller solid canvas inside the new frame")
    for point in [
      NSPoint(x: content.size.width - 0.5, y: content.size.height / 2),
      NSPoint(x: content.size.width / 2, y: content.size.height - 0.5),
      NSPoint(x: content.size.width - 6, y: content.size.height - 6),
    ] {
      let oldEdge = try XCTUnwrap(
        bitmap.colorAt(
          x: min(bitmap.pixelsWide - 1, Int(point.x * xScale)),
          y: min(
            bitmap.pixelsHigh - 1, Int(point.y * CGFloat(bitmap.pixelsHigh) / content.size.height)))
      )
      XCTAssertLessThan(
        oldEdge.alphaComponent, 0.05,
        "The old perimeter must not survive inside the enlarged material")
    }
  }

  func testExpandedPreviewMaterialFollowsTheCurrentFrameWithoutResizingSource() throws {
    let window = NSPanel(
      contentRect: NSRect(x: 80, y: 80, width: 400, height: 300),
      styleMask: [.borderless, .nonactivatingPanel], backing: .buffered, defer: false)
    window.isReleasedWhenClosed = false
    window.isOpaque = false
    window.backgroundColor = .clear
    window.contentView = SourceView(frame: NSRect(x: 0, y: 0, width: 400, height: 300))
    let original = window.frame
    let preview = OverlayResizePreview(
      windows: [window], group: false, headerHeight: 40,
      palette: .resolve(ColorScheme.light), capturesContent: true)
    defer {
      preview.finish()
      window.close()
    }
    for size in [NSSize(width: 600, height: 420), NSSize(width: 300, height: 240)] {
      preview.update(frame: NSRect(origin: original.origin, size: size))
      let drawing = try previewDrawing()
      let root = try XCTUnwrap(drawing.window?.contentView)
      if #available(macOS 26, *) {
        let glass = try XCTUnwrap(
          descendants(root).compactMap { $0 as? OverlayDesktopGlassView }.first)
        XCTAssertEqual(glass.convert(glass.bounds, to: root), root.bounds)
      } else {
        XCTAssertTrue(descendants(root).contains { $0 is OverlayDesktopEffectView })
      }
      XCTAssertEqual(window.frame, original, "No per-pointer source/Markdown layout")
    }
  }

  func testExpandedPreviewRetainsTheSourcesReducedTransparencyPolicy() throws {
    let palette = OverlayAppearancePalette.resolve(ColorScheme.light)
    let source = NSHostingView(
      rootView:
        OverlayCanvasSurface(palette: palette) { Color.clear }
        .environment(\.csReduceTransparencyOverride, true))
    source.sizingOptions = []
    let window = NSPanel(
      contentRect: NSRect(x: 80, y: 80, width: 240, height: 200),
      styleMask: [.borderless, .nonactivatingPanel], backing: .buffered, defer: false)
    window.isReleasedWhenClosed = false
    window.isOpaque = false
    window.backgroundColor = .clear
    window.contentView = source
    source.layoutSubtreeIfNeeded()
    let backing = try XCTUnwrap(OverlayCanvasBackdropView.all(in: source).first)
    XCTAssertTrue(backing.configuration.opaque)
    let wasHidden = backing.isHidden
    let preview = OverlayResizePreview(
      windows: [window], group: false, headerHeight: 40,
      palette: palette, capturesContent: true)
    defer {
      preview.finish()
      window.close()
    }
    XCTAssertEqual(backing.isHidden, wasHidden, "Capture must restore the source's visibility")
    preview.update(frame: NSRect(x: 80, y: 80, width: 500, height: 360))
    let drawing = try previewDrawing()
    let root = try XCTUnwrap(drawing.window?.contentView)
    let resizedBacking = try XCTUnwrap(OverlayCanvasBackdropView.all(in: root).first)
    XCTAssertEqual(resizedBacking.configuration, backing.configuration)
    XCTAssertTrue(
      resizedBacking.configuration.opaque, "Respect the source override, not desktop defaults")
    let bitmap = try XCTUnwrap(root.bitmapImageRepForCachingDisplay(in: root.bounds))
    let context = try XCTUnwrap(NSGraphicsContext(bitmapImageRep: bitmap))
    root.displayIfNeeded()
    // Composite the native layers as the window does. Recursive cacheDisplay
    // flattens a transparent child's clear into its siblings' drawing context.
    let layer = try XCTUnwrap(root.layer)
    let clip = context.cgContext.boundingBoxOfClipPath
    context.cgContext.scaleBy(
      x: clip.width / root.bounds.width, y: clip.height / root.bounds.height)
    layer.render(in: context.cgContext)
    context.flushGraphics()
    func describe(_ view: NSView) -> String {
      "\(type(of: view)) frame=\(view.frame) hidden=\(view.isHidden) alpha=\(view.alphaValue) "
        + "layerFrame=\(String(describing: view.layer?.frame)) "
        + "layerBounds=\(String(describing: view.layer?.bounds)) "
        + "opacity=\(String(describing: view.layer?.opacity)) "
        + "background=\(String(describing: view.layer?.backgroundColor))"
    }
    print("RESIZE_OPAQUE_DIAGNOSTIC clip=\(clip) bitmap=\(bitmap.pixelsWide)x\(bitmap.pixelsHigh)")
    for item in descendants(root) { print("RESIZE_OPAQUE_DIAGNOSTIC \(describe(item))") }
    context.cgContext.setFillColor(NSColor.red.cgColor)
    context.cgContext.fill(NSRect(x: 1, y: 1, width: 2, height: 2))
    context.flushGraphics()
    print(
      "RESIZE_OPAQUE_DIAGNOSTIC writerControl=\(String(describing: bitmap.colorAt(x: 2, y: 2)))")
    let addedArea = try XCTUnwrap(
      bitmap.colorAt(
        x: bitmap.pixelsWide * 4 / 5, y: bitmap.pixelsHigh / 2))
    XCTAssertGreaterThan(addedArea.alphaComponent, 0.99)
  }

  private func descendants(_ view: NSView) -> [NSView] {
    [view] + view.subviews.flatMap { descendants($0) }
  }

  private func previewDrawing() throws -> OverlayResizePreviewView {
    try XCTUnwrap(
      NSApp.windows.filter(\.isVisible).compactMap(\.contentView)
        .flatMap { descendants($0) }.compactMap { $0 as? OverlayResizePreviewView }
        .first { $0.headers.contains { $0?.content != nil } })
  }
}
