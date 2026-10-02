import XCTest

@testable import Codescribe

@MainActor
final class OverlayTranscriptHistoryTests: XCTestCase {
  func testLoadsSavedTranscriptsAcrossDaysWithoutAnActiveTake() async {
    let yesterday = entry("2026-09-27/old.txt", timestamp: 1, preview: "Older recording")
    let today = entry("2026-09-28/new.txt", timestamp: 2, preview: "New recording")
    let failure = CsHistoryEntry(
      path: "failed.txt", timestampMs: 3, preview: "Failed", kind: .failed)
    let reader = HistoryReader(entries: [today, failure, yesterday])
    let model = OverlayTranscriptHistoryModel(reader: reader)
    await model.load()
    XCTAssertEqual(model.entries.map(\.path), [today.path, yesterday.path])
    XCTAssertNil(model.selected)
    XCTAssertNil(model.text)
  }

  func testHistoryCountUsesFullUnicodeTextRatherThanPreviewOrBytes() async throws {
    let item = entry("polish-emoji.txt", timestamp: 1, preview: "Zażółć…")
    let fullText = "Zażółć gęślą jaźń 👩‍⚕️🇵🇱"
    let model = OverlayTranscriptHistoryModel(
      reader: HistoryReader(entries: [item], texts: [item.path: fullText]))
    await model.load()
    let record = try XCTUnwrap(model.entries.first)
    XCTAssertEqual(record.characterCount, fullText.count)
    XCTAssertNotEqual(record.characterCount, item.preview.count)
    XCTAssertNotEqual(record.characterCount, fullText.utf8.count)
  }

  func testCharacterCountUsesLocaleGroupingWithoutAbbreviation() {
    XCTAssertEqual(
      OverlayTranscriptHistoryModel.formattedCharacterCount(
        1_284, locale: Locale(identifier: "pl_PL")),
      "1\u{00A0}284 chars")
    XCTAssertEqual(
      OverlayTranscriptHistoryModel.formattedCharacterCount(
        1_284, locale: Locale(identifier: "en_US")),
      "1,284 chars")
    XCTAssertEqual(
      OverlayTranscriptHistoryModel.formattedCharacterCount(
        12_345, locale: Locale(identifier: "en_US")),
      "12,345 chars")
  }

  func testCharacterCountSelectsSingularAndPluralForms() {
    for identifier in ["en_US", "pl_PL"] {
      let locale = Locale(identifier: identifier)
      for (count, expected) in [(0, "0 chars"), (1, "1 char"), (2, "2 chars")] {
        XCTAssertEqual(
          OverlayTranscriptHistoryModel.formattedCharacterCount(count, locale: locale), expected)
      }
      XCTAssertEqual(
        OverlayTranscriptHistoryModel.formattedCharacterCount(nil, locale: locale),
        "Length unavailable")
    }
  }

  func testCharacterCountPreservesGroupingAcrossTheFourDigitBoundary() {
    let cases: [(Int, String, String)] = [
      (999, "999 chars", "999 chars"),
      (1_000, "1,000 chars", "1\u{00A0}000 chars"),
      (9_999, "9,999 chars", "9\u{00A0}999 chars"),
      (10_000, "10,000 chars", "10\u{00A0}000 chars"),
      (12_345, "12,345 chars", "12\u{00A0}345 chars"),
      (1_000_000, "1,000,000 chars", "1\u{00A0}000\u{00A0}000 chars"),
    ]
    for (count, english, polish) in cases {
      XCTAssertEqual(
        OverlayTranscriptHistoryModel.formattedCharacterCount(
          count, locale: Locale(identifier: "en_US")),
        english)
      XCTAssertEqual(
        OverlayTranscriptHistoryModel.formattedCharacterCount(
          count, locale: Locale(identifier: "pl_PL")),
        polish)
    }
  }

  func testSelectingAnArchiveLoadsItsFullTextNotItsPreview() async {
    let item = entry("take.txt", timestamp: 1, preview: "First words")
    let reader = HistoryReader(entries: [item], texts: [item.path: "First words\nWhole recording."])
    let model = OverlayTranscriptHistoryModel(reader: reader)
    await model.select(item)
    XCTAssertEqual(model.text, "First words\nWhole recording.")
    XCTAssertFalse(model.reading)
    XCTAssertNil(model.error)
  }

  func testReturningToListRejectsAnInFlightTranscriptRead() async {
    let item = entry("slow.txt", timestamp: 1, preview: "Slow")
    let reader = DelayedHistoryReader()
    let model = OverlayTranscriptHistoryModel(reader: reader)
    let read = Task { await model.select(item) }
    while !(await reader.started) { await Task.yield() }
    model.back()
    await reader.finish()
    await read.value
    XCTAssertNil(model.selected)
    XCTAssertNil(model.text)
    XCTAssertFalse(model.reading)
  }

  func testMissingArchiveShowsErrorWithoutKeepingPreviouslyReadText() async {
    let item = entry("missing.txt", timestamp: 1, preview: "Missing")
    let model = OverlayTranscriptHistoryModel(reader: HistoryReader(entries: []))
    await model.select(item)
    XCTAssertNotNil(model.error)
    XCTAssertNil(model.text)
    XCTAssertFalse(model.reading)
  }

  private func entry(_ path: String, timestamp: Int64, preview: String) -> CsHistoryEntry {
    CsHistoryEntry(path: path, timestampMs: timestamp, preview: preview, kind: .raw)
  }
}

private struct HistoryReader: TranscriptHistoryReading {
  let saved: [CsHistoryEntry]
  let texts: [String: String]

  init(entries: [CsHistoryEntry], texts: [String: String] = [:]) {
    saved = entries
    self.texts = texts
  }

  func entries() async -> [CsHistoryEntry] { saved }
  func text(at path: String) async throws -> String {
    guard let text = texts[path] else { throw CocoaError(.fileReadNoSuchFile) }
    return text
  }
}

private actor DelayedHistoryReader: TranscriptHistoryReading {
  private var continuation: CheckedContinuation<String, Never>?
  var started: Bool { continuation != nil }
  func entries() async -> [CsHistoryEntry] { [] }
  func text(at path: String) async throws -> String {
    await withCheckedContinuation { continuation = $0 }
  }
  func finish() {
    continuation?.resume(returning: "Late text")
    continuation = nil
  }
}
