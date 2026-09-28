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

/// Read-only archive browser. Selecting an old take never revises the active take.
@MainActor @Observable
final class OverlayTranscriptHistoryModel {
  private(set) var entries: [CsHistoryEntry] = []
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
    entries = await reader.entries().filter { $0.kind.isCopyableTranscript }
    loading = false
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
