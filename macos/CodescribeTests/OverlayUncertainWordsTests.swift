import AppKit
import XCTest

@testable import Codescribe

@MainActor
final class OverlayUncertainWordsTests: XCTestCase {
  private func span(
    _ start: UInt32, _ end: UInt32, rewritten: Bool = false,
    source: String = "whisper_token_logprob", producer: String = "whisper", value: Float = -1.9
  ) -> CsUncertainSpan {
    CsUncertainSpan(
      occurrenceSessionId: "a6-session",
      occurrenceCaptureEpoch: 1,
      slotSampleStart: 32_000,
      slotSampleEnd: 48_000,
      utf16Start: start,
      utf16End: end,
      source: source,
      value: value,
      surfaceRewritten: rewritten,
      producer: producer
    )
  }

  func testProjectionClampsToTextAndTrimsPunctuationEdges() {
    let text = "To słowo, tak"
    // "słowo," is UTF-16 [3, 9); the comma edge must be trimmed.
    let words = OverlayUncertainWordProjection.project(spans: [span(3, 9)], text: text)
    XCTAssertEqual(words.count, 1)
    XCTAssertEqual(words[0].word, "słowo")
    XCTAssertEqual(words[0].range, NSRange(location: 3, length: 5))
  }

  func testProjectionDropsOutOfBoundsAndEmptySpansHonestly() {
    let text = "krótki"
    XCTAssertTrue(
      OverlayUncertainWordProjection.project(spans: [span(100, 200)], text: text).isEmpty)
    XCTAssertTrue(
      OverlayUncertainWordProjection.project(spans: [span(2, 2)], text: text).isEmpty)
    // A span over pure punctuation trims to nothing and is dropped.
    XCTAssertTrue(
      OverlayUncertainWordProjection.project(spans: [span(6, 7)], text: "słowo ,").isEmpty)
  }

  func testProjectionNeverSplitsASurrogatePair() {
    let text = "👩🏽‍⚕️ lekarz"
    let emojiLength = ("👩🏽‍⚕️" as NSString).length  // 7 UTF-16 units
    // A span starting mid-pair or ending mid-pair is dropped, never cut.
    XCTAssertTrue(
      OverlayUncertainWordProjection.project(spans: [span(1, 8)], text: text).isEmpty,
      "a boundary inside the 👩 surrogate pair rejects the span")
    XCTAssertTrue(
      OverlayUncertainWordProjection.project(spans: [span(0, 1)], text: text).isEmpty,
      "an end boundary after the 👩 high surrogate rejects the span")
    // A span ending mid-grapheme trims whole scalars down to nothing instead
    // of cutting the grapheme.
    XCTAssertTrue(
      OverlayUncertainWordProjection.project(
        spans: [span(0, UInt32(emojiLength - 1))], text: text
      ).isEmpty)
    // The real word still projects exactly.
    let words = OverlayUncertainWordProjection.project(
      spans: [span(UInt32(emojiLength + 1), UInt32(emojiLength + 7))], text: text)
    XCTAssertEqual(words.first?.word, "lekarz")
    XCTAssertEqual(words.first?.range, NSRange(location: emojiLength + 1, length: 6))
  }

  func testProjectionKeepsPolishDiacriticsOnUtf16Geometry() {
    let text = "ążł ęść"
    let words = OverlayUncertainWordProjection.project(spans: [span(4, 7)], text: text)
    XCTAssertEqual(words.first?.word, "ęść")
    XCTAssertEqual(words.first?.range, NSRange(location: 4, length: 3))
  }

  func testHitTestingFindsTheWordUnderTheClick() {
    let words = OverlayUncertainWordProjection.project(
      spans: [span(6, 10)], text: "alpha beta gamma")
    XCTAssertEqual(words.count, 1)
    XCTAssertNotNil(OverlayUncertainWordProjection.hit(at: 6, in: words))
    XCTAssertNotNil(OverlayUncertainWordProjection.hit(at: 8, in: words))
    XCTAssertNotNil(
      OverlayUncertainWordProjection.hit(at: 10, in: words),
      "a click on the last glyph's trailing edge stays inside the word")
    XCTAssertNil(OverlayUncertainWordProjection.hit(at: 5, in: words))
    XCTAssertNil(OverlayUncertainWordProjection.hit(at: 11, in: words))
  }

  func testPopoverReasonExplainsTheSourceInHumanWords() {
    let whisper = OverlayUncertainWordProjection.project(
      spans: [span(0, 3, source: "whisper_token_logprob")], text: "Iwo")
    XCTAssertEqual(UncertainWordCopy.reason(for: whisper[0]), "Whisper was unsure of this word")
    let rewritten = OverlayUncertainWordProjection.project(
      spans: [span(0, 3, rewritten: true)], text: "Iwo")
    XCTAssertEqual(UncertainWordCopy.reason(for: rewritten[0]), "Rewritten by your dictionary")
    let cloud = OverlayUncertainWordProjection.project(
      spans: [span(0, 3, source: "vendor_word_probability", producer: "cloud_live")],
      text: "Iwo")
    XCTAssertEqual(
      UncertainWordCopy.reason(for: cloud[0]), "The cloud transcriber was unsure of this word")
  }

  func testPopoverModelGatesRawValueBehindDiagnostics() {
    let words = OverlayUncertainWordProjection.project(spans: [span(0, 3)], text: "Iwo")
    let daily = UncertainWordPopoverModel(
      word: words[0], showsDiagnostics: false, canPlay: true, canTeach: true)
    XCTAssertNil(daily.diagnosticValue, "d8: raw values are diagnostic-only")
    XCTAssertEqual(daily.reason, "Whisper was unsure of this word")
    XCTAssertTrue(daily.canPlay)
    XCTAssertTrue(daily.canTeach)

    let diagnostic = UncertainWordPopoverModel(
      word: words[0], showsDiagnostics: true, canPlay: false, canTeach: false)
    XCTAssertEqual(diagnostic.diagnosticValue, "value -1.900 (logprob)")
    XCTAssertFalse(diagnostic.canPlay)
  }
}
