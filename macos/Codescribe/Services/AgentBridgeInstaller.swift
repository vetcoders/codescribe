import CryptoKit
import Darwin
import Foundation
import OSLog

/// Agent clients that can consume the installed Codescribe foundation skill.
/// The raw values are receipt/API tokens and must remain stable.
enum AgentBridgeClient: String, CaseIterable, Codable, Hashable, Identifiable {
  case codex
  case claudeCode = "claude-code"

  var id: String { rawValue }

  var displayName: String {
    switch self {
    case .codex: return "Codex"
    case .claudeCode: return "Claude Code"
    }
  }

  fileprivate func skillDirectory(home: URL) -> URL {
    switch self {
    case .codex:
      return home.appendingPathComponent(".codex/skills/codescribe", isDirectory: true)
    case .claudeCode:
      return home.appendingPathComponent(".claude/skills/codescribe", isDirectory: true)
    }
  }
}

struct AgentBridgeInstallationStatus: Equatable {
  let payloadAvailable: Bool
  let bundleVersion: String?
  let installedClients: [AgentBridgeClient]
  let installedPaths: [String]
  let detail: String

  static let unavailable = AgentBridgeInstallationStatus(
    payloadAvailable: false,
    bundleVersion: nil,
    installedClients: [],
    installedPaths: [],
    detail: String(localized: "The signed app does not contain the agent bridge payload.")
  )
}

struct AgentBridgeAdoptionResult {
  let status: AgentBridgeInstallationStatus
  let backupPaths: [String]
}

protocol AgentBridgeInstalling {
  func status() -> AgentBridgeInstallationStatus
  func install(selectedClients: Set<AgentBridgeClient>) throws -> AgentBridgeInstallationStatus
  func adoptManualSkill(client: AgentBridgeClient) throws -> AgentBridgeAdoptionResult
}

enum AgentBridgeInstallationError: LocalizedError {
  case selectionRequired
  case payloadUnavailable
  case invalidManifest(String)
  case conflict(path: String, reason: String)
  case transaction(String)

  var errorDescription: String? {
    switch self {
    case .selectionRequired:
      return String(
        localized: "Select Codex, Claude Code, or both before installing.",
        comment: "Codex and Claude Code are product names — do not translate"
      )
    case .payloadUnavailable:
      return String(
        localized: "The app bundle does not contain the Codescribe agent bridge payload."
      )
    case .invalidManifest(let reason):
      return String(
        localized: "The bundled agent bridge failed checksum verification: \(reason)",
        comment: "The placeholder is a technical diagnostic produced by the installer"
      )
    case .conflict(let path, let reason):
      return String(
        localized: "Codescribe will not overwrite \(path): \(reason)",
        comment: "First placeholder is a file path, second a technical diagnostic"
      )
    case .transaction(let reason):
      return String(
        localized: "Agent bridge installation could not be completed atomically: \(reason)",
        comment: "The placeholder is a technical diagnostic produced by the installer"
      )
    }
  }
}

private struct AgentBridgeManifestFile: Codable, Equatable {
  let path: String
  let sha256: String
  let bytes: UInt64
  let mode: String
}

private struct AgentBridgeBundleManifest: Codable {
  let schema: String
  let bundleVersion: String
  let helperVersion: String?
  let sourceCommit: String?
  let helper: String
  let skill: String
  let files: [AgentBridgeManifestFile]

  enum CodingKeys: String, CodingKey {
    case schema
    case bundleVersion = "bundle_version"
    case helperVersion = "helper_version"
    case sourceCommit = "source_commit"
    case helper
    case skill
    case files
  }
}

private struct AgentBridgeReceipt: Codable, Equatable {
  let schema: String
  let bundleVersion: String
  let helperVersion: String?
  let sourceCommit: String?
  let managedID: String
  let selectedClients: [AgentBridgeClient]
  let installedPaths: [String: String]
  let runtimePath: String
  let payloadFiles: [AgentBridgeManifestFile]
  let installedAt: String
  let preservedManualBackups: [String]?
  let preservedEntries: [String]?

  enum CodingKeys: String, CodingKey {
    case schema
    case bundleVersion = "bundle_version"
    case helperVersion = "helper_version"
    case sourceCommit = "source_commit"
    case managedID = "managed_id"
    case selectedClients = "selected_clients"
    case installedPaths = "installed_paths"
    case runtimePath = "runtime_path"
    case payloadFiles = "payload_files"
    case installedAt = "installed_at"
    case preservedManualBackups = "preserved_manual_backups"
    case preservedEntries = "preserved_entries"
  }
}

private struct AgentBridgeManagedMarker: Codable {
  let schema: String
  let managedID: String
  let client: AgentBridgeClient
  let agentBridgeRoot: String
  let bundleVersion: String

  enum CodingKeys: String, CodingKey {
    case schema
    case managedID = "managed_id"
    case client
    case agentBridgeRoot = "agent_bridge_root"
    case bundleVersion = "bundle_version"
  }
}

/// Installs the signed bundle payload into a stable runtime root and copies the
/// skill tree into explicitly selected clients. All preflight conflicts are
/// detected before mutation. Payload units swap in place — the runtime
/// directory itself is never renamed, so follower logs and ack cursors keep
/// their inodes — and every rename forms one rollback-capable transaction;
/// receipt replacement is the final commit point.
final class RealAgentBridgeInstaller: AgentBridgeInstalling {
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

  static let bundleSchema = "codescribe.agent-bridge.bundle.v1"
  static let receiptSchema = "codescribe.agent-bridge.receipt.v1"
  static let markerSchema = "codescribe.agent-bridge.managed.v1"
  private static let logger = Logger(
    subsystem: Bundle.main.bundleIdentifier ?? "codescribe",
    category: "agent-bridge-installer"
  )

  private let resourceRoot: URL?
  private let homeDirectory: URL
  private let fileManager: FileManager
  private let bridgeRoot: URL
  private let runtimeDirectory: URL
  private let receiptURL: URL

