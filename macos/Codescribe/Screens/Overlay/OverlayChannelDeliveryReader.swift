import CryptoKit
import Foundation
import OSLog

/// File I/O stays off the main actor. Verified generations form one logical
/// byte stream; storage rollover retains observation state and cursor identity.
/// A cold monitor starts at the newest tail window of that whole stream.
/// Inventory metadata is checked without reading historical segment bytes.
actor OverlayChannelDeliveryReader {
  /// Bytes of bus history a cold or reset reader parses: the tail window.
  static let tailWindow: UInt64 = 64 << 20
  private static let chunkSize = 262_144
  private static let logger = Logger(
    subsystem: Bundle.main.bundleIdentifier ?? "com.vetcoders.codescribe",
    category: "overlay-delivery-reader")

  private let root: URL
  private var buses: [URL: Cursor] = [:]
  private var persisted: [String: OverlayDeliveryCursorMark]?
  /// Total bus bytes this reader consumed from disk. Tests use it to prove a
  /// restart reads only new bytes instead of replaying history.
  private(set) var consumedBytes: UInt64 = 0

  private struct Cursor {
    var inode: UInt64 = 0
    var offset: UInt64 = 0
    var headHash = ""
    /// The tail window landed mid-row: drop everything through the first newline.
    var dropsCutRow = false
    var partial = Data()
    var projection = OverlayChannelDelivery.Bus()
    var storage = StorageDecoder()
    var storageStart: UInt64?
  }

  init(root: URL) { self.root = root }

  static func productionRoot(
    environment: [String: String] = ProcessInfo.processInfo.environment,
    home: URL = FileManager.default.homeDirectoryForCurrentUser
  ) -> URL {
    if let path = environment["CODESCRIBE_AGENT_BRIDGE_HOME"]?
      .trimmingCharacters(in: .whitespacesAndNewlines), !path.isEmpty
    {
      return URL(fileURLWithPath: (path as NSString).expandingTildeInPath)
    }
    return home.appendingPathComponent(".codescribe/agent-bridge", isDirectory: true)
  }

  func read() throws -> [OverlayChannelDelivery] {
    let bindingsURL = root.appendingPathComponent("vc.agent-audience-binding.v1.json")
    guard FileManager.default.fileExists(atPath: bindingsURL.path) else {
      buses.removeAll()
      return []
    }
    let bindingFile = try object(at: bindingsURL)
    guard bindingFile["schema"] as? String == "vc.agent-audience-binding.v1",
      let bindings = bindingFile["bindings"] as? [String: [String: Any]]
    else { throw CocoaError(.fileReadCorruptFile) }
    let leasesURL = root.appendingPathComponent("leases", isDirectory: true)
    guard FileManager.default.fileExists(atPath: leasesURL.path) else { return [] }
    let files = try FileManager.default.contentsOfDirectory(
      at: leasesURL, includingPropertiesForKeys: nil)
    let leases = try files.filter { $0.pathExtension == "json" }.map { file in
      let lease = try object(at: file)
      guard lease["lease_id"] as? String == file.deletingPathExtension().lastPathComponent else {
        throw CocoaError(.fileReadCorruptFile)
      }
      return lease
    }
    var result: [OverlayChannelDelivery] = []
    var usedBuses: Set<URL> = []
    for channel in bindings.keys.sorted() where channel.count == 1 && "123456789".contains(channel)
    {
      guard let binding = bindings[channel] else { continue }
      let owners = leases.filter {
        $0["provider"] as? String == binding["provider"] as? String
          && $0["provider_session_id"] as? String == binding["provider_session_id"] as? String
      }
      guard !owners.isEmpty else { continue }
      guard owners.count == 1, let lease = owners.first,
        let path = lease["bus"] as? String, path.hasPrefix("/")
      else { throw CocoaError(.fileReadCorruptFile) }
      // The lease names the actual bus resolved by its producer, including
      // data-dir/XDG overrides; the UI never invents another path authority.
      let bus = URL(fileURLWithPath: path).standardizedFileURL
      if usedBuses.insert(bus).inserted { try refresh(bus) }
      if let status = buses[bus]?.projection.project(
        channel: channel, binding: binding, lease: lease)
      {
        result.append(status)
      }
    }
    buses = buses.filter { usedBuses.contains($0.key) }
    return result
  }

  private func object(at url: URL) throws -> [String: Any] {
    guard
      let value = try JSONSerialization.jsonObject(with: Data(contentsOf: url)) as? [String: Any]
    else { throw CocoaError(.fileReadCorruptFile) }
    return value
  }

  private func refresh(_ url: URL) throws {
    let file = try GenerationStream(url)
    let inode = file.inode
    let size = file.size
    // Explicit branches stay on this actor. Nil-coalescing would capture the
    // non-Sendable stream in an autoclosure.
    var cursor: Cursor
    if let stored = buses[url] {
      cursor = stored
    } else if file.linked {
      cursor = Cursor()
    } else if let persisted = persistedCursor(for: url) {
      cursor = persisted
    } else {
      cursor = Cursor()
    }
    if cursor.offset > 0 {
      let stale = !file.linked && size >= cursor.offset && size - cursor.offset > Self.tailWindow
      let headHash = try file.headHash()
      if cursor.inode != inode || size < cursor.offset || stale || headHash != cursor.headHash {
        // Unlinked rotation, truncation, or a gap larger than the window resets
        // to a fresh tail. A linked cursor keeps its logical offset: the stream
        // inode does not change when the hot file rolls over.
        cursor = Cursor()
      }
    }
    if cursor.headHash.isEmpty {
      let start = file.coldStart
      cursor.inode = inode
      cursor.headHash = try file.headHash()
      cursor.offset = start
      if start > 0 {
        // Skip only the row the window cut in half: when the byte before the
        // window is a newline, the window begins exactly on a row boundary.
        try file.seek(toOffset: start - 1)
        cursor.dropsCutRow = try file.read(1)?.first != 10
      }
    }
    try file.seek(toOffset: cursor.offset)
    var passBytes: UInt64 = 0
    while passBytes < Self.tailWindow, let chunk = try file.read(Self.chunkSize), !chunk.isEmpty {
      passBytes += UInt64(chunk.count)
      try autoreleasepool {
        cursor.offset += UInt64(chunk.count)
        consumedBytes += UInt64(chunk.count)
        cursor.partial.append(chunk)
        // Track a start index instead of removing each line: the consumed
        // prefix is cut once per chunk, never once per line.
        var start = cursor.partial.startIndex
        while let newline = cursor.partial[start...].firstIndex(of: 10) {
          let line = Data(cursor.partial[start..<newline])
          start = cursor.partial.index(after: newline)
          if cursor.dropsCutRow {
            cursor.dropsCutRow = false
            continue
          }
          if line.isEmpty { continue }
          if let row = try? JSONSerialization.jsonObject(with: line) as? [String: Any] {
            if !cursor.storage.incomplete {
              cursor.storageStart =
                cursor.offset - UInt64(cursor.partial.count) + UInt64(start - line.count - 1)
            }
            for document in try cursor.storage.consume(row) {
              try consumeDocument(document, cursor: &cursor)
            }
            if !cursor.storage.incomplete { cursor.storageStart = nil }
          } else {
            // Ordinary malformed rows retain the existing skip policy.
            // Broken storage transactions throw before advancing durable marks.
            Self.logger.debug(
              "overlay bus \(url.lastPathComponent, privacy: .public): skipped an unparseable \(line.count)-byte row"
            )
          }
        }
        cursor.partial.removeSubrange(..<start)
      }
    }
    buses[url] = cursor
    try persist(url, cursor: cursor, size: size)
  }

  private func persistedCursor(for url: URL) -> Cursor? {
    if persisted == nil { persisted = OverlayDeliveryCursorStore.load(root: root) }
    guard let mark = persisted?[url.path] else { return nil }
    return Cursor(inode: mark.inode, offset: mark.offset, headHash: mark.headHash)
  }

  private func persist(_ url: URL, cursor: Cursor, size: UInt64) throws {
    var marks = persisted ?? [:]
    // The durable offset points past the last consumed newline; an unfinished
    // trailing row is re-read by the next generation once it completes.
    let mark = OverlayDeliveryCursorMark(
      offset: cursor.storageStart ?? (cursor.offset - UInt64(cursor.partial.count)),
      inode: cursor.inode, length: size, headHash: cursor.headHash)
    guard marks[url.path] != mark else { return }
    marks[url.path] = mark
    try OverlayDeliveryCursorStore.save(root: root, marks: marks)
    persisted = marks
  }

  /// Expand every persisted occurrence before the existing UI projection acts.
  private func consumeDocument(_ value: [String: Any], cursor: inout Cursor) throws {
    var common = value
    guard let encoding = common.removeValue(forKey: "persistence_encoding") else {
      cursor.projection.consume(common)
      return
    }
    let keys: Set<String> = [
      "sequence", "emitted_at", "occurrence_session_id", "capture_epoch",
      "sample_start", "sample_end", "document_index", "label", "acoustic_receipts",
    ]
    guard encoding as? String == "shared-revision.v1",
      common["schema"] as? String == "codescribe.transcript-evidence.v1",
      let occurrences = common.removeValue(forKey: "occurrence_rows") as? [[String: Any]],
      occurrences.allSatisfy({ Set($0.keys) == keys && $0["acoustic_receipts"] is [[String: Any]] })
    else { throw CocoaError(.fileReadCorruptFile) }
    // Validate the inventory before publishing even its first occurrence.
    for occurrence in occurrences {
      guard
        ["emitted_at", "occurrence_session_id", "label"].allSatisfy({ occurrence[$0] is String }),
        ["sequence", "capture_epoch", "sample_start", "sample_end", "document_index"].allSatisfy({
          occurrence[$0] is NSNumber
        })
      else { throw CocoaError(.fileReadCorruptFile) }
    }
    cursor.projection.consume(common)
    for occurrence in occurrences {
      cursor.projection.consume(common.merging(occurrence) { _, receipt in receipt })
    }
  }

  private struct StorageDecoder {
    var payload = Data()
    var id = ""
    var part = 0
    var parts = 0
    var length = 0
    var header: [String: Any] = [:]
    var incomplete: Bool { parts != 0 }

    mutating func consume(_ row: [String: Any]) throws -> [[String: Any]] {
      guard row["schema"] as? String == "codescribe.bus-chunk.v1" else {
        guard !incomplete else { throw CocoaError(.fileReadCorruptFile) }
        return [row]
      }
      guard let next = row["part"] as? Int, let count = row["parts"] as? Int,
        let bytes = row["length"] as? Int, bytes > 0, bytes <= 256 << 20,
        count == (bytes + 32767) / 32768, next >= 0, next < count,
        let identity = row["id"] as? String, let metadata = row["event"] as? [String: Any],
        let encoded = row["payload"] as? String, let block = Data(base64Encoded: encoded),
        block.count <= 32768
      else { throw CocoaError(.fileReadCorruptFile) }
      if next == 0 {
        guard !incomplete else { throw CocoaError(.fileReadCorruptFile) }
        id = identity
        part = 0
        parts = count
        length = bytes
        header = metadata
        payload = Data()
      }
      guard identity == id, next == part, count == parts, bytes == length,
        NSDictionary(dictionary: metadata).isEqual(to: header), payload.count + block.count <= bytes
      else { throw CocoaError(.fileReadCorruptFile) }
      payload.append(block)
      part += 1
      guard part == parts else { return [] }
      let digest = SHA256.hash(data: payload).map { String(format: "%02x", $0) }.joined()
      guard payload.count == length, digest == id,
        let document = try JSONSerialization.jsonObject(with: payload) as? [String: Any],
        header.allSatisfy({ key, value in
          let actual = document[key] ?? NSNull()
          return NSDictionary(dictionary: [key: actual]).isEqual(to: [key: value])
        })
      else { throw CocoaError(.fileReadCorruptFile) }
      self = StorageDecoder()
      return [document]
    }
  }

  /// The maintenance owner's transaction is the sole storage linkage. Logical
  /// offsets survive daily inode switches; unknown linkage throws before use.
  private final class GenerationStream {
    struct Segment {
      let url: URL
      let start: UInt64
      let length: UInt64
      let inode: UInt64
      let dev: UInt64
      let day: String?
    }
    let segments: [Segment]
    let inode: UInt64
    let size: UInt64
    let linked: Bool
    let coldStart: UInt64
    var offset: UInt64 = 0

    init(_ root: URL) throws {
      func metadata(_ url: URL) throws -> (UInt64, UInt64, UInt64) {
        let a = try FileManager.default.attributesOfItem(atPath: url.path)
        guard let ino = a[.systemFileNumber] as? NSNumber, let dev = a[.systemNumber] as? NSNumber,
          let length = a[.size] as? NSNumber, a[.type] as? FileAttributeType == .typeRegular
        else { throw CocoaError(.fileReadCorruptFile) }
        return (ino.uint64Value, dev.uint64Value, length.uint64Value)
      }
      let current = try metadata(root)
      let receipt = URL(fileURLWithPath: root.path + ".generations.json")
      if !FileManager.default.fileExists(atPath: receipt.path) {
        segments = [
          Segment(
            url: root, start: 0, length: current.2, inode: current.0, dev: current.1, day: nil)
        ]
        inode = current.0
        size = current.2
        linked = false
        coldStart =
          current.2 > OverlayChannelDeliveryReader.tailWindow
          ? current.2 - OverlayChannelDeliveryReader.tailWindow : 0
        return
      }
      let handle = try FileHandle(forReadingFrom: receipt)
      defer { try? handle.close() }
      let bytes = try handle.read(upToCount: (4 << 20) + 1) ?? Data()
      guard bytes.count <= 4 << 20,
        let manifest = try JSONSerialization.jsonObject(with: bytes) as? [String: Any],
        Set(manifest.keys)
          == Set([
            "schema", "root", "stream_id", "stream_inode", "stream_dev", "stream_birthtime",
            "segments", "active", "pending",
          ]),
        manifest["schema"] as? String == "codescribe.bus-generations.v1",
        manifest["root"] as? String == root.path,
        let stream = manifest["stream_inode"] as? NSNumber,
        var active = manifest["active"] as? [String: Any],
        var closed = manifest["segments"] as? [[String: Any]]
      else { throw CocoaError(.fileReadCorruptFile) }
      guard manifest["pending"] is NSNull || manifest["pending"] is [String: Any] else {
        throw CocoaError(.fileReadCorruptFile)
      }
      if let pending = manifest["pending"] as? [String: Any] {
        guard Set(pending.keys) == Set(["closed", "next"]),
          let next = pending["next"] as? [String: Any],
          let old = pending["closed"] as? [String: Any]
        else { throw CocoaError(.fileReadCorruptFile) }
        if (next["ino"] as? NSNumber)?.uint64Value == current.0
          && (next["dev"] as? NSNumber)?.uint64Value == current.1
        {
          closed.append(old)
          active = next
        } else if (active["ino"] as? NSNumber)?.uint64Value != current.0
          || (active["dev"] as? NSNumber)?.uint64Value != current.1
        {
          throw CocoaError(.fileReadCorruptFile)
        }
      }
      let events = root.deletingLastPathComponent().appendingPathComponent("events").path + "/"
      var expected: UInt64 = 0
      var inventory: [Segment] = []
      for entry in closed {
        guard
          Set(entry.keys)
            == Set([
              "id", "path", "start", "length", "dev", "ino", "day", "compressed", "sha256",
              "superseded",
            ]),
          let path = entry["path"] as? String, path.hasPrefix(events),
          URL(fileURLWithPath: path).standardizedFileURL.path == path,
          let start = entry["start"] as? NSNumber, let length = entry["length"] as? NSNumber,
          let ino = entry["ino"] as? NSNumber, let dev = entry["dev"] as? NSNumber,
          start.uint64Value == expected
        else { throw CocoaError(.fileReadCorruptFile) }
        let url = URL(fileURLWithPath: path)
        let actual = try metadata(url)
        guard actual.0 == ino.uint64Value, actual.1 == dev.uint64Value,
          actual.2 == length.uint64Value,
          expected <= UInt64.max - actual.2
        else { throw CocoaError(.fileReadCorruptFile) }
        inventory.append(
          Segment(
            url: url, start: expected, length: actual.2, inode: actual.0, dev: actual.1,
            day: entry["day"] as? String))
        expected += actual.2
      }
      guard (active["start"] as? NSNumber)?.uint64Value == expected,
        (active["ino"] as? NSNumber)?.uint64Value == current.0,
        (active["dev"] as? NSNumber)?.uint64Value == current.1, expected <= UInt64.max - current.2
      else { throw CocoaError(.fileReadCorruptFile) }
      inventory.append(
        Segment(
          url: root, start: expected, length: current.2, inode: current.0, dev: current.1,
          day: active["day"] as? String))
      segments = inventory
      inode = stream.uint64Value
      size = expected + current.2
      linked = true
      // Cold observation is the newest tail of the whole logical stream, for
      // dated chains and an undated prefix alike. Inventory metadata above is
      // the check; historical segment bytes stay unread.
      coldStart =
        size > OverlayChannelDeliveryReader.tailWindow
        ? size - OverlayChannelDeliveryReader.tailWindow : 0
    }

    func seek(toOffset position: UInt64) throws { offset = position }
    func read(_ count: Int) throws -> Data? {
      guard offset < size else { return nil }
      var out = Data()
      for segment in segments
      where offset >= segment.start && offset < segment.start + segment.length {
        let file = try FileHandle(forReadingFrom: segment.url)
        defer { try? file.close() }
        let attributes = try FileManager.default.attributesOfItem(atPath: segment.url.path)
        guard (attributes[.systemFileNumber] as? NSNumber)?.uint64Value == segment.inode,
          (attributes[.systemNumber] as? NSNumber)?.uint64Value == segment.dev,
          ((attributes[.size] as? NSNumber)?.uint64Value ?? 0) >= segment.length
        else { throw CocoaError(.fileReadCorruptFile) }
        try file.seek(toOffset: offset - segment.start)
        let width = min(count - out.count, Int(segment.start + segment.length - offset))
        let block = try file.read(upToCount: width) ?? Data()
        guard !block.isEmpty else { throw CocoaError(.fileReadCorruptFile) }
        offset += UInt64(block.count)
        out.append(block)
        if out.count == count { break }
      }
      return out
    }
    func headHash() throws -> String {
      guard let first = segments.first(where: { $0.length > 0 }) else { return "" }
      let file = try FileHandle(forReadingFrom: first.url)
      defer { try? file.close() }
      return try OverlayDeliveryCursorStore.headHash(of: file)
    }
  }

}
