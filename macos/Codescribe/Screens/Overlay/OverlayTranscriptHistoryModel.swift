import Foundation
import Observation

protocol TranscriptHistoryReading: Sendable {
  func entries() async -> [CsHistoryEntry]
  func text(at path: String) async throws -> String
  /// The audio the archive wrote with this transcript, or nil when it is gone
  /// or was never kept. Never another take's audio.
  func audioPath(forTranscript path: String) async -> String?
  /// The archive as its history owner holds it: untouched original plus the
  /// accepted head of its revision chain.
  func document(at path: String) async throws -> CsArchivedDocument
}

extension TranscriptHistoryReading {
  /// A reader without a revision owner sees every archive unrevised.
  func document(at path: String) async throws -> CsArchivedDocument {
    let text = try await text(at: path)
    return .unrevised(path: path, text: text)
  }
}

struct ArchivedTranscriptHistory: TranscriptHistoryReading {
  func entries() async -> [CsHistoryEntry] {
    await Task.detached {
      CodescribeThreads().recentHistory(limit: 100)
    }.value
  }

  func text(at path: String) async throws -> String {
    try await Task.detached {
      try CodescribeThreads().readHistoryText(path: path)
    }.value
  }

  func audioPath(forTranscript path: String) async -> String? {
    await Task.detached {
      CodescribeThreads().historyAudioPath(path: path)
    }.value
  }

  func document(at path: String) async throws -> CsArchivedDocument {
    try await Task.detached {
      try CodescribeThreads().historyDocument(path: path)
    }.value
  }
}

extension CsArchivedDocument {
  /// An archive with no revision chain: its original is its only version.
  static func unrevised(path: String, text: String) -> Self {
    CsArchivedDocument(
      path: path, originalText: text, revision: 0, renderedText: text,
      provenance: "original", receiptId: "",
      versions: [
        CsDocumentVersion(
          step: 0, provenance: "original", detail: nil, renderedText: text, emittedAt: "",
          receiptId: "")
      ],
      cursor: 0)
  }
}

/// One archived take reopened on the overlay canvas.
///
/// `path` is the archive's durable identity and `audioPath` is whatever the
/// archive owner paired with that exact file. `archivedText` is the raw
/// archived transcript and is never rewritten. `text` and `revision` are the
/// accepted head of the archive's revision chain, which Rust owns: the canvas
/// paints them and addresses the next request with them, and only a document
/// Rust returned for this same path can move them.
struct OverlayArchivedTranscript: Equatable {
  let path: String
  let recordedAt: Date
  let audioPath: String?
  let archivedText: String
  private(set) var text: String
  private(set) var revision: UInt64
  private(set) var provenance: String
  private(set) var receiptId: String
  /// Every accepted version of the archive, oldest first, and the selected
  /// one, replayed by Rust from the revision chain.
  private(set) var versions: [CsDocumentVersion]
  private(set) var cursor: UInt64

  init(entry: CsHistoryEntry, document: CsArchivedDocument, audioPath: String?) {
    path = entry.path
    recordedAt = Date(timeIntervalSince1970: TimeInterval(entry.timestampMs) / 1000)
    self.audioPath = audioPath
    archivedText = document.originalText
    text = document.renderedText
    revision = document.revision
    provenance = document.provenance
    receiptId = document.receiptId
    versions = document.versions
    cursor = document.cursor
  }

  /// An unrevised archive.
  init(entry: CsHistoryEntry, text: String, audioPath: String?) {
    self.init(
      entry: entry,
      document: .unrevised(path: entry.path, text: text),
      audioPath: audioPath)
  }

  /// Take a newer head Rust returned for this archive. A document for another
  /// path, or one older than what is shown, never moves the canvas.
  @discardableResult
  mutating func accept(_ document: CsArchivedDocument) -> Bool {
    guard document.path == path, document.revision >= revision else { return false }
    text = document.renderedText
    revision = document.revision
    provenance = document.provenance
    receiptId = document.receiptId
    versions = document.versions
    cursor = document.cursor
    return true
  }