  init(
    resourceRoot: URL? = Bundle.main.resourceURL?
      .appendingPathComponent("agent-bridge", isDirectory: true),
    homeDirectory: URL = FileManager.default.homeDirectoryForCurrentUser,
    fileManager: FileManager = .default,
    environment: [String: String] = ProcessInfo.processInfo.environment
  ) {
    self.resourceRoot = resourceRoot
    self.homeDirectory = homeDirectory
    self.fileManager = fileManager
    let override = environment["CODESCRIBE_AGENT_BRIDGE_HOME"]?
      .trimmingCharacters(in: .whitespacesAndNewlines)
    self.bridgeRoot =
      override.flatMap { value in
        value.isEmpty
          ? nil
          : URL(
            fileURLWithPath: (value as NSString).expandingTildeInPath,
            isDirectory: true
          ).standardizedFileURL
      }
      ?? homeDirectory
      .appendingPathComponent(".codescribe/agent-bridge", isDirectory: true)
    self.runtimeDirectory = bridgeRoot.appendingPathComponent("runtime", isDirectory: true)
    self.receiptURL = bridgeRoot.appendingPathComponent("receipt.json")
  }

  func status() -> AgentBridgeInstallationStatus {
    let manifest: AgentBridgeBundleManifest
    do {
      manifest = try verifiedManifest()
    } catch {
      Self.logger.error(
        "Agent bridge payload verification failed; installation status is unavailable (error type: \(String(describing: type(of: error)), privacy: .public))"
      )
      return .unavailable
    }

    let receipt = validReceipt()
    var clients: [AgentBridgeClient] = []
    var paths: [String] = []
    var details: [String] = []
    for client in AgentBridgeClient.allCases.sorted(by: { $0.rawValue < $1.rawValue }) {
      let destination = client.skillDirectory(home: homeDirectory)
      let marker = managedMarker(destination: destination, client: client)
      let recorded = receipt?.selectedClients.contains(client) == true
      guard recorded || marker != nil else { continue }
      clients.append(client)
      paths.append(
        marker != nil
          ? destination.standardizedFileURL.path
          : receipt?.installedPaths[client.rawValue] ?? destination.standardizedFileURL.path)
      let evidence: String
      if let marker {
        if let receipt {
          if recorded, receipt.managedID == marker.managedID,
            receipt.installedPaths[client.rawValue] == destination.standardizedFileURL.path
          {
            evidence = String(
              localized: "receipt and managed folder found.",
              comment: "Installer evidence, follows \"<client name>: \" on one line"
            )
          } else {
            evidence = String(
              localized: "managed folder found, receipt differs — Update will re-adopt it.",
              comment: "Update is the button label on the Agent settings panel"
            )
          }
        } else {
          evidence = String(
            localized:
              "managed folder found, receipt missing or unreadable — Update will re-adopt it.",
            comment: "Update is the button label on the Agent settings panel"
          )
        }
      } else {
        evidence = String(
          localized:
            "receipt found, managed folder missing or invalid — existing unowned folders will not be overwritten."
        )
      }
      details.append("\(client.displayName): \(evidence)")
    }
    return AgentBridgeInstallationStatus(
      payloadAvailable: true,
      bundleVersion: receipt?.bundleVersion ?? manifest.bundleVersion,
      installedClients: clients,
      installedPaths: paths,
      detail: details.isEmpty
        ? String(localized: "Ready to install after you select an agent client.")
        : details.joined(separator: "\n")
    )
  }

  func install(selectedClients: Set<AgentBridgeClient>) throws -> AgentBridgeInstallationStatus {
    try install(selectedClients: selectedClients, adopting: nil).status
  }

  /// Explicit source/app install replaces managed payload, including a helper
  /// edited during a live experiment. Startup synchronization stays conservative.
  func installRuntime() throws -> String {
    let selected = Set(status().installedClients)
    _ = try install(
      selectedClients: selected, adopting: nil, runtimeOnly: true,
      initializing: validReceipt() == nil && selected.isEmpty)
    return "Agent bridge runtime and selected client skills installed."
  }

  /// Startup installs the common runtime and updates owned client skills.
  /// Selecting clients and adopting a manual skill remain explicit.
  /// The caller runs this disk work outside the main actor.
  func synchronizeManagedPayload() -> String {
    do {
      return logSynchronization(try installBundledRuntime())
    } catch {
      return logSynchronization(
        "Agent bridge synchronization skipped: " + error.localizedDescription)
    }
  }

  func installBundledRuntime() throws -> String {
    guard let receipt = validReceipt() else {
      guard !fileManager.fileExists(atPath: runtimeDirectory.path),
        (try? runtimeDirectory.resourceValues(forKeys: [.isSymbolicLinkKey]).isSymbolicLink) != true
      else {
        throw AgentBridgeInstallationError.conflict(
          path: runtimeDirectory.path, reason: "unowned runtime retained")
      }
      _ = try install(selectedClients: [], adopting: nil, runtimeOnly: true, initializing: true)
      return "Agent bridge runtime installed; client skills remain unselected."
    }
    try requireSynchronizationOwnership(receipt)
    let manifest = try verifiedManifest()
    if payloadMatches(receipt: receipt, manifest: manifest) {
      try publishCommands(manifest: manifest)
      return "Agent bridge synchronization unchanged: bundled payload matches the managed receipt."
    } else if let reason = automaticReplacementRefusal(receipt: receipt, manifest: manifest) {
      return "Agent bridge synchronization retained the installed runtime: " + reason
        + ". Use an explicit runtime installation to replace it."
    } else {
      _ = try install(
        selectedClients: Set(receipt.selectedClients), adopting: nil,
        synchronizing: receipt, runtimeOnly: receipt.selectedClients.isEmpty
      )
      return "Agent bridge synchronized from this app for "
        + receipt.selectedClients.map(\.displayName).joined(separator: ", ") + "."
    }
  }

  private func logSynchronization(_ detail: String) -> String {
    Self.logger.info("\(detail, privacy: .public)")
    return detail
  }

  /// File order and the app version do not identify a payload generation.
  private func payloadMatches(
    receipt: AgentBridgeReceipt, manifest: AgentBridgeBundleManifest
  ) -> Bool {
    receipt.payloadFiles.sorted { $0.path < $1.path }
      == manifest.files.sorted { $0.path < $1.path }
  }

