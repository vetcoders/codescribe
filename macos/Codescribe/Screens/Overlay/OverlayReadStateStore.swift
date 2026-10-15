import Foundation

/// Durable "read" marks for agent replies: one small JSON next to the delivery
/// cursor, `<bridge home>/runtime/overlay-read-state.v1.json`. Conversation
/// history survives a launch through the cursor's projections, so the marks
/// must survive with it; otherwise every restored reply counts as unread.
/// Written atomically (tmp file + rename) with owner-only permissions. The
/// root is injected, so tests never touch the real bridge home.
enum OverlayReadStateStore {
  static let schema = "codescribe.overlay-read-state.v1"
  static let filename = "overlay-read-state.v1.json"

  static func url(root: URL) -> URL {
    root.appendingPathComponent("runtime", isDirectory: true)
      .appendingPathComponent(filename)
  }

  /// `nil` means no marks were ever written here (or the file is unreadable),
  /// which is different from an empty set of marks.
  static func load(root: URL) -> Set<String>? {
    guard let handle = try? FileHandle(forReadingFrom: url(root: root)) else { return nil }
    defer { try? handle.close() }
    guard let data = try? handle.read(upToCount: (8 << 20) + 1), data.count <= 8 << 20,
      let object = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
      object["schema"] as? String == schema,
      let ids = object["read_reply_ids"] as? [String]
    else { return nil }
    return Set(ids)
  }

  static func save(root: URL, readReplyIDs: Set<String>) throws {
    let directory = root.appendingPathComponent("runtime", isDirectory: true)
    try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
    let data = try JSONSerialization.data(
      withJSONObject: ["schema": schema, "read_reply_ids": readReplyIDs.sorted()] as [String: Any],
      options: [.sortedKeys])
    let temporary = directory.appendingPathComponent(".\(filename).\(UUID().uuidString).tmp")
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
      // rename(2) replaces the previous marks atomically, unlike moveItem.
      guard rename(temporary.path, url(root: root).path) == 0 else {
        throw CocoaError(.fileWriteUnknown)
      }
    } catch {
      try? FileManager.default.removeItem(at: temporary)
      throw error
    }
  }
}
