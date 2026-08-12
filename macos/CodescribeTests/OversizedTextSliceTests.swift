import XCTest

@testable import Codescribe

/// Pins the string-slicing half of the oversized-bubble contract
/// (`OversizedBubblePolicy` extension in OversizedText.swift). Disposition
/// thresholds themselves are pinned by ChatRenderCostTests — these tests only
/// guard how a disposition turns into displayed characters.
final class OversizedTextSliceTests: XCTestCase {
  private func prose(bytes: Int, line: String = "sialalala bumcyk cyk ") -> String {
    var out = ""
    out.reserveCapacity(bytes + line.utf8.count + 1)
    while out.utf8.count < bytes {
      out += line + "\n"
    }
    return out
  }

  // MARK: head(of:)

  func testHeadOfInlineTextIsIdentity() {
    let text = prose(bytes: 1_000)
    XCTAssertEqual(OversizedBubblePolicy.head(of: text), text)
  }

  func testHeadStaysWithinBudget() {
    let head = OversizedBubblePolicy.head(of: prose(bytes: 200_000))
    XCTAssertLessThanOrEqual(head.utf8.count, OversizedBubblePolicy.headPreviewUTF8)
    XCTAssertFalse(head.isEmpty)
  }

  func testHeadPrefersNewlineBoundary() {
    // Every fixture line ends with a newline well inside the search
    // window, so the fold must land on a complete line — never mid-word.
    let line = "sialalala bumcyk cyk"
    let head = OversizedBubblePolicy.head(of: prose(bytes: 200_000, line: line))
    XCTAssertTrue(head.hasSuffix(line), "fold should land on a complete line")
  }

  func testHeadNeverSplitsGraphemes() {
    // 👩‍👩‍👧‍👦 is many UTF-8 bytes per Character; a byte-blind cut would leave a
    // dangling scalar. Walking Characters must keep every family whole.
    let family = String(repeating: "👩‍👩‍👧‍👦", count: 8_000)
    let head = OversizedBubblePolicy.head(of: family)
    XCTAssertLessThanOrEqual(head.utf8.count, OversizedBubblePolicy.headPreviewUTF8)
    XCTAssertTrue(head.allSatisfy { String($0) == "👩‍👩‍👧‍👦" })
  }

  // MARK: streamingWindow(_:)

  func testStreamingWindowOfInlineTextIsIdentity() {
    let text = prose(bytes: 1_000)
    XCTAssertEqual(OversizedBubblePolicy.streamingWindow(text), text)
  }

  func testStreamingWindowKeepsTheLiveTail() {
    let text = prose(bytes: 200_000) + "THE-LIVE-EDGE"
    let window = OversizedBubblePolicy.streamingWindow(text)
    XCTAssertTrue(window.hasSuffix("THE-LIVE-EDGE"))
    // One grapheme of slack: the loop stops after the character that
    // crosses the budget so a wide Character is kept whole, not split.
    XCTAssertLessThanOrEqual(
      window.utf8.count,
      OversizedBubblePolicy.headPreviewUTF8 + 4
    )
  }

  // MARK: byteSummary(_:)

  func testByteSummaryFormatsKBAndMB() {
    XCTAssertEqual(OversizedBubblePolicy.byteSummary(prose(bytes: 100_000)).suffix(2), "KB")
    XCTAssertEqual(OversizedBubblePolicy.byteSummary(prose(bytes: 3_000_000)).suffix(2), "MB")
  }
}