  /// Startup may upgrade a known generation, but cannot remove published
  /// commands or order different source revisions by the app's version.
  /// Explicit installations remain the user's replacement authority.
  private func automaticReplacementRefusal(
    receipt: AgentBridgeReceipt, manifest: AgentBridgeBundleManifest
  ) -> String? {
    let commands = Set(["bin/cs-bus", "bin/cs-say"])
    let installedCommands = Set(receipt.payloadFiles.map(\.path)).intersection(commands)
    let bundledCommands = Set(manifest.files.map(\.path)).intersection(commands)
    guard installedCommands.isSubset(of: bundledCommands) else {
      return "the bundled payload would remove installed commands"
    }
    guard let installedVersion = receipt.helperVersion else { return nil }
    guard let bundledVersion = manifest.helperVersion else {
      return "the bundled helper has no generation version"
    }
    switch bundledVersion.compare(installedVersion, options: .numeric) {
    case .orderedAscending:
      return "the bundled helper version is older than the installed helper"
    case .orderedSame:
      if let installedCommit = receipt.sourceCommit, manifest.sourceCommit != installedCommit {
        return "different source revisions share the same helper version"
      }
      return nil
    case .orderedDescending:
      return nil
    }
  }

  private func requireSynchronizationOwnership(_ receipt: AgentBridgeReceipt) throws {
    let selected = Set(receipt.selectedClients)
    guard !receipt.managedID.isEmpty, !receipt.bundleVersion.isEmpty,
      selected.count == receipt.selectedClients.count,
      Set(receipt.installedPaths.keys) == Set(selected.map(\.rawValue)),
      receipt.runtimePath == runtimeDirectory.standardizedFileURL.path,
      ISO8601DateFormatter().date(from: receipt.installedAt) != nil,
      !receipt.payloadFiles.isEmpty,
      Set(receipt.payloadFiles.map(\.path)).count == receipt.payloadFiles.count,
      receipt.payloadFiles.allSatisfy({ entry in
        isSafeRelativePath(entry.path)
          && entry.sha256.count == 64
          && entry.sha256.allSatisfy({ $0.isHexDigit })
          && UInt16(entry.mode, radix: 8).map({ $0 <= 0o777 }) == true
      })
    else {
      throw AgentBridgeInstallationError.conflict(
        path: receiptURL.path, reason: "the receipt does not identify this managed installation")
    }
    let homePrefix = homeDirectory.standardizedFileURL.path + "/"
    let expectedRoot: URL
    if bridgeRoot.standardizedFileURL.path.hasPrefix(homePrefix) {
      let relative = String(bridgeRoot.standardizedFileURL.path.dropFirst(homePrefix.count))
      expectedRoot = homeDirectory.resolvingSymlinksInPath().appendingPathComponent(relative)
    } else {
      expectedRoot = bridgeRoot.deletingLastPathComponent().resolvingSymlinksInPath()
        .appendingPathComponent(bridgeRoot.lastPathComponent)
    }
    for (directory, expected) in [
      (bridgeRoot, expectedRoot),
      (runtimeDirectory, expectedRoot.appendingPathComponent("runtime")),
    ] {
      let values = try directory.resourceValues(forKeys: [.isDirectoryKey, .isSymbolicLinkKey])
      guard values.isDirectory == true, values.isSymbolicLink != true,
        directory.resolvingSymlinksInPath().standardizedFileURL == expected.standardizedFileURL
      else {
        throw AgentBridgeInstallationError.conflict(
          path: directory.path, reason: "the managed runtime directory is redirected or missing")
      }
    }
    let receiptValues = try receiptURL.resourceValues(forKeys: [
      .isRegularFileKey, .isSymbolicLinkKey,
    ])
    guard receiptValues.isRegularFile == true, receiptValues.isSymbolicLink != true else {
      throw AgentBridgeInstallationError.conflict(
        path: receiptURL.path, reason: "the receipt is not an ordinary managed file")
    }
    for entry in receipt.payloadFiles {
      let file = runtimeDirectory.appendingPathComponent(entry.path)
      let expected = expectedRoot.appendingPathComponent("runtime").appendingPathComponent(
        entry.path)
      let values = try file.resourceValues(forKeys: [.isRegularFileKey, .isSymbolicLinkKey])
      guard values.isRegularFile == true, values.isSymbolicLink != true,
        file.resolvingSymlinksInPath().standardizedFileURL == expected.standardizedFileURL
      else {
        throw AgentBridgeInstallationError.conflict(
          path: file.path, reason: "the recorded payload file is redirected or missing")
      }
      let data = try Data(contentsOf: file)
      guard UInt64(data.count) == entry.bytes, Self.sha256(data) == entry.sha256.lowercased() else {
        throw AgentBridgeInstallationError.conflict(
          path: file.path, reason: "the runtime payload differs from its managed receipt")
      }
    }
    for client in selected {
      let destination = client.skillDirectory(home: homeDirectory)
      let expected = client.skillDirectory(home: homeDirectory.resolvingSymlinksInPath())
        .standardizedFileURL
      guard receipt.installedPaths[client.rawValue] == destination.standardizedFileURL.path,
        destination.resolvingSymlinksInPath().standardizedFileURL == expected,
        let marker = managedMarker(destination: destination, client: client),
        marker.managedID == receipt.managedID
      else {
        throw AgentBridgeInstallationError.conflict(
          path: destination.path, reason: "the selected folder is not owned by this receipt")
      }
    }
  }

  /// Only an explicit user-confirmed action may replace a manual skill folder.
  /// The original directory is retained after success and restored on failure.
  func adoptManualSkill(client: AgentBridgeClient) throws -> AgentBridgeAdoptionResult {
    let selected = Set(status().installedClients).union([client])
    return try install(selectedClients: selected, adopting: client)
  }

