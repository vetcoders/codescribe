import Foundation

/// File references use the existing markdown link/openURL corridor. Displayed
/// text and copied paths remain unchanged; no shell command is constructed.
enum ConversationLocalFileLinks {
  private static let paths = try? NSRegularExpression(
    pattern: #"(?<![\p{L}\p{N}_:/])(?:file://(?:localhost)?/|~/|/)[^\s<>"`]+"#)
  private static let location = try? NSRegularExpression(pattern: #":[0-9]+(?::[0-9]+)?$"#)

  static func fileURL(for reference: String) -> URL? {
    guard !reference.unicodeScalars.contains(where: { CharacterSet.controlCharacters.contains($0) })
    else { return nil }
    let path: String
    if reference.hasPrefix("file://") {
      guard let url = URL(string: reference), url.isFileURL,
        url.host == nil || url.host == "" || url.host == "localhost"
      else { return nil }
      path = url.path
    } else {
      guard !reference.hasPrefix("//") else { return nil }
      path = (reference as NSString).expandingTildeInPath
    }
    guard path.hasPrefix("/"), !path.hasPrefix("//"), let location else { return nil }
    let range = NSRange(path.startIndex..., in: path)
    let withoutLocation = location.stringByReplacingMatches(
      in: path, range: range, withTemplate: "")
    return URL(fileURLWithPath: withoutLocation).standardizedFileURL
  }

  static func urlForOpening(_ url: URL) -> URL {
    let reference =
      url.scheme == nil
      ? (url.relativeString.removingPercentEncoding ?? url.relativeString) : url.absoluteString
    return fileURL(for: reference) ?? url
  }

  static func annotate(_ text: inout AttributedString) {
    // Markdown destinations and entire inline-code paths can contain spaces.
    let runs = Array(text.runs)
    for run in runs {
      if let link = run.link {
        text[run.range].link = urlForOpening(link)
      } else if run.inlinePresentationIntent?.contains(.code) == true,
        let url = fileURL(for: String(text[run.range].characters))
      {
        text[run.range].link = url
      }
    }
    let plain = String(text.characters)
    guard let paths else { return }
    for match in paths.matches(in: plain, range: NSRange(plain.startIndex..., in: plain)) {
      guard let range = Range(match.range, in: plain) else { continue }
      var candidate = String(plain[range])
      // Sentence punctuation is outside a bare reference. Balanced parentheses
      // inside a filename remain part of the path.
      while let last = candidate.last,
        ".,;:!?)]}".contains(last),
        !(last == ")"
          && candidate.filter({ $0 == "(" }).count >= candidate.filter({ $0 == ")" }).count)
      {
        candidate.removeLast()
      }
      guard !candidate.isEmpty, let url = fileURL(for: candidate) else { continue }
      let start = text.characters.index(
        text.startIndex, offsetBy: plain.distance(from: plain.startIndex, to: range.lowerBound))
      let end = text.characters.index(start, offsetBy: candidate.count)
      let attributedRange = start..<end
      guard !text[attributedRange].runs.contains(where: { $0.link != nil }) else { continue }
      text[attributedRange].link = url
    }
  }
}
