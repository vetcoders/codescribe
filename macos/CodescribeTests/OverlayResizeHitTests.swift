import XCTest

@testable import Codescribe

final class OverlayResizeHitTests: XCTestCase {
  private let bounds = NSRect(x: 0, y: 0, width: 400, height: 300)
  private let band = OverlayResizeHit.band

  func testInteriorIsNotAResizeHit() {
    XCTAssertNil(OverlayResizeHit.edge(at: NSPoint(x: 200, y: 150), in: bounds))
  }

  func testEdgesAndCornersUseTheFatBand() {
    XCTAssertEqual(OverlayResizeHit.edge(at: NSPoint(x: 2, y: 150), in: bounds), .left)
    XCTAssertEqual(OverlayResizeHit.edge(at: NSPoint(x: 398, y: 150), in: bounds), .right)
    XCTAssertEqual(OverlayResizeHit.edge(at: NSPoint(x: 200, y: 298), in: bounds), .top)
    XCTAssertEqual(OverlayResizeHit.edge(at: NSPoint(x: 200, y: 2), in: bounds), .bottom)
    XCTAssertEqual(OverlayResizeHit.edge(at: NSPoint(x: 2, y: 298), in: bounds), .topLeft)
    XCTAssertEqual(OverlayResizeHit.edge(at: NSPoint(x: 398, y: 298), in: bounds), .topRight)
    XCTAssertEqual(OverlayResizeHit.edge(at: NSPoint(x: 2, y: 2), in: bounds), .bottomLeft)
    XCTAssertEqual(OverlayResizeHit.edge(at: NSPoint(x: 398, y: 2), in: bounds), .bottomRight)
  }

  func testJustInsideTheBandIsStillInterior() {
    let inset = band + 1
    XCTAssertNil(OverlayResizeHit.edge(at: NSPoint(x: inset, y: 150), in: bounds))
    XCTAssertNil(OverlayResizeHit.edge(at: NSPoint(x: 200, y: inset), in: bounds))
  }

  func testApplyKeepsMinSizeWhenDraggingInward() {
    let start = NSRect(x: 100, y: 80, width: 400, height: 320)
    let minSize = DictationOverlayWindow.minSize
    let crushed = OverlayResizeHit.apply(
      edge: .right,
      start: start,
      dx: -200,
      dy: 0,
      minSize: minSize
    )
    XCTAssertEqual(crushed.width, minSize.width)
    XCTAssertEqual(crushed.origin.x, start.origin.x)
  }

  func testLeftAndBottomKeepTheOppositeEdgePinned() {
    let start = NSRect(x: 100, y: 80, width: 400, height: 320)
    let minSize = DictationOverlayWindow.minSize
    let left = OverlayResizeHit.apply(
      edge: .left,
      start: start,
      dx: 20,
      dy: 0,
      minSize: minSize
    )
    XCTAssertEqual(left.maxX, start.maxX)
    XCTAssertEqual(left.width, 380)
    let bottom = OverlayResizeHit.apply(
      edge: .bottom,
      start: start,
      dx: 0,
      dy: 20,
      minSize: minSize
    )
    XCTAssertEqual(bottom.maxY, start.maxY)
    XCTAssertEqual(bottom.height, 300)
  }
}