  private func install(
    selectedClients: Set<AgentBridgeClient>, adopting: AgentBridgeClient?,
    synchronizing expectedReceipt: AgentBridgeReceipt? = nil,
    runtimeOnly: Bool = false,
    initializing: Bool = false
  ) throws -> AgentBridgeAdoptionResult {
    guard runtimeOnly || !selectedClients.isEmpty else {
      throw AgentBridgeInstallationError.selectionRequired
    }
    let manifest = try verifiedManifest()
    guard let resourceRoot else {
      throw AgentBridgeInstallationError.payloadUnavailable
    }

    if expectedReceipt == nil {
      try fileManager.createDirectory(
        at: bridgeRoot,
        withIntermediateDirectories: true,
        attributes: [.posixPermissions: 0o700]
      )
      try? fileManager.setAttributes([.posixPermissions: 0o700], ofItemAtPath: bridgeRoot.path)
    }
    let lease = try acquireInstallationLease()
    defer {
      _ = flock(lease, LOCK_UN)
      _ = Darwin.close(lease)
    }

    let previousReceipt = validReceipt()
    if initializing, expectedReceipt == nil,
      previousReceipt != nil || fileManager.fileExists(atPath: runtimeDirectory.path)
        || fileManager.fileExists(atPath: receiptURL.path)
    {
      throw AgentBridgeInstallationError.conflict(
        path: receiptURL.path, reason: "the installation changed before runtime initialization")
    }
    if let expectedReceipt {
      // UI installation may have changed the selection while startup verified
      // the bundle. Never overwrite that newer choice with a stale snapshot.
      guard previousReceipt == expectedReceipt else {
        throw AgentBridgeInstallationError.conflict(
          path: receiptURL.path, reason: "the managed receipt changed during synchronization")
      }
      try requireSynchronizationOwnership(expectedReceipt)
    }
    let managedID = previousReceipt?.managedID ?? UUID().uuidString.lowercased()
    // Capture existing ownership before replacing skill markers or the receipt.
    // A lost receipt can be recovered from this root's validated client markers.
    var ownedCommandIDs = Set(
      AgentBridgeClient.allCases.compactMap { client in
        managedMarker(destination: client.skillDirectory(home: homeDirectory), client: client)?
          .managedID
      }.filter { !$0.isEmpty }
    )
    if let previousReceipt, !previousReceipt.managedID.isEmpty {
      ownedCommandIDs.insert(previousReceipt.managedID)
    }
    let previouslySelected = Set(previousReceipt?.selectedClients ?? [])
    // Adoption is additive, including clients committed before we got the lease.
    let effectiveSelection =
      adopting == nil ? selectedClients : selectedClients.union(previouslySelected)
    let selected = effectiveSelection.sorted { $0.rawValue < $1.rawValue }
    let deselected = previouslySelected.subtracting(effectiveSelection)

    // Conflict discovery is deliberately complete before the first rename.
    try requireCommandOwnership(manifest: manifest, ownedCommandIDs: ownedCommandIDs)
    if let adopting {
      try requireManualSkill(client: adopting)
    }
    for client in effectiveSelection {
      let destination = client.skillDirectory(home: homeDirectory)
      if client != adopting, fileManager.fileExists(atPath: destination.path) {
        try requireManaged(
          destination: destination,
          client: client
        )
      }
    }
    for client in deselected {
      let destination = client.skillDirectory(home: homeDirectory)
      if fileManager.fileExists(atPath: destination.path) {
        try requireManaged(
          destination: destination,
          client: client
        )
      }
    }

    let transactionID = UUID().uuidString.lowercased()
    let runtimeStage = bridgeRoot.appendingPathComponent(
      ".runtime-stage-\(transactionID)",
      isDirectory: true
    )
    var clientStages: [AgentBridgeClient: URL] = [:]
    var records: [ReplacementRecord] = []
    var preservedBackups: [String] = []
    let runtimePreexisted = fileManager.fileExists(atPath: runtimeDirectory.path)
    var oldHelperFollowers: [FollowerProcess] = []

    do {
      try fileManager.copyItem(at: resourceRoot, to: runtimeStage)
      try applyManifestModes(manifest.files, root: runtimeStage)
      let stagedSkill = runtimeStage.appendingPathComponent(manifest.skill, isDirectory: true)
      for client in selected {
        let destination = client.skillDirectory(home: homeDirectory)
        let parent = destination.deletingLastPathComponent()
        try fileManager.createDirectory(at: parent, withIntermediateDirectories: true)
        let stage = parent.appendingPathComponent(
          ".codescribe-stage-\(transactionID)-\(client.rawValue)",
          isDirectory: true
        )
        try fileManager.copyItem(at: stagedSkill, to: stage)
        try applyManifestModes(
          manifest.files,
          root: stage,
          strippingPrefix: manifest.skill + "/"
        )
        let marker = AgentBridgeManagedMarker(
          schema: Self.markerSchema,
          managedID: managedID,
          client: client,
          agentBridgeRoot: bridgeRoot.standardizedFileURL.path,
          bundleVersion: manifest.bundleVersion
        )
        try writeJSON(marker, to: stage.appendingPathComponent(".codescribe-managed.json"))
        clientStages[client] = stage
      }

      // Payload units swap in place, never the runtime directory itself:
      // renaming `runtime/` away strands every open follower log and ack
      // cursor on a deleted inode (2026-09-30 incident), and copying state
      // back after a swap would still hand followers a new inode. Anything
      // outside the manifest keeps its path and inode; state never enters
      // `records`, so rollback cannot touch it either.
      try fileManager.createDirectory(at: runtimeDirectory, withIntermediateDirectories: true)
      // Files an earlier receipt committed but this manifest no longer ships
      // are stale payload, not state: they move into the rollback record and
      // are deleted only after the receipt commit succeeds.
      let stalePayload = stalePayloadFiles(previousReceipt: previousReceipt, manifest: manifest)
      let preservedEntries = collectPreservedEntries(
        manifest: manifest, excludingStalePayload: Set(stalePayload))
      let installedHelper = runtimeDirectory.appendingPathComponent(manifest.helper)
      if let oldHelper = try? Data(contentsOf: installedHelper),
        let bundledHelper = manifest.files.first(where: { $0.path == manifest.helper }),
        Self.sha256(oldHelper) != bundledHelper.sha256.lowercased()
      {
        oldHelperFollowers = liveFollowerProcesses()
      }
      for unit in payloadUnits(manifest: manifest) {
        try replace(
          destination: runtimeDirectory.appendingPathComponent(unit, isDirectory: true),
          with: runtimeStage.appendingPathComponent(unit, isDirectory: true),
          transactionID: transactionID,
          records: &records
        )
      }
      // manifest.json is bundle metadata describing the payload, not state.
      try replace(
        destination: runtimeDirectory.appendingPathComponent("manifest.json"),
        with: runtimeStage.appendingPathComponent("manifest.json"),
        transactionID: transactionID,
        records: &records
      )
      for stale in stalePayload {
        let destination = runtimeDirectory.appendingPathComponent(stale)
        guard fileManager.fileExists(atPath: destination.path) else { continue }
        try replace(
          destination: destination,
          with: nil,
          transactionID: transactionID,
          records: &records
        )
      }
      try? fileManager.removeItem(at: runtimeStage)
      for client in selected {
        guard let stage = clientStages[client] else { continue }
        if client == adopting { try requireManualSkill(client: client) }
        try replace(
          destination: client.skillDirectory(home: homeDirectory),
          with: stage,
          transactionID: transactionID,
          records: &records
        )
      }
      for client in deselected {
        let destination = client.skillDirectory(home: homeDirectory)
        guard fileManager.fileExists(atPath: destination.path) else { continue }
        try replace(
          destination: destination,
          with: nil,
          transactionID: transactionID,
          records: &records
        )
      }

      let installedPaths = Dictionary(
        uniqueKeysWithValues: selected.map {
          ($0.rawValue, $0.skillDirectory(home: homeDirectory).standardizedFileURL.path)
        }
      )
      preservedBackups = records.compactMap { record in
        guard let adopting, record.destination == adopting.skillDirectory(home: homeDirectory)
        else { return nil }
        return record.backup?.path
      }
      let receipt = AgentBridgeReceipt(
        schema: Self.receiptSchema,
        bundleVersion: manifest.bundleVersion,
        helperVersion: manifest.helperVersion,
        sourceCommit: manifest.sourceCommit,
        managedID: managedID,
        selectedClients: selected,
        installedPaths: installedPaths,
        runtimePath: runtimeDirectory.standardizedFileURL.path,
        payloadFiles: manifest.files,
        installedAt: ISO8601DateFormatter().string(from: Date()),
        preservedManualBackups: (previousReceipt?.preservedManualBackups ?? []) + preservedBackups,
        preservedEntries: preservedEntries
      )
      try stageCommands(
        manifest: manifest, managedID: managedID, ownedCommandIDs: ownedCommandIDs,
        transactionID: transactionID, records: &records)
      try writeJSON(receipt, to: receiptURL)
      for follower in oldHelperFollowers where follower.isAlive {
        Self.logger.warning(
          "Agent bridge helper replaced; follower \(follower.leaseID, privacy: .public) pid=\(follower.pid, privacy: .public) is still running the previous helper. It was not stopped."
        )
      }
      for record in records where record.backup != nil {
        if !preservedBackups.contains(record.backup!.path) {
          try? fileManager.removeItem(at: record.backup!)
        }
      }
      // Prune payload directories left empty by stale-file removal.
      for stale in stalePayload {
        var directory = runtimeDirectory.appendingPathComponent(stale).deletingLastPathComponent()
        while directory.standardizedFileURL != runtimeDirectory.standardizedFileURL {
          if (try? fileManager.contentsOfDirectory(atPath: directory.path))?.isEmpty == true {
            try? fileManager.removeItem(at: directory)
          }
          directory = directory.deletingLastPathComponent()
        }
      }
    } catch {
      let recoveryFailures = rollback(records: records)
      try? fileManager.removeItem(at: runtimeStage)
      for stage in clientStages.values {
        try? fileManager.removeItem(at: stage)
      }
      // A runtime directory this transaction created holds no state; once
      // rollback empties it, remove it so a failed first install leaves no trace.
      if !runtimePreexisted {
        pruneEmptyDirectories(under: runtimeDirectory)
        if (try? fileManager.contentsOfDirectory(atPath: runtimeDirectory.path))?.isEmpty == true {
          try? fileManager.removeItem(at: runtimeDirectory)
        }
      }
      if !recoveryFailures.isEmpty {
        let failure = error.localizedDescription
        let preserve = recoveryFailures.joined(separator: "\n")
        throw AgentBridgeInstallationError.transaction(
          String(
            localized:
              "\(failure)\nRollback incomplete. Preserve these paths for recovery:\n\(preserve)",
            comment:
              "First placeholder is the underlying failure, second a newline-separated path list"
          )
        )
      }
      if let typed = error as? AgentBridgeInstallationError {
        throw typed
      }
      throw AgentBridgeInstallationError.transaction(error.localizedDescription)
    }

    return AgentBridgeAdoptionResult(status: status(), backupPaths: preservedBackups)
  }

