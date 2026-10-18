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

/// Display metadata read from the same verified lease as playback identity.
/// Model is optional: provider identity alone never establishes a model.
struct AgentPlaybackBinding: Equatable, Sendable {
  let identity: AgentPlaybackIdentity
  let model: String?
  var repositoryName: String? = nil
  var workspace: String? = nil

  var descriptor: String {
    let provider =
      switch identity.provider {
      case "codex": "Codex"
      case "claude-code": "Claude Code"
      case "grok": "Grok"
      default: identity.provider
      }
    return [provider, model, repositoryName]
      .compactMap { $0?.trimmingCharacters(in: .whitespacesAndNewlines) }
      .filter { !$0.isEmpty }
      .joined(separator: " · ")
  }
}

private struct AgentPlaybackMuteReceipt: Decodable, Sendable {
  let schema: String
  let provider: String
  let providerSessionID: String
  let leaseID: String
  let bus: String
  let muted: Bool

  private enum CodingKeys: String, CodingKey {
    case schema, provider, bus, muted
    case providerSessionID = "provider_session_id"
    case leaseID = "lease_id"
  }

  func belongs(to identity: AgentPlaybackIdentity) -> Bool {
    schema == "codescribe.agent-playback-mute.v1" && provider == identity.provider
      && providerSessionID == identity.session && leaseID == identity.leaseID
      && bus == identity.bus
  }
}

private struct GlobalPlaybackMuteReceipt: Decodable, Sendable {
  let schema: String
  let scope: String
  let muted: Bool
}

enum AgentBroadcastPolicy: String, Decodable, Sendable, CaseIterable {
  case on, quiet, off

  var receivesWake: Bool { self == .on }
  /// One mark per state (Maciej, 2026-10-17: channel 0 and channel 0 plus
  /// steer could not be told apart): on and quiet are both members of
  /// channel 0, off is not; quiet queues every delivery without a wake.
  var statusSymbol: String {
    switch self {
    case .on: "antenna.radiowaves.left.and.right"
    case .quiet: "tray.and.arrow.down"
    case .off: "antenna.radiowaves.left.and.right.slash"
    }
  }
  var statusHelp: String {
    switch self {
    case .on:
      String(
        localized: "Channel 0 · steer: deliveries wake the agent",
        comment: "Agent row mark: member of channel 0, deliveries start a turn")
    case .quiet:
      String(
        localized: "Channel 0 · queued: deliveries wait, the agent is not woken",
        comment: "Agent row mark: member of channel 0, deliveries queue without a wake")
    case .off:
      String(
        localized: "Off channel 0: broadcasts are skipped, direct messages still arrive",
        comment: "Agent row mark: not a member of channel 0")
    }
  }
}

enum OverlayDeliveryControl: String, CaseIterable, Sendable {
  case revise, withdraw, release
}

private struct AgentBroadcastPolicyReceipt: Decodable, Sendable {
  let schema: String
  let provider: String
  let providerSessionID: String
  let leaseID: String
  let bus: String
  let policy: AgentBroadcastPolicy

  private enum CodingKeys: String, CodingKey {
    case schema, provider, bus, policy
    case providerSessionID = "provider_session_id"
    case leaseID = "lease_id"
  }

  func belongs(to identity: AgentPlaybackIdentity) -> Bool {
    schema == "codescribe.broadcast-policy.v1" && provider == identity.provider
      && providerSessionID == identity.session && leaseID == identity.leaseID
      && bus == identity.bus
  }
}

