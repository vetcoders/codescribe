import Foundation

/// One selectable version of the current take, derived only from the entries
/// Rust read from the Bus journal. Identity is the journal revision; the text is
/// shown, never assigned to the canvas. Selecting it asks the reducer for a new
/// revision, and only the reducer's projection repaints.
struct OverlayTranscriptVersion: Equatable, Identifiable {
  enum Source: Equatable {
    case raw
    case lightPlus
    case consultation
    case formatted(FormattingPolicyOption?)
    case retranscribed
    case edited
    indirect case restored(Source)
    case other
  }

  let revision: UInt64
  let source: Source
  let text: String
  let emittedAt: String
  let isCurrent: Bool
  /// Why this version cannot be chosen from the current document; nil allows it.
  let refusal: String?

  var id: UInt64 { revision }
  var isSelectable: Bool { !isCurrent && refusal == nil }
  var title: String { Self.title(for: source) }

  static func title(for source: Source) -> String {
    switch source {
    case .raw:
      String(
        localized: "overlay.versions.source.raw", defaultValue: "Raw",
        comment: "Transcript version straight from transcription, before formatting")
    case .lightPlus:
      String(
        localized: "overlay.versions.source.lightPlus", defaultValue: "Light+",
        comment: "Transcript version cleaned up live by Light+ during transcription")
    case .consultation:
      String(
        localized: "overlay.versions.source.consultation", defaultValue: "Max consultation",
        comment: "Transcript version produced by a Max consultation")
    case .formatted(let level?):
      String(
        localized: "overlay.versions.source.formattedLevel",
        defaultValue: "Formatted · \(level.visibleName)",
        comment: "Transcript version produced by AI formatting; the placeholder is the level")
    case .formatted(nil):
      String(
        localized: "overlay.versions.source.formatted", defaultValue: "Formatted",
        comment: "Transcript version produced by AI formatting")
    case .retranscribed:
      String(
        localized: "overlay.versions.source.retranscribed", defaultValue: "Transcribed again",
        comment: "Transcript version from a new transcription of the same audio")
    case .edited:
      String(
        localized: "overlay.versions.source.edited", defaultValue: "Edited",
        comment: "Transcript version the user edited by hand")
    case .restored(let original):
      String(
        localized: "overlay.versions.source.restored",
        defaultValue: "Restored: \(title(for: original))",
        comment: "Transcript version brought back from an earlier one; the placeholder names it")
    case .other:
      String(
        localized: "overlay.versions.source.other", defaultValue: "Earlier version",
        comment: "Transcript version whose origin the history does not name")
    }
  }

  /// Journal provenance → source. Formatter rows carry their level as
  /// `formatter-<level>`; reducer actions without a receipt fall to `.other`.
  static func source(forProvenance provenance: String) -> Source {
    switch provenance {
    case "acoustic-ledger": return .raw
    case "light-plus": return .lightPlus
    case "consultation": return .consultation
    case "retranscribe": return .retranscribed
    case "user-edit": return .edited
    case "formatter": return .formatted(nil)
    default:
      guard provenance.hasPrefix("formatter-") else { return .other }
      return .formatted(
        FormattingPolicyOption(storedValue: String(provenance.dropFirst("formatter-".count))))
    }
  }

  /// Local wall-clock time of the journal row, when it parses.
  var timeLabel: String? {
    let fractional = Date.ISO8601FormatStyle(includingFractionalSeconds: true)
    guard
      let date = (try? fractional.parse(emittedAt))
        ?? (try? Date.ISO8601FormatStyle().parse(emittedAt))
    else { return nil }
    return date.formatted(date: .omitted, time: .standard)
  }
}

/// What the versions control shows for the take the overlay currently projects.
/// Pure: built from the read-only Bus history and the latest reducer projection.
struct OverlayTranscriptVersionsPresentation: Equatable {
  /// Distinct texts in journal order, oldest first.
  let options: [OverlayTranscriptVersion]
  /// Why no version can be chosen right now (archive, pending, unsaved edit).
  let blockedReason: String?
  /// The projection names a document the loaded history does not contain yet.
  let currentMissing: Bool
  /// An archive owns the canvas; versions of the projected take are not shown.
  let archived: Bool

  static var empty: Self {
    Self(options: [], blockedReason: nil, currentMissing: false, archived: false)
  }

  var current: OverlayTranscriptVersion? { options.first(where: \.isCurrent) }
  /// Only a choice is worth a control; an archive still explains its limit.
  var isVisible: Bool { archived || options.count >= 2 }
  var selectableRevisions: Set<UInt64> {
    blockedReason == nil ? Set(options.filter(\.isSelectable).map(\.revision)) : []
  }