  private struct FollowerProcess: Decodable {
    let leaseID: String
    let pid: Int32

    enum CodingKeys: String, CodingKey {
      case leaseID = "lease_id"
      case pid
    }

    var isAlive: Bool {
      guard pid > 0 else { return false }
      return Darwin.kill(pid, 0) == 0 || errno == EPERM
    }
  }

  private func liveFollowerProcesses() -> [FollowerProcess] {
    let directory = runtimeDirectory.appendingPathComponent("followers", isDirectory: true)
    guard
      let files = try? fileManager.contentsOfDirectory(
        at: directory, includingPropertiesForKeys: [.isRegularFileKey, .isSymbolicLinkKey]
      )
    else { return [] }
    return files.sorted { $0.lastPathComponent < $1.lastPathComponent }.compactMap { file in
      guard file.pathExtension == "pid",
        let values = try? file.resourceValues(forKeys: [.isRegularFileKey, .isSymbolicLinkKey]),
        values.isRegularFile == true, values.isSymbolicLink != true,
        let follower = try? decode(FollowerProcess.self, from: file), follower.isAlive
      else { return nil }
      return follower
    }
  }

  /// One kernel-owned writer across app processes. Keep the lock file: unlinking
  /// it would allow two writers to lock different inodes at the same path.
  private func acquireInstallationLease() throws -> Int32 {
    let path = bridgeRoot.appendingPathComponent("installation.lock").path
    let descriptor = Darwin.open(path, O_CREAT | O_RDWR | O_NOFOLLOW | O_CLOEXEC, mode_t(0o600))
    guard descriptor >= 0 else {
      throw AgentBridgeInstallationError.transaction(
        String(
          localized: "cannot open the installation lock",
          comment: "Reason clause appended to the atomic-installation failure sentence"
        ))
    }
    var metadata = stat()
    guard Darwin.fstat(descriptor, &metadata) == 0,
      (metadata.st_mode & mode_t(S_IFMT)) == mode_t(S_IFREG),
      flock(descriptor, LOCK_EX | LOCK_NB) == 0
    else {
      _ = Darwin.close(descriptor)
      throw AgentBridgeInstallationError.transaction(
        String(
          localized:
            "another installation is active or its lock is unavailable; try again after it finishes",
          comment: "Reason clause appended to the atomic-installation failure sentence"
        ))
    }
    return descriptor
  }

