import CryptoKit
import Foundation

/// One bus's durable read position, plus the fingerprint that detects a rotation
/// to a different file of the same length. Mirrors `BusMark` in
/// app/presentation/agent_ack.rs.
struct OverlayDeliveryCursorMark: Equatable {
  var offset: UInt64
  var inode: UInt64
  var length: UInt64
  var headHash: String
}

/// Persistence for OverlayChannelDeliveryReader positions: one small JSON at
/// `<bridge home>/runtime/overlay-delivery-cursor.v1.json`, written atomically
/// (tmp file + rename) with owner-only permissions. The file is a cache: a
/// missing or corrupt cursor costs one bounded tail-window read, never a full
/// replay. The root is injected, so tests never touch the real bridge home.
enum OverlayDeliveryCursorStore {
  static let schema = "codescribe.overlay-delivery-cursor.v1"
  static let filename = "overlay-delivery-cursor.v1.json"

  static func url(root: URL) -> URL {
    root.appendingPathComponent("runtime", isDirectory: true)
      .appendingPathComponent(filename)
  }

  static func load(root: URL) -> [String: OverlayDeliveryCursorMark] {
    guard let data = try? Data(contentsOf: url(root: root)),
      let object = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
      object["schema"] as? String == schema,
      let buses = object["buses"] as? [String: [String: Any]]
    else { return [:] }
    var marks: [String: OverlayDeliveryCursorMark] = [:]
    for (path, entry) in buses {
      guard let offset = entry["offset"] as? NSNumber,
        let inode = entry["inode"] as? NSNumber,
        let length = entry["length"] as? NSNumber,
        let headHash = entry["head_hash"] as? String
      else { continue }
      marks[path] = OverlayDeliveryCursorMark(
        offset: offset.uint64Value, inode: inode.uint64Value,
        length: length.uint64Value, headHash: headHash)
    }
    return marks
  }

  static func save(root: URL, marks: [String: OverlayDeliveryCursorMark]) throws {
    let directory = root.appendingPathComponent("runtime", isDirectory: true)
    try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
    let buses = marks.mapValues { mark in
      [
        "offset": mark.offset, "inode": mark.inode,
        "length": mark.length, "head_hash": mark.headHash,
      ] as [String: Any]
    }
    let data = try JSONSerialization.data(
      withJSONObject: ["schema": schema, "buses": buses] as [String: Any], options: [.sortedKeys])
    let temporary = directory.appendingPathComponent(".\(filename).\(UUID().uuidString).tmp")
    guard FileManager.default.createFile(atPath: temporary.path, contents: data) else {
      throw CocoaError(.fileWriteUnknown)
    }
    do {
      try FileManager.default.setAttributes(
        [.posixPermissions: 0o600], ofItemAtPath: temporary.path)
      // rename(2) replaces an existing cursor atomically, unlike moveItem.
      guard rename(temporary.path, url(root: root).path) == 0 else {
        throw CocoaError(.fileWriteUnknown)
      }
    } catch {
      try? FileManager.default.removeItem(at: temporary)
      throw error
    }
  }

  /// SHA256 of the first ≤256 bytes — the same head fingerprint `BusMark`
  /// keeps, so a rotation to an identical length is still detected.
  static func headHash(of file: FileHandle) throws -> String {
    try file.seek(toOffset: 0)
    let head = try file.read(upToCount: 256) ?? Data()
    return SHA256.hash(data: head).map { String(format: "%02x", $0) }.joined()
  }
}
