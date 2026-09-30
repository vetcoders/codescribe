import AppKit

/// One uncertain (or lexicon-rewritten) word projected onto the transcript
/// canvas. The identity stays pinned to the physical occurrence (PCM); the
/// UTF-16 `range` is only this revision's label geometry, never a key.
struct OverlayUncertainWord: Equatable, Sendable {
  let range: NSRange
  let word: String
  let source: String
  let value: Float
  let surfaceRewritten: Bool
  let producer: String
  let occurrenceSessionId: String
  let occurrenceCaptureEpoch: UInt64
  let slotSampleStart: UInt64
  let slotSampleEnd: UInt64
}

/// Maps reducer `CsUncertainSpan`s onto the exact canvas text.
///
/// The overlay is a pure projection: it never re-classifies and never widens
/// a range. A span that cannot be reproduced exactly on this text (out of
/// bounds, splitting a surrogate pair, emptied by edge trimming) is dropped —
/// an honest absence, never paint on a neighbor.
enum OverlayUncertainWordProjection {
  /// All geometry is UTF-16 arithmetic on the NSString: `Range(NSRange:in:)`
  /// can hand back mixed-encoding `String.Index` values whose comparison
  /// traps, so trimming never touches Swift indices until the final word cut.
  static func project(spans: [CsUncertainSpan], text: String) -> [OverlayUncertainWord] {
    let ns = text as NSString
    let length = ns.length
    return spans.compactMap { span -> OverlayUncertainWord? in
      guard span.utf16End > span.utf16Start else { return nil }
      var start = min(Int(span.utf16Start), length)
      var end = min(Int(span.utf16End), length)
      guard end > start else { return nil }
      // A boundary inside a surrogate pair can never paint honestly.
      if isLowSurrogate(ns.character(at: start)) { return nil }
      if isHighSurrogate(ns.character(at: end - 1)) { return nil }
      // Trim non-alphanumeric edges one whole scalar at a time.
      while start < end {
        let width = isHighSurrogate(ns.character(at: start)) ? 2 : 1
        if isAlphanumericScalar(ns, at: start, width: width) { break }
        start += width
      }
      while end > start {
        let width = isLowSurrogate(ns.character(at: end - 1)) ? 2 : 1
        if isAlphanumericScalar(ns, at: end - width, width: width) { break }
        end -= width
      }
      guard end > start else { return nil }
      let range = NSRange(location: start, length: end - start)
      guard let swiftRange = Range(range, in: text) else { return nil }
      return OverlayUncertainWord(
        range: range,
        word: String(text[swiftRange]),
        source: span.source,
        value: span.value,
        surfaceRewritten: span.surfaceRewritten,
        producer: span.producer,
        occurrenceSessionId: span.occurrenceSessionId,
        occurrenceCaptureEpoch: span.occurrenceCaptureEpoch,
        slotSampleStart: span.slotSampleStart,
        slotSampleEnd: span.slotSampleEnd
      )
    }
  }

  private static func isHighSurrogate(_ unit: UInt16) -> Bool {
    (0xD800...0xDBFF).contains(unit)
  }

  private static func isLowSurrogate(_ unit: UInt16) -> Bool {
    (0xDC00...0xDFFF).contains(unit)
  }

  private static func isAlphanumericScalar(_ ns: NSString, at index: Int, width: Int) -> Bool {
    if width == 2 {
      let high = UInt32(ns.character(at: index))
      let low = UInt32(ns.character(at: index + 1))
      let value = 0x10000 + ((high - 0xD800) << 10) + (low - 0xDC00)
      guard let scalar = Unicode.Scalar(value) else { return false }
      return CharacterSet.alphanumerics.contains(scalar)
    }
    let unit = ns.character(at: index)
    guard !isLowSurrogate(unit), let scalar = Unicode.Scalar(UInt32(unit)) else { return false }
    return CharacterSet.alphanumerics.contains(scalar)
  }

  /// The painted word under an insertion index. `characterIndex` is the
  /// TextKit insertion position under the click; the trailing-edge probe keeps
  /// a click on the word's last glyph inside the word.
  static func hit(
    at characterIndex: Int,
    in words: [OverlayUncertainWord]
  ) -> OverlayUncertainWord? {
    words.first { word in
      (characterIndex >= word.range.location && characterIndex < NSMaxRange(word.range))
        || (characterIndex > word.range.location
          && characterIndex - 1 < NSMaxRange(word.range))
    }
  }
}

/// Human copy for the uncertain-word popover. The raw source-scale value is
/// diagnostic-only (d8); the reasons below are all a daily user ever sees.
enum UncertainWordCopy {
  static func reason(for word: OverlayUncertainWord) -> String {
    if word.surfaceRewritten { return "Rewritten by your dictionary" }
    switch word.source {
    case "whisper_token_logprob":
      return "Whisper was unsure of this word"
    case "apple_segment_confidence":
      return "Apple was unsure of this word"
    case "vendor_word_probability":
      return "The cloud transcriber was unsure of this word"
    default:
      return "The transcriber was unsure of this word"
    }
  }

  static func sourceLine(for word: OverlayUncertainWord) -> String {
    let producer =
      switch word.producer {
      case "whisper": "Whisper"
      case "cloud_live": "Cloud live"
      default: word.producer
      }
    return "Source: \(producer)"
  }

  static func diagnosticValue(for word: OverlayUncertainWord) -> String {
    let scale =
      switch word.source {
      case "whisper_token_logprob": "logprob"
      case "apple_segment_confidence", "vendor_word_probability": "probability"
      default: "raw"
      }
    return String(format: "value %.3f (%@)", Double(word.value), scale)
  }
}
