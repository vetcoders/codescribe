import XCTest

@testable import Codescribe

// Executed by `make test-swift`. Pure projection of journal entries; the
// OverlayState/engine round trips live in OverlayStateTests.
@MainActor
final class OverlayTranscriptVersionsTests: XCTestCase {
  private func entry(_ revision: UInt64, _ text: String, _ provenance: String, _ second: Int)
    -> CsDocumentHistoryEntry
  {
    CsDocumentHistoryEntry(
      revision: revision, renderedText: text, provenance: provenance,
      emittedAt: String(format: "2026-10-10T10:00:%02dZ", second))
  }

  func testLiveRunsCollapseToTheTextEachKindProducedAndExplicitRevisionsStaySeparate() {
    let history = [
      entry(1, "a", "acoustic-ledger", 1),
      entry(2, "a b", "acoustic-ledger", 2),
      entry(3, "a b c", "acoustic-ledger", 3),
      entry(4, "A b c.", "light-plus", 4),
      entry(5, "A, b c.", "light-plus", 5),
      entry(6, "A, b, c.", "retranscribe", 6),
      entry(7, "A b c, edited.", "user-edit", 7),
    ]
    let versions = OverlayTranscriptVersionsPresentation.make(
      history: history, currentRevision: 7, committedText: "A b c, edited.",
      shownText: "A b c, edited.", showsDerivedPresentation: false, blockedReason: nil)

    XCTAssertEqual(versions.options.map(\.revision), [3, 5, 6, 7])
    XCTAssertEqual(versions.options.map(\.source), [.raw, .lightPlus, .retranscribed, .edited])
    XCTAssertEqual(versions.current?.revision, 7)
    XCTAssertEqual(versions.selectableRevisions, [3, 5, 6])
    XCTAssertFalse(versions.currentMissing)
    XCTAssertTrue(versions.isVisible)
  }

  func testRestoreIsNamedFromTheJournalAndDuplicateTextsCollapseIntoTheShownOne() {
    let history = [
      entry(2, "raw words", "acoustic-ledger", 1),
      entry(3, "Edited words.", "user-edit", 2),
      entry(4, "raw words", "user-edit", 3),
    ]
    let versions = OverlayTranscriptVersionsPresentation.make(
      history: history, currentRevision: 4, committedText: "raw words",
      shownText: "raw words", showsDerivedPresentation: false, blockedReason: nil)

    XCTAssertEqual(versions.options.map(\.revision), [3, 4])
    XCTAssertEqual(versions.current?.source, .restored(.raw))
    XCTAssertEqual(versions.current?.title, "Restored: Raw")
    XCTAssertEqual(versions.selectableRevisions, [3])
  }

  func testOnlyJournalProvenanceNamesAVersionAndNoRawIsInvented() {
    XCTAssertEqual(
      OverlayTranscriptVersion.source(forProvenance: "formatter-smart"), .formatted(.smart))
    XCTAssertEqual(OverlayTranscriptVersion.source(forProvenance: "formatter"), .formatted(nil))
    XCTAssertEqual(
      OverlayTranscriptVersion.source(forProvenance: "formatter-unknown"), .formatted(nil))
    XCTAssertEqual(OverlayTranscriptVersion.source(forProvenance: "apply_something"), .other)

    // A take whose journal holds only edits never grows a Raw option.
    let edits = OverlayTranscriptVersionsPresentation.make(
      history: [entry(5, "one", "user-edit", 1), entry(6, "two", "user-edit", 2)],
      currentRevision: 6, committedText: "two", shownText: "two",
      showsDerivedPresentation: false, blockedReason: nil)
    XCTAssertFalse(edits.options.contains(where: { $0.source == .raw }))
    XCTAssertEqual(edits.options.map(\.source), [.edited, .edited])
  }

  func testSingleVersionHidesTheControlAndAStaleJournalIsReported() {
    let single = OverlayTranscriptVersionsPresentation.make(
      history: [entry(1, "only", "acoustic-ledger", 1)], currentRevision: 1,
      committedText: "only", shownText: "only", showsDerivedPresentation: false,
      blockedReason: nil)
    XCTAssertFalse(single.isVisible, "nothing to choose")

    let stale = OverlayTranscriptVersionsPresentation.make(
      history: [entry(1, "old", "acoustic-ledger", 1), entry(2, "older edit", "user-edit", 2)],
      currentRevision: 3, committedText: "newest", shownText: "newest",
      showsDerivedPresentation: false, blockedReason: nil)
    XCTAssertTrue(stale.currentMissing)
    XCTAssertNil(stale.current)
  }

  func testBlockedReasonDisablesEveryOptionAndArchiveExplainsItsLimit() {
    let blocked = OverlayTranscriptVersionsPresentation.make(
      history: [entry(1, "a", "acoustic-ledger", 1), entry(2, "b", "user-edit", 2)],
      currentRevision: 2, committedText: "b", shownText: "b",
      showsDerivedPresentation: false,
      blockedReason: OverlayTranscriptVersionsPresentation.pendingReason)
    XCTAssertTrue(blocked.selectableRevisions.isEmpty)
    XCTAssertTrue(blocked.options.contains(where: \.isSelectable), "the row itself stays legal")

    let archived = OverlayTranscriptVersionsPresentation.archivedTranscript()
    XCTAssertTrue(archived.isVisible)
    XCTAssertTrue(archived.options.isEmpty)
    XCTAssertNotNil(archived.blockedReason)
  }
}