  /// - Parameters:
  ///   - currentRevision: reducer revision of the projected document.
  ///   - committedText: the reducer's committed document (`renderedText`).
  ///   - shownText: the text the canvas paints and Copy/Insert deliver.
  ///   - showsDerivedPresentation: a formatter result is presented over the
  ///     committed document without being a revision of it.
  static func make(
    history: [CsDocumentHistoryEntry],
    currentRevision: UInt64,
    committedText: String,
    shownText: String,
    showsDerivedPresentation: Bool,
    blockedReason: String?
  ) -> Self {
    // A presented formatter result lives in the derived revision namespace
    // (high bit); the reducer revision still names the document behind it.
    let derivedNamespace: UInt64 = 1 << 63
    let currentEntryRevision: UInt64?
    if showsDerivedPresentation {
      currentEntryRevision =
        history.last { entry in
          entry.revision & derivedNamespace != 0 && entry.renderedText == shownText
        }?.revision
    } else {
      currentEntryRevision = history.first { $0.revision == currentRevision }?.revision
    }

    // A take's live revisions arrive as long runs of one kind; the last entry
    // of each run is the text that kind actually produced. Explicit revisions
    // (format, retranscription, edit) are each their own version.
    var collapsed: [CsDocumentHistoryEntry] = []
    for entry in history {
      let source = OverlayTranscriptVersion.source(forProvenance: entry.provenance)
      if let previous = collapsed.last, !isExplicit(source),
        OverlayTranscriptVersion.source(forProvenance: previous.provenance) == source,
        previous.revision != currentEntryRevision
      {
        collapsed[collapsed.count - 1] = entry
      } else {
        collapsed.append(entry)
      }
    }

    // One option per distinct text: choosing either copy produces the same
    // document. The current copy wins; otherwise the earliest names its origin.
    var options: [OverlayTranscriptVersion] = []
    for entry in collapsed {
      var source = OverlayTranscriptVersion.source(forProvenance: entry.provenance)
      let isCurrent = entry.revision == currentEntryRevision
      if let index = options.firstIndex(where: { $0.text == entry.renderedText }) {
        guard isCurrent else { continue }
        if source == .edited { source = .restored(options[index].source) }
        options.remove(at: index)
      }
      // The reducer refuses a revision equal to its committed document.
      let refusal: String? =
        isCurrent || entry.renderedText != committedText
        ? nil
        : (showsDerivedPresentation ? rawBehindFormattedRefusal : sameAsCurrentRefusal)
      options.append(
        OverlayTranscriptVersion(
          revision: entry.revision, source: source, text: entry.renderedText,
          emittedAt: entry.emittedAt, isCurrent: isCurrent, refusal: refusal))
    }
    return Self(
      options: options, blockedReason: blockedReason,
      currentMissing: currentEntryRevision == nil && !history.isEmpty, archived: false)
  }

  static func archivedTranscript() -> Self {
    Self(
      options: [],
      blockedReason: String(
        localized: "overlay.versions.blocked.archived",
        defaultValue:
          "Versions are listed for the current take only. A transcript opened from history can undo its last format or retranscription.",
        comment: "Versions control while a transcript reopened from history owns the canvas"),
      currentMissing: false, archived: true)
  }

  private static func isExplicit(_ source: OverlayTranscriptVersion.Source) -> Bool {
    switch source {
    case .formatted, .retranscribed, .edited, .restored: true
    case .raw, .lightPlus, .consultation, .other: false
    }
  }

  /// While a formatter result is only presented, the committed document is the
  /// raw text behind it, and no reducer path drops the presentation.
  static var rawBehindFormattedRefusal: String {
    String(
      localized: "overlay.versions.refusal.rawBehindFormatted",
      defaultValue:
        "This is the text behind the formatted view. Switching back to it is not supported yet.",
      comment: "Disabled version: the formatted view is shown over this exact text")
  }

  static var sameAsCurrentRefusal: String {
    String(
      localized: "overlay.versions.refusal.sameAsCurrent",
      defaultValue: "Same text as the current version.",
      comment: "Disabled version: choosing it would not change the transcript")
  }

  static var pendingReason: String {
    String(
      localized: "overlay.versions.blocked.pending",
      defaultValue: "Wait for the current change to finish.",
      comment: "Versions control while a revision, format or restore runs")
  }

  static var dirtyReason: String {
    String(
      localized: "overlay.versions.blocked.dirty",
      defaultValue: "Save or discard your edit before choosing a version.",
      comment: "Versions control while the canvas holds an unsaved edit")
  }
}
