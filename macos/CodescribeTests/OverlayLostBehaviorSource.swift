import Foundation
import XCTest

/// Source contracts supplement behavioral tests; they do not certify native hit testing.
/// Resolve beside this test so the same tests can be applied to either generation.
enum OverlayLostBehaviorSource {
  static func read(_ relativePath: String) throws -> String {
    let macos = URL(fileURLWithPath: #filePath)
      .deletingLastPathComponent().deletingLastPathComponent()
    return try String(
      contentsOf: macos.appendingPathComponent("Codescribe/" + relativePath), encoding: .utf8)
  }

  static func overlay(_ file: String) throws -> String {
    try read("Screens/Overlay/" + file)
  }

  static func section(_ source: String, from start: String, to end: String) throws -> String {
    let first = try XCTUnwrap(source.range(of: start), "Missing contract entry: \(start)")
    let last = try XCTUnwrap(
      source.range(of: end, range: first.upperBound..<source.endIndex),
      "Missing contract boundary: \(end)")
    return String(source[first.lowerBound..<last.lowerBound])
  }

  static func compact(_ source: String) -> String {
    source.replacingOccurrences(of: #"(?m)^\s*//[^\n]*"#, with: "", options: .regularExpression)
      .split(whereSeparator: \.isWhitespace).joined(separator: " ")
  }

  static func ordered(_ fragments: [String], in source: String) throws {
    let source = compact(source)
    var cursor = source.startIndex
    for fragment in fragments {
      let match = try XCTUnwrap(
        source.range(of: fragment, range: cursor..<source.endIndex),
        "Missing or out-of-order contract: \(fragment)")
      cursor = match.upperBound
    }
  }

  /// Deliberately read the declaration instead of linking the new symbol:
  /// f98631e32 must produce an assertion failure, not a compilation failure.
  static func bodyFontSize() throws -> CGFloat {
    let source = try overlay("OverlayConversationView.swift")
    let regex = try NSRegularExpression(
      pattern: #"static\s+let\s+bodyFontSize\s*:\s*CGFloat\s*=\s*([0-9]+(?:\.[0-9]+)?)"#)
    let matches = regex.matches(in: source, range: NSRange(source.startIndex..., in: source))
    XCTAssertEqual(matches.count, 1, "One shared, tunable rich-body baseline is required")
    let match = try XCTUnwrap(matches.first)
    let range = try XCTUnwrap(Range(match.range(at: 1), in: source))
    let value = try XCTUnwrap(Double(source[range]))
    XCTAssertGreaterThan(value, 0)
    return CGFloat(value)
  }
}