/// App-side invocations of the installed managed `cs-bus` command. They live
/// apart from `AgentBridgeInstaller.swift` because they reference app types,
/// and that file is also compiled on its own by `make install-bus`.
extension RealAgentBridgeInstaller {
  /// Archive one frozen owner through the binding file's only writer.
  /// `releaseReader` asks the helper to stop the owner's own live follower
  /// first; without it a live reader is refused with a reason.
  @MainActor
  static func archiveBusAgent(
    owner: OverlayConversationOwner,
    releaseReader: Bool = false,
    installer: RealAgentBridgeInstaller = RealAgentBridgeInstaller(),
    archive: @escaping @MainActor @Sendable (CsAgentArchiveRequest) async throws -> String
  ) async throws {
    guard let channel = UInt8(owner.channel), (1...9).contains(channel) else {
      throw CocoaError(.fileReadCorruptFile)
    }
    let executable = installer.commandURL("cs-bus")
    guard installer.fileManager.isExecutableFile(atPath: executable.path),
      installer.managedCommandID(executable) != nil
    else { throw CocoaError(.fileNoSuchFile) }
    let root = installer.bridgeRoot
    let bus = try await Task.detached(priority: .userInitiated) {
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
      return bus
    }.value
    let request = CsAgentArchiveRequest(
      channel: channel, provider: owner.provider, providerSessionId: owner.providerSessionID,
      leaseId: owner.leaseID, bus: bus, executable: executable.path, bridgeHome: root.path,
      releaseReader: releaseReader)
    let receiptText = try await archive(request)
    let data = Data(receiptText.utf8)
    guard data.count <= 65536,
      let receipt = try JSONSerialization.jsonObject(with: data) as? [String: Any],
      receipt["schema"] as? String == "codescribe.agent-archive.v1",
      receipt["released"] as? Bool == true,
      let archivedOwner = OverlayConversationOwner(row: receipt),
      archivedOwner.id == owner.id, archivedOwner.channel == owner.channel,
      receipt["bus"] as? String == bus
    else { throw CocoaError(.fileWriteUnknown) }
  }

