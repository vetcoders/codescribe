import CryptoKit
import Darwin
import Foundation

/// Session playback control identity; agent names and channel numbers are labels.
struct AgentPlaybackIdentity: Hashable, Sendable {
  let provider: String
  let session: String
  let bus: String

  init(provider: String, session: String, bus: String) {
    self.provider = provider
    self.session = session
    self.bus = URL(fileURLWithPath: bus).resolvingSymlinksInPath().path
  }

  var leaseID: String { Self.leaseIdentifier(provider: provider, session: session) }
  static func leaseIdentifier(provider: String, session: String) -> String {
    Self.digest([provider.lowercased(), session], bytes: 16)
  }
  var storageKey: String { Self.digest([provider, session, bus], bytes: 12) }

  private static func digest(_ parts: [String], bytes: Int) -> String {
    SHA256.hash(data: Data(parts.joined(separator: "\0").utf8))
      .prefix(bytes).map { String(format: "%02x", $0) }.joined()
  }
}

private struct AgentPlaybackMuteReceipt: Decodable, Sendable {
  let schema: String
  let provider: String
  let provider_session_id: String
  let lease_id: String
  let bus: String
  let muted: Bool

  func belongs(to identity: AgentPlaybackIdentity) -> Bool {
    schema == "codescribe.agent-playback-mute.v1" && provider == identity.provider
      && provider_session_id == identity.session && lease_id == identity.leaseID
      && bus == identity.bus
  }
}

/// App-side invocations of the installed managed `cs-bus` command. They live
/// apart from `AgentBridgeInstaller.swift` because they reference app types,
/// and that file is also compiled on its own by `make install-bus`.
extension RealAgentBridgeInstaller {
  /// Archive one dead, frozen owner through the binding file's only writer.
  @MainActor
  static func archiveBusAgent(
    owner: OverlayConversationOwner,
    installer: RealAgentBridgeInstaller = RealAgentBridgeInstaller()
  ) async throws {
    let executable = installer.commandURL("cs-bus")
    guard installer.fileManager.isExecutableFile(atPath: executable.path),
      installer.managedCommandID(executable) != nil
    else { throw CocoaError(.fileNoSuchFile) }
    let root = installer.bridgeRoot
    try await Task.detached(priority: .userInitiated) {
      let leaseURL = root.appendingPathComponent("leases/\(owner.leaseID).json")
      let leaseData = try Data(contentsOf: leaseURL)
      guard leaseData.count <= 16 << 20,
        let lease = try JSONSerialization.jsonObject(with: leaseData) as? [String: Any],
        lease["schema"] as? String == "codescribe.agent-bridge.lease.v1",
        lease["provider"] as? String == owner.provider,
        lease["provider_session_id"] as? String == owner.providerSessionID,
        lease["lease_id"] as? String == owner.leaseID,
        let bus = lease["bus"] as? String, bus.hasPrefix("/")
      else { throw CocoaError(.fileReadCorruptFile) }
      let process = Process()
      let output = Pipe()
      process.executableURL = executable
      process.arguments = [
        "--archive-agent", owner.channel, "--provider", owner.provider,
        "--session", owner.providerSessionID, "--lease", owner.leaseID,
        "--bus", bus, "--bridge-home", root.path,
      ]
      process.standardInput = FileHandle.nullDevice
      process.standardOutput = output
      process.standardError = FileHandle.nullDevice
      try process.run()
      defer { if process.isRunning { process.terminate() } }
      let deadline = ContinuousClock.now.advanced(by: .seconds(15))
      while process.isRunning, ContinuousClock.now < deadline {
        try await Task.sleep(for: .milliseconds(50))
      }
      guard !process.isRunning, process.terminationReason == .exit, process.terminationStatus == 0,
        let data = try output.fileHandleForReading.read(upToCount: 65537), data.count <= 65536,
        let receipt = try JSONSerialization.jsonObject(with: data) as? [String: Any],
        receipt["schema"] as? String == "codescribe.agent-archive.v1",
        receipt["released"] as? Bool == true,
        let archivedOwner = OverlayConversationOwner(row: receipt),
        archivedOwner.id == owner.id, archivedOwner.channel == owner.channel,
        receipt["bus"] as? String == bus
      else { throw CocoaError(.fileWriteUnknown) }
    }.value
  }

