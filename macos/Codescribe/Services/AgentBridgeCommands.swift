import AppKit
import CryptoKit
import Darwin
import Foundation
import OSLog

/// Frame coordinates published by an attached follower in `leases/<lease_id>.json`
/// and echoed in `codescribe.agent-panel-focus-result.v1`.
struct AgentFrameBinding: Equatable, Sendable, Codable {
  let session: String
  let paneID: Int
  let hostBundleID: String?
  let hostPID: Int32?

  var terminalPaneID: String { "terminal_\(paneID)" }

  init(
    session: String,
    paneID: Int,
    hostBundleID: String? = nil,
    hostPID: Int32? = nil
  ) {
    self.session = session
    self.paneID = paneID
    self.hostBundleID = hostBundleID
    self.hostPID = hostPID
  }

  init?(row: [String: Any]) {
    guard
      let session = (row["session"] as? String)?.trimmingCharacters(in: .whitespacesAndNewlines),
      !session.isEmpty, session.count <= 160
    else { return nil }
    let parsedPaneID: Int?
    if let intValue = row["pane_id"] as? Int {
      parsedPaneID = intValue
    } else if let numberValue = row["pane_id"] as? NSNumber {
      parsedPaneID = numberValue.intValue
    } else if let textValue = row["pane_id"] as? String {
      let trimmed = textValue.trimmingCharacters(in: .whitespacesAndNewlines)
      let digits =
        trimmed.hasPrefix("terminal_")
        ? String(trimmed.dropFirst("terminal_".count))
        : trimmed
      parsedPaneID = Int(digits)
    } else {
      parsedPaneID = nil
    }
    guard let paneID = parsedPaneID, paneID >= 0 else { return nil }
    let hostBundleID = (row["host_bundle_id"] as? String)?.trimmingCharacters(
      in: .whitespacesAndNewlines)
    let parsedPID: Int32?
    if let int32Value = row["host_pid"] as? Int32 {
      parsedPID = int32Value
    } else if let intValue = row["host_pid"] as? Int, let exact = Int32(exactly: intValue) {
      parsedPID = exact
    } else if let numberValue = row["host_pid"] as? NSNumber {
      parsedPID = numberValue.int32Value
    } else {
      parsedPID = nil
    }
    self.session = session
    self.paneID = paneID
    self.hostBundleID = hostBundleID?.isEmpty == false ? hostBundleID : nil
    self.hostPID = parsedPID.flatMap { $0 > 0 ? $0 : nil }
  }
}

/// The iTerm2 or Terminal.app tab a follower names in a
/// `codescribe.agent-panel-focus-result.v1` row when no Frame pane is known.
/// The follower selects the tab itself; the app only activates `hostBundleID`.
struct AgentTerminalBinding: Equatable, Sendable, Codable {
  let host: String
  let hostBundleID: String
  let tty: String?

  init(host: String, hostBundleID: String, tty: String? = nil) {
    self.host = host
    self.hostBundleID = hostBundleID
    self.tty = tty
  }

  init?(row: [String: Any]) {
    guard
      let host = (row["host"] as? String)?.trimmingCharacters(in: .whitespacesAndNewlines),
      !host.isEmpty,
      let hostBundleID = (row["host_bundle_id"] as? String)?.trimmingCharacters(
        in: .whitespacesAndNewlines),
      !hostBundleID.isEmpty, hostBundleID.count <= 160
    else { return nil }
    let tty = (row["tty"] as? String)?.trimmingCharacters(in: .whitespacesAndNewlines)
    self.host = host
    self.hostBundleID = hostBundleID
    self.tty = tty?.isEmpty == false ? tty : nil
  }
}

/// Request published on the agent's bus when the user clicks the agent's focus
/// button in the Overlay; the follower picks the Frame pane or the terminal tab.
struct OverlayAgentPanelFocusRequest: Equatable, Sendable {
  let requestID: String
  let owner: OverlayConversationOwner
  let emittedAt: String

  init(
    requestID: String = Self.makeRequestID(),
    owner: OverlayConversationOwner,
    emittedAt: String = Self.makeTimestamp()
  ) {
    self.requestID = requestID
    self.owner = owner
    self.emittedAt = emittedAt
  }

  static func makeRequestID() -> String {
    String(UUID().uuidString.replacingOccurrences(of: "-", with: "").lowercased().prefix(24))
  }

  static func makeTimestamp(now: Date = Date()) -> String {
    let formatter = ISO8601DateFormatter()
    formatter.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
    return formatter.string(from: now)
  }