  private func requireManualSkill(client: AgentBridgeClient) throws {
    let destination = client.skillDirectory(home: homeDirectory)
    // Refuse redirected parents as well as a symlink at the selected folder.
    let expected = client.skillDirectory(home: homeDirectory.resolvingSymlinksInPath())
      .standardizedFileURL
    guard destination.resolvingSymlinksInPath().standardizedFileURL == expected,
      let values = try? destination.resourceValues(forKeys: [.isDirectoryKey, .isSymbolicLinkKey]),
      values.isDirectory == true, values.isSymbolicLink != true,
      !fileManager.fileExists(
        atPath: destination.appendingPathComponent(".codescribe-managed.json").path),
      let skill = try? destination.appendingPathComponent("SKILL.md")
        .resourceValues(forKeys: [.isRegularFileKey, .isSymbolicLinkKey]),
      skill.isRegularFile == true, skill.isSymbolicLink != true
    else {
      throw AgentBridgeInstallationError.conflict(
        path: destination.path,
        reason: String(
          localized:
            "manual adoption requires an ordinary skill folder with SKILL.md and no managed marker",
          comment: "SKILL.md is a file name — keep it verbatim"
        )
      )
    }
  }

  private func verifiedManifest() throws -> AgentBridgeBundleManifest {
    guard let resourceRoot else {
      throw AgentBridgeInstallationError.payloadUnavailable
    }
    let manifestURL = resourceRoot.appendingPathComponent("manifest.json")
    let manifest: AgentBridgeBundleManifest
    do {
      manifest = try decode(AgentBridgeBundleManifest.self, from: manifestURL)
    } catch {
      throw AgentBridgeInstallationError.invalidManifest("manifest.json is missing or unreadable")
    }
    guard manifest.schema == Self.bundleSchema, !manifest.bundleVersion.isEmpty else {
      throw AgentBridgeInstallationError.invalidManifest("schema or bundle version is invalid")
    }
    guard !manifest.files.isEmpty else {
      throw AgentBridgeInstallationError.invalidManifest("the file list is empty")
    }

    var listed = Set<String>()
    for entry in manifest.files {
      guard isSafeRelativePath(entry.path), listed.insert(entry.path).inserted else {
        throw AgentBridgeInstallationError.invalidManifest("unsafe or duplicate path \(entry.path)")
      }
      guard let mode = UInt16(entry.mode, radix: 8), mode <= 0o777 else {
        throw AgentBridgeInstallationError.invalidManifest("invalid mode for \(entry.path)")
      }
      let file = resourceRoot.appendingPathComponent(entry.path)
      var isDirectory: ObjCBool = false
      guard fileManager.fileExists(atPath: file.path, isDirectory: &isDirectory),
        !isDirectory.boolValue
      else {
        throw AgentBridgeInstallationError.invalidManifest("missing file \(entry.path)")
      }
      let values = try? file.resourceValues(forKeys: [.isSymbolicLinkKey])
      guard values?.isSymbolicLink != true else {
        throw AgentBridgeInstallationError.invalidManifest("symlink refused at \(entry.path)")
      }
      let data = try Data(contentsOf: file)
      guard UInt64(data.count) == entry.bytes, Self.sha256(data) == entry.sha256.lowercased()
      else {
        throw AgentBridgeInstallationError.invalidManifest("checksum mismatch for \(entry.path)")
      }
    }

    let actual = try payloadFiles(root: resourceRoot)
    guard actual == listed else {
      let difference = actual.symmetricDifference(listed).sorted().joined(separator: ", ")
      throw AgentBridgeInstallationError.invalidManifest(
        "manifest coverage mismatch: \(difference)")
    }
    guard listed.contains(manifest.helper), listed.contains("\(manifest.skill)/SKILL.md") else {
      throw AgentBridgeInstallationError.invalidManifest("helper or skill entrypoint is missing")
    }
    return manifest
  }

  /// Payload units swapped as a whole: every top-level manifest entry (e.g.
  /// `bin/`) plus each `skills/<name>/` skill tree. State such as `followers/`
  /// or the ack cursors is never a unit, so it is never renamed or removed.
  private func payloadUnits(manifest: AgentBridgeBundleManifest) -> [String] {
    var units = Set<String>()
    for entry in manifest.files {
      let components = entry.path.split(separator: "/")
      guard let first = components.first else { continue }
      if first == "skills", components.count > 1 {
        units.insert("skills/\(components[1])")
      } else {
        units.insert(String(first))
      }
    }
    return units.sorted()
  }

  /// Files an earlier receipt committed that this manifest no longer ships.
  /// They are payload, not state, and their removal stays rollback-capable.
  private func stalePayloadFiles(
    previousReceipt: AgentBridgeReceipt?,
    manifest: AgentBridgeBundleManifest
  ) -> [String] {
    guard let previousReceipt else { return [] }
    let current = Set(manifest.files.map(\.path))
    return previousReceipt.payloadFiles.map(\.path).filter { !current.contains($0) }.sorted()
  }

  /// Relative paths inside `runtime/` that neither this manifest nor the
  /// stale-payload removal owns. They survive installation byte-for-byte and
  /// are echoed into the receipt.
  private func collectPreservedEntries(
    manifest: AgentBridgeBundleManifest,
    excludingStalePayload stale: Set<String>
  ) -> [String] {
    let units = Set(payloadUnits(manifest: manifest))
    guard
      let enumerator = fileManager.enumerator(
        at: runtimeDirectory,
        includingPropertiesForKeys: nil,
        options: []
      )
    else { return [] }
    let prefix = runtimeDirectory.standardizedFileURL.path + "/"
    var preserved: [String] = []
    for case let item as URL in enumerator {
      let absolute = item.standardizedFileURL.path
      guard absolute.hasPrefix(prefix) else { continue }
      let relative = String(absolute.dropFirst(prefix.count))
      guard !isPayloadPath(relative, units: units) else { continue }
      // Stale payload (and directories pruned with it) is removed, not preserved.
      guard !stale.contains(relative),
        !stale.contains(where: { $0.hasPrefix(relative + "/") })
      else { continue }
      preserved.append(relative)
    }
    return preserved.sorted()
  }

  private func isPayloadPath(_ relative: String, units: Set<String>) -> Bool {
    if relative == "manifest.json" { return true }
    let components = relative.split(separator: "/")
    guard let first = components.first else { return false }
    if first == "skills" {
      return components.count == 1 || units.contains("skills/\(components[1])")
    }
    return units.contains(String(first))
  }

