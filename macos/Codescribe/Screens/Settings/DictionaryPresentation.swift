import Foundation

// Settings → Dictionary: pure presenters over the quality corpus and the
// custom lexicon. The Rust engine owns the data and the diff; Swift only
// names what it shows. No presenter here writes anything.

// MARK: - Counters

/// Three separate counts for the page header. A correction is a take whose
/// text changed; an unchanged take is kept for its telemetry only; an active
/// rule is one flattened variant → canonical pair the engine applies. None of
/// them implies another: a vocabulary correction is not a learned rule.
func dictionaryCounters(corrections: Int, unchangedTakes: Int, activeRules: Int) -> [String] {
  [
    String(localized: "\(corrections) corrections", comment: "Dictionary counter; plural"),
    String(localized: "\(unchangedTakes) unchanged takes", comment: "Dictionary counter; plural"),
    String(localized: "\(activeRules) active rules", comment: "Dictionary counter; plural"),
  ]
}

/// Where the active rules come from, shown under the rules section instead of
/// in the page subtitle. Nil when there is nothing to attribute.
func lexiconProvenanceLine(fromCorrections: Int, addedByHand: Int, other: Int) -> String? {
  guard fromCorrections + addedByHand + other > 0 else { return nil }
  var parts: [String] = []
  if fromCorrections > 0 {
    parts.append(
      String(localized: "\(fromCorrections) from corrections", comment: "Rule provenance; plural"))
  }
  if addedByHand > 0 {
    parts.append(
      String(localized: "\(addedByHand) added by hand", comment: "Rule provenance; plural"))
  }
  if other > 0 {
    parts.append(
      String(localized: "\(other) from earlier versions", comment: "Rule provenance; plural"))
  }
  return parts.joined(separator: " · ")
}

/// Rows the rules section shows as a plain list. Above this it pages.
let dictionaryRuleListLimit = 5

// MARK: - Differences between versions

/// What one changed span did to the text. The engine's tiers answer "how
/// big is the change"; the kind answers "what happened", which is what the
/// comparison labels. A span that swaps several words is a replaced fragment,
/// never a single-word correction.
enum DiffSpanKind: Equatable {
  case added
  case removed
  case replaced

  var label: String {
    switch self {
    case .added: return String(localized: "Added", comment: "Diff span kind")
    case .removed: return String(localized: "Removed", comment: "Diff span kind")
    case .replaced: return String(localized: "Replaced", comment: "Diff span kind")
    }
  }
}

func diffSpanKind(_ span: CsDiffSpan) -> DiffSpanKind {
  let raw = span.raw.trimmingCharacters(in: .whitespaces)
  let edited = span.edited.trimmingCharacters(in: .whitespaces)
  if raw.isEmpty { return .added }
  if edited.isEmpty { return .removed }
  return .replaced
}

/// One stage of a correction's history. Formatting (Smart/Max) rewrites the
/// raw STT before delivery; the manual correction edits the delivered text.
/// Showing them apart keeps a formatter's rewrite from being read as a
/// recognition error.
struct CorrectionStageDiff: Equatable {
  enum Stage: Equatable {
    case formatting
    case manual

    var title: String {
      switch self {
      case .formatting:
        return String(
          localized: "Formatting changed (raw STT → delivered)",
          comment: "Dictionary: header of the diff between raw STT and the delivered text")
      case .manual:
        return String(
          localized: "Your correction (delivered → corrected)",
          comment: "Dictionary: header of the diff between the delivered and the corrected text")
      }
    }
  }

  let stage: Stage
  let spans: [CsDiffSpan]
}

/// The stages whose text actually changed, in the order they happened.
/// Whitespace-only differences are not a stage (same rule as `hasTextChange`).
func correctionStageDiffs(raw: String, delivered: String, edited: String) -> [CorrectionStageDiff] {
  let rawNorm = normalizedCorrectionText(raw)
  let deliveredNorm = normalizedCorrectionText(delivered)
  let editedNorm = normalizedCorrectionText(edited)
  var stages: [CorrectionStageDiff] = []
  if deliveredNorm != rawNorm {
    stages.append(
      CorrectionStageDiff(stage: .formatting, spans: voiceLabDiffSpans(raw: raw, edited: delivered)))
  }
  if editedNorm != deliveredNorm {
    stages.append(
      CorrectionStageDiff(stage: .manual, spans: voiceLabDiffSpans(raw: delivered, edited: edited)))
  }
  return stages
}

