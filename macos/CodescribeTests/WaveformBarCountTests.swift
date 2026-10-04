import AppKit
import SwiftUI
import XCTest

@testable import Codescribe

@MainActor
final class WaveformBarCountTests: XCTestCase {
  func testBarCountGrowsWithWidth() {
    let counts = [120, 240, 900].map {
      WaveformView.effectiveBarCount(width: CGFloat($0), barWidth: 2, gap: 3, minimum: 10)
    }
    XCTAssertEqual(counts, [24, 48, 180])
    XCTAssertLessThan(counts[0], counts[1])
    XCTAssertLessThan(counts[1], counts[2])
    XCTAssertEqual(
      WaveformView.effectiveBarCount(width: 0, barWidth: 2, gap: 3, minimum: 34), 34)
  }

  func testBarWidthIsConstantAcrossWidths() throws {
    for width in [120, 240, 900] {
      let renderer = ImageRenderer(
        content: WaveformView(barCount: 10, active: false, inactiveColor: .red, stretches: true)
          .frame(width: CGFloat(width), height: 16)
      )
      renderer.scale = 2
      let bitmap = NSBitmapImageRep(cgImage: try XCTUnwrap(renderer.cgImage))
      var runs: [Int] = []
      var run = 0
      for x in 0..<bitmap.pixelsWide {
        let alpha = try XCTUnwrap(bitmap.colorAt(x: x, y: bitmap.pixelsHigh / 2)).alphaComponent
        if alpha > 0.5 {
          run += 1
        } else if run > 0 {
          runs.append(run)
          run = 0
        }
      }
      if run > 0 { runs.append(run) }
      XCTAssertEqual(runs.count, [120: 24, 240: 48, 900: 180][width])
      XCTAssertTrue(runs.allSatisfy { $0 == 4 }, "Each bar must remain 2 pt: \(runs)")
    }
  }

  func testResampleNeverIndexesOutOfRange() {
    let samples = (0..<34).map { CGFloat($0) / 33 }
    let levels = WaveformView.resampledLevels(samples, count: 60)
    XCTAssertEqual(levels.count, 60)
    XCTAssertEqual(levels.first, 0)
    XCTAssertEqual(levels.last, 1)
    XCTAssertTrue(levels.allSatisfy { $0.isFinite && (0...1).contains($0) })
    XCTAssertEqual(WaveformView.resampledLevels([], count: 60), Array(repeating: 0, count: 60))
    XCTAssertEqual(WaveformView.resampledLevels([0.5], count: 60), Array(repeating: 0.5, count: 60))
    XCTAssertEqual(WaveformView.resampledLevels(samples, count: 0), [])
    XCTAssertEqual(WaveformView.resampledLevels(samples, count: 1), [0])
    XCTAssertEqual(WaveformView.resampledLevels(samples, count: 34), samples)
  }
}
