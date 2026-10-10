import Foundation

// Stored identifiers from the quality corpus and the custom lexicon, as the
// Dictionary panel names them. The identifiers themselves are data written
// by Rust and never change with the interface language.

/// `action` of a quality record: how the take ended when the correction was
/// captured.
enum QualityActionLabel {
  static func text(for action: String) -> String {
    switch action {
    case "revision":
      return String(localized: "saved correction", comment: "Dictionary: quality record action")
    case "edit":
      return String(localized: "edited", comment: "Dictionary: quality record action")
    case "paste":
      return String(localized: "pasted", comment: "Dictionary: quality record action")
    case "copy":
      return String(localized: "copied", comment: "Dictionary: quality record action")
    case "send":
      return String(localized: "sent to Agent", comment: "Dictionary: quality record action")
    case "close":
      return String(localized: "closed", comment: "Dictionary: quality record action")
    case "close-unreviewed":
      return String(
        localized: "closed without review", comment: "Dictionary: quality record action")
    default:
      return action
    }
  }
}

/// `source` of a custom lexicon rule: where the variant → canonical pair came from.
enum LexiconSourceLabel {
  static func text(for source: String) -> String {
    switch source {
    case "correction":
      return String(
        localized: "dictionary.rule.origin.correction", defaultValue: "From a correction",
        comment: "Dictionary: lexicon rule source")
    case "manual":
      return String(
        localized: "dictionary.rule.origin.manual", defaultValue: "Added by hand",
        comment: "Dictionary: lexicon rule source")
    case "import":
      return String(
        localized: "dictionary.rule.origin.import", defaultValue: "From an import",
        comment: "Dictionary: lexicon rule source")
    default:
      return String(
        localized: "dictionary.rule.origin.unknown", defaultValue: "Origin not recorded",
        comment: "Dictionary: lexicon rule source")
    }
  }
}