  var isChangedFromArchive: Bool { !text.utf8.elementsEqual(archivedText.utf8) }
}

struct TranscriptHistoryRecord {
  let entry: CsHistoryEntry
  let characterCount: Int?

  var path: String { entry.path }
}

/// Archive list for the overlay. Opening an entry reads its full text and its
/// paired audio and hands one `OverlayArchivedTranscript` to the canvas; the
/// list itself never revises the active take.
@MainActor @Observable
final class OverlayTranscriptHistoryModel {
  private(set) var entries: [TranscriptHistoryRecord] = []
  /// The entry whose open is in flight; only the latest click may land.
  private(set) var opening: String?
  private(set) var error: String?
  private(set) var loading = false
  private let reader: any TranscriptHistoryReading
  private var readGeneration: UInt64 = 0

  init(reader: any TranscriptHistoryReading = ArchivedTranscriptHistory()) {
    self.reader = reader
  }

  func load() async {
    guard !loading else { return }
    loading = true
    let archives = await reader.entries().filter { $0.kind.isCopyableTranscript }
    var records: [TranscriptHistoryRecord] = []
    for entry in archives {
      let text = try? await reader.text(at: entry.path)
      records.append(TranscriptHistoryRecord(entry: entry, characterCount: text?.count))
    }
    entries = records
    loading = false
  }

  static func formattedCharacterCount(_ count: Int?, locale: Locale) -> String {
    guard let count else { return String(localized: "Length unavailable", locale: locale) }
    let formatter = NumberFormatter()
    formatter.numberStyle = .decimal
    formatter.locale = locale
    let formatted = formatter.string(from: NSNumber(value: count)) ?? String(count)
    let separator = formatter.groupingSeparator ?? " "
    let grouped =
      (1_000...9_999).contains(count) && !formatted.contains(separator)
      ? "\(formatted.prefix(1))\(separator)\(formatted.dropFirst())"
      : formatted
    let localized = AttributedString(
      localized: "\(count) chars",
      options: .applyReplacementIndexAttribute,
      locale: locale,
      comment: "Length of an archived take; the integer counts characters")
    // A plural replacement marks the whole phrase, including its noun. Locate
    // the locale-formatted integer inside that replacement before regrouping it.
    let localizedNumber = String(format: "%lld", locale: locale, count)
    return localized.runs[\.replacementIndex].map { index, range in
      let phrase = String(localized[range].characters)
      guard index == 1, let numberRange = phrase.range(of: localizedNumber) else { return phrase }
      return phrase.replacingCharacters(in: numberRange, with: grouped)
    }.joined()
  }

  /// Read one archive for the canvas. A later click or a cancelled open
  /// supersedes this one: the stale read returns nil, so take A can never
  /// land after B was chosen or after the list went away.
  func open(_ entry: CsHistoryEntry) async -> OverlayArchivedTranscript? {
    readGeneration &+= 1
    let generation = readGeneration
    opening = entry.path
    error = nil
    let archived: OverlayArchivedTranscript?
    do {
      let document = try await reader.document(at: entry.path)
      let audio = await reader.audioPath(forTranscript: entry.path)
      archived = OverlayArchivedTranscript(entry: entry, document: document, audioPath: audio)
    } catch {
      archived = nil
    }
    guard generation == readGeneration, !Task.isCancelled else { return nil }
    opening = nil
    guard let archived else {
      error = String(localized: "Could not open this transcript.")
      return nil
    }
    guard !archived.text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else {
      error = String(
        localized: "This saved transcript is empty.",
        comment: "History entry whose archived text file has no words")
      return nil
    }
    return archived
  }

  /// The list went away or a newer request took over: whatever read is still
  /// in flight can no longer return an archive.
  func cancelOpen() {
    readGeneration &+= 1
    opening = nil
  }

  /// The canvas refused the open (for example a take started meanwhile).
  func refuseOpen(_ reason: String) {
    error = reason
  }
}
