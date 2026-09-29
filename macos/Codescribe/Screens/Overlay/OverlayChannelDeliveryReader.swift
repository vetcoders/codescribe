import Foundation

/// File I/O stays off the main actor. Tails complete JSONL rows, including after
/// rotation/truncation; partial writes wait for the following refresh.
actor OverlayChannelDeliveryReader {
  private let root: URL
  private var buses: [URL: Cursor] = [:]

  private struct Cursor {
    var inode: UInt64 = 0
    var offset: UInt64 = 0
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
    var cursor = buses[url] ?? Cursor()
    if cursor.inode != inode || size < cursor.offset { cursor = Cursor(inode: inode) }
    let file = try FileHandle(forReadingFrom: url)
    defer { try? file.close() }
    try file.seek(toOffset: cursor.offset)
    while let chunk = try file.read(upToCount: 262_144), !chunk.isEmpty {
      cursor.offset += UInt64(chunk.count)
      cursor.partial.append(chunk)
      while let newline = cursor.partial.firstIndex(of: 10) {
        let line = cursor.partial[..<newline]
        if !line.isEmpty {
          guard let row = try JSONSerialization.jsonObject(with: Data(line)) as? [String: Any]
          else {
            throw CocoaError(.fileReadCorruptFile)
          }
          cursor.projection.consume(row)
        }
        cursor.partial.removeSubrange(...newline)
      }
    }
    buses[url] = cursor
  }
}
