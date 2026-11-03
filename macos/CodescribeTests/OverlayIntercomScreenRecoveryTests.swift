import AppKit
import XCTest

@testable import Codescribe

@MainActor
final class OverlayIntercomScreenRecoveryTests: XCTestCase {
  func testExpandedGroupRetainsAllRowsWithinTheVisibleScreenWhenCompactRowsFit() {
    typealias Layout = OverlayIntercomWidgetLayout
    for screen in [
      CGRect(x: 0, y: 24, width: 1440, height: 900),
      CGRect(x: -1680, y: -160, width: 1680, height: 1050),
      CGRect(x: 1920, y: 60, width: 900, height: 700),
    ] {
      for scale in [CGFloat(0.8), 1, 1.6] {
        let metrics = IntercomContentMetrics(names: ["Lena", "Klaudiusz"], digits: ["2", "9"], scale: scale)
        for count in [1, 4, 9] {
          let height = metrics.fittedCardHeight(2000, count: count, available: screen.height)
          let range = Layout.visibleRange(
            count: count, offset: 0, visibleHeight: screen.height, scale: scale, cardHeight: height)
          XCTAssertEqual(range.count, count, "Nine compact rows fit; expanding must not hide agents")
          let proposed = CGRect(
            x: screen.minX + 10, y: screen.minY + 4,
            width: 500, height: Layout.controlHeight(scale: scale))
          let header = Layout.clampedHeader(
            proposed, count: count, visibleFrame: screen, scale: scale, cardHeight: height)
          XCTAssertTrue(screen.contains(header))
          for index in 0..<count {
            let row = Layout.rowFrame(
              index: index, header: header, count: count, visibleFrame: screen,
              scale: scale, cardHeight: height)
            XCTAssertTrue(screen.contains(row), "row \(index)/\(count), scale \(scale): \(row) outside \(screen)")
          }
        }
      }
    }
  }

  func testOversizeDetachedResizeKeepsTheWholeWindowWithinItsDisplay() {
    typealias Layout = OverlayIntercomWidgetLayout
    let screen = CGRect(x: -1440, y: -120, width: 1440, height: 780)
    let frame = CGRect(x: -1300, y: 120, width: 380, height: 360)
    for edge in [Layout.ResizeEdge.top, .bottom, .bottomLeft, .bottomRight] {
      for delta in [CGSize(width: 10000, height: 10000), CGSize(width: -10000, height: -10000)] {
        let resized = Layout.resizedFrame(
          frame, delta: delta, edge: edge, minimum: CGSize(width: 320, height: 240), visibleFrame: screen)
        XCTAssertTrue(screen.contains(resized), "\(edge), \(delta): \(resized) outside \(screen)")
      }
    }
  }
}