  func eventObject(busPath: String) -> [String: Any] {
    [
      "schema": "codescribe.agent-panel-focus.v1",
      "kind": "focus_agent_panel",
      "request_id": requestID,
      "provider": owner.provider,
      "provider_session_id": owner.providerSessionID,
      "lease_id": owner.leaseID,
      "channel": owner.channel,
      "bus": busPath,
      "emitted_at": emittedAt,
      "source": "overlay_click",
    ]
  }
}

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
  var frame: AgentFrameBinding? = nil
  var terminal: AgentTerminalBinding? = nil

  /// Only coordinates supplied by this exact lease. No machine name is inferred.
  var location: String? {
    if let frame { return "Frame · \(frame.session) · \(frame.paneID)" }
    if let terminal {
      return [terminal.host, terminal.tty].compactMap { $0 }.joined(separator: " · ")
    }
    return workspace
  }

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

/// One lease's reception on two independent axes (Maciej, 2026-10-23): the
/// antenna says whether the agent is on channel 0, the moon whether anything
/// may wake it right now — every channel, the Founder's directs and peers
/// included. Each combination is legal. The former single knob (on, quiet,
/// off) could not say "off channel 0 with the moon lit", so a meeting kept
/// being interrupted from other channels while the quiet box stayed greyed.
struct AgentBroadcastPolicy: Equatable, Hashable, Sendable {
  /// On channel 0: broadcasts reach this agent's mailbox.
  var member = true
  /// The moon ("tryb księżyc"): deliveries queue without a wake, whatever
  /// their channel; the agent reads them on its own next turn.
  var moon = false

  static let on = Self(member: true, moon: false)
  /// The pre-split spelling of a lit moon on channel 0.
  static let quiet = Self(member: true, moon: true)
  static let off = Self(member: false, moon: false)
  static let offQuiet = Self(member: false, moon: true)
  static let allCases = [on, quiet, off, offQuiet]

  var receivesWake: Bool { member && !moon }

  /// The antenna, lit on channel 0 and slashed off it (Maciej, 2026-10-17:
  /// channel 0 and channel 0 plus steer could not be told apart; the moon
  /// now carries the second reading as its own mark).
  var statusSymbol: String {
    member ? "antenna.radiowaves.left.and.right" : "antenna.radiowaves.left.and.right.slash"
  }
  var statusHelp: String {
    member
      ? String(
        localized: "Channel 0: broadcasts reach this agent — click to leave",
        comment: "Agent row mark: member of channel 0; the mark is a one-click switch")
      : String(
        localized:
          "Off channel 0: broadcasts are skipped, direct messages still arrive — click to join",
        comment: "Agent row mark: not a member of channel 0; the mark is a one-click switch")
  }

  /// The moon, filled while do-not-disturb holds every delivery.
  var quietSymbol: String { moon ? "moon.fill" : "moon" }
  var quietHelp: String {
    moon
      ? String(
        localized:
          "Do not disturb: nothing wakes the agent, every message waits in its mailbox — click to clear",
        comment: "Agent row mark: the moon is lit; the mark is a one-click switch")
      : String(
        localized: "Messages wake the agent — click for do not disturb",
        comment: "Agent row mark: the moon is dark; the mark is a one-click switch")
  }

  /// The helper's words for each axis.
  var membershipArgument: String { member ? "on" : "off" }
  var moonArgument: String { moon ? "on" : "off" }
}

enum OverlayDeliveryControl: String, CaseIterable, Sendable {
  case revise, withdraw, release
}

enum AgentBusMessageValidationError: LocalizedError {
  case messageTooLong
  case emptyRevision
  case invalidAttachment

  var errorDescription: String? {
    switch self {
    case .messageTooLong:
      String(localized: "The message is too long (64 KB limit).")
    case .emptyRevision:
      String(localized: "The revised message cannot be empty.")
    case .invalidAttachment:
      String(localized: "The files cannot be attached to this live capture.")
    }
  }
}

private struct AgentBroadcastPolicyReceipt: Decodable, Sendable {
  let schema: String
  let provider: String
  let providerSessionID: String
  let leaseID: String
  let bus: String
  /// The membership word; "quiet" is the pre-split spelling of a lit moon.
  let policy: String
  let quiet: Bool?

  private enum CodingKeys: String, CodingKey {
    case schema, provider, bus, policy, quiet
    case providerSessionID = "provider_session_id"
    case leaseID = "lease_id"
  }

