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
    XCTAssertNil(model.opening)
    XCTAssertNil(model.error)
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

  func testOpeningAnArchiveCarriesItsFullTextAndItsOwnAudio() async throws {
    let item = entry("a_raw.txt", timestamp: 1_000, preview: "First words")
    let other = entry("b_raw.txt", timestamp: 2_000, preview: "Other")
    let reader = HistoryReader(
      entries: [item, other],
      texts: [item.path: "First words\nWhole recording.", other.path: "Other take"],
      audio: [item.path: "a_raw.m4a", other.path: "b_raw.m4a"])
    let model = OverlayTranscriptHistoryModel(reader: reader)
    let result = await model.open(item)
    let opened = try XCTUnwrap(result)
    XCTAssertEqual(opened.path, item.path)
    XCTAssertEqual(opened.text, "First words\nWhole recording.")
    XCTAssertEqual(opened.archivedText, opened.text)
    XCTAssertEqual(opened.audioPath, "a_raw.m4a")
    XCTAssertEqual(opened.recordedAt, Date(timeIntervalSince1970: 1))
    XCTAssertNil(model.opening)
    XCTAssertNil(model.error)
  }

  func testArchiveWithoutPairedAudioOpensWithoutBorrowingAnother() async throws {
    let item = entry("a_raw.txt", timestamp: 1, preview: "A")
    let reader = HistoryReader(
      entries: [item], texts: [item.path: "Tekst A"], audio: ["b_raw.txt": "b_raw.m4a"])
    let result = await OverlayTranscriptHistoryModel(reader: reader).open(item)
    let opened = try XCTUnwrap(result)
    XCTAssertNil(opened.audioPath)
  }

  func testReopenUsesPersistedHeadAndKeepsRawAvailableWithoutAudio() async throws {
    let item = entry("a_raw.txt", timestamp: 1, preview: "Raw A")
    let document = CsArchivedDocument(
      path: item.path, originalText: "Raw A", revision: 4, renderedText: "Saved formatted A",
      provenance: "formatter", receiptId: "durable-four", versions: [
        CsDocumentVersion(step: 0, provenance: "original", detail: nil, renderedText: "Raw A",
          emittedAt: "", receiptId: ""),
        CsDocumentVersion(step: 1, provenance: "formatter", detail: "smart", renderedText: "Saved formatted A",
          emittedAt: "", receiptId: "durable-four")], cursor: 1)
    let reader = HistoryReader(
      entries: [item], texts: [item.path: "Raw A"], documents: [item.path: document])
    let result = await OverlayTranscriptHistoryModel(reader: reader).open(item)
    let opened = try XCTUnwrap(result)
    XCTAssertEqual(opened.archivedText, "Raw A")
    XCTAssertEqual(opened.text, "Saved formatted A")
    XCTAssertEqual(opened.revision, 4)
    XCTAssertEqual(opened.receiptId, "durable-four")
    XCTAssertEqual(opened.cursor, 1)
    XCTAssertEqual(opened.versions.count, 2)
    XCTAssertNil(opened.audioPath)
  }

  /// Rapid A → B: the slow read of A returns after B was chosen and is dropped.
  func testLaterOpenSupersedesAnInFlightRead() async {
    let slow = entry("slow.txt", timestamp: 1, preview: "Slow")
    let fast = entry("fast.txt", timestamp: 2, preview: "Fast")
    let reader = DelayedHistoryReader(slowPath: slow.path)
    let model = OverlayTranscriptHistoryModel(reader: reader)
    let first = Task { await model.open(slow) }
    while !(await reader.started) { await Task.yield() }
    let second = await model.open(fast)
    await reader.finish()
    let late = await first.value
    XCTAssertNil(late, "a superseded read never reaches the canvas")
    XCTAssertEqual(second?.path, fast.path)
    XCTAssertEqual(second?.text, "Fast text")
    XCTAssertNil(model.opening)
  }

  func testMissingOrEmptyArchiveIsReportedAndNeverOpened() async {
    let missing = entry("missing.txt", timestamp: 1, preview: "Missing")
    let empty = entry("empty.txt", timestamp: 2, preview: "")
    let model = OverlayTranscriptHistoryModel(
      reader: HistoryReader(entries: [], texts: [empty.path: "  \n"]))
    let missingResult = await model.open(missing)
    XCTAssertNil(missingResult)
    XCTAssertNotNil(model.error)
    XCTAssertNil(model.opening)
    let emptyResult = await model.open(empty)
    XCTAssertNil(emptyResult)
    XCTAssertNotNil(model.error)
  }

  private func entry(_ path: String, timestamp: Int64, preview: String) -> CsHistoryEntry {
    CsHistoryEntry(path: path, timestampMs: timestamp, preview: preview, kind: .raw)
  }
}

private struct HistoryReader: TranscriptHistoryReading {
  let saved: [CsHistoryEntry]
  let texts: [String: String]
  let audio: [String: String]
  let documents: [String: CsArchivedDocument]

  init(
    entries: [CsHistoryEntry], texts: [String: String] = [:], audio: [String: String] = [:],
    documents: [String: CsArchivedDocument] = [:]
  ) {
    saved = entries
    self.texts = texts
    self.audio = audio
    self.documents = documents
  }

  func audioPath(forTranscript path: String) async -> String? { audio[path] }

  func document(at path: String) async throws -> CsArchivedDocument {
    if let document = documents[path] { return document }
    let raw = try await text(at: path)
    return .unrevised(path: path, text: raw)
  }

  func entries() async -> [CsHistoryEntry] { saved }
  func text(at path: String) async throws -> String {
    guard let text = texts[path] else { throw CocoaError(.fileReadNoSuchFile) }
    return text
  }
}

private actor DelayedHistoryReader: TranscriptHistoryReading {
  let slowPath: String
  private var continuation: CheckedContinuation<String, Never>?
  var started: Bool { continuation != nil }
  init(slowPath: String) { self.slowPath = slowPath }
  func entries() async -> [CsHistoryEntry] { [] }
  func text(at path: String) async throws -> String {
    guard path == slowPath else { return "Fast text" }
    return await withCheckedContinuation { continuation = $0 }
  }
  func audioPath(forTranscript path: String) async -> String? { nil }
  func finish() {
    continuation?.resume(returning: "Late text")
    continuation = nil
  }
}