  /// Removes empty directories below `root`, deepest first. `root` itself stays.
  private func pruneEmptyDirectories(under root: URL) {
    guard
      let enumerator = fileManager.enumerator(
        at: root,
        includingPropertiesForKeys: [.isDirectoryKey],
        options: []
      )
    else { return }
    var directories: [URL] = []
    for case let item as URL in enumerator {
      if (try? item.resourceValues(forKeys: [.isDirectoryKey]))?.isDirectory == true {
        directories.append(item)
      }
    }
    for directory in directories.sorted(by: { $0.path.count > $1.path.count }) {
      if (try? fileManager.contentsOfDirectory(atPath: directory.path))?.isEmpty == true {
        try? fileManager.removeItem(at: directory)
      }
    }
  }

  private func payloadFiles(root: URL) throws -> Set<String> {
    guard
      let enumerator = fileManager.enumerator(
        at: root,
        includingPropertiesForKeys: [.isRegularFileKey],
        options: []
      )
    else {
      throw AgentBridgeInstallationError.invalidManifest("payload cannot be enumerated")
    }
    var result = Set<String>()
    let rootManifest = root.appendingPathComponent("manifest.json").standardizedFileURL
    for case let file as URL in enumerator {
      let values = try file.resourceValues(forKeys: [.isRegularFileKey])
      guard values.isRegularFile == true, file.standardizedFileURL != rootManifest else {
        continue
      }
      let prefix = root.standardizedFileURL.path + "/"
      let absolute = file.standardizedFileURL.path
      guard absolute.hasPrefix(prefix) else {
        throw AgentBridgeInstallationError.invalidManifest("payload escaped its resource root")
      }
      result.insert(String(absolute.dropFirst(prefix.count)))
    }
    return result
  }

  private func validReceipt() -> AgentBridgeReceipt? {
    guard let receipt = try? decode(AgentBridgeReceipt.self, from: receiptURL),
      receipt.schema == Self.receiptSchema
    else { return nil }
    return receipt
  }

  private func managedMarker(
    destination: URL,
    client: AgentBridgeClient
  ) -> AgentBridgeManagedMarker? {
    let markerURL = destination.appendingPathComponent(".codescribe-managed.json")
    guard
      let values = try? destination.resourceValues(forKeys: [.isDirectoryKey, .isSymbolicLinkKey]),
      values.isDirectory == true, values.isSymbolicLink != true,
      let markerValues = try? markerURL.resourceValues(forKeys: [
        .isRegularFileKey, .isSymbolicLinkKey,
      ]),
      markerValues.isRegularFile == true, markerValues.isSymbolicLink != true,
      let marker = try? decode(AgentBridgeManagedMarker.self, from: markerURL),
      marker.schema == Self.markerSchema,
      marker.client == client,
      marker.agentBridgeRoot == bridgeRoot.standardizedFileURL.path
    else { return nil }
    return marker
  }

  /// Read-only ownership preflight, shared by updates and deselection.
  func requireManaged(destination: URL, client: AgentBridgeClient) throws {
    guard managedMarker(destination: destination, client: client) != nil else {
      throw AgentBridgeInstallationError.conflict(
        path: destination.path,
        reason: String(
          localized:
            "the Codescribe-managed marker is missing or does not match this client and bridge root"
        )
      )
    }
  }

  private func applyManifestModes(
    _ files: [AgentBridgeManifestFile],
    root: URL,
    strippingPrefix prefix: String? = nil
  ) throws {
    for entry in files {
      let relative: String
      if let prefix {
        guard entry.path.hasPrefix(prefix) else { continue }
        relative = String(entry.path.dropFirst(prefix.count))
      } else {
        relative = entry.path
      }
      guard !relative.isEmpty, let mode = UInt16(entry.mode, radix: 8), mode <= 0o777 else {
        throw AgentBridgeInstallationError.invalidManifest("invalid mode for \(entry.path)")
      }
      try fileManager.setAttributes(
        [.posixPermissions: NSNumber(value: mode)],
        ofItemAtPath: root.appendingPathComponent(relative).path
      )
    }
  }

  private struct ReplacementRecord {
    let destination: URL
    let backup: URL?
    let installedReplacement: Bool
  }

  private func commandNames(manifest: AgentBridgeBundleManifest) -> [String] {
    ["cs-bus", "cs-say"].filter { name in
      manifest.files.contains { $0.path == "bin/" + name }
    }
  }

  private func commandURL(_ name: String) -> URL {
    homeDirectory.appendingPathComponent(".local/bin/" + name)
  }

  private static let commandMarker = "# codescribe-managed-command: "

  private func managedCommandID(_ file: URL) -> String? {
    guard let values = try? file.resourceValues(forKeys: [.isRegularFileKey, .isSymbolicLinkKey]),
      values.isRegularFile == true, values.isSymbolicLink != true,
      let text = try? String(contentsOf: file, encoding: .utf8),
      let line = text.split(separator: "\n", maxSplits: 2).dropFirst().first,
      line.hasPrefix(Self.commandMarker),
      let metadata = try? JSONSerialization.jsonObject(
        with: Data(line.dropFirst(Self.commandMarker.count).utf8)) as? [String: Any]
    else { return nil }
    return metadata["managed_id"] as? String
  }

  private func requireCommandOwnership(
    manifest: AgentBridgeBundleManifest, ownedCommandIDs: Set<String>
  ) throws {
    let directory = homeDirectory.appendingPathComponent(".local/bin")
    let expected = homeDirectory.resolvingSymlinksInPath().appendingPathComponent(".local/bin")
    guard directory.resolvingSymlinksInPath().standardizedFileURL == expected.standardizedFileURL
    else {
      throw AgentBridgeInstallationError.conflict(
        path: directory.path, reason: "the command directory is redirected")
    }
    for name in commandNames(manifest: manifest) {
      let destination = commandURL(name)
      let link = try? fileManager.destinationOfSymbolicLink(atPath: destination.path)
      if fileManager.fileExists(atPath: destination.path) || link != nil {
        let ownedLink = link == runtimeDirectory.appendingPathComponent("bin/" + name).path
        let ownedFile =
          link == nil
          && managedCommandID(destination).map {
            ownedCommandIDs.contains($0)
          } == true
        guard ownedLink || ownedFile else {
          throw AgentBridgeInstallationError.conflict(
            path: destination.path, reason: "the command belongs to another installation")
        }
      }
    }
  }