  /// Nil for a word the helper would also refuse.
  var reception: AgentBroadcastPolicy? {
    switch policy {
    case "on": AgentBroadcastPolicy(member: true, moon: quiet ?? false)
    case "off": AgentBroadcastPolicy(member: false, moon: quiet ?? false)
    case "quiet": .quiet
    default: nil
    }
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
          },
          frame: (row["frame"] as? [String: Any]).flatMap(AgentFrameBinding.init(row:)),
          terminal: (row["terminal"] as? [String: Any]).flatMap(AgentTerminalBinding.init(row:)))
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
          row.belongs(to: identity), let reception = row.reception
        else { continue }
        result[identity] = reception
      }
      return result
    }.value
  }

  static func broadcastPolicyArguments(
    _ policy: AgentBroadcastPolicy, for identity: AgentPlaybackIdentity, root: URL
  ) -> [String] {
    // Both axes in one command: the helper moves only the flags it is given,
    // and a full receipt never depends on what the previous one said.
    [
      "--broadcast", policy.membershipArgument, "--quiet", policy.moonArgument,
      "--provider", identity.provider, "--session", identity.session, "--bus", identity.bus,
      "--bridge-home", root.path,
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
    userReplyTo: OverlayUserReplyReference? = nil,
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
      var arguments = try busTextArguments(
        holdSeconds: hold, owner: owner, root: root, userReplyTo: userReplyTo)
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
      guard message.text.utf8.count <= 65536 else {
        throw AgentBusMessageValidationError.messageTooLong
      }
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

  /// Stages files against one exact open channel capture row. The helper
  /// rechecks session, epoch and source sequence under the bus append lock;
  /// stale targets fail so callers can keep the files in their draft.
  @MainActor
  static func stageCaptureAttachments(
    paths: [String], target: OverlayCaptureAttachmentTarget,
    installer: RealAgentBridgeInstaller = RealAgentBridgeInstaller()
  ) async throws {
    guard !paths.isEmpty, paths.count <= 64,
      paths.allSatisfy({ $0.hasPrefix("/") && $0.utf8.count <= 4096 && !$0.contains("\0") }),
      target.captureSessionID.hasPrefix("agent-channel-"),
      !target.busPath.isEmpty, target.busPath.hasPrefix("/"),
      target.sampleEnd > target.sampleStart,
      !target.recipientIDs.isEmpty
    else { throw AgentBusMessageValidationError.invalidAttachment }
    let executable = installer.commandURL("cs-bus")
    guard installer.fileManager.isExecutableFile(atPath: executable.path),
      installer.managedCommandID(executable) != nil
    else {
      throw NSError(
        domain: "Codescribe.BusAttachment", code: 1,
        userInfo: [
          NSLocalizedDescriptionKey: String(localized: "Install the agent bridge to attach files.")
        ])
    }
    var arguments = [
      "--stage-capture-attachment",
      "--capture-session", target.captureSessionID,
      "--capture-epoch", String(target.captureEpoch),
      "--capture-source-sequence", String(target.sourceSequence),
      "--bus", target.busPath,
    ]
    for path in paths { arguments += ["--attach-file", path] }
    let commandArguments = arguments
    try await Task.detached(priority: .userInitiated) {
      let process = Process()
      let diagnostics = Pipe()
      process.executableURL = executable
      process.arguments = commandArguments
      process.standardInput = FileHandle.nullDevice
      process.standardOutput = FileHandle.nullDevice
      process.standardError = diagnostics
      try process.run()
      let diagnosticData = diagnostics.fileHandleForReading.readDataToEndOfFile()
      process.waitUntilExit()
      guard process.terminationReason == .exit, process.terminationStatus == 0 else {
        let refusal =
          String(data: diagnosticData.suffix(4096), encoding: .utf8)?
          .trimmingCharacters(in: .whitespacesAndNewlines)
          .components(separatedBy: "\n").last?
          .replacingOccurrences(of: "cs-bus: ", with: "") ?? ""
        throw NSError(
          domain: "Codescribe.BusAttachment", code: Int(process.terminationStatus),
          userInfo: [
            NSLocalizedDescriptionKey: refusal.isEmpty
              ? String(
                localized:
                  "Files could not be attached to this live capture. Keep them in your draft and try again."
              )
              : refusal
          ])
      }
    }.value
  }

  static func busTextArguments(
    holdSeconds: UInt32, owner: OverlayConversationOwner?, root: URL,
    userReplyTo: OverlayUserReplyReference? = nil
  ) throws -> [String] {
    // The owner block stays right behind --send-text; the hold is a trailing
    // option, so existing publisher receipts keep their argument prefix.
    var arguments =
      ["--send-text"] + (try busOwnerArguments(owner: owner, root: root))
      + ["--hold-seconds", String(holdSeconds)]
    if let userReplyTo {
      let encoded = try JSONEncoder().encode(userReplyTo)
      arguments += ["--user-reply-to", String(decoding: encoded, as: UTF8.self)]
    }
    return arguments
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
        guard let text, !text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else {
          throw AgentBusMessageValidationError.emptyRevision
        }
        guard text.utf8.count <= 65536 else {
          throw AgentBusMessageValidationError.messageTooLong
        }
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

  /// Publishes a `codescribe.agent-panel-focus.v1` request on the bound lease's
  /// bus so the agent's own follower focuses its `vc-frame` pane.
  @MainActor
  static func publishAgentPanelFocus(
    _ request: OverlayAgentPanelFocusRequest,
    installer: RealAgentBridgeInstaller = RealAgentBridgeInstaller()
  ) async throws {
    let owner = request.owner
    guard (1...10).contains(Int(owner.channel) ?? -1),
      owner.leaseID.count == 32,
      owner.leaseID.allSatisfy({ "0123456789abcdef".contains($0) }),
      request.requestID.count == 24,
      request.requestID.allSatisfy({ "0123456789abcdef".contains($0) })
    else { throw CocoaError(.fileReadCorruptFile) }
    guard let executable = canonicalBusPublisherURL(installer: installer) else {
      throw NSError(
        domain: "Codescribe.PanelFocus", code: 1,
        userInfo: [
          NSLocalizedDescriptionKey: String(
            localized: "Install the agent bridge to focus agent panels in Frame.")
        ])
    }
    let root = installer.bridgeRoot
    let executablePath = executable.path
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
      guard followerProcessAlive(root: root, leaseID: owner.leaseID, lease: lease) else {
        throw NSError(
          domain: "Codescribe.PanelFocus", code: 2,
          userInfo: [
            NSLocalizedDescriptionKey: String(
              localized: "Agent listener is not running in Frame.")
          ])
      }
      let payload = try JSONSerialization.data(
        withJSONObject: request.eventObject(busPath: bus))
      let process = Process()
      let input = Pipe()
      let diagnostics = Pipe()
      process.executableURL = URL(fileURLWithPath: executablePath)
      process.arguments = ["bus", "append-event", "--bus", bus]
      process.standardInput = input
      process.standardOutput = FileHandle.nullDevice
      process.standardError = diagnostics
      try process.run()
      try input.fileHandleForWriting.write(contentsOf: payload)
      try input.fileHandleForWriting.close()
      let refusalData = diagnostics.fileHandleForReading.readDataToEndOfFile()
      process.waitUntilExit()
      guard process.terminationReason == .exit, process.terminationStatus == 0 else {
        let refusal =
          String(data: refusalData.suffix(4096), encoding: .utf8)?
          .trimmingCharacters(in: .whitespacesAndNewlines)
          .components(separatedBy: "\n").last?
          .replacingOccurrences(of: "codescribe: ", with: "") ?? ""
        Logger(
          subsystem: Bundle.main.bundleIdentifier ?? "com.vetcoders.codescribe",
          category: "agent-panel-focus"
        ).error(
          "panel focus publish failed channel=\(owner.channel, privacy: .public) request_id=\(request.requestID, privacy: .public) exit=\(process.terminationStatus, privacy: .public) executable=\(executablePath, privacy: .public) stderr=\(refusal, privacy: .public)"
        )
        let headline = String(localized: "Could not request panel focus in Frame.")
        throw NSError(
          domain: "Codescribe.PanelFocus", code: Int(process.terminationStatus),
          userInfo: [
            NSLocalizedDescriptionKey: refusal.isEmpty ? headline : "\(headline) (\(refusal))"
          ])
      }
    }.value
  }

  private static func canonicalBusPublisherURL(
    installer: RealAgentBridgeInstaller
  ) -> URL? {
    let candidates: [URL?] = [
      Bundle.main.resourceURL?.appendingPathComponent("agent-bridge/bin/codescribe"),
      installer.bridgeRoot.appendingPathComponent("runtime/bin/codescribe"),
      installer.homeDirectory.appendingPathComponent(".cargo/bin/codescribe"),
      installer.homeDirectory.appendingPathComponent(".local/bin/codescribe"),
    ]
    for candidate in candidates.compactMap({ $0 }) {
      if installer.fileManager.isExecutableFile(atPath: candidate.path) {
        return candidate.resolvingSymlinksInPath()
      }
    }
    return nil
  }

  private nonisolated static func followerProcessAlive(
    root: URL,
    leaseID: String,
    lease: [String: Any]
  ) -> Bool {
    let pidfile = root.appendingPathComponent("runtime/followers/\(leaseID).pid")
    if let data = try? Data(contentsOf: pidfile),
      data.count <= 65536,
      let row = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
      let pid = (row["pid"] as? NSNumber)?.int32Value,
      pid > 0,
      Darwin.kill(pid, 0) == 0 || errno == EPERM
    {
      return true
    }
    if let leasePID = (lease["pid"] as? NSNumber)?.int32Value,
      leasePID > 0,
      Darwin.kill(leasePID, 0) == 0 || errno == EPERM
    {
      return true
    }
    return false
  }

  /// Resolves the public `vc-frame` command on `PATH`. When a `launchd`-spawned
  /// GUI process inherits only the bare system `PATH` (`/usr/bin:/bin:/usr/sbin:/sbin`)
  /// and the user's home directory defines `.zshenv` or `.zprofile`, the user's
  /// non-interactive `zsh` `PATH` is consulted. Never falls back to a literal
  /// `~/.local/bin` path, `VIBECRAFTED_VC_FRAME_BIN`, or private `releases/*/bin`
  /// directories.
  static let vcFrameShellFailureCooldown: TimeInterval = 5.0

  nonisolated static func vcFrameExecutableURL(
    homeDirectory: URL,
    environment: [String: String],
    now: Date = Date()
  ) -> URL? {
    let rawPath = effectiveVcFramePath(
      homeDirectory: homeDirectory,
      environment: environment,
      now: now
    )
    var directories: [String] = []
    for entry in rawPath.split(separator: ":", omittingEmptySubsequences: true) {
      let trimmed = entry.trimmingCharacters(in: .whitespacesAndNewlines)
      guard !trimmed.isEmpty else { continue }
      let expanded: String
      if trimmed == "~" {
        expanded = homeDirectory.path
      } else if trimmed.hasPrefix("~/") {
        expanded = homeDirectory.appendingPathComponent(String(trimmed.dropFirst(2))).path
      } else {
        expanded = (trimmed as NSString).expandingTildeInPath
      }
      if !directories.contains(expanded) {
        directories.append(expanded)
      }
    }
    let fileManager = FileManager.default
    for directory in directories {
      let candidate = URL(fileURLWithPath: directory, isDirectory: true)
        .appendingPathComponent("vc-frame")
      var isDirectory: ObjCBool = false
      if fileManager.fileExists(atPath: candidate.path, isDirectory: &isDirectory),
        !isDirectory.boolValue,
        fileManager.isExecutableFile(atPath: candidate.path)
      {
        return candidate
      }
    }
    return nil
  }

  private nonisolated static func effectiveVcFramePath(
    homeDirectory: URL,
    environment: [String: String],
    now: Date = Date()
  ) -> String {
    let launchdDefaultPath = "/usr/bin:/bin:/usr/sbin:/sbin"
    let provided = environment["PATH"]?.trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
    if !provided.isEmpty, provided != launchdDefaultPath {
      return provided
    }
    let profileStamp = VcFrameShellProfileStamp(homeDirectory: homeDirectory)
    let fileManager = FileManager.default
    guard
      profileStamp.zshenv.exists || profileStamp.zprofile.exists,
      fileManager.isExecutableFile(atPath: "/bin/zsh")
    else {
      return provided
    }
    switch VcFrameShellPathCache.shared.lookup(for: profileStamp, now: now) {
    case .hit(let cached):
      return cached
    case .throttledFailure:
      return provided
    case .miss:
      break
    }

    func recordFailureIfUnchanged() {
      let postStamp = VcFrameShellProfileStamp(homeDirectory: homeDirectory)
      if postStamp == profileStamp {
        VcFrameShellPathCache.shared.storeFailure(at: now, for: profileStamp)
      } else {
        VcFrameShellPathCache.shared.remove(for: profileStamp)
        VcFrameShellPathCache.shared.remove(for: postStamp)
      }
    }

    let process = Process()
    let stdoutPipe = Pipe()
    process.executableURL = URL(fileURLWithPath: "/bin/zsh")
    process.arguments = [
      "-c",
      "[[ -r \"$HOME/.zprofile\" ]] && source \"$HOME/.zprofile\" >/dev/null 2>&1; /usr/bin/printenv PATH",
    ]
    process.environment = [
      "HOME": homeDirectory.path,
      "ZDOTDIR": homeDirectory.path,
      "PATH": launchdDefaultPath,
    ]
    process.standardInput = FileHandle.nullDevice
    process.standardOutput = stdoutPipe
    process.standardError = FileHandle.nullDevice
    do {
      try process.run()
    } catch {
      recordFailureIfUnchanged()
      return provided
    }
    let deadline = Date().addingTimeInterval(1.0)
    while process.isRunning, Date() < deadline {
      Thread.sleep(forTimeInterval: 0.01)
    }
    if process.isRunning {
      process.terminate()
      recordFailureIfUnchanged()
      return provided
    }
    let data = stdoutPipe.fileHandleForReading.readDataToEndOfFile()
    guard process.terminationReason == .exit, process.terminationStatus == 0,
      let resolved = String(data: data, encoding: .utf8)?
        .trimmingCharacters(in: .whitespacesAndNewlines),
      !resolved.isEmpty
    else {
      recordFailureIfUnchanged()
      return provided
    }
    let postStamp = VcFrameShellProfileStamp(homeDirectory: homeDirectory)
    if postStamp == profileStamp {
      VcFrameShellPathCache.shared.storeResolved(resolved, for: profileStamp)
    } else {
      VcFrameShellPathCache.shared.remove(for: profileStamp)
      VcFrameShellPathCache.shared.remove(for: postStamp)
    }
    return resolved
  }

  /// Side-effect-free capability probe: runs `vc-frame action focus-pane-id --help`
  /// with session and socket environment variables stripped so it never connects
  /// to a live server, changes focus, or spawns a session. Accepts both older
  /// compatible releases (such as `4.4.0+g959bf184`) and current releases.
  nonisolated static func probeVcFramePanelFocusCapability(
    executableURL: URL,
    environment: [String: String]
  ) -> Bool {
    let process = Process()
    let stdoutPipe = Pipe()
    let stderrPipe = Pipe()
    process.executableURL = executableURL
    process.arguments = ["action", "focus-pane-id", "--help"]
    process.standardInput = FileHandle.nullDevice
    process.standardOutput = stdoutPipe
    process.standardError = stderrPipe

    var probeEnvironment = environment
    for key in [
      "ZELLIJ_SESSION_NAME",
      "VC_FRAME_SESSION_NAME",
      "ZELLIJ_SOCKET_DIR",
      "VC_FRAME_SOCKET_DIR",
    ] {
      probeEnvironment.removeValue(forKey: key)
    }
    let systemPath = "/usr/bin:/bin:/usr/sbin:/sbin"
    if let existingPath = probeEnvironment["PATH"], !existingPath.isEmpty {
      probeEnvironment["PATH"] = "\(existingPath):\(systemPath)"
    } else {
      probeEnvironment["PATH"] = systemPath
    }
    process.environment = probeEnvironment

    do {
      try process.run()
    } catch {
      return false
    }
    let deadline = Date().addingTimeInterval(2.0)
    while process.isRunning, Date() < deadline {
      Thread.sleep(forTimeInterval: 0.01)
    }
    if process.isRunning {
      process.terminate()
      return false
    }
    let stdoutData = stdoutPipe.fileHandleForReading.readDataToEndOfFile()
    let stderrData = stderrPipe.fileHandleForReading.readDataToEndOfFile()
    guard process.terminationReason == .exit, process.terminationStatus == 0 else {
      return false
    }
    let stdoutText = String(data: stdoutData, encoding: .utf8) ?? ""
    let stderrText = String(data: stderrData, encoding: .utf8) ?? ""
    let combined = stdoutText + "\n" + stderrText
    return combined.contains("focus-pane-id") && combined.contains("<PANE_ID>")
  }

  /// True when a public `vc-frame` supporting `action focus-pane-id` is installed.
  /// Positive results are memoized by executable and wrapper-target file stamps
  /// so periodic HUD polls do not re-spawn `vc-frame` unless the binary, symlink
  /// target, or referenced wrapper dependency changes on disk. Negative results
  /// are never cached, allowing repaired dependencies to take effect on the next
  /// poll without restarting Codescribe.
  @MainActor
  static func supportsAgentPanelFocus(
    installer: RealAgentBridgeInstaller = RealAgentBridgeInstaller(),
    now: Date = Date()
  ) async -> Bool {
    let homeDirectory = installer.homeDirectory
    let environment = installer.environment
    return await Task.detached(priority: .utility) {
      guard
        let executableURL = vcFrameExecutableURL(
          homeDirectory: homeDirectory,
          environment: environment,
          now: now
        ),
        let stamp = VcFrameExecutableStamp(url: executableURL)
      else {
        return false
      }
      if let cached = await AgentPanelFocusCapabilityCache.shared.value(for: stamp), cached {
        return true
      }
      let supported = probeVcFramePanelFocusCapability(
        executableURL: executableURL,
        environment: environment
      )
      if supported {
        await AgentPanelFocusCapabilityCache.shared.store(true, for: stamp)
        if let postProbeStamp = VcFrameExecutableStamp(url: executableURL), postProbeStamp != stamp
        {
          await AgentPanelFocusCapabilityCache.shared.store(true, for: postProbeStamp)
        }
      } else {
        await AgentPanelFocusCapabilityCache.shared.remove(for: stamp)
      }
      return supported
    }.value
  }

  /// Brings the terminal host application that owns the `vc-frame` pane to the front.
  @MainActor
  static func activateAgentTerminalHost(
    hostPID: Int32?,
    hostBundleID: String?
  ) -> Bool {
    if let hostPID, hostPID > 0,
      let app = NSRunningApplication(processIdentifier: hostPID),
      !app.isTerminated,
      app.activate()
    {
      return true
    }
    if let hostBundleID = hostBundleID?.trimmingCharacters(in: .whitespacesAndNewlines),
      !hostBundleID.isEmpty
    {
      let matches = NSRunningApplication.runningApplications(withBundleIdentifier: hostBundleID)
        .filter { !$0.isTerminated }
      if let app = matches.first, app.activate() {
        return true
      }
    }
    return false
  }
}

private struct VcFrameFileStat: Hashable, Sendable {
  let path: String
  let resolvedPath: String
  let exists: Bool
  let isExecutable: Bool
  let mode: mode_t
  let device: dev_t
  let inode: ino_t
  let size: off_t
  let modifiedSeconds: Int
  let modifiedNanoseconds: Int
  let statusChangedSeconds: Int
  let statusChangedNanoseconds: Int

  init(path: String) {
    let url = URL(fileURLWithPath: path)
    let resolved = url.resolvingSymlinksInPath()
    var statBuf = Darwin.stat()
    if fstatat(AT_FDCWD, resolved.path, &statBuf, 0) == 0 {
      self.path = path
      self.resolvedPath = resolved.path
      self.exists = true
      self.isExecutable = FileManager.default.isExecutableFile(atPath: resolved.path)
      self.mode = statBuf.st_mode
      self.device = statBuf.st_dev
      self.inode = statBuf.st_ino
      self.size = statBuf.st_size
      self.modifiedSeconds = statBuf.st_mtimespec.tv_sec
      self.modifiedNanoseconds = statBuf.st_mtimespec.tv_nsec
      self.statusChangedSeconds = statBuf.st_ctimespec.tv_sec
      self.statusChangedNanoseconds = statBuf.st_ctimespec.tv_nsec
    } else {
      self.path = path
      self.resolvedPath = resolved.path
      self.exists = false
      self.isExecutable = false
      self.mode = 0
      self.device = 0
      self.inode = 0
      self.size = 0
      self.modifiedSeconds = 0
      self.modifiedNanoseconds = 0
      self.statusChangedSeconds = 0
      self.statusChangedNanoseconds = 0
    }
  }
}

private struct VcFrameExecutableStamp: Hashable, Sendable {
  let root: VcFrameFileStat
  let referencedFiles: [VcFrameFileStat]

  init?(url: URL) {
    let rootStat = VcFrameFileStat(path: url.path)
    guard rootStat.exists else { return nil }
    self.root = rootStat

    var visited: Set<String> = [rootStat.path, rootStat.resolvedPath]
    var frontier: [String] = [rootStat.resolvedPath]
    var discovered: [String] = []
    for _ in 0..<2 {
      var nextFrontier: [String] = []
      for scriptPath in frontier {
        for candidate in Self.referencedPathsInWrapperScript(atPath: scriptPath)
        where !visited.contains(candidate) {
          visited.insert(candidate)
          discovered.append(candidate)
          nextFrontier.append(candidate)
        }
      }
      frontier = nextFrontier
      if frontier.isEmpty { break }
    }
    self.referencedFiles = discovered.sorted().map(VcFrameFileStat.init(path:))
  }

  private static func referencedPathsInWrapperScript(atPath path: String) -> [String] {
    var statBuf = Darwin.stat()
    guard fstatat(AT_FDCWD, path, &statBuf, 0) == 0,
      statBuf.st_size > 0, statBuf.st_size <= 65536,
      let data = try? Data(contentsOf: URL(fileURLWithPath: path)),
      let text = String(data: data, encoding: .utf8),
      text.hasPrefix("#!")
    else {
      return []
    }
    let patterns = [
      #"'(/[^'\n]+)'"#,
      #""(/[^"$\n]+)""#,
      #"(?:^|[\s=:])(/[A-Za-z0-9._+@/-]+)"#,
    ]
    let ignoredPrefixes = ["/bin/", "/usr/bin/", "/sbin/", "/usr/sbin/", "/dev/"]
    let ignoredExact: Set<String> = ["/bin", "/usr/bin", "/sbin", "/usr/sbin", "/dev"]
    var results: [String] = []

    let scriptURL = URL(fileURLWithPath: path)
    if scriptURL.deletingLastPathComponent().lastPathComponent == "bin" {
      let siblingLibexec =
        scriptURL
        .deletingLastPathComponent()
        .deletingLastPathComponent()
        .appendingPathComponent("libexec")
        .appendingPathComponent(scriptURL.lastPathComponent)
        .path
      results.append(siblingLibexec)
    }

    for rawLine in text.components(separatedBy: .newlines) {
      let line = rawLine.trimmingCharacters(in: .whitespaces)
      if line.isEmpty || line.hasPrefix("#") { continue }
      let scanSegment: String
      if let redirectRange = line.range(of: #"[0-9]*>>?"#, options: .regularExpression) {
        scanSegment = String(line[..<redirectRange.lowerBound])
      } else {
        scanSegment = line
      }
      let nsRange = NSRange(scanSegment.startIndex..<scanSegment.endIndex, in: scanSegment)
      for pattern in patterns {
        guard let regex = try? NSRegularExpression(pattern: pattern) else { continue }
        for match in regex.matches(in: scanSegment, range: nsRange) {
          guard match.numberOfRanges > 1,
            let range = Range(match.range(at: 1), in: scanSegment)
          else { continue }
          let candidate = String(scanSegment[range])
          guard candidate.count > 1,
            !ignoredExact.contains(candidate),
            !ignoredPrefixes.contains(where: { candidate.hasPrefix($0) }),
            !results.contains(candidate)
          else { continue }
          results.append(candidate)
        }
      }
    }
    return results
  }
}

private actor AgentPanelFocusCapabilityCache {
  static let shared = AgentPanelFocusCapabilityCache()
  private var entries: [VcFrameExecutableStamp: Bool] = [:]

  func value(for stamp: VcFrameExecutableStamp) -> Bool? {
    entries[stamp]
  }

  func store(_ supported: Bool, for stamp: VcFrameExecutableStamp) {
    if entries.count >= 64 {
      entries.removeAll(keepingCapacity: true)
    }
    entries[stamp] = supported
  }

  func remove(for stamp: VcFrameExecutableStamp) {
    entries.removeValue(forKey: stamp)
  }
}

private struct VcFrameShellProfileStamp: Hashable, Sendable {
  let homePath: String
  let zshenv: VcFrameFileStat
  let zprofile: VcFrameFileStat

  init(homeDirectory: URL) {
    let zshenvURL = homeDirectory.appendingPathComponent(".zshenv")
    let zprofileURL = homeDirectory.appendingPathComponent(".zprofile")
    self.homePath = homeDirectory.resolvingSymlinksInPath().path
    self.zshenv = VcFrameFileStat(path: zshenvURL.path)
    self.zprofile = VcFrameFileStat(path: zprofileURL.path)
  }
}

private final class VcFrameShellPathCache: @unchecked Sendable {
  static let shared = VcFrameShellPathCache()

  enum LookupResult: Equatable, Sendable {
    case hit(String)
    case throttledFailure
    case miss
  }

  private enum Entry: Sendable {
    case resolved(String)
    case failed(at: Date)
  }

  private let lock = NSLock()
  private var entries: [VcFrameShellProfileStamp: Entry] = [:]

  func lookup(for stamp: VcFrameShellProfileStamp, now: Date) -> LookupResult {
    lock.lock()
    defer { lock.unlock() }
    guard let entry = entries[stamp] else {
      return .miss
    }
    switch entry {
    case .resolved(let path):
      return .hit(path)
    case .failed(let failedAt):
      if now.timeIntervalSince(failedAt) < RealAgentBridgeInstaller.vcFrameShellFailureCooldown {
        return .throttledFailure
      }
      entries.removeValue(forKey: stamp)
      return .miss
    }
  }

  func storeResolved(_ path: String, for stamp: VcFrameShellProfileStamp) {
    lock.lock()
    defer { lock.unlock() }
    if entries.count >= 64 {
      entries.removeAll(keepingCapacity: true)
    }
    entries[stamp] = .resolved(path)
  }

  func storeFailure(at now: Date, for stamp: VcFrameShellProfileStamp) {
    lock.lock()
    defer { lock.unlock() }
    if entries.count >= 64 {
      entries.removeAll(keepingCapacity: true)
    }
    entries[stamp] = .failed(at: now)
  }

  func remove(for stamp: VcFrameShellProfileStamp) {
    lock.lock()
    defer { lock.unlock() }
    entries.removeValue(forKey: stamp)
  }
}
