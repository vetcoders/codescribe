import AppKit
import XCTest

@testable import Codescribe

final class OverlayIntercomWidgetLayoutTests: XCTestCase {
  func testHoverIsTransientAndClickPinsOnlyTheSmallPreview() {
    var choice = OverlayIntercomPreviewChoice.hidden
    choice = choice.entering("3")
    XCTAssertEqual(choice, .transient("3"))
    XCTAssertEqual(
      choice.leaving("1"), .transient("3"), "an old preview exit cannot dismiss another agent")
    XCTAssertEqual(choice.leaving("3"), .hidden)
    choice = choice.clicking("3")
    XCTAssertEqual(choice, .pinned("3"))
    XCTAssertEqual(choice.entering("1"), .pinned("3"))
    XCTAssertEqual(choice.leaving("3"), .pinned("3"))
    choice = choice.clicking("1")
    XCTAssertEqual(choice, .pinned("1"))
    XCTAssertEqual(choice.clicking("1"), .hidden)
  }

  func testWholeColumnAndPreviewStayOnEveryNormalDesktopWithRealGaps() {
    for visible in [
      CGRect(x: 0, y: 0, width: 1440, height: 900),
      CGRect(x: -1920, y: 40, width: 1920, height: 1040),
      CGRect(x: 0, y: 0, width: 800, height: 450),
    ] {
      for count in 1...9 {
        let header = OverlayIntercomWidgetLayout.clampedHeader(
          CGRect(x: visible.maxX + 600, y: visible.minY - 200, width: 300, height: 30),
          count: count, visibleFrame: visible)
        XCTAssertTrue(visible.contains(header))
        var previous = header
        for index in 0..<count {
          let row = OverlayIntercomWidgetLayout.rowFrame(
            index: index, header: header, count: count, visibleFrame: visible)
          XCTAssertGreaterThanOrEqual(row.minY, visible.minY - 0.001)
          XCTAssertLessThanOrEqual(row.maxX, visible.maxX + 0.001)
          XCTAssertLessThan(row.maxY, previous.minY, "the desktop retains a real pointer gap")
          let preview = OverlayIntercomWidgetLayout.previewFrame(beside: row, visibleFrame: visible)
          XCTAssertTrue(visible.contains(preview))
          previous = row
        }
      }
    }
  }

  func testPreviewChoosesAvailableSideRatherThanLeavingTheDisplay() {
    let screen = CGRect(x: 0, y: 0, width: 1440, height: 900)
    let left = OverlayIntercomWidgetLayout.previewFrame(
      beside: CGRect(x: 8, y: 300, width: 280, height: 44), visibleFrame: screen)
    XCTAssertGreaterThan(left.minX, 280)
    let right = OverlayIntercomWidgetLayout.previewFrame(
      beside: CGRect(x: 1150, y: 300, width: 280, height: 44), visibleFrame: screen)
    XCTAssertLessThan(right.maxX, 1150)
  }
}
