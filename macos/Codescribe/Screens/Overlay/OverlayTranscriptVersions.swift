import Foundation

/// One accepted version of the transcript on the canvas, exactly as Rust's
/// linear history lists it. Identity is the step; equal texts stay separate
/// versions because each is an attempt the user made. The text is shown,
/// never assigned to the canvas: choosing a version asks Rust to move its
/// cursor, and only the projection that answers repaints.
struct OverlayTranscriptVersion: Equatable, Identifiable {
  enum Source: Equatable {
    case raw
    case lightPlus
    case original
    case formatted(FormattingPolicyOption?)
    case retranscribed
    case edited
    case other
  }

  let step: UInt64
  let source: Source
  let text: String
  let emittedAt: String
  let isCurrent: Bool

  var id: UInt64 { step }
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
    case .original:
      String(
        localized: "overlay.versions.source.original", defaultValue: "Saved transcript",
        comment: "The transcript as it was saved to history, before any later change")
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
    case .other:
      String(
        localized: "overlay.versions.source.other", defaultValue: "Earlier version",
        comment: "Transcript version whose origin the history does not name")
    }
  }

  /// The operation that produced a version, as Undo and Redo name it.
  static func actionName(for source: Source) -> String {
    switch source {
    case .formatted(let level?):
      String(
        localized: "overlay.versions.action.formatLevel",
        defaultValue: "format (\(level.visibleName))",
        comment: "Undo/Redo tooltip object: an AI format; the placeholder is the level")
    case .formatted(nil):
      String(
        localized: "overlay.versions.action.format", defaultValue: "format",
        comment: "Undo/Redo tooltip object: an AI format")
    case .retranscribed:
      String(
        localized: "overlay.versions.action.retranscribe", defaultValue: "retranscription",
        comment: "Undo/Redo tooltip object: a new transcription of the same audio")
    case .edited:
      String(
        localized: "overlay.versions.action.edit", defaultValue: "edit",
        comment: "Undo/Redo tooltip object: a hand edit of the transcript")
    case .raw, .lightPlus, .original, .other:
      String(
        localized: "overlay.versions.action.transcript", defaultValue: "transcript",
        comment: "Undo/Redo tooltip object: the first transcript of the take")
    }
  }

  /// Rust provenance → source. A formatted version carries its level as
  /// `detail`; an unknown provenance is named, never guessed.
  static func source(provenance: String, detail: String?) -> Source {
    switch provenance {
    case "raw", "acoustic-ledger": return .raw
    case "light-plus": return .lightPlus
    case "original": return .original
    case "retranscribe": return .retranscribed
    case "user-edit": return .edited
    case "formatter":
      return .formatted(FormattingPolicyOption(storedValue: detail))
    default: return .other
    }
  }

  /// Local wall-clock time of the version, when it parses.
  var timeLabel: String? {
    let fractional = Date.ISO8601FormatStyle(includingFractionalSeconds: true)
    guard
      let date = (try? fractional.parse(emittedAt))
        ?? (try? Date.ISO8601FormatStyle().parse(emittedAt))
    else { return nil }
    return date.formatted(date: .omitted, time: .standard)
  }
}

/// The versions control, Undo and Redo for the transcript on the canvas. Pure:
/// built from Rust's linear history (steps + cursor) and the overlay's
/// pending/dirty facts. Picker, Undo and Redo share one cursor.
struct OverlayTranscriptVersionsPresentation: Equatable {
  /// Every accepted version, oldest first.
  let options: [OverlayTranscriptVersion]
  /// The selected version.
  let cursor: UInt64
  /// Why no version can be chosen right now (pending work, unsaved edit).
  let blockedReason: String?

  static var empty: Self { Self(options: [], cursor: 0, blockedReason: nil) }

  var current: OverlayTranscriptVersion? { options.first(where: \.isCurrent) }
  /// A choice exists only with two versions.
  var isVisible: Bool { options.count >= 2 }
  var canUndo: Bool { blockedReason == nil && isVisible && cursor > 0 }
  var canRedo: Bool { blockedReason == nil && cursor + 1 < UInt64(options.count) }
  var undoStep: UInt64? { canUndo ? cursor - 1 : nil }
  var redoStep: UInt64? { canRedo ? cursor + 1 : nil }
  var selectableSteps: Set<UInt64> {
    blockedReason == nil ? Set(options.filter { !$0.isCurrent }.map(\.step)) : []
  }

  static func make(
    versions: [CsDocumentVersion], cursor: UInt64, blockedReason: String?
  ) -> Self {
    let options = versions.map { version in
      OverlayTranscriptVersion(
        step: version.step,
        source: OverlayTranscriptVersion.source(
          provenance: version.provenance, detail: version.detail),
        text: version.renderedText, emittedAt: version.emittedAt,
        isCurrent: version.step == cursor)
    }
    return Self(options: options, cursor: cursor, blockedReason: blockedReason)
  }

  /// Undo names the operation it takes back: the one that made the current
  /// version. At the first version, or while blocked, it says why not.
  var undoTitle: String {
    if let blockedReason, isVisible, cursor > 0 { return blockedReason }
    guard let current, cursor > 0 else {
      return String(
        localized: "overlay.versions.undo.none", defaultValue: "Nothing to undo",
        comment: "Disabled Undo: the transcript shows its first version")
    }
    return String(
      localized: "overlay.versions.undo.action",
      defaultValue: "Undo \(OverlayTranscriptVersion.actionName(for: current.source))",
      comment: "Undo tooltip; the placeholder names the operation taken back")
  }

  /// Redo names the operation it brings back: the one after the current
  /// version. At the newest version, or while blocked, it says why not.
  var redoTitle: String {
    let next = Int(cursor) + 1
    if let blockedReason, next < options.count { return blockedReason }
    guard next < options.count else {
      return String(
        localized: "overlay.versions.redo.none", defaultValue: "Nothing to redo",
        comment: "Disabled Redo: the transcript shows its newest version")
    }
    return String(
      localized: "overlay.versions.redo.action",
      defaultValue: "Redo \(OverlayTranscriptVersion.actionName(for: options[next].source))",
      comment: "Redo tooltip; the placeholder names the operation brought back")
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