  /// Resolve each live roster session to its actual leased bus, including custom buses.
  @MainActor
  static func boundPlaybackIdentities(
    for candidates: [String: AgentPlaybackIdentity],
    installer: RealAgentBridgeInstaller = RealAgentBridgeInstaller()
  ) async -> [String: AgentPlaybackIdentity] {
    let root = installer.bridgeRoot
    return await Task.detached(priority: .utility) {
      var result: [String: AgentPlaybackIdentity] = [:]
      for (channel, candidate) in candidates {
        let path = root.appendingPathComponent("leases/\(candidate.leaseID).json")
        guard let handle = try? FileHandle(forReadingFrom: path) else { continue }
        defer { try? handle.close() }
        guard let data = try? handle.read(upToCount: (16 << 20) + 1), data.count <= (16 << 20),
          let row = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
          row["schema"] as? String == "codescribe.agent-bridge.lease.v1",
          row["lease_id"] as? String == candidate.leaseID,
          row["provider"] as? String == candidate.provider,
          row["provider_session_id"] as? String == candidate.session,
          let bus = row["bus"] as? String, bus.hasPrefix("/")
        else { continue }
        result[channel] = AgentPlaybackIdentity(
          provider: candidate.provider, session: candidate.session, bus: bus)
      }
      return result
    }.value
  }

  /// Read the playback owner's atomically published receipts off the UI thread.
  /// Missing means audible; malformed or unreadable remains unknown in the UI.
  @MainActor
  static func playbackMuteSnapshot(
    for identities: Set<AgentPlaybackIdentity>,
    installer: RealAgentBridgeInstaller = RealAgentBridgeInstaller()
  ) async -> [AgentPlaybackIdentity: Bool] {
    let root = installer.bridgeRoot
    return await Task.detached(priority: .utility) {
      var result: [AgentPlaybackIdentity: Bool] = [:]
      for identity in identities {
        let path = root.appendingPathComponent("runtime/playback-mutes/\(identity.storageKey).json")
        do {
          let handle = try FileHandle(forReadingFrom: path)
          defer { try? handle.close() }
          let data = try handle.read(upToCount: 65537) ?? Data()
          guard data.count <= 65536,
            let row = try? JSONDecoder().decode(AgentPlaybackMuteReceipt.self, from: data),
            row.belongs(to: identity)
          else { continue }
          result[identity] = row.muted
        } catch let error as NSError {
          if error.domain == NSCocoaErrorDomain && error.code == NSFileReadNoSuchFileError {
            result[identity] = false
          }
        }
      }
      return result
    }.value
  }

  @MainActor
  static func setPlaybackMuted(_ muted: Bool, for identity: AgentPlaybackIdentity) async throws {
    let installer = RealAgentBridgeInstaller()
    let executable = installer.commandURL("cs-bus")
    guard installer.fileManager.isExecutableFile(atPath: executable.path),
      installer.managedCommandID(executable) != nil
    else {
      throw NSError(
        domain: "Codescribe.BusPlayback", code: 1,
        userInfo: [
          NSLocalizedDescriptionKey: String(
            localized: "Install the agent bridge to control playback.")
        ])
    }
    let arguments = [
      muted ? "--mute-agent" : "--unmute-agent", "--provider", identity.provider,
      "--session", identity.session, "--bus", identity.bus,
      "--bridge-home", installer.bridgeRoot.path,
    ]
    var environment = ProcessInfo.processInfo.environment
    let commandDirectories = [
      installer.homeDirectory.appendingPathComponent(".cargo/bin").path,
      installer.homeDirectory.appendingPathComponent(".local/bin").path,
      "/opt/homebrew/bin", "/usr/local/bin", "/usr/bin", "/bin",
    ]
    environment["PATH"] = (commandDirectories + [environment["PATH"] ?? ""]).joined(separator: ":")
    let commandEnvironment = environment
    try await Task.detached(priority: .userInitiated) {
      let process = Process()
      process.executableURL = executable
      process.arguments = arguments
      process.environment = commandEnvironment
      process.standardInput = FileHandle.nullDevice
      process.standardOutput = FileHandle.nullDevice
      process.standardError = FileHandle.nullDevice
      try process.run()
      defer { if process.isRunning { process.terminate() } }
      let deadline = Date().addingTimeInterval(15)
      while process.isRunning, Date() < deadline {
        try await Task.sleep(for: .milliseconds(50))
      }
      if process.isRunning { process.terminate() }
      guard !process.isRunning, process.terminationReason == .exit,
        process.terminationStatus == 0
      else {
        throw NSError(
          domain: "Codescribe.BusPlayback", code: 2,
          userInfo: [
            NSLocalizedDescriptionKey: String(
              localized: "Agent playback preference could not be saved.")
          ])
      }
    }.value
  }

