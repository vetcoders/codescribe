import Foundation
import Observation

protocol TranscriptHistoryReading: Sendable {
  func entries() async -> [CsHistoryEntry]
  func text(at path: String) async throws -> String
  /// The audio the archive wrote with this transcript, or nil when it is gone
  /// or was never kept. Never another take's audio.
  func audioPath(forTranscript path: String) async -> String?
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
}

/// One archived take reopened on the overlay canvas.
///
/// The archive file is the source and stays untouched: `path` is its durable
/// identity and `audioPath` is whatever the archive owner paired with that
/// exact file when it was opened. `text` is the canvas document for this
/// source only — edits, formatting and retranscription replace it here, and
/// nothing writes it back over the archived evidence.
struct OverlayArchivedTranscript: Equatable {
  let path: String
  let recordedAt: Date
  let audioPath: String?
  let archivedText: String
  var text: String

  init(entry: CsHistoryEntry, text: String, audioPath: String?) {
    path = entry.path
    recordedAt = Date(timeIntervalSince1970: TimeInterval(entry.timestampMs) / 1000)
    self.audioPath = audioPath
    archivedText = text
    self.text = text
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

  /// Read one archive for the canvas. A later click supersedes this one: the
  /// stale read returns nil, so take A can never land after B was chosen.
  func open(_ entry: CsHistoryEntry) async -> OverlayArchivedTranscript? {
    readGeneration &+= 1
    let generation = readGeneration
    opening = entry.path
    error = nil
    let archived: OverlayArchivedTranscript?
    do {
      let text = try await reader.text(at: entry.path)
      let audio = await reader.audioPath(forTranscript: entry.path)
      archived = OverlayArchivedTranscript(entry: entry, text: text, audioPath: audio)
    } catch {
      archived = nil
    }
    guard generation == readGeneration else { return nil }
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

  /// The canvas refused the open (for example a take started meanwhile).
  func refuseOpen(_ reason: String) {
    error = reason
  }
}
