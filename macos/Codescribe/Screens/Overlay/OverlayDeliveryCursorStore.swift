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
  var projection: Data? = nil
  var headLength: Int = 256
  var streamID: String? = nil
  var discardsLeadingParts = false
  var dropsCutRow = false
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
    guard let handle = try? FileHandle(forReadingFrom: url(root: root)) else { return [:] }
    defer { try? handle.close() }
    guard let data = try? handle.read(upToCount: (128 << 20) + 1), data.count <= 128 << 20,
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
        length: length.uint64Value, headHash: headHash,
        projection: (entry["projection"] as? String).flatMap { Data(base64Encoded: $0) },
        headLength: (entry["head_length"] as? NSNumber)?.intValue ?? 256,
        streamID: entry["stream_id"] as? String,
        discardsLeadingParts: entry["discards_leading_parts"] as? Bool == true,
        dropsCutRow: entry["drops_cut_row"] as? Bool == true)
    }
    return marks
  }

  @discardableResult
  static func save(root: URL, marks: [String: OverlayDeliveryCursorMark]) throws
    -> [String: OverlayDeliveryCursorMark] {
    let directory = root.appendingPathComponent("runtime", isDirectory: true)
    try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
    var retained = marks
    func encode(_ values: [String: OverlayDeliveryCursorMark]) throws -> Data {
      let buses = values.mapValues { mark in
        [
          "offset": mark.offset, "inode": mark.inode,
          "length": mark.length, "head_hash": mark.headHash,
          "projection": mark.projection?.base64EncodedString() ?? "",
          "head_length": mark.headLength, "stream_id": mark.streamID ?? "",
          "discards_leading_parts": mark.discardsLeadingParts, "drops_cut_row": mark.dropsCutRow,
        ] as [String: Any]
      }
      return try JSONSerialization.data(
        withJSONObject: ["schema": schema, "buses": buses] as [String: Any], options: [.sortedKeys])
    }
    var data = try encode(retained)
    // A cache unit is projection plus cursor. Budget pressure evicts whole units,
    // so a restart replays their bounded tail rather than resuming with no history.
    while data.count > 128 << 20, let largest = retained.max(by: {
      ($0.value.projection?.count ?? 0) < ($1.value.projection?.count ?? 0)
    }) {
      retained.removeValue(forKey: largest.key)
      data = try encode(retained)
    }
    let temporary = directory.appendingPathComponent(".\(filename).\(UUID().uuidString).tmp")
    guard FileManager.default.createFile(
      atPath: temporary.path, contents: nil, attributes: [.posixPermissions: 0o600]) else {
      throw CocoaError(.fileWriteUnknown)
    }
    do {
      let handle = try FileHandle(forWritingTo: temporary)
      do {
        try handle.write(contentsOf: data)
        try handle.synchronize()
        try handle.close()
      } catch {
        try? handle.close()
        throw error
      }
      // rename(2) replaces an existing cursor atomically, unlike moveItem.
      guard rename(temporary.path, url(root: root).path) == 0 else {
        throw CocoaError(.fileWriteUnknown)
      }
    } catch {
      try? FileManager.default.removeItem(at: temporary)
      throw error
    }
    return retained
  }

  /// SHA256 of the first ≤256 bytes — the same head fingerprint `BusMark`
  /// keeps, so a rotation to an identical length is still detected.
  static func headHash(of file: FileHandle, length: Int = 256) throws -> String {
    try file.seek(toOffset: 0)
    guard (0...256).contains(length) else { throw CocoaError(.fileReadCorruptFile) }
    let head = try file.read(upToCount: length) ?? Data()
    return SHA256.hash(data: head).map { String(format: "%02x", $0) }.joined()
  }
}
