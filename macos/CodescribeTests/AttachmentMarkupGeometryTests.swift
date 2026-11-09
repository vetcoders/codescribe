import AppKit
import PaperKit
import XCTest

@testable import Codescribe

/// Exercise the production PNG exporter with deliberately asymmetric pixels.
/// Red marks the source image's top edge, blue its bottom; a green rectangle
/// placed near the top left must stay there after the PaperKit round trip.
@MainActor
final class AttachmentMarkupGeometryTests: XCTestCase {
  func testUnchangedMarkupPreservesTheOriginalPixelsAndDimensions() async throws {
    let fixture = try sourceImage(scale: 1)
    let session = try XCTUnwrap(AttachmentMarkupSession(url: fixture.url))
    XCTAssertFalse(session.hasChanges)
    let data = await session.flattenedPNG()
    let output = try bitmap(try XCTUnwrap(data))
    XCTAssertEqual(output.pixelsWide, fixture.image.width)
    XCTAssertEqual(output.pixelsHigh, fixture.image.height)
    var largestError = CGFloat.zero
    for y in 0..<output.pixelsHigh {
      for x in 0..<output.pixelsWide {
        let expected = try color(fixture.bitmap, x: x, y: y)
        let actual = try color(output, x: x, y: y)
        let errors = [
          abs(expected.redComponent - actual.redComponent),
          abs(expected.greenComponent - actual.greenComponent),
          abs(expected.blueComponent - actual.blueComponent),
          abs(expected.alphaComponent - actual.alphaComponent),
        ]
        largestError = max(largestError, errors.max() ?? 0)
      }
    }
    XCTAssertLessThanOrEqual(largestError, 1.0 / 255)
  }

  func testTopLeftRectangleIsNotReflectedIntoTheBottomOfTheSavedPNG() async throws {
    try await assertTopLeftRectangle(scale: 1)
  }

  func testRetinaImageKeepsPixelResolutionAndAnnotationPosition() async throws {
    try await assertTopLeftRectangle(scale: 2)
  }

  func testFittedZoomRangeAllowsZoomingOutRetinaCanvasesToFitHumanViewport() throws {
    let canvas = CGSize(width: 2800, height: 1800)
    let viewport = CGSize(width: 560, height: 420)
    let range = AttachmentMarkupGeometry.fittedZoomRange(canvas: canvas, viewport: viewport)
    let fitScale = min(viewport.width / canvas.width, viewport.height / canvas.height)
    XCTAssertLessThanOrEqual(range.lowerBound, 0.05)
    XCTAssertGreaterThanOrEqual(range.upperBound, 4.0)
    XCTAssertTrue(
      range.contains(fitScale),
      "PaperMarkupViewController zoomRange must include the Retina fit scale (\(fitScale))")

    let fixture = try sourceImage(scale: 2)
    let session = try XCTUnwrap(AttachmentMarkupSession(url: fixture.url))
    XCTAssertTrue(session.paper.delegate === session)
    XCTAssertLessThanOrEqual(session.paper.zoomRange.lowerBound, 0.05)
    session.fitCanvasToViewport(viewport)
    XCTAssertEqual(
      session.paper.zoomRange,
      AttachmentMarkupGeometry.fittedZoomRange(
        canvas: session.canvas.size,
        pointSize: session.pointSize,
        viewport: viewport))
  }

  func testFittedScaleAndVisibleFrameNormalizeRetinaAndFitProportionally() {
    // Large 2x Retina capture (2992x1934 px, 1496x967 pt) in a 536x418 pt viewport:
    // logical size exceeds viewport, so it scales down proportionally to fit viewport width.
    let largeCanvas = CGRect(x: 0, y: 0, width: 2992, height: 1934)
    let largePointSize = CGSize(width: 1496, height: 967)
    let viewport = CGSize(width: 536, height: 418)
    let largeScale = AttachmentMarkupGeometry.fittedScale(
      canvas: largeCanvas.size, pointSize: largePointSize, viewport: viewport)
    XCTAssertEqual(largeScale, 536.0 / 2992.0, accuracy: 0.0001)

    let largeVisible = AttachmentMarkupGeometry.fittedVisibleFrame(
      canvas: largeCanvas, pointSize: largePointSize, viewport: viewport)
    XCTAssertEqual(largeVisible.midX, largeCanvas.midX, accuracy: 0.001)
    XCTAssertEqual(largeVisible.midY, largeCanvas.midY, accuracy: 0.001)
    XCTAssertEqual(largeVisible.width, 2992.0, accuracy: 0.01)
    let sxLarge = viewport.width / largeVisible.width
    let syLarge = viewport.height / largeVisible.height
    XCTAssertEqual(sxLarge, syLarge, accuracy: 0.0001, "Horizontal and vertical scale must be uniform")
    XCTAssertEqual(sxLarge, largeScale, accuracy: 0.0001)

    // Small 2x Retina capture (672x516 px, 336x258 pt) in a 536x350 pt viewport:
    // logical size fits inside viewport, so it normalizes to 100% logical points (0.5x pixel scale)
    // instead of blowing up to 2x pixel size or stretching to fill the viewport.
    let smallCanvas = CGRect(x: 0, y: 0, width: 672, height: 516)
    let smallPointSize = CGSize(width: 336, height: 258)
    let smallViewport = CGSize(width: 536, height: 350)
    let smallScale = AttachmentMarkupGeometry.fittedScale(
      canvas: smallCanvas.size, pointSize: smallPointSize, viewport: smallViewport)
    XCTAssertEqual(smallScale, 0.5, accuracy: 0.0001)

    let smallVisible = AttachmentMarkupGeometry.fittedVisibleFrame(
      canvas: smallCanvas, pointSize: smallPointSize, viewport: smallViewport)
    XCTAssertEqual(smallVisible.midX, smallCanvas.midX, accuracy: 0.001)
    XCTAssertEqual(smallVisible.midY, smallCanvas.midY, accuracy: 0.001)
    XCTAssertEqual(smallVisible.width, smallViewport.width / 0.5, accuracy: 0.001)
    XCTAssertEqual(smallVisible.height, smallViewport.height / 0.5, accuracy: 0.001)
    XCTAssertTrue(AttachmentMarkupGeometry.visibleFrameMatches(smallVisible, target: smallVisible))
    XCTAssertFalse(
      AttachmentMarkupGeometry.visibleFrameMatches(
        CGRect(origin: .zero, size: smallViewport), target: smallVisible))
  }

