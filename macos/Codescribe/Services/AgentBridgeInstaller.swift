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
    detail: "The signed app does not contain the agent bridge payload."
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
      return "Select Codex, Claude Code, or both before installing."
    case .payloadUnavailable:
      return "The app bundle does not contain the Codescribe agent bridge payload."
    case .invalidManifest(let reason):
      return "The bundled agent bridge failed checksum verification: \(reason)"
    case .conflict(let path, let reason):
      return "Codescribe will not overwrite \(path): \(reason)"
    case .transaction(let reason):
      return "Agent bridge installation could not be completed atomically: \(reason)"
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
  let helper: String
  let skill: String
  let files: [AgentBridgeManifestFile]

  enum CodingKeys: String, CodingKey {
    case schema
    case bundleVersion = "bundle_version"
    case helper
    case skill
    case files
  }
}

private struct AgentBridgeReceipt: Codable {
  let schema: String
  let bundleVersion: String
  let managedID: String
  let selectedClients: [AgentBridgeClient]
  let installedPaths: [String: String]
  let runtimePath: String
  let payloadFiles: [AgentBridgeManifestFile]
  let installedAt: String
  let preservedManualBackups: [String]?

  enum CodingKeys: String, CodingKey {
    case schema
    case bundleVersion = "bundle_version"
    case managedID = "managed_id"
    case selectedClients = "selected_clients"
    case installedPaths = "installed_paths"
    case runtimePath = "runtime_path"
    case payloadFiles = "payload_files"
    case installedAt = "installed_at"
    case preservedManualBackups = "preserved_manual_backups"
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
/// detected before mutation. Directory renames form one rollback-capable
/// transaction; receipt replacement is the final commit point.
final class RealAgentBridgeInstaller: AgentBridgeInstalling {
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
            evidence = "receipt and managed folder found."
          } else {
            evidence = "managed folder found, receipt differs — Update will re-adopt it."
          }
        } else {
          evidence =
            "managed folder found, receipt missing or unreadable — Update will re-adopt it."
        }
      } else {
        evidence =
          "receipt found, managed folder missing or invalid — existing unowned folders will not be overwritten."
      }
      details.append("\(client.displayName): \(evidence)")
    }
    return AgentBridgeInstallationStatus(
      payloadAvailable: true,
      bundleVersion: receipt?.bundleVersion ?? manifest.bundleVersion,
      installedClients: clients,
      installedPaths: paths,
      detail: details.isEmpty
        ? "Ready to install after you select an agent client."
        : details.joined(separator: "\n")
    )
  }

  func install(selectedClients: Set<AgentBridgeClient>) throws -> AgentBridgeInstallationStatus {
    try install(selectedClients: selectedClients, adopting: nil).status
  }

  /// Only an explicit user-confirmed action may replace a manual skill folder.
  /// The original directory is retained after success and restored on failure.
  func adoptManualSkill(client: AgentBridgeClient) throws -> AgentBridgeAdoptionResult {
    let selected = Set(status().installedClients).union([client])
    return try install(selectedClients: selected, adopting: client)
  }

  private func install(
    selectedClients: Set<AgentBridgeClient>, adopting: AgentBridgeClient?
  ) throws -> AgentBridgeAdoptionResult {
    guard !selectedClients.isEmpty else {
      throw AgentBridgeInstallationError.selectionRequired
    }
    let manifest = try verifiedManifest()
    guard let resourceRoot else {
      throw AgentBridgeInstallationError.payloadUnavailable
    }

    try fileManager.createDirectory(
      at: bridgeRoot,
      withIntermediateDirectories: true,
      attributes: [.posixPermissions: 0o700]
    )
    try? fileManager.setAttributes([.posixPermissions: 0o700], ofItemAtPath: bridgeRoot.path)
    let lease = try acquireInstallationLease()
    defer {
      _ = Darwin.flock(lease, LOCK_UN)
      _ = Darwin.close(lease)
    }

    let previousReceipt = validReceipt()
    let managedID = previousReceipt?.managedID ?? UUID().uuidString.lowercased()
    let previouslySelected = Set(previousReceipt?.selectedClients ?? [])
    // Adoption is additive, including clients committed before we got the lease.
    let effectiveSelection = adopting == nil ? selectedClients : selectedClients.union(previouslySelected)
    let selected = effectiveSelection.sorted { $0.rawValue < $1.rawValue }
    let deselected = previouslySelected.subtracting(effectiveSelection)

    // Conflict discovery is deliberately complete before the first rename.
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

      try replace(
        destination: runtimeDirectory,
        with: runtimeStage,
        transactionID: transactionID,
        records: &records
      )
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
        managedID: managedID,
        selectedClients: selected,
        installedPaths: installedPaths,
        runtimePath: runtimeDirectory.standardizedFileURL.path,
        payloadFiles: manifest.files,
        installedAt: ISO8601DateFormatter().string(from: Date()),
        preservedManualBackups: (previousReceipt?.preservedManualBackups ?? []) + preservedBackups
      )
      try writeJSON(receipt, to: receiptURL)
      for record in records where record.backup != nil {
        if !preservedBackups.contains(record.backup!.path) {
          try? fileManager.removeItem(at: record.backup!)
        }
      }
    } catch {
      let recoveryFailures = rollback(records: records)
      try? fileManager.removeItem(at: runtimeStage)
      for stage in clientStages.values {
        try? fileManager.removeItem(at: stage)
      }
      if !recoveryFailures.isEmpty {
        throw AgentBridgeInstallationError.transaction(
          error.localizedDescription + "\nRollback incomplete. Preserve these paths for recovery:\n"
            + recoveryFailures.joined(separator: "\n")
        )
      }
      if let typed = error as? AgentBridgeInstallationError {
        throw typed
      }
      throw AgentBridgeInstallationError.transaction(error.localizedDescription)
    }

    return AgentBridgeAdoptionResult(status: status(), backupPaths: preservedBackups)
  }

  /// One kernel-owned writer across app processes. Keep the lock file: unlinking
  /// it would allow two writers to lock different inodes at the same path.
  private func acquireInstallationLease() throws -> Int32 {
    let path = bridgeRoot.appendingPathComponent("installation.lock").path
    let descriptor = Darwin.open(path, O_CREAT | O_RDWR | O_NOFOLLOW | O_CLOEXEC, mode_t(0o600))
    guard descriptor >= 0 else {
      throw AgentBridgeInstallationError.transaction("cannot open the installation lock")
    }
    var metadata = stat()
    guard Darwin.fstat(descriptor, &metadata) == 0,
      (metadata.st_mode & mode_t(S_IFMT)) == mode_t(S_IFREG),
      Darwin.flock(descriptor, LOCK_EX | LOCK_NB) == 0
    else {
      _ = Darwin.close(descriptor)
      throw AgentBridgeInstallationError.transaction("another installation is active or its lock is unavailable; try again after it finishes")
    }
    return descriptor
  }

  private func requireManualSkill(client: AgentBridgeClient) throws {
    let destination = client.skillDirectory(home: homeDirectory)
    // Refuse redirected parents as well as a symlink at the selected folder.
    let expected = client.skillDirectory(home: homeDirectory.resolvingSymlinksInPath()).standardizedFileURL
    guard destination.resolvingSymlinksInPath().standardizedFileURL == expected,
      let values = try? destination.resourceValues(forKeys: [.isDirectoryKey, .isSymbolicLinkKey]),
      values.isDirectory == true, values.isSymbolicLink != true,
      !fileManager.fileExists(atPath: destination.appendingPathComponent(".codescribe-managed.json").path),
      let skill = try? destination.appendingPathComponent("SKILL.md")
        .resourceValues(forKeys: [.isRegularFileKey, .isSymbolicLinkKey]),
      skill.isRegularFile == true, skill.isSymbolicLink != true
    else {
      throw AgentBridgeInstallationError.conflict(
        path: destination.path,
        reason: "manual adoption requires an ordinary skill folder with SKILL.md and no managed marker"
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
        reason:
          "the Codescribe-managed marker is missing or does not match this client and bridge root"
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

  private func replace(
    destination: URL,
    with staged: URL?,
    transactionID: String,
    records: inout [ReplacementRecord]
  ) throws {
    let parent = destination.deletingLastPathComponent()
    try fileManager.createDirectory(at: parent, withIntermediateDirectories: true)
    var backup: URL?
    if fileManager.fileExists(atPath: destination.path) {
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
      records.append(ReplacementRecord(
        destination: destination, backup: backup, installedReplacement: false))
      throw error
    }
  }

  private func rollback(records: [ReplacementRecord]) -> [String] {
    var failures: [String] = []
    for record in records.reversed() {
      if record.installedReplacement, fileManager.fileExists(atPath: record.destination.path) {
        do {
          try fileManager.removeItem(at: record.destination)
        } catch {
          failures.append("Could not remove incomplete replacement at \(record.destination.path). Original: \(record.backup?.path ?? "no prior folder"). \(error.localizedDescription)")
          continue
        }
      }
      if let backup = record.backup {
        do {
          try fileManager.moveItem(at: backup, to: record.destination)
        } catch {
          failures.append("Could not restore \(backup.path) to \(record.destination.path): \(error.localizedDescription)")
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
