import Foundation

// Pure presenters for the About panel. They turn runtime facts (the build
// receipt and the launch repair summary) into interface copy without touching
// the facts themselves: the raw values stay available for diagnostics next to
// the readable sentence.

/// The build timestamp as the interface language reads it. `CSBuiltAt` is an
/// ISO 8601 instant written by the build script; anything else is shown raw.
enum BuildDatePresentation {
  static func readable(
    _ raw: String, locale: Locale, timeZone: TimeZone = .current
  ) -> String? {
    guard let instant = try? Date.ISO8601FormatStyle().parse(raw) else { return nil }
    let style = Date.FormatStyle(
      date: .long, time: .shortened, locale: locale, timeZone: timeZone)
    return instant.formatted(style)
  }
}

/// What the launch repair receipt means to a person. The Rust summary is one
/// line of operator prose ("Config: 0 repairs at launch; env key(s) need
/// review: CODESCRIBE_STT_ENGINE"); the panel shows what is out of date, what
/// it changes and whether anything needs doing, and keeps the original line
/// for diagnostics.
struct ConfigRepairNotice: Equatable {
  enum Kind: Equatable {
    /// One or more `.env` keys are unknown or retired, or an override differs
    /// from a saved setting; nothing was changed for them and nothing will be
    /// changed here.
    case keysNeedReview
    /// Fields were reset or files recreated at launch; no key needs review.
    case repaired
    /// The receipt is a refusal or an unrecognized line.
    case unresolved
  }

  /// One key named by the receipt: what it does now and what, if anything,
  /// to do about it. The receipt carries key names only, never their values.
  struct ReviewItem: Equatable, Identifiable {
    let key: String
    let impact: String
    let action: String
    var id: String { key }
  }

  let raw: String
  let kind: Kind
  let reviewKeys: [String]

  private static let reviewMarker = "need review: "
  /// The only key the loader reports for a precedence conflict rather than
  /// for an unread `.env` entry (`core/config/loader.rs`, `docs/CONFIG.md`).
  static let formattingLevelKey = "FORMATTING_LEVEL"

  init(raw: String) {
    self.raw = raw
    if let marker = raw.range(of: Self.reviewMarker) {
      var tail = String(raw[marker.upperBound...])
      if let backup = tail.range(of: " (backup ") {
        tail = String(tail[..<backup.lowerBound])
      }
      let keys = tail.split(separator: ",").map {
        $0.trimmingCharacters(in: .whitespacesAndNewlines)
      }.filter { !$0.isEmpty }
      kind = keys.isEmpty ? .unresolved : .keysNeedReview
      reviewKeys = keys
    } else if raw.hasPrefix("Config repaired at launch") {
      kind = .repaired
      reviewKeys = []
    } else {
      kind = .unresolved
      reviewKeys = []
    }
  }

  /// Whether the row is a warning. A completed repair only informs.
  var isWarning: Bool { kind != .repaired }

  /// Whether every named key is simply one Codescribe does not read: the
  /// entry is out of date, yet nothing in the running app behaves differently
  /// because of it, so the notice carries one quiet line instead of a warning
  /// card (Founder brief, round 16, 2026-10-10). `FORMATTING_LEVEL` is a
  /// precedence conflict whose value is in effect, so it is never benign, and
  /// a repair or a refusal keeps its own full record.
  var isBenignStaleEntry: Bool {
    kind == .keysNeedReview && !reviewKeys.contains(Self.formattingLevelKey)
  }

  /// The one-line collapsed form of a benign stale entry. The key, what it
  /// does in this build and how to clear the notice stay in the expansion.
  var compactTitle: String {
    reviewKeys.count == 1
      ? String(
        localized: "Stale configuration entry — does not affect operation",
        comment: "About panel: one .env entry Codescribe does not read")
      : String(
        localized: "Stale configuration entries — do not affect operation",
        comment: "About panel: several .env entries Codescribe does not read")
  }

  /// The collapsed row: what happened.
  var title: String {
    switch kind {
    case .keysNeedReview:
      return String(
        localized: "The configuration needs a review",
        comment: "About panel: a .env key is unknown, retired or overrides a setting")
    case .repaired:
      return String(
        localized: "The configuration was repaired at startup",
        comment: "About panel: fields were reset or files recreated at launch")
    case .unresolved:
      return String(
        localized: "The configuration could not be fully checked",
        comment: "About panel: the launch repair receipt is a refusal or unknown")
    }
  }

  /// The collapsed row: what opening it shows.
  var subtitle: String {
    switch kind {
    case .keysNeedReview:
      return reviewKeys.count == 1
        ? String(
          localized: "See which setting is out of date",
          comment: "About panel: opens the one .env key needing review")
        : String(
          localized: "See which settings are out of date",
          comment: "About panel: opens the several .env keys needing review")
    case .repaired:
      return String(
        localized: "No action needed. See what changed",
        comment: "About panel: opens the launch repair record")
    case .unresolved:
      return String(
        localized: "See what to correct",
        comment: "About panel: opens the refusal with the file to correct")
    }
  }

  /// One entry per key, worded for what the key does in this build.
  /// `envFile` is the display path of the optional `.env` file.
  func reviewItems(envFile: String) -> [ReviewItem] {
    reviewKeys.map { key in
      if key == Self.formattingLevelKey {
        return ReviewItem(
          key: key,
          impact: String(
            localized:
              "A formatting level set outside the app differs from the one chosen in Settings. The value from outside the app is in effect.",
            comment: "About panel: FORMATTING_LEVEL override differs from the saved level"),
          action: String(
            localized:
              "To use the level from Settings, remove \(key) from the launch environment or from \(envFile), then restart Codescribe.",
            comment:
              "About panel: how to drop the override; the key and the file path stay verbatim"))
      }
      return ReviewItem(
        key: key,
        impact: String(
          localized: "Codescribe does not read this entry in \(envFile), so it has no effect.",
          comment: "About panel: an unknown or retired .env key; the file path stays verbatim"),
        action: String(
          localized:
            "No action is required. To clear this notice, delete or correct the line, then restart Codescribe.",
          comment: "About panel: what to do about an unread .env key"))
    }
  }

  /// The sentence under the record when no key is involved.
  var outcomeLine: String? {
    switch kind {
    case .keysNeedReview:
      return nil
    case .repaired:
      return String(
        localized:
          "Codescribe corrected settings.json when it started. The record below names the number of changes and any backup it kept.",
        comment: "About panel: a completed launch repair; settings.json stays verbatim")
    case .unresolved:
      return String(
        localized:
          "Codescribe left the file unchanged. Correct the file named in the record below, then restart Codescribe.",
        comment: "About panel: a refused launch repair")
    }
  }
}
