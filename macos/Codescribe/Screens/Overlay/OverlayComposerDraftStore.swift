import Foundation

/// Unsent composer drafts, one per exact conversation: `0` for All, `10` for
/// the inbox, an agent's owner id (`provider`, `session`, `lease`) otherwise.
/// On 2026-11-08 a crash took a roughly 1500-character All draft that lived
/// only in memory; these words now outlive the process.
/// `<data root>/data.noindex/overlay-composer-drafts.v1.json`, written like the
/// read marks: atomically and owner-only. Keys are exact conversation ids, so a
/// replacement agent on the same channel (another lease) never receives a
/// previous owner's draft.
enum OverlayComposerDraftStore {
  static let schema = "codescribe.overlay-composer-drafts.v1"
  static let filename = "overlay-composer-drafts.v1.json"
  /// A draft whose conversation has not come back within this window is let go.
  static let retention: TimeInterval = 30 * 24 * 60 * 60

  struct Entry: Codable, Equatable {
    var text: String
    var reply: OverlayUserReplyReference?
    var savedAt: Date

    enum CodingKeys: String, CodingKey {
      case text, reply
      case savedAt = "saved_at"
    }
  }

  private struct Payload: Codable {
    var schema: String
    var drafts: [String: Entry]
  }

  static func url(cacheRoot: URL) -> URL {
    cacheRoot.appendingPathComponent(filename)
  }

  /// `nil` when nothing was ever stored here or the file is unreadable.
  /// Entries older than `retention`, or blank, are not returned.
  static func load(cacheRoot: URL, now: Date = Date()) -> [String: Entry]? {
    guard let handle = try? FileHandle(forReadingFrom: url(cacheRoot: cacheRoot)) else {
      return nil
    }
    defer { try? handle.close() }
    guard let data = try? handle.read(upToCount: (8 << 20) + 1), data.count <= 8 << 20 else {
      return nil
    }
    let decoder = JSONDecoder()
    decoder.dateDecodingStrategy = .iso8601
    guard let payload = try? decoder.decode(Payload.self, from: data), payload.schema == schema
    else { return nil }
    return payload.drafts.filter { _, entry in
      now.timeIntervalSince(entry.savedAt) <= retention
        && !entry.text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
    }
  }

  static func save(cacheRoot: URL, drafts: [String: Entry]) throws {
    let encoder = JSONEncoder()
    encoder.dateEncodingStrategy = .iso8601
    encoder.outputFormatting = [.sortedKeys]
    let data = try encoder.encode(Payload(schema: schema, drafts: drafts))
    try OverlayPrivateFile.write(data, to: url(cacheRoot: cacheRoot))
  }
}

/// One owner-only file replaced atomically: a temporary sibling, then
/// rename(2), so a reader or a crash never sees a half-written file.
enum OverlayPrivateFile {
  static func write(_ data: Data, to destination: URL) throws {
    let directory = destination.deletingLastPathComponent()
    try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
    let temporary = directory.appendingPathComponent(
      ".\(destination.lastPathComponent).\(UUID().uuidString).tmp")
    guard
      FileManager.default.createFile(
        atPath: temporary.path, contents: nil, attributes: [.posixPermissions: 0o600])
    else {
      throw CocoaError(.fileWriteUnknown)
    }
    do {
      let handle = try FileHandle(forWritingTo: temporary)
      do {
        try handle.write(contentsOf: data)
        try handle.close()
      } catch {
        try? handle.close()
        throw error
      }
      // rename(2) replaces the previous file atomically, unlike moveItem.
      guard rename(temporary.path, destination.path) == 0 else {
        throw CocoaError(.fileWriteUnknown)
      }
    } catch {
      try? FileManager.default.removeItem(at: temporary)
      throw error
    }
  }
}
