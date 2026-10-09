import Foundation

// Pure presenters for the About panel. They turn runtime facts (the build
// receipt, the launch repair summary, the activation-ping contract) into
// interface copy without touching the facts themselves: the raw values stay
// available for diagnostics next to the readable sentence.

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
/// review: CODESCRIBE_STT_ENGINE"); the panel shows a sentence instead and
/// keeps the original line and the key names in its details.
struct ConfigRepairNotice: Equatable {
  enum Kind: Equatable {
    /// One or more `.env` keys are unknown or shadow a setting; nothing was
    /// changed for them and nothing will be changed here.
    case keysNeedReview
    /// Fields were reset or files recreated at launch; no key needs review.
    case repaired
    /// The receipt is a refusal or an unrecognized line.
    case unresolved
  }

  let raw: String
  let kind: Kind
  let reviewKeys: [String]

  private static let reviewMarker = "need review: "

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

  /// The sentence shown in place of the raw line.
  var headline: String {
    switch kind {
    case .keysNeedReview:
      return String(
        localized: "An outdated configuration setting was detected. It needs a review.",
        comment: "About panel: a .env key is unknown or shadows a setting; nothing is changed automatically"
      )
    case .repaired:
      return String(
        localized: "The configuration was repaired when Codescribe started.",
        comment: "About panel: fields were reset or files recreated at launch")
    case .unresolved:
      return String(
        localized: "The configuration could not be fully checked. See the details.",
        comment: "About panel: the launch repair receipt is a refusal or unknown")
    }
  }

  /// The key names, for the details block. Empty when no key is involved.
  var reviewKeysLine: String? {
    guard !reviewKeys.isEmpty else { return nil }
    return String(
      localized: "Setting to review: \(reviewKeys.joined(separator: ", "))",
      comment: "About panel details: the .env key names needing review, comma-separated")
  }
}

/// Whether the first-dictation confirmation can be sent in this build, and the
/// sentence that says so. The switch stays an opt-in preference, but a build
/// without an analytics domain never sends anything, whatever the switch says.
struct ActivationPingAvailability: Equatable {
  let serviceEnabled: Bool
  let optIn: Bool

  init(serviceEnabled: Bool = ActivationPingConfiguration.production.isEnabled, optIn: Bool) {
    self.serviceEnabled = serviceEnabled
    self.optIn = optIn
  }

  /// "Default: off · Now: on" — the stored choice next to the shipped default.
  var stateLine: String {
    let current =
      optIn
      ? String(localized: "on", comment: "Switch state, lowercase, in a sentence")
      : String(localized: "off", comment: "Switch state, lowercase, in a sentence")
    return String(
      localized: "Default: off · Now: \(current)",
      comment: "About panel: shipped default and the current switch state")
  }

  /// Present only while the service is not live in this build.
  var unavailableLine: String? {
    guard !serviceEnabled else { return nil }
    return String(
      localized:
        "Not available in this version: nothing is sent, whatever the switch says.",
      comment: "About panel: the analytics domain is empty in this build")
  }
}