  func testDefaultShapeConfigurationIsUnfilledWithVisibleStroke() {
    let retinaCanvas = CGSize(width: 2800, height: 1800)
    let rectConfig = AttachmentMarkupGeometry.defaultShapeConfiguration(
      for: .rectangle, canvas: retinaCanvas)
    XCTAssertNil(rectConfig.fillColor, "inserted shapes must not fill and obscure the screenshot")
    XCTAssertNotNil(rectConfig.strokeColor, "inserted shapes must carry a visible stroke")
    XCTAssertGreaterThanOrEqual(rectConfig.lineWidth, 4)
    XCTAssertEqual(rectConfig.lineWidth, 11)

    let smallCanvas = CGSize(width: 200, height: 150)
    let ellipseConfig = AttachmentMarkupGeometry.defaultShapeConfiguration(
      for: .ellipse, canvas: smallCanvas)
    XCTAssertNil(ellipseConfig.fillColor)
    XCTAssertNotNil(ellipseConfig.strokeColor)
    XCTAssertEqual(ellipseConfig.lineWidth, 4)
  }

  func testDefaultShapeFrameCentersInVisibleViewportOrFallsBackToCanvas() {
    let canvas = CGRect(x: 0, y: 0, width: 2000, height: 1200)
    let visible = CGRect(x: 100, y: 200, width: 400, height: 300)
    let visibleFrame = AttachmentMarkupGeometry.defaultShapeFrame(in: visible, canvas: canvas)
    XCTAssertEqual(visibleFrame.midX, visible.midX, accuracy: 0.001)
    XCTAssertEqual(visibleFrame.midY, visible.midY, accuracy: 0.001)
    XCTAssertEqual(visibleFrame.width, round(300 * 0.28), accuracy: 0.001)
    XCTAssertEqual(visibleFrame.height, visibleFrame.width, accuracy: 0.001)

    let fallbackFrame = AttachmentMarkupGeometry.defaultShapeFrame(in: .zero, canvas: canvas)
    XCTAssertEqual(fallbackFrame.midX, canvas.midX, accuracy: 0.001)
    XCTAssertEqual(fallbackFrame.midY, canvas.midY, accuracy: 0.001)
    XCTAssertGreaterThanOrEqual(fallbackFrame.width, 48)
  }

  func testToolbarInsertedShapeRemainsHollowInFlattenedPNG() async throws {
    let fixture = try sourceImage(scale: 2)
    let session = try XCTUnwrap(AttachmentMarkupSession(url: fixture.url))
    XCTAssertTrue(session.toolbar.delegate === session)
    session.markupToolbarViewController(session.toolbar, insertNewShape: .rectangle)
    XCTAssertTrue(session.hasChanges)

    let png = await session.flattenedPNG()
    let data = try XCTUnwrap(png)
    let output = try bitmap(data)
    let center = try color(output, x: output.pixelsWide / 2, y: output.pixelsHigh / 2)
    XCTAssertGreaterThan(
      center.redComponent, 0.9, "hollow shape must leave the screenshot interior visible")
    XCTAssertGreaterThan(
      center.greenComponent, 0.9, "hollow shape must not fill the center with black")
    XCTAssertGreaterThan(
      center.blueComponent, 0.9, "hollow shape must not fill the center with black")
  }