  /// Uses the same bus predicate and turn lease as the idle-safe installer.
  /// Retaining the returned exclusive lease prevents a new agent turn until
  /// the restarting process exits. An unreadable bus refuses the restart.
  static func acquireIdleLanguageRestartLease(
    installer: RealAgentBridgeInstaller = RealAgentBridgeInstaller()
  ) async throws -> FileHandle {
    let executable = installer.commandURL("cs-bus")
    guard installer.fileManager.isExecutableFile(atPath: executable.path),
      installer.managedCommandID(executable) != nil
    else { throw InterfaceLanguageRestartError.unavailable }
    return try await Task.detached(priority: .userInitiated) {
      let pathData = try languageRestartHelper(executable, flag: "--print-agent-turn-lease-path")
      guard
        let path = String(data: pathData, encoding: .utf8)?.trimmingCharacters(
          in: .whitespacesAndNewlines),
        path.hasPrefix("/")
      else { throw InterfaceLanguageRestartError.unavailable }
      let descriptor = Darwin.open(path, O_RDWR | O_CREAT | O_CLOEXEC, 0o600)
      guard descriptor >= 0 else { throw InterfaceLanguageRestartError.unavailable }
      let lease = FileHandle(fileDescriptor: descriptor, closeOnDealloc: true)
      guard flock(descriptor, LOCK_EX | LOCK_NB) == 0 else {
        try? lease.close()
        throw InterfaceLanguageRestartError.busy
      }
      do {
        _ = try languageRestartHelper(executable, flag: "--assert-install-idle")
        return lease
      } catch {
        try? lease.close()
        throw error
      }
    }.value
  }

  private static func languageRestartHelper(_ executable: URL, flag: String) throws -> Data {
    let process = Process()
    let output = Pipe()
    process.executableURL = executable
    process.arguments = [flag]
    process.standardInput = FileHandle.nullDevice
    process.standardOutput = output
    process.standardError = FileHandle.nullDevice
    try process.run()
    let deadline = Date().addingTimeInterval(15)
    while process.isRunning, Date() < deadline { Thread.sleep(forTimeInterval: 0.05) }
    guard !process.isRunning else {
      process.terminate()
      throw InterfaceLanguageRestartError.unavailable
    }
    guard process.terminationReason == .exit, process.terminationStatus == 0 else {
      throw InterfaceLanguageRestartError.busy
    }
    return output.fileHandleForReading.readDataToEndOfFile()
  }

