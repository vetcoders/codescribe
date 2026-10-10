import XCTest

@testable import Codescribe

final class OverlayTranscriptVersionsTests: XCTestCase {
  func testThreeRetranscriptionsAndFormatOfferFourStepsBackIncludingIdenticalAttempts() {
    let versions = ["raw", "retranscribe", "retranscribe", "retranscribe", "formatter"]
      .enumerated().map { index, provenance in
        version(UInt64(index), provenance: provenance, text: "same words")
      }
    for cursor in 0..<5 {
      let projection = OverlayTranscriptVersionsPresentation.make(
        versions: versions, cursor: UInt64(cursor), blockedReason: nil)
      XCTAssertEqual(projection.options.count, 5)
      XCTAssertEqual(Set(projection.options.map(\.id)).count, 5)
      XCTAssertEqual(projection.current?.step, UInt64(cursor))
      XCTAssertEqual(projection.canUndo, cursor > 0)
      XCTAssertEqual(projection.canRedo, cursor < 4)
      XCTAssertEqual(projection.undoStep, cursor > 0 ? UInt64(cursor - 1) : nil)
      XCTAssertEqual(projection.redoStep, cursor < 4 ? UInt64(cursor + 1) : nil)
    }
  }

  func testPendingOperationAndDirtyEditBlockBothDirectionsAndPicker() {
    let versions = (0..<3).map { version(UInt64($0), provenance: "retranscribe") }
    for reason in [
      OverlayTranscriptVersionsPresentation.pendingReason,
      OverlayTranscriptVersionsPresentation.dirtyReason,
    ] {
      let projection = OverlayTranscriptVersionsPresentation.make(
        versions: versions, cursor: 1, blockedReason: reason)
      XCTAssertTrue(projection.isVisible)
      XCTAssertFalse(projection.canUndo)
      XCTAssertFalse(projection.canRedo)
      XCTAssertNil(projection.undoStep)
      XCTAssertNil(projection.redoStep)
      XCTAssertTrue(projection.selectableSteps.isEmpty)
      XCTAssertEqual(projection.undoTitle, reason)
      XCTAssertEqual(projection.redoTitle, reason)
    }
  }

  func testAcceptedBranchAfterUndoUsesOnlyReturnedRustSteps() {
    let projection = OverlayTranscriptVersionsPresentation.make(
      versions: [version(0, provenance: "raw"), version(1, provenance: "user-edit")],
      cursor: 1, blockedReason: nil)
    XCTAssertTrue(projection.canUndo)
    XCTAssertFalse(projection.canRedo)
    XCTAssertEqual(projection.selectableSteps, [0])
    XCTAssertEqual(projection.current?.source, .edited)
  }

  func testSingleBaselineDoesNotOfferVersionNavigation() {
    let projection = OverlayTranscriptVersionsPresentation.make(
      versions: [version(0, provenance: "raw")], cursor: 0, blockedReason: nil)
    XCTAssertFalse(projection.isVisible)
    XCTAssertFalse(projection.canUndo)
    XCTAssertFalse(projection.canRedo)
  }

  private func version(_ step: UInt64, provenance: String, text: String = "text")
    -> CsDocumentVersion
  {
    CsDocumentVersion(
      step: step, provenance: provenance, detail: nil, renderedText: text,
      emittedAt: "2026-10-10T00:00:00Z", receiptId: "attempt-\(step)")
  }
}