/// Toggle label for the three full texts.
func fullComparisonLabel(rawCount: Int, editedCount: Int) -> String {
  String(
    localized: "Full comparison · \(rawCount) → \(editedCount) characters",
    comment: "Dictionary: toggle showing the full texts; the numbers are character counts")
}

// MARK: - Card footer

/// "Version N · date", with the closing action in front when the record was
/// captured by something other than a saved revision.
func correctionFooter(action: String, revision: UInt64, timestamp: String) -> String {
  let version = String(localized: "Version \(revision)", comment: "Dictionary: correction revision")
  if action == "revision" {
    return "\(version) · \(timestamp)"
  }
  return "\(QualityActionLabel.text(for: action)) · \(version) · \(timestamp)"
}

// MARK: - Archived audio

/// Result of pairing a correction with its archived recording. The lookup
/// walks the whole `transcriptions` archive, so callers run it off the main
/// actor and the value crosses back as a Sendable.
enum ArchivedAudioLookup: Equatable, Sendable {
  case found(URL)
  case missing
  /// Several archived takes carry this exact raw transcript. Nothing in the
  /// record says which one is this correction, so neither playback nor a
  /// retranscribe may pick one: a different take would be a lie.
  case ambiguous(Int)

  var url: URL? {
    if case .found(let url) = self { return url }
    return nil
  }
}

/// Why Retranscribe is disabled, or nil when it is available.
func retranscribeUnavailableReason(
  asrMode: String, lookup: ArchivedAudioLookup, pending: Bool
) -> String? {
  if pending {
    return String(
      localized: "Retranscribing…", comment: "Dictionary: the helper pass is still running")
  }
  if helperRetranscribePass(asrMode: asrMode) == nil {
    return String(
      localized: "Retranscribe needs the Local power or Cloud mode.",
      comment: "Dictionary: why Retranscribe is disabled")
  }
  switch lookup {
  case .found: return nil
  case .missing:
    return String(
      localized: "No archived recording for this correction.",
      comment: "Dictionary: why Retranscribe is disabled")
  case .ambiguous(let count):
    return String(
      localized:
        "\(count) archived recordings share this exact transcript, so Codescribe cannot tell which one is this take.",
      comment: "Dictionary: why Retranscribe and playback are disabled; plural")
  }
}

// MARK: - Learn from corrections

/// What the Learn action is about to do, shown before it runs.
func learnScopeMessage(corrections: Int) -> String {
  String(
    localized:
      "Codescribe reviews all \(corrections) saved corrections and the suggested rules, then adds the new vocabulary rules it can derive to My rules. Existing rules, corrections and their history stay as they are.",
    comment: "Dictionary: confirmation before Learn from corrections; plural")
}

/// The result line after Learn. `added` is the real growth of the rules list
/// (rows after minus rows before): the engine's own counters report every
/// eligible pair, including ones already learned, so they cannot say what is
/// new. Suggestions are mentioned only when they contributed.
func learnResultMessage(added: Int, fromSuggestions: Int, activeRules: Int) -> String {
  let rules = String(localized: "\(activeRules) active rules", comment: "Dictionary counter; plural")
  if added <= 0 {
    return String(
      localized: "No new rules: everything eligible is already in My rules · \(rules)",
      comment: "Dictionary: Learn found nothing new; the placeholder is the active rules count")
  }
  if fromSuggestions > 0 {
    return String(
      localized: "Added \(added) rules from corrections and suggestions · \(rules)",
      comment: "Dictionary: Learn result; plural on the first number")
  }
  return String(
    localized: "Added \(added) rules from corrections · \(rules)",
    comment: "Dictionary: Learn result; plural on the first number")
}