  /// Written user messages share the installed canonical bus publisher and follower.
  @MainActor
  static func sendBusText(owner: OverlayConversationOwner, text: String) async throws {
    let installer = RealAgentBridgeInstaller()
    let executable = installer.commandURL("cs-bus")
    guard installer.fileManager.isExecutableFile(atPath: executable.path),
      installer.managedCommandID(executable) != nil
    else {
      throw NSError(
        domain: "Codescribe.BusText", code: 1,
        userInfo: [
          NSLocalizedDescriptionKey: String(localized: "Install the agent bridge to send messages.")
        ])
    }
    let root = installer.bridgeRoot
    try await Task.detached(priority: .userInitiated) {
      let lease = root.appendingPathComponent("leases/\(owner.leaseID).json")
      guard
        let receipt = try JSONSerialization.jsonObject(with: Data(contentsOf: lease))
          as? [String: Any],
        receipt["provider"] as? String == owner.provider,
        receipt["provider_session_id"] as? String == owner.providerSessionID,
        receipt["lease_id"] as? String == owner.leaseID,
        let bus = receipt["bus"] as? String, bus.hasPrefix("/")
      else {
        throw CocoaError(.fileReadCorruptFile)
      }
      let process = Process()
      let input = Pipe()
      process.executableURL = executable
      process.arguments = [
        "--send-text", "--channel", owner.channel, "--provider", owner.provider,
        "--session", owner.providerSessionID, "--lease", owner.leaseID, "--bus", bus,
        "--bridge-home", root.path,
      ]
      process.standardInput = input
      process.standardOutput = FileHandle.nullDevice
      process.standardError = FileHandle.nullDevice
      // Bound text fits in the pipe; writing happens off the UI thread.
      guard text.utf8.count <= 65536 else { throw CocoaError(.fileWriteOutOfSpace) }
      try process.run()
      try input.fileHandleForWriting.write(contentsOf: Data(text.utf8))
      try input.fileHandleForWriting.close()
      process.waitUntilExit()
      guard process.terminationReason == .exit, process.terminationStatus == 0 else {
        throw NSError(
          domain: "Codescribe.BusText", code: Int(process.terminationStatus),
          userInfo: [
            NSLocalizedDescriptionKey: String(
              localized: "Message could not be sent. Your draft is retained.")
          ])
      }
    }.value
  }

  /// Uses the installed bus speech owner; the built-in chat player is separate.
  @MainActor
  static func controlBusReply(
    replyID: String, ticket: String, provider: String, session: String,
    busPath: String, stop: Bool
  ) async throws {
    let installer = RealAgentBridgeInstaller()
    let executable = installer.commandURL("cs-bus")
    guard installer.fileManager.isExecutableFile(atPath: executable.path),
      installer.managedCommandID(executable) != nil
    else {
      throw NSError(
        domain: "Codescribe.BusPlayback", code: 1,
        userInfo: [
          NSLocalizedDescriptionKey: String(localized: "Install the agent bridge to play replies.")
        ])
    }
    let arguments = [
      stop ? "--stop-reply" : "--play-reply", replyID,
      "--playback-ticket", ticket, "--provider", provider, "--session", session,
      "--bus", busPath,
    ]
    let executablePath = executable.path
    var environment = ProcessInfo.processInfo.environment
    let commandDirectories = [
      installer.homeDirectory.appendingPathComponent(".cargo/bin").path,
      installer.homeDirectory.appendingPathComponent(".local/bin").path,
      "/opt/homebrew/bin", "/usr/local/bin", "/usr/bin", "/bin",
    ]
    environment["PATH"] = (commandDirectories + [environment["PATH"] ?? ""]).joined(separator: ":")
    let commandEnvironment = environment
    try await Task.detached(priority: .userInitiated) {
      let process = Process()
      process.executableURL = URL(fileURLWithPath: executablePath)
      process.arguments = arguments
      process.environment = commandEnvironment
      process.standardInput = FileHandle.nullDevice
      process.standardOutput = FileHandle.nullDevice
      process.standardError = FileHandle.nullDevice
      try process.run()
      process.waitUntilExit()
      guard process.terminationReason == .exit, process.terminationStatus == 0 else {
        throw NSError(
          domain: "Codescribe.BusPlayback", code: Int(process.terminationStatus),
          userInfo: [
            NSLocalizedDescriptionKey: String(
              localized: "Reply playback could not complete. The reply text is retained.")
          ])
      }
    }.value
  }
}