  private func assertTopLeftRectangle(scale: Int) async throws {
    let fixture = try sourceImage(scale: scale)
    let session = try XCTUnwrap(AttachmentMarkupSession(url: fixture.url))
    let frame = CGRect(x: 12 * scale, y: 16 * scale, width: 20 * scale, height: 12 * scale)
    var markup = PaperMarkup(bounds: session.canvas)
    markup.insertNewShape(
      configuration: ShapeConfiguration(
        type: .rectangle,
        fillColor: CGColor(srgbRed: 0, green: 1, blue: 0, alpha: 1)),
      frame: frame)
    session.paper.markup = markup
    XCTAssertTrue(session.hasChanges)
    let data = await session.flattenedPNG()
    let output = try bitmap(try XCTUnwrap(data))
    XCTAssertEqual(output.pixelsWide, 128 * scale)
    XCTAssertEqual(output.pixelsHigh, 96 * scale)
    XCTAssertEqual(output.size.width, fixture.bitmap.size.width, accuracy: 0.02)
    XCTAssertEqual(output.size.height, fixture.bitmap.size.height, accuracy: 0.02)

    var green: [CGPoint] = []
    for y in 0..<output.pixelsHigh {
      for x in 0..<output.pixelsWide {
        let pixel = try color(output, x: x, y: y)
        if pixel.greenComponent > 0.8 && pixel.redComponent < 0.2
          && pixel.blueComponent < 0.2 && pixel.alphaComponent > 0.9
        {
          green.append(CGPoint(x: x, y: y))
        }
      }
    }
    XCTAssertGreaterThan(green.count, 100 * scale * scale, "The annotation must actually render")
    let minX = try XCTUnwrap(green.map(\.x).min())
    let maxX = try XCTUnwrap(green.map(\.x).max())
    let minY = try XCTUnwrap(green.map(\.y).min())
    let maxY = try XCTUnwrap(green.map(\.y).max())
    XCTAssertEqual(minX, frame.minX, accuracy: 1.5)
    XCTAssertEqual(maxX, frame.maxX - 1, accuracy: 1.5)
    XCTAssertEqual(minY, frame.minY, accuracy: 1.5, "Markup uses the image's top-left origin")
    XCTAssertEqual(maxY, frame.maxY - 1, accuracy: 1.5)

    // Flipping the whole composition to fix the annotation is also wrong.
    for point in [
      (4 * scale, 3 * scale), (4 * scale, 92 * scale),
      (120 * scale, 40 * scale), (scale, 40 * scale),
    ] {
      let actual = try color(output, x: point.0, y: point.1)
      let expected = try color(fixture.bitmap, x: point.0, y: point.1)
      XCTAssertEqual(actual.redComponent, expected.redComponent, accuracy: 1.0 / 255)
      XCTAssertEqual(actual.greenComponent, expected.greenComponent, accuracy: 1.0 / 255)
      XCTAssertEqual(actual.blueComponent, expected.blueComponent, accuracy: 1.0 / 255)
    }
  }

  private struct SourceImage {
    let url: URL
    let image: CGImage
    let bitmap: NSBitmapImageRep
  }

  private func sourceImage(scale: Int) throws -> SourceImage {
    let width = 128 * scale
    let height = 96 * scale
    var pixels = [UInt8](repeating: 255, count: width * height * 4)
    for y in 0..<height {
      for x in 0..<width {
        let offset = (y * width + x) * 4
        if y < 8 * scale {
          pixels[offset + 1] = 0
          pixels[offset + 2] = 0
        } else if y >= height - 8 * scale {
          pixels[offset] = 0
          pixels[offset + 1] = 0
        } else if x < 4 * scale {
          pixels[offset] = 0
          pixels[offset + 1] = 0
          pixels[offset + 2] = 0
        }
      }
    }
    let provider = try XCTUnwrap(CGDataProvider(data: Data(pixels) as CFData))
    let image = try XCTUnwrap(
      CGImage(
        width: width, height: height, bitsPerComponent: 8, bitsPerPixel: 32,
        bytesPerRow: width * 4, space: try XCTUnwrap(CGColorSpace(name: CGColorSpace.sRGB)),
        bitmapInfo: CGBitmapInfo(rawValue: CGImageAlphaInfo.premultipliedLast.rawValue),
        provider: provider, decode: nil, shouldInterpolate: false, intent: .defaultIntent))
    let original = NSBitmapImageRep(cgImage: image)
    // The 2x fixture has twice as many pixels in the same logical image size.
    original.size = NSSize(width: 128, height: 96)
    let data = try XCTUnwrap(original.representation(using: .png, properties: [:]))
    let directory = FileManager.default.temporaryDirectory
      .appendingPathComponent("markup-geometry-\(UUID().uuidString)")
    try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
    addTeardownBlock { try? FileManager.default.removeItem(at: directory) }
    let url = directory.appendingPathComponent("asymmetric-\(scale)x.png")
    try data.write(to: url)
    return SourceImage(url: url, image: image, bitmap: try bitmap(data))
  }

  private func bitmap(_ data: Data) throws -> NSBitmapImageRep {
    try XCTUnwrap(NSBitmapImageRep(data: data))
  }

  private func color(_ bitmap: NSBitmapImageRep, x: Int, y: Int) throws -> NSColor {
    try XCTUnwrap(try XCTUnwrap(bitmap.colorAt(x: x, y: y)).usingColorSpace(.sRGB))
  }
}