  /// Resolve each live roster session to its actual leased bus, including custom buses.
  @MainActor
  static func boundPlaybackAgents(
    for candidates: [String: AgentPlaybackIdentity],
    installer: RealAgentBridgeInstaller = RealAgentBridgeInstaller()
  ) async -> [String: AgentPlaybackBinding] {
    let root = installer.bridgeRoot
    return await Task.detached(priority: .utility) {
      var result: [String: AgentPlaybackBinding] = [:]
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
        let model = (row["model"] as? String)?.trimmingCharacters(in: .whitespacesAndNewlines)
        result[channel] = AgentPlaybackBinding(
          identity: AgentPlaybackIdentity(
            provider: candidate.provider, session: candidate.session, bus: bus),
          model: model.flatMap { $0.isEmpty || $0.count > 160 ? nil : $0 },
          repositoryName: (row["repository_name"] as? String).flatMap {
            $0.isEmpty || $0.count > 160 ? nil : $0
          },
          workspace: (row["workspace"] as? String).flatMap {
            $0.hasPrefix("/") && $0.count <= 4096 ? $0 : nil
          })
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
          // FileHandle reports Cocoa's general missing-file code (4), whereas
          // other Foundation readers can report the read-specific code (260).
          let underlying = error.userInfo[NSUnderlyingErrorKey] as? NSError
          let missing =
            error.domain == NSCocoaErrorDomain
            && [NSFileNoSuchFileError, NSFileReadNoSuchFileError].contains(error.code)
            || error.domain == NSPOSIXErrorDomain && error.code == Int(ENOENT)
            || underlying?.domain == NSPOSIXErrorDomain && underlying?.code == Int(ENOENT)
          if missing {
            result[identity] = false
          }
        }
      }
      return result
    }.value
  }

  @MainActor
  static func globalPlaybackMuteSnapshot(
    installer: RealAgentBridgeInstaller = RealAgentBridgeInstaller()
  ) async -> Bool? {
    let path = installer.bridgeRoot.appendingPathComponent("runtime/playback-mutes/global.json")
    return await Task.detached(priority: .utility) {
      do {
        let handle = try FileHandle(forReadingFrom: path)
        defer { try? handle.close() }
        let data = try handle.read(upToCount: 65537) ?? Data()
        guard data.count <= 65536,
          let row = try? JSONDecoder().decode(GlobalPlaybackMuteReceipt.self, from: data),
          row.schema == "codescribe.agent-playback-mute.v1", row.scope == "all"
        else { return nil as Bool? }
        return row.muted
      } catch let error as NSError {
        let underlying = error.userInfo[NSUnderlyingErrorKey] as? NSError
        let missing =
          error.domain == NSCocoaErrorDomain
          && [NSFileNoSuchFileError, NSFileReadNoSuchFileError].contains(error.code)
          || error.domain == NSPOSIXErrorDomain && error.code == Int(ENOENT)
          || underlying?.domain == NSPOSIXErrorDomain && underlying?.code == Int(ENOENT)
        return missing ? false : nil
      }
    }.value
  }

  @MainActor
  static func setGlobalPlaybackMuted(_ muted: Bool) async throws {
    let installer = RealAgentBridgeInstaller()
    try await runPlaybackMuteCommand(
      [muted ? "--mute-all" : "--unmute-all", "--bridge-home", installer.bridgeRoot.path],
      installer: installer)
  }

  @MainActor
  static func setPlaybackMuted(_ muted: Bool, for identity: AgentPlaybackIdentity) async throws {
    let installer = RealAgentBridgeInstaller()
    let arguments = [
      muted ? "--mute-agent" : "--unmute-agent", "--provider", identity.provider,
      "--session", identity.session, "--bus", identity.bus,
      "--bridge-home", installer.bridgeRoot.path,
    ]
    try await runPlaybackMuteCommand(arguments, installer: installer)
  }

  @MainActor
  static func broadcastPolicySnapshot(
    for identities: Set<AgentPlaybackIdentity>,
    installer: RealAgentBridgeInstaller = RealAgentBridgeInstaller()
  ) async -> [AgentPlaybackIdentity: AgentBroadcastPolicy] {
    let root = installer.bridgeRoot
    return await Task.detached(priority: .utility) {
      var result: [AgentPlaybackIdentity: AgentBroadcastPolicy] = [:]
      for identity in identities {
        result[identity] = .on
        let path = root.appendingPathComponent(
          "runtime/broadcast-policy/\(identity.storageKey).json")
        // Unlike playback mute's fail-closed silence, policy fails open to on:
        // a broken receipt must never silently disconnect an agent.
        guard let handle = try? FileHandle(forReadingFrom: path) else { continue }
        defer { try? handle.close() }
        guard let data = try? handle.read(upToCount: 65537), data.count <= 65536,
          let row = try? JSONDecoder().decode(AgentBroadcastPolicyReceipt.self, from: data),
          row.belongs(to: identity)
        else { continue }
        result[identity] = row.policy
      }
      return result
    }.value
  }

  static func broadcastPolicyArguments(
    _ policy: AgentBroadcastPolicy, for identity: AgentPlaybackIdentity, root: URL
  ) -> [String] {
    [
      "--broadcast", policy.rawValue, "--provider", identity.provider,
      "--session", identity.session, "--bus", identity.bus, "--bridge-home", root.path,
    ]
  }

  @MainActor
  static func setBroadcastPolicy(
    _ policy: AgentBroadcastPolicy, for identity: AgentPlaybackIdentity,
    installer: RealAgentBridgeInstaller = RealAgentBridgeInstaller()
  ) async throws {
    try await runPlaybackMuteCommand(
      broadcastPolicyArguments(policy, for: identity, root: installer.bridgeRoot),
      installer: installer)
  }

  @MainActor
  private static func runPlaybackMuteCommand(
    _ arguments: [String], installer: RealAgentBridgeInstaller
  ) async throws {
    let executable = installer.commandURL("cs-bus")
    guard installer.fileManager.isExecutableFile(atPath: executable.path),
      installer.managedCommandID(executable) != nil
    else {
      throw NSError(
        domain: "Codescribe.BusPlayback", code: 1,
        userInfo: [
          NSLocalizedDescriptionKey: String(
            localized: "Install the agent bridge to control agent preferences.")
        ])
    }
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
  static func sendBusText(
    owner: OverlayConversationOwner?, text: String, holdSeconds: UInt32? = nil,
    installer: RealAgentBridgeInstaller = RealAgentBridgeInstaller()
  ) async throws {
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
    let message = ConversationAttachmentPointer.extract(from: text)
    let hold = holdSeconds ?? CodescribeConfig().loadSettings().agentSendHoldSec
    try await Task.detached(priority: .userInitiated) {
      var arguments = try busTextArguments(holdSeconds: hold, owner: owner, root: root)
      for path in message.paths { arguments += ["--attach-file", path] }
      let process = Process()
      let input = Pipe()
      // The helper names its refusal on stderr ("channel was rebound or its
      // agent is not listening…"); swallowing it left the Founder with a
      // bare "could not be sent" and no culprit (P0, 2026-10-17).
      let diagnostics = Pipe()
      process.executableURL = executable
      process.arguments = arguments
      process.standardInput = input
      process.standardOutput = FileHandle.nullDevice
      process.standardError = diagnostics
      // Bound text fits in the pipe; writing happens off the UI thread.
      guard message.text.utf8.count <= 65536 else { throw CocoaError(.fileWriteOutOfSpace) }
      try process.run()
      try input.fileHandleForWriting.write(contentsOf: Data(message.text.utf8))
      try input.fileHandleForWriting.close()
      // Reading to EOF first keeps a chatty helper from filling the pipe
      // and deadlocking against waitUntilExit.
      let refusalData = diagnostics.fileHandleForReading.readDataToEndOfFile()
      process.waitUntilExit()
      guard process.terminationReason == .exit, process.terminationStatus == 0 else {
        let headline =
          process.terminationStatus == 4
          ? String(
            localized:
              "Some agents may have received this message. Check read receipts before sending again."
          )
          : String(localized: "Message could not be sent. Your draft is retained.")
        let refusal =
          String(data: refusalData.suffix(4096), encoding: .utf8)?
          .trimmingCharacters(in: .whitespacesAndNewlines)
          .components(separatedBy: "\n").last?
          .replacingOccurrences(of: "cs-bus: ", with: "") ?? ""
        throw NSError(
          domain: "Codescribe.BusText", code: Int(process.terminationStatus),
          userInfo: [
            NSLocalizedDescriptionKey: refusal.isEmpty ? headline : headline + "\n" + refusal
          ])
      }
    }.value
  }

  static func busTextArguments(
    holdSeconds: UInt32, owner: OverlayConversationOwner?, root: URL
  ) throws -> [String] {
    // The owner block stays right behind --send-text; the hold is a trailing
    // option, so existing publisher receipts keep their argument prefix.
    ["--send-text"] + (try busOwnerArguments(owner: owner, root: root))
      + ["--hold-seconds", String(holdSeconds)]
  }

  static func deliveryControlArguments(
    _ control: OverlayDeliveryControl, identity: String, owner: OverlayConversationOwner?,
    busPath: String, root: URL
  ) throws -> [String] {
    guard !identity.isEmpty, identity.utf8.count <= 256,
      !identity.unicodeScalars.contains(where: { CharacterSet.controlCharacters.contains($0) }),
      busPath.hasPrefix("/")
    else { throw CocoaError(.fileReadCorruptFile) }
    return ["--" + control.rawValue, identity]
      + (try busOwnerArguments(owner: owner, root: root, busPath: busPath))
  }

  private static func busOwnerArguments(
    owner: OverlayConversationOwner?, root: URL, busPath: String? = nil
  ) throws -> [String] {
    guard let owner else {
      return ["--channel", "0", "--bridge-home", root.path]
    }
    guard (1...9).contains(Int(owner.channel) ?? -1), owner.leaseID.count == 32,
      owner.leaseID.allSatisfy({ "0123456789abcdef".contains($0) })
    else { throw CocoaError(.fileReadCorruptFile) }
    let lease = root.appendingPathComponent("leases/\(owner.leaseID).json")
    let data = try Data(contentsOf: lease)
    guard data.count <= 16 << 20,
      let receipt = try JSONSerialization.jsonObject(with: data) as? [String: Any],
      receipt["schema"] as? String == "codescribe.agent-bridge.lease.v1",
      receipt["provider"] as? String == owner.provider,
      receipt["provider_session_id"] as? String == owner.providerSessionID,
      receipt["lease_id"] as? String == owner.leaseID,
      let bus = receipt["bus"] as? String, bus.hasPrefix("/"),
      busPath.map({
        URL(fileURLWithPath: $0).resolvingSymlinksInPath().path
          == URL(fileURLWithPath: bus).resolvingSymlinksInPath().path
      }) ?? true
    else { throw CocoaError(.fileReadCorruptFile) }
    return [
      "--channel", owner.channel, "--provider", owner.provider,
      "--session", owner.providerSessionID, "--lease", owner.leaseID, "--bus", bus,
      "--bridge-home", root.path,
    ]
  }

  /// Writes a control to the same bus. Only the follower decides delivery eligibility.
  @MainActor
  static func controlBusMessage(
    _ control: OverlayDeliveryControl, identity: String, owner: OverlayConversationOwner?,
    busPath: String, text: String? = nil,
    installer: RealAgentBridgeInstaller = RealAgentBridgeInstaller()
  ) async throws {
    let executable = installer.commandURL("cs-bus")
    guard installer.fileManager.isExecutableFile(atPath: executable.path),
      installer.managedCommandID(executable) != nil
    else {
      throw NSError(
        domain: "Codescribe.BusDelivery", code: 1,
        userInfo: [
          NSLocalizedDescriptionKey: String(localized: "Install the agent bridge to send messages.")
        ])
    }
    let root = installer.bridgeRoot
    try await Task.detached(priority: .userInitiated) {
      let arguments = try deliveryControlArguments(
        control, identity: identity, owner: owner, busPath: busPath, root: root)
      if control == .revise {
        guard let text, !text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty,
          text.utf8.count <= 65536
        else { throw CocoaError(.fileWriteOutOfSpace) }
      }
      let process = Process()
      let input = Pipe()
      process.executableURL = executable
      process.arguments = arguments
      if control == .revise {
        process.standardInput = input
      } else {
        process.standardInput = FileHandle.nullDevice
      }
      process.standardOutput = FileHandle.nullDevice
      process.standardError = FileHandle.nullDevice
      try process.run()
      if control == .revise, let text {
        try input.fileHandleForWriting.write(contentsOf: Data(text.utf8))
        try input.fileHandleForWriting.close()
      }
      process.waitUntilExit()
      guard process.terminationReason == .exit, process.terminationStatus == 0 else {
        throw NSError(
          domain: "Codescribe.BusDelivery", code: Int(process.terminationStatus),
          userInfo: [
            NSLocalizedDescriptionKey: String(localized: "Message could not be changed.")
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
    let outcomePublished: Int32 = stop ? 0 : playbackOutcomePublishedStatus
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
      guard process.terminationReason == .exit,
        process.terminationStatus == 0
          || process.terminationStatus == outcomePublished
      else {
        throw NSError(
          domain: "Codescribe.BusPlayback", code: Int(process.terminationStatus),
          userInfo: [
            NSLocalizedDescriptionKey: String(
              localized: "Reply playback could not complete. The reply text is retained.")
          ])
      }
    }.value
  }

  /// `cs-bus --play-reply` exits 5 when the reply was not spoken and its
  /// outcome (stopped, take began, refused, failed) is already published on
  /// the reply's playback record (`speak_published_reply`). The record is
  /// what the conversation shows, so the exit is not a second error.
  nonisolated static let playbackOutcomePublishedStatus: Int32 = 5
}
