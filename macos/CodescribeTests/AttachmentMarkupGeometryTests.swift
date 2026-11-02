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
