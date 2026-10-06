import Darwin
import Foundation

/// App-side invocations of the installed managed `cs-bus` command. They live
/// apart from `AgentBridgeInstaller.swift` because they reference app types,
/// and that file is also compiled on its own by `make install-bus`.
extension RealAgentBridgeInstaller {
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
