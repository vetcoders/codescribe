import CryptoKit
import Darwin
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
  private let sharedBus: URL?
  private var buses: [URL: Cursor] = [:]
  private var persisted: [String: OverlayDeliveryCursorMark]?
  private var savedRevisions: [URL: UInt64] = [:]
  private var persistenceAttempts: [URL: PersistenceAttempt] = [:]
  // This is a restart cache, not transcript or delivery authority. Checkpoint
  // complete snapshots, rather than fsyncing the entire roster for each bus.
  private static let checkpointInterval: Duration = .seconds(30)
  private static let checkpointByteLag: UInt64 = 8 << 20
  private var lastCheckpoint: ContinuousClock.Instant?
  private(set) var checkpointWrites: UInt64 = 0
  private struct PersistenceAttempt: Equatable {
    let offset: UInt64
    let inode: UInt64
    let headHash: String
    let streamID: String?
    let revision: UInt64
    let headLength: Int
    let discardsLeadingParts: Bool
    let dropsCutRow: Bool
  }
  /// Total bus bytes this reader consumed from disk. Tests use it to prove a
  /// restart reads only new bytes instead of replaying history.
  private(set) var consumedBytes: UInt64 = 0
  /// Metadata bytes actually read, independent of the incremental bus cursor.
  private(set) var consumedMetadataBytes: UInt64 = 0
  /// Opened metadata descriptors, including reads satisfied by the parse cache.
  private(set) var metadataFileOpens: UInt64 = 0
  private static let metadataCacheBudget = 8 << 20
  private static let metadataCacheEntries = 256
  private var metadataObjects: [URL: MetadataObject] = [:]
  private var metadataCacheBytes = 0
  private var metadataAccess: UInt64 = 0
  /// Lease and binding documents stay out of the receipt cache. A 500 ms poll
  /// rotates through acknowledgment files and was evicting multi-megabyte
  /// lease parses, so the next pass rebuilt them from scratch.
  private var pinnedObjects: [URL: MetadataObject] = [:]
  private var pinnedCacheBytes = 0
  private static let pinnedCacheBudget = 8 << 20
  private static let pinnedCacheEntries = 24

  private struct MetadataObjectStamp: Equatable {
    let device: dev_t
    let inode: ino_t
    let size: off_t
    let modifiedSeconds: Int
    let modifiedNanoseconds: Int
    let changedSeconds: Int
    let changedNanoseconds: Int

    init(_ value: stat) {
      device = value.st_dev
      inode = value.st_ino
      size = value.st_size
      modifiedSeconds = value.st_mtimespec.tv_sec
      modifiedNanoseconds = value.st_mtimespec.tv_nsec
      changedSeconds = value.st_ctimespec.tv_sec
      changedNanoseconds = value.st_ctimespec.tv_nsec
    }
  }

  private struct MetadataObject {
    let stamp: MetadataObjectStamp
    let value: [String: Any]
    let bytes: Int
    var access: UInt64
  }

  private struct Cursor {
    var inode: UInt64 = 0
    var offset: UInt64 = 0
    var headHash = ""
    var headLength: Int = 0
    var streamID: String? = nil
    /// The tail window landed mid-row: drop everything through the first newline.
    var dropsCutRow = false
    var partial = Data()
    var projection = OverlayChannelDelivery.Bus()
    var storage = StorageDecoder()
    var storageStart: UInt64?
  }

  init(root: URL, sharedBus: URL? = nil) {
    self.root = root
    self.sharedBus = sharedBus?.standardizedFileURL
  }

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

  func read() throws -> [OverlayChannelDelivery] { try readSnapshot().deliveries }

  func readSnapshot(
    selectedOwner: OverlayConversationOwner? = nil,
    checkpointTime: ContinuousClock.Instant = .now
  ) throws
    -> OverlayChannelDeliverySnapshot
  {
    if persisted == nil { persisted = OverlayDeliveryCursorStore.load(root: root) }
    let bindingsURL = root.appendingPathComponent("vc.agent-audience-binding.v1.json")
    var bindings: [String: [String: Any]] = [:]
    if FileManager.default.fileExists(atPath: bindingsURL.path) {
      let file = try object(at: bindingsURL)
      guard file["schema"] as? String == "vc.agent-audience-binding.v1",
        let values = file["bindings"] as? [String: [String: Any]]
      else { throw CocoaError(.fileReadCorruptFile) }
      bindings = values
    }
    let archivesURL = root.appendingPathComponent("archives", isDirectory: true)
    var archives: [(owner: OverlayConversationOwner, bus: String)] = []
    if FileManager.default.fileExists(atPath: archivesURL.path) {
      for file in try FileManager.default.contentsOfDirectory(
        at: archivesURL, includingPropertiesForKeys: nil
      ).sorted(by: { $0.path < $1.path })
      where file.pathExtension == "json" {
        guard let handle = try? FileHandle(forReadingFrom: file) else { continue }
        defer { try? handle.close() }
        guard let data = try? handle.read(upToCount: 65537), data.count <= 65536,
          let row = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
          row["schema"] as? String == "codescribe.agent-archive.v1",
          row["released"] as? Bool == true, let owner = OverlayConversationOwner(row: row),
          owner.channel.count == 1, "123456789".contains(owner.channel),
          file.lastPathComponent == "\(owner.leaseID)-\(owner.channel).json",
          owner.leaseID
            == AgentPlaybackIdentity.leaseIdentifier(
              provider: owner.provider, session: owner.providerSessionID),
          let bus = row["bus"] as? String, bus.hasPrefix("/")
        else { continue }
        // Publication precedes binding release. A write failure or deliberate
        // same-session reattachment leaves this owner on the active list.
        let binding = bindings[owner.channel]
        if binding?["provider"] as? String == owner.provider,
          binding?["provider_session_id"] as? String == owner.providerSessionID
        {
          continue
        }
        archives.append((owner, bus))
      }
    }
    let leasesURL = root.appendingPathComponent("leases", isDirectory: true)
    var leases: [[String: Any]] = []
    if FileManager.default.fileExists(atPath: leasesURL.path) {
      let files = try FileManager.default.contentsOfDirectory(
        at: leasesURL, includingPropertiesForKeys: nil
      ).filter { $0.pathExtension == "json" }
      var selected: [URL] = []
      for binding in bindings.values {
        guard let provider = binding["provider"] as? String,
          let session = binding["provider_session_id"] as? String
        else { continue }
        let identity = SHA256.hash(
          data: Data([provider.lowercased(), session].joined(separator: "\0").utf8)
        )
        .prefix(16).map { String(format: "%02x", $0) }.joined()
        let file = leasesURL.appendingPathComponent(identity + ".json")
        if !selected.contains(file) { selected.append(file) }
      }
      for file in files.sorted(by: { $0.path < $1.path }).prefix(16) where !selected.contains(file)
      {
        selected.append(file)
      }
      for file in selected {
        guard let lease = try? object(at: file) else { continue }
        guard lease["schema"] as? String == "codescribe.agent-bridge.lease.v1",
          lease["lease_id"] as? String == file.deletingPathExtension().lastPathComponent
        else { continue }
        leases.append(lease)
      }
    }
    var paths: [String] = sharedBus.map { [$0.path] } ?? []
    for channel in bindings.keys.sorted() {
      guard let binding = bindings[channel],
        let lease = leases.first(where: {
          $0["provider"] as? String == binding["provider"] as? String
            && $0["provider_session_id"] as? String == binding["provider_session_id"] as? String
        }), let path = lease["bus"] as? String, path.hasPrefix("/"), !paths.contains(path)
      else { continue }
      paths.append(path)
    }
    for archive in archives where !paths.contains(archive.bus) { paths.append(archive.bus) }
    for lease in leases {
      if let path = lease["bus"] as? String, path.hasPrefix("/"), !paths.contains(path) {
        paths.append(path)
      }
    }
    // Closed channels retain their last verified projection even with no live follower.
    for path in (persisted ?? [:]).keys.sorted() where !paths.contains(path) && path.hasPrefix("/")
    {
      paths.append(path)
    }
    // Manual archive navigation takes one slot in the existing polling budget.
    // Resolve its path from validated metadata, never a reused channel number.
    var openedArchiveBus: URL?
    if let selectedOwner,
      let archive = archives.first(where: {
        $0.owner.id == selectedOwner.id && $0.owner.channel == selectedOwner.channel
      })
    {
      let bus = URL(fileURLWithPath: archive.bus).standardizedFileURL
      if buses[bus] == nil { openedArchiveBus = bus }
      paths.removeAll { $0 == archive.bus }
      paths.insert(archive.bus, at: 0)
    }
    let usedBuses = Set(paths.prefix(16).map { URL(fileURLWithPath: $0).standardizedFileURL })
    for bus in usedBuses.sorted(by: { $0.path < $1.path }) {
      if FileManager.default.fileExists(atPath: bus.path) {
        try refresh(bus)
      } else if buses[bus] == nil, let restored = persistedCursor(for: bus) {
        buses[bus] = restored
      }
    }
    var deliveries: [OverlayChannelDelivery] = []
    for channel in bindings.keys.sorted() where channel.count == 1 && "123456789".contains(channel)
    {
      guard let binding = bindings[channel] else { continue }
      let owners = leases.filter {
        $0["provider"] as? String == binding["provider"] as? String
          && $0["provider_session_id"] as? String == binding["provider_session_id"] as? String
      }
      guard owners.count == 1, let lease = owners.first,
        let path = lease["bus"] as? String
      else { continue }
      let bus = URL(fileURLWithPath: path).standardizedFileURL
      guard var cursor = buses[bus] else { continue }
      cursor.projection.observeLease(lease, channel: channel, binding: binding)
      if let status = cursor.projection.project(channel: channel, binding: binding, lease: lease) {
        deliveries.append(status)
      }
      buses[bus] = cursor
    }
    for bus in usedBuses {
      guard var cursor = buses[bus] else { continue }
      for (owner, delivery) in cursor.projection.receiptCoordinates() {
        guard delivery.count == 24, delivery.allSatisfy({ "0123456789abcdef".contains($0) }) else {
          continue
        }
        let wakeup = root.appendingPathComponent("wakeups").appendingPathComponent(owner.leaseID)
          .appendingPathComponent(delivery + ".json")
        if let receipt = try? object(at: wakeup) { cursor.projection.observeAcceptance(receipt) }
        let acknowledgment = root.appendingPathComponent("acknowledgments")
          .appendingPathComponent(owner.leaseID).appendingPathComponent(delivery + ".json")
        if let receipt = try? object(at: acknowledgment) {
          cursor.projection.observeAcknowledgment(
            receipt, owner: owner, delivery: delivery, busPath: bus.path)
        }
      }
      buses[bus] = cursor
    }
    for index in deliveries.indices {
      let channel = deliveries[index].channel
      guard let binding = bindings[channel],
        let lease = leases.first(where: {
          $0["provider"] as? String == binding["provider"] as? String
            && $0["provider_session_id"] as? String == binding["provider_session_id"] as? String
        }), let path = lease["bus"] as? String,
        let status = buses[URL(fileURLWithPath: path).standardizedFileURL]?.projection.project(
          channel: channel, binding: binding, lease: lease)
      else { continue }
      deliveries[index] = status
    }
    buses = buses.filter { usedBuses.contains($0.key) }
    persistenceAttempts = persistenceAttempts.filter { usedBuses.contains($0.key) }
    savedRevisions = savedRevisions.filter { usedBuses.contains($0.key) }
    var named: [String: OverlayConversation] = [:]
    var all: [String: OverlayConversationMessage] = [:]
    var historyReplyIDs: Set<String> = []
    for bus in buses.keys.sorted(by: { $0.path < $1.path }) {
      guard let cursor = buses[bus] else { continue }
      for conversation in cursor.projection.conversations(busPath: bus.path) {
        if bus == openedArchiveBus { historyReplyIDs.formUnion(conversation.replyIDs) }
        if conversation.id == "0" {
          for message in conversation.messages {
            all[message.id] = all[message.id].map { Self.merge($0, message) } ?? message
          }
        } else if let existing = named[conversation.id] {
          var rows = Dictionary(uniqueKeysWithValues: existing.messages.map { ($0.id, $0) })
          for message in conversation.messages {
            rows[message.id] = rows[message.id].map { Self.merge($0, message) } ?? message
          }
          named[conversation.id] = OverlayConversation(
            id: existing.id, channel: existing.channel,
            name: existing.name, owner: existing.owner,
            messages: rows.values.sorted {
              if $0.emittedAt != $1.emittedAt { return $0.emittedAt < $1.emittedAt }
              return $0.order < $1.order
            })
        } else {
          named[conversation.id] = conversation
        }
      }
    }
    let ordered = all.values.sorted {
      if $0.emittedAt != $1.emittedAt { return $0.emittedAt < $1.emittedAt }
      if $0.order != $1.order { return $0.order < $1.order }
      return $0.id < $1.id
    }
    let broadcast = OverlayConversation(
      id: "0", channel: "0", name: "All", owner: nil,
      messages: Array(ordered.suffix(256)))
    for archive in archives where named[archive.owner.id] == nil {
      let owner = archive.owner
      named[owner.id] = OverlayConversation(
        id: owner.id, channel: owner.channel, name: owner.name, owner: owner, messages: [],
        historyLoaded: buses[URL(fileURLWithPath: archive.bus).standardizedFileURL] != nil)
    }
    try checkpoint(at: checkpointTime)
    return OverlayChannelDeliverySnapshot(
      deliveries: deliveries,
      conversations: [broadcast]
        + named.values.sorted {
          if $0.channel != $1.channel { return $0.channel < $1.channel }
          return $0.id < $1.id
        }, archivedOwners: Set(archives.map(\.owner)), historyReplyIDs: historyReplyIDs)
  }

  private static func merge(
    _ first: OverlayConversationMessage, _ second: OverlayConversationMessage
  )
    -> OverlayConversationMessage
  {
    // Capture opening time stays constant while the source document evolves.
    // Delayed mirrors must follow its revision, never replace it by path order.
    let firstIsNewer: Bool
    if let firstRevision = first.sourceRevision, let secondRevision = second.sourceRevision,
      firstRevision != secondRevision
    {
      firstIsNewer = firstRevision > secondRevision
    } else {
      firstIsNewer = first.emittedAt > second.emittedAt
    }
    var result = firstIsNewer ? first : second
    let other = firstIsNewer ? second : first
    result.supportsSpeechPlayback = first.supportsSpeechPlayback || second.supportsSpeechPlayback
    if let otherOccurrences = other.occurrenceIDs {
      var occurrences = result.occurrenceIDs ?? []
      for occurrence in otherOccurrences where !occurrences.contains(occurrence) {
        occurrences.append(occurrence)
      }
      result.occurrenceIDs = occurrences
    }
    for recipient in other.recipients {
      if let index = result.recipients.firstIndex(where: { $0.owner.id == recipient.owner.id }) {
        if result.recipients[index].deliveryID == recipient.deliveryID {
          result.recipients[index].queued = result.recipients[index].queued || recipient.queued
          result.recipients[index].accepted =
            result.recipients[index].accepted || recipient.accepted
          result.recipients[index].acknowledged =
            result.recipients[index].acknowledged || recipient.acknowledged
        }
      } else {
        result.recipients.append(recipient)
      }
    }
    if let playback = other.playback,
      result.playback == nil || (result.playback?.emittedAt ?? "") < playback.emittedAt
    {
      result.playback = playback
    }
    return result
  }

  private func pinsMetadata(_ url: URL) -> Bool {
    if url.lastPathComponent == "vc.agent-audience-binding.v1.json" { return true }
    return url.deletingLastPathComponent().lastPathComponent == "leases"
  }

  private func object(at url: URL) throws -> [String: Any] {
    let pinned = pinsMetadata(url)
    do {
      var metadata = stat()
      guard fstatat(AT_FDCWD, url.path, &metadata, 0) == 0 else {
        throw CocoaError(.fileReadUnknown)
      }
      let pathStamp = MetadataObjectStamp(metadata)
      metadataAccess &+= 1
      if pinned {
        if var cached = pinnedObjects[url], cached.stamp == pathStamp {
          cached.access = metadataAccess
          pinnedObjects[url] = cached
          return cached.value
        }
      } else if var cached = metadataObjects[url], cached.stamp == pathStamp {
        cached.access = metadataAccess
        metadataObjects[url] = cached
        return cached.value
      }
      metadataFileOpens &+= 1
      let handle = try FileHandle(forReadingFrom: url)
      defer { try? handle.close() }
      guard fstat(handle.fileDescriptor, &metadata) == 0 else {
        throw CocoaError(.fileReadUnknown)
      }
      let stamp = MetadataObjectStamp(metadata)
      if pinned {
        if let old = pinnedObjects.removeValue(forKey: url) { pinnedCacheBytes -= old.bytes }
      } else if let old = metadataObjects.removeValue(forKey: url) {
        metadataCacheBytes -= old.bytes
      }
      let data = try handle.read(upToCount: (16 << 20) + 1) ?? Data()
      consumedMetadataBytes &+= UInt64(data.count)
      guard data.count <= 16 << 20,
        let value = try JSONSerialization.jsonObject(with: data) as? [String: Any]
      else { throw CocoaError(.fileReadCorruptFile) }
      // The open descriptor observes atomic replacement and follows symlinks.
      // In-place writes also invalidate through nanosecond mtime/ctime. Never
      // retain a parse whose file changed during that read.
      if fstat(handle.fileDescriptor, &metadata) == 0, MetadataObjectStamp(metadata) == stamp {
        if pinned, data.count <= Self.pinnedCacheBudget {
          while pinnedCacheBytes + data.count > Self.pinnedCacheBudget
            || pinnedObjects.count >= Self.pinnedCacheEntries
          {
            guard let oldest = pinnedObjects.min(by: { $0.value.access < $1.value.access })?.key,
              let removed = pinnedObjects.removeValue(forKey: oldest)
            else { break }
            pinnedCacheBytes -= removed.bytes
          }
          pinnedObjects[url] = MetadataObject(
            stamp: stamp, value: value, bytes: data.count, access: metadataAccess)
          pinnedCacheBytes += data.count
        } else if !pinned, data.count <= Self.metadataCacheBudget {
          while metadataCacheBytes + data.count > Self.metadataCacheBudget
            || metadataObjects.count >= Self.metadataCacheEntries
          {
            guard let oldest = metadataObjects.min(by: { $0.value.access < $1.value.access })?.key,
              let removed = metadataObjects.removeValue(forKey: oldest)
            else { break }
            metadataCacheBytes -= removed.bytes
          }
          metadataObjects[url] = MetadataObject(
            stamp: stamp, value: value, bytes: data.count, access: metadataAccess)
          metadataCacheBytes += data.count
        }
      }
      return value
    } catch {
      if pinned {
        if let removed = pinnedObjects.removeValue(forKey: url) {
          pinnedCacheBytes -= removed.bytes
        }
      } else if let removed = metadataObjects.removeValue(forKey: url) {
        metadataCacheBytes -= removed.bytes
      }
      throw error
    }
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
    } else if let persisted = persistedCursor(for: url) {
      cursor = persisted
    } else {
      cursor = Cursor()
    }
    if cursor.headLength == 0 && size > 0 && cursor.offset == 0 { cursor = Cursor() }
    if !cursor.headHash.isEmpty {
      let stale = !file.linked && size >= cursor.offset && size - cursor.offset > Self.tailWindow
      let headHash = try file.headHash(length: cursor.headLength)
      let adoptsLinkedStream =
        file.linked && cursor.streamID == file.originalFileStreamID
        && cursor.inode == inode && headHash == cursor.headHash
      if cursor.inode != inode || (cursor.streamID != file.streamID && !adoptsLinkedStream)
        || size < cursor.offset
        || stale || headHash != cursor.headHash
      {
        // Unlinked rotation, truncation, or a gap larger than the window resets
        // to a fresh tail. A linked cursor keeps its logical offset: the stream
        // inode does not change when the hot file rolls over.
        cursor = Cursor()
      } else if adoptsLinkedStream {
        cursor.streamID = file.streamID
      }
    }
    if cursor.headHash.isEmpty {
      let start = file.coldStart
      cursor.inode = inode
      cursor.headLength = file.headLength
      cursor.streamID = file.streamID
      cursor.headHash = try file.headHash(length: cursor.headLength)
      cursor.offset = start
      if start > 0 {
        cursor.storage.discardsLeadingParts = true
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
        guard cursor.partial.count <= 1 << 20 else { throw CocoaError(.fileReadCorruptFile) }
      }
    }
    buses[url] = cursor
  }

  private func persistedCursor(for url: URL) -> Cursor? {
    if persisted == nil { persisted = OverlayDeliveryCursorStore.load(root: root) }
    guard let mark = persisted?[url.path], let data = mark.projection,
      let projection = try? JSONDecoder().decode(OverlayChannelDelivery.Bus.self, from: data)
    else { return nil }
    guard (0...256).contains(mark.headLength) else { return nil }
    savedRevisions[url] = projection.revision
    var cursor = Cursor(
      inode: mark.inode, offset: mark.offset, headHash: mark.headHash,
      headLength: mark.headLength, streamID: mark.streamID, projection: projection)
    cursor.storage.discardsLeadingParts = mark.discardsLeadingParts
    cursor.dropsCutRow = mark.dropsCutRow
    return cursor
  }

  private func checkpoint(at time: ContinuousClock.Instant) throws {
    var changed: [(URL, Cursor, PersistenceAttempt)] = []
    var largeLag = false
    for url in buses.keys.sorted(by: { $0.path < $1.path }) {
      guard let cursor = buses[url] else { continue }
      let offset = cursor.storageStart ?? (cursor.offset - UInt64(cursor.partial.count))
      let attempt = PersistenceAttempt(
        offset: offset, inode: cursor.inode,
        headHash: cursor.headHash, streamID: cursor.streamID, revision: cursor.projection.revision,
        headLength: cursor.headLength, discardsLeadingParts: cursor.storage.discardsLeadingParts,
        dropsCutRow: cursor.dropsCutRow)
      if persistenceAttempts[url] == attempt { continue }
      let previous = persisted?[url.path]
      if let previous, savedRevisions[url] == cursor.projection.revision,
        previous.offset == offset, previous.inode == cursor.inode,
        previous.headHash == cursor.headHash, previous.streamID == cursor.streamID,
        previous.headLength == cursor.headLength,
        previous.discardsLeadingParts == cursor.storage.discardsLeadingParts,
        previous.dropsCutRow == cursor.dropsCutRow
      {
        continue
      }
      let savedOffset = persistenceAttempts[url]?.offset ?? previous?.offset ?? 0
      if offset >= savedOffset, offset - savedOffset >= Self.checkpointByteLag {
        largeLag = true
      }
      changed.append((url, cursor, attempt))
    }
    guard !changed.isEmpty else { return }
    if let lastCheckpoint, !largeLag,
      lastCheckpoint.duration(to: time) < Self.checkpointInterval
    {
      return
    }

    var marks = persisted ?? [:]
    let encoder = JSONEncoder()
    encoder.outputFormatting = [.sortedKeys]
    for (url, cursor, attempt) in changed {
      // Offset and projection are one atomic unit. Never checkpoint past an
      // unfinished row or storage transaction, even though its bytes were read.
      marks[url.path] = OverlayDeliveryCursorMark(
        offset: attempt.offset,
        inode: cursor.inode, length: cursor.offset, headHash: cursor.headHash,
        projection: try encoder.encode(cursor.projection), headLength: cursor.headLength,
        streamID: cursor.streamID, discardsLeadingParts: cursor.storage.discardsLeadingParts,
        dropsCutRow: cursor.dropsCutRow)
    }
    if marks.count > 16 {
      for path in marks.keys.sorted()
      where buses[URL(fileURLWithPath: path)] == nil {
        marks.removeValue(forKey: path)
        if marks.count <= 16 { break }
      }
    }
    persisted = try OverlayDeliveryCursorStore.save(root: root, marks: marks)
    lastCheckpoint = time
    checkpointWrites &+= 1
    // Save failure leaves all fingerprints dirty for retry. Successful eviction
    // still records an attempt, so an oversized unit cannot spin on idle polls.
    for (url, cursor, attempt) in changed {
      persistenceAttempts[url] = attempt
      if persisted?[url.path] != nil { savedRevisions[url] = cursor.projection.revision }
    }
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
    var discardsLeadingParts = false
    var incomplete: Bool { parts != 0 }

    mutating func consume(_ row: [String: Any]) throws -> [[String: Any]] {
      guard row["schema"] as? String == "codescribe.bus-chunk.v1" else {
        guard !incomplete else { throw CocoaError(.fileReadCorruptFile) }
        discardsLeadingParts = false
        return [row]
      }
      guard let next = row["part"] as? Int, let count = row["parts"] as? Int,
        let bytes = row["length"] as? Int, bytes > 0, bytes <= 256 << 20,
        count == (bytes + 32767) / 32768, next >= 0, next < count,
        let identity = row["id"] as? String, let metadata = row["event"] as? [String: Any],
        let encoded = row["payload"] as? String, let block = Data(base64Encoded: encoded),
        block.count <= 32768
      else { throw CocoaError(.fileReadCorruptFile) }
      if next > 0 && !incomplete && discardsLeadingParts { return [] }
      if next == 0 {
        discardsLeadingParts = false
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
    let streamID: String
    let originalFileStreamID: String
    var headLength: Int { Int(min(256, segments.first(where: { $0.length > 0 })?.length ?? 0)) }
    let coldStart: UInt64
    var offset: UInt64 = 0

    init(_ root: URL) throws {
      func metadata(_ url: URL) throws -> (UInt64, UInt64, UInt64) {
        var value = stat()
        guard fstatat(AT_FDCWD, url.path, &value, 0) == 0 else {
          throw CocoaError(.fileReadUnknown)
        }
        guard value.st_mode & S_IFMT == S_IFREG, value.st_size >= 0 else {
          throw CocoaError(.fileReadCorruptFile)
        }
        return (UInt64(value.st_ino), UInt64(value.st_dev), UInt64(value.st_size))
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
        streamID = "\(current.1):\(current.0)"
        originalFileStreamID = streamID
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
        Set(manifest.keys).subtracting(["volume_uuid"])
          == Set([
            "schema", "root", "stream_id", "stream_inode", "stream_dev", "stream_birthtime",
            "segments", "active", "pending",
          ]),
        (manifest["volume_uuid"] == nil || manifest["volume_uuid"] is NSNull
          || (manifest["volume_uuid"] as? String).flatMap { UUID(uuidString: $0) } != nil),
        manifest["schema"] as? String == "codescribe.bus-generations.v1",
        manifest["root"] as? String == root.path,
        let stream = manifest["stream_inode"] as? NSNumber,
        let streamDevice = manifest["stream_dev"] as? NSNumber,
        let identity = manifest["stream_id"] as? String, !identity.isEmpty,
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
      originalFileStreamID = "\(streamDevice.uint64Value):\(stream.uint64Value)"
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
      streamID = identity
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
    func headHash(length: Int) throws -> String {
      guard let first = segments.first(where: { $0.length > 0 }) else { return "" }
      let file = try FileHandle(forReadingFrom: first.url)
      defer { try? file.close() }
      return try OverlayDeliveryCursorStore.headHash(of: file, length: length)
    }
  }

}
