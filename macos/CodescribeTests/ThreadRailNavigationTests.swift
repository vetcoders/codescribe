import XCTest

@testable import Codescribe

final class ThreadRailNavigationTests: XCTestCase {
  private let rows = [
    ChatThread(title: "First", meta: ""), ChatThread(title: "Second", meta: ""),
    ChatThread(title: "Third", meta: ""),
  ]

  func testDownUsesVisibleOrder() {
    XCTAssertEqual(
      ThreadRailNavigation.adjacentThread(from: rows[0].id, in: rows, step: 1)?.id, rows[1].id)
  }
  func testUpUsesVisibleOrder() {
    XCTAssertEqual(
      ThreadRailNavigation.adjacentThread(from: rows[2].id, in: rows, step: -1)?.id, rows[1].id)
  }
  func testEndsDoNotWrap() {
    XCTAssertNil(ThreadRailNavigation.adjacentThread(from: rows[0].id, in: rows, step: -1))
    XCTAssertNil(ThreadRailNavigation.adjacentThread(from: rows[2].id, in: rows, step: 1))
  }
  func testUnknownAndEmptyDoNotSelectAnotherThread() {
    XCTAssertNil(ThreadRailNavigation.adjacentThread(from: UUID(), in: rows, step: 1))
    XCTAssertNil(ThreadRailNavigation.adjacentThread(from: rows[0].id, in: [], step: 1))
  }
  func testExplicitReorderingChangesTheNeighbor() {
    let reordered = [rows[0], rows[2], rows[1]]
    XCTAssertEqual(
      ThreadRailNavigation.adjacentThread(from: rows[0].id, in: reordered, step: 1)?.id, rows[2].id)
  }
}