  private func stageCommands(
    manifest: AgentBridgeBundleManifest, managedID: String, ownedCommandIDs: Set<String>,
    transactionID: String,
    records: inout [ReplacementRecord]
  ) throws {
    try requireCommandOwnership(manifest: manifest, ownedCommandIDs: ownedCommandIDs)
    for name in commandNames(manifest: manifest) {
      let destination = commandURL(name)
      // cs-bus receives the complete verified helper, so neither entrypoint
      // depends on the replaceable runtime/bin payload after publication.
      let source = runtimeDirectory.appendingPathComponent(
        name == "cs-bus" ? manifest.helper : "bin/" + name)
      let script = try String(contentsOf: source, encoding: .utf8)
      let body = script.split(separator: "\n", maxSplits: 1, omittingEmptySubsequences: false)
        .dropFirst().joined(separator: "\n")
      var metadata: [String: Any] = [
        "managed_id": managedID, "bundle_version": manifest.bundleVersion,
        "helper_version": manifest.helperVersion ?? manifest.bundleVersion,
      ]
      if let commit = manifest.sourceCommit { metadata["source_commit"] = commit }
      let generation = try JSONSerialization.data(withJSONObject: metadata, options: [.sortedKeys])
      let header =
        "#!/usr/bin/env python3\n" + Self.commandMarker
        + String(decoding: generation, as: UTF8.self) + "\n"
      let data = Data((header + body).utf8)
      if (try? fileManager.destinationOfSymbolicLink(atPath: destination.path)) == nil,
        (try? Data(contentsOf: destination)) == data
      {
        try fileManager.setAttributes([.posixPermissions: 0o755], ofItemAtPath: destination.path)
        continue
      }
      try fileManager.createDirectory(
        at: destination.deletingLastPathComponent(), withIntermediateDirectories: true)
      let stage = destination.deletingLastPathComponent()
        .appendingPathComponent(".\(name).stage-\(transactionID)")
      defer { try? fileManager.removeItem(at: stage) }
      try data.write(to: stage)
      try fileManager.setAttributes([.posixPermissions: 0o755], ofItemAtPath: stage.path)
      try replace(
        destination: destination, with: stage, transactionID: transactionID, records: &records)
    }
  }

  private func publishCommands(manifest: AgentBridgeBundleManifest) throws {
    let lease = try acquireInstallationLease()
    defer {
      _ = flock(lease, LOCK_UN)
      _ = Darwin.close(lease)
    }
    var records: [ReplacementRecord] = []
    do {
      guard let receipt = validReceipt(), payloadMatches(receipt: receipt, manifest: manifest)
      else {
        throw AgentBridgeInstallationError.conflict(
          path: receiptURL.path, reason: "the runtime generation changed while publishing commands")
      }
      try requireSynchronizationOwnership(receipt)
      try stageCommands(
        manifest: manifest, managedID: receipt.managedID, ownedCommandIDs: [receipt.managedID],
        transactionID: UUID().uuidString, records: &records)
      for record in records {
        if let backup = record.backup { try? fileManager.removeItem(at: backup) }
      }
    } catch {
      let failures = rollback(records: records)
      if !failures.isEmpty {
        throw AgentBridgeInstallationError.transaction(failures.joined(separator: "\n"))
      }
      throw error
    }
  }

  private func replace(
    destination: URL,
    with staged: URL?,
    transactionID: String,
    records: inout [ReplacementRecord]
  ) throws {
    let parent = destination.deletingLastPathComponent()
    try fileManager.createDirectory(at: parent, withIntermediateDirectories: true)
    var backup: URL?
    if fileManager.fileExists(atPath: destination.path)
      || (try? fileManager.destinationOfSymbolicLink(atPath: destination.path)) != nil
    {
      let candidate = parent.appendingPathComponent(
        ".\(destination.lastPathComponent).backup-\(transactionID)",
        isDirectory: true
      )
      try fileManager.moveItem(at: destination, to: candidate)
      backup = candidate
    }
    do {
      if let staged {
        try fileManager.moveItem(at: staged, to: destination)
      }
      records.append(
        ReplacementRecord(
          destination: destination,
          backup: backup,
          installedReplacement: staged != nil
        )
      )
    } catch {
      // Preserve this rename in the same rollback record as all prior steps.
      // A failed stage move must not hide a failed restoration of the original.
      records.append(
        ReplacementRecord(
          destination: destination, backup: backup, installedReplacement: false))
      throw error
    }
  }

  private func rollback(records: [ReplacementRecord]) -> [String] {
    var failures: [String] = []
    for record in records.reversed() {
      if record.installedReplacement,
        fileManager.fileExists(atPath: record.destination.path)
          || (try? fileManager.destinationOfSymbolicLink(atPath: record.destination.path)) != nil
      {
        do {
          try fileManager.removeItem(at: record.destination)
        } catch {
          failures.append(
            "Could not remove incomplete replacement at \(record.destination.path). Original: \(record.backup?.path ?? "no prior folder"). \(error.localizedDescription)"
          )
          continue
        }
      }
      if let backup = record.backup {
        do {
          try fileManager.moveItem(at: backup, to: record.destination)
        } catch {
          failures.append(
            "Could not restore \(backup.path) to \(record.destination.path): \(error.localizedDescription)"
          )
        }
      }
    }
    return failures
  }

  private func writeJSON<T: Encodable>(_ value: T, to url: URL) throws {
    let encoder = JSONEncoder()
    encoder.outputFormatting = [.prettyPrinted, .sortedKeys, .withoutEscapingSlashes]
    let data = try encoder.encode(value) + Data([0x0A])
    try data.write(to: url, options: .atomic)
    try? fileManager.setAttributes([.posixPermissions: 0o600], ofItemAtPath: url.path)
  }

  private func decode<T: Decodable>(_ type: T.Type, from url: URL) throws -> T {
    try JSONDecoder().decode(type, from: Data(contentsOf: url))
  }

  private func isSafeRelativePath(_ path: String) -> Bool {
    guard !path.isEmpty, !path.hasPrefix("/") else { return false }
    let components = path.split(separator: "/", omittingEmptySubsequences: false)
    return !components.contains(where: { $0.isEmpty || $0 == "." || $0 == ".." })
  }

  private static func sha256(_ data: Data) -> String {
    SHA256.hash(data: data).map { String(format: "%02x", $0) }.joined()
  }
}
