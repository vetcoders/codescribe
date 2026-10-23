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

/// Persistence for OverlayChannelDeliveryReader positions. Each bus is one
/// cache unit (cursor plus projection) in its own JSON file under
/// `<bridge home>/runtime/overlay-delivery-cursor/`, named by a digest of the
/// bus path, which the file also stores. A checkpoint rewrites only the units
/// that changed, each atomically (tmp file + rename) with owner-only
/// permissions. The files are a cache: a missing or corrupt unit costs one
/// bounded tail-window read, never a full replay. The root is injected, so
/// tests never touch the real bridge home.
enum OverlayDeliveryCursorStore {
  static let schema = "codescribe.overlay-delivery-cursor-unit.v1"
  static let directoryName = "overlay-delivery-cursor"
  /// The earlier single-file cache held every bus in one document and was
  /// rewritten whole on each checkpoint. It is never read; the first
  /// successful save deletes it.
  static let singleFileCacheName = "overlay-delivery-cursor.v1.json"
  /// Total bytes of all unit files together.
  static let budget = 128 << 20

  static func directory(root: URL) -> URL {
    root.appendingPathComponent("runtime", isDirectory: true)
      .appendingPathComponent(directoryName, isDirectory: true)
  }

  static func filename(bus: String) -> String {
    SHA256.hash(data: Data(bus.utf8)).prefix(16).map { String(format: "%02x", $0) }.joined()
      + ".json"
  }

  static func url(root: URL, bus: String) -> URL {
    directory(root: root).appendingPathComponent(filename(bus: bus))
  }

  static func load(root: URL) -> [String: OverlayDeliveryCursorMark] {
    guard
      let files = try? FileManager.default.contentsOfDirectory(
        at: directory(root: root), includingPropertiesForKeys: nil)
    else { return [:] }
    var marks: [String: OverlayDeliveryCursorMark] = [:]
    for file in files
    where file.pathExtension == "json" && !file.lastPathComponent.hasPrefix(".") {
      guard let handle = try? FileHandle(forReadingFrom: file) else { continue }
      defer { try? handle.close() }
      guard let data = try? handle.read(upToCount: budget + 1), data.count <= budget,
        let entry = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
        entry["schema"] as? String == schema,
        let path = entry["bus"] as? String,
        file.lastPathComponent == filename(bus: path),
        let offset = entry["offset"] as? NSNumber,
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

  /// Persists `marks` as the complete set of cache units. Only the units named
  /// in `changed` are encoded and written (`nil` writes every unit); the others
  /// keep their files untouched. Units absent from `marks`, or evicted by the
  /// budget, lose their files. Returns the retained marks.
  @discardableResult
  static func save(
    root: URL, marks: [String: OverlayDeliveryCursorMark], changed: Set<String>? = nil,
    budget: Int = OverlayDeliveryCursorStore.budget
  ) throws -> [String: OverlayDeliveryCursorMark] {
    let folder = Self.directory(root: root)
    try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
    var encoded: [String: Data] = [:]
    var sizes: [String: Int] = [:]
    for (path, mark) in marks {
      if changed?.contains(path) ?? true {
        let data = try encode(bus: path, mark: mark)
        encoded[path] = data
        sizes[path] = data.count
      } else {
        let attributes = try? FileManager.default.attributesOfItem(
          atPath: url(root: root, bus: path).path)
        sizes[path] = (attributes?[.size] as? NSNumber)?.intValue ?? 0
      }
    }
    var retained = marks
    var total = sizes.values.reduce(0, +)
    // A cache unit is projection plus cursor. Budget pressure evicts whole units,
    // so a restart replays their bounded tail rather than resuming with no history.
    while total > budget,
      let largest = retained.max(by: {
        ($0.value.projection?.count ?? 0) < ($1.value.projection?.count ?? 0)
      })
    {
      retained.removeValue(forKey: largest.key)
      total -= sizes[largest.key] ?? 0
    }
    for (path, data) in encoded.sorted(by: { $0.key < $1.key }) where retained[path] != nil {
      try write(data, to: url(root: root, bus: path), in: folder)
    }
    let kept = Set(retained.keys.map { filename(bus: $0) })
    for file in try FileManager.default.contentsOfDirectory(
      at: folder, includingPropertiesForKeys: nil)
    where file.pathExtension == "json" && !kept.contains(file.lastPathComponent) {
      try removeIfPresent(file)
    }
    try removeIfPresent(
      root.appendingPathComponent("runtime", isDirectory: true)
        .appendingPathComponent(singleFileCacheName))
    return retained
  }

  private static func encode(bus: String, mark: OverlayDeliveryCursorMark) throws -> Data {
    let unit: [String: Any] = [
      "schema": schema, "bus": bus,
      "offset": mark.offset, "inode": mark.inode,
      "length": mark.length, "head_hash": mark.headHash,
      "projection": mark.projection?.base64EncodedString() ?? "",
      "head_length": mark.headLength, "stream_id": mark.streamID ?? "",
      "discards_leading_parts": mark.discardsLeadingParts, "drops_cut_row": mark.dropsCutRow,
    ]
    return try JSONSerialization.data(withJSONObject: unit, options: [.sortedKeys])
  }

  private static func write(_ data: Data, to destination: URL, in directory: URL) throws {
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
        try handle.synchronize()
        try handle.close()
      } catch {
        try? handle.close()
        throw error
      }
      // rename(2) replaces an existing unit atomically, unlike moveItem.
      guard rename(temporary.path, destination.path) == 0 else {
        throw CocoaError(.fileWriteUnknown)
      }
    } catch {
      try? FileManager.default.removeItem(at: temporary)
      throw error
    }
  }

  /// A failed removal throws, so the reader keeps its state dirty and retries.
  private static func removeIfPresent(_ file: URL) throws {
    do {
      try FileManager.default.removeItem(at: file)
    } catch CocoaError.fileNoSuchFile {
      // Already absent: nothing to remove.
    }
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
