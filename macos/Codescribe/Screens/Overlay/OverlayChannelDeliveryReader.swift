import Foundation
import OSLog

/// File I/O stays off the main actor. Tails complete JSONL rows, including after
/// rotation/truncation; partial writes wait for the following refresh.
///
/// The shared bus grows without bound (the Founder's bus passed 21 GB, and reading
/// it from byte zero once took 3m40s at a 63.7 GB memory peak). A cold reader
/// therefore never starts at byte zero: the first read of a bus begins
/// `Self.tailWindow` before EOF and drops the row the window cut in half.
/// Projection semantics are unchanged — the latest channel session, seal and ack
/// rows live in the tail, and an orphan or seal older than the window is
/// invisible, the same contract the Rust readers already use
/// (`ORPHAN_SCAN_WINDOW_BYTES` in app/controller/agent_channel.rs, the fold
/// budget in app/presentation/agent_ack.rs). A durable cursor
/// (`OverlayDeliveryCursorStore`) carries the position across restarts: a valid
/// cursor resumes where the previous generation stopped, while a rotated,
/// truncated or stale bus resets to the tail window — never to zero.
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
    let attributes = try FileManager.default.attributesOfItem(atPath: url.path)
    let inode = (attributes[.systemFileNumber] as? NSNumber)?.uint64Value ?? 0
    let size = (attributes[.size] as? NSNumber)?.uint64Value ?? 0
    var cursor = buses[url] ?? persistedCursor(for: url) ?? Cursor()
    let file = try FileHandle(forReadingFrom: url)
    defer { try? file.close() }
    if cursor.offset > 0 {
      let stale = size >= cursor.offset && size - cursor.offset > Self.tailWindow
      let headHash = try OverlayDeliveryCursorStore.headHash(of: file)
      if cursor.inode != inode || size < cursor.offset || stale || headHash != cursor.headHash {
        // Rotated, truncated, or left behind by more than the window: replaying
        // history is exactly the cost this reader exists to avoid, so reset to
        // the tail window of the current file with a fresh projection.
        cursor = Cursor()
      }
    }
    if cursor.headHash.isEmpty {
      let start = size > Self.tailWindow ? size - Self.tailWindow : 0
      cursor.inode = inode
      cursor.headHash = try OverlayDeliveryCursorStore.headHash(of: file)
      cursor.offset = start
      if start > 0 {
        // Skip only the row the window cut in half: when the byte before the
        // window is a newline, the window begins exactly on a row boundary.
        try file.seek(toOffset: start - 1)
        cursor.dropsCutRow = try file.read(upToCount: 1)?.first != 10
      }
    }
    try file.seek(toOffset: cursor.offset)
    while let chunk = try file.read(upToCount: Self.chunkSize), !chunk.isEmpty {
      autoreleasepool {
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
            cursor.projection.consume(row)
          } else {
            // One corrupt row is skipped with a log and the cursor advances
            // past it; it can never fail or rewind the read. Only corrupted
            // bindings/lease files throw, in read().
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
      offset: cursor.offset - UInt64(cursor.partial.count),
      inode: cursor.inode, length: size, headHash: cursor.headHash)
    guard marks[url.path] != mark else { return }
    marks[url.path] = mark
    try OverlayDeliveryCursorStore.save(root: root, marks: marks)
    persisted = marks
  }
}
