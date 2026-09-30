import Foundation
import Observation

protocol TranscriptHistoryReading: Sendable {
  func entries() async -> [CsHistoryEntry]
  func text(at path: String) async throws -> String
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
}

struct TranscriptHistoryRecord {
  let entry: CsHistoryEntry
  let characterCount: Int?

  var path: String { entry.path }
}

/// Read-only archive browser. Selecting an old take never revises the active take.
@MainActor @Observable
final class OverlayTranscriptHistoryModel {
  private(set) var entries: [TranscriptHistoryRecord] = []
  private(set) var selected: CsHistoryEntry?
  private(set) var text: String?
  private(set) var error: String?
  private(set) var loading = false
  private(set) var reading = false
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
    guard let count else { return "Length unavailable" }
    let formatter = NumberFormatter()
    formatter.numberStyle = .decimal
    formatter.locale = locale
    let formatted = formatter.string(from: NSNumber(value: count)) ?? String(count)
    let separator = formatter.groupingSeparator ?? " "
    if (1_000...9_999).contains(count) && !formatted.contains(separator) {
      return "\(formatted.prefix(1))\(separator)\(formatted.dropFirst()) chars"
    }
    return "\(formatted) chars"
  }

  func select(_ entry: CsHistoryEntry) async {
    readGeneration &+= 1
    let generation = readGeneration
    selected = entry
    text = nil
    error = nil
    reading = true
    do {
      let value = try await reader.text(at: entry.path)
      guard generation == readGeneration else { return }
      text = value
    } catch {
      guard generation == readGeneration else { return }
      self.error = "Could not open this transcript."
    }
    reading = false
  }

  func back() {
    readGeneration &+= 1
    selected = nil
    text = nil
    error = nil
    reading = false
  }
}
