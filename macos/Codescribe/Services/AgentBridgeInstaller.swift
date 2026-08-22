import CryptoKit
import Foundation

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

protocol AgentBridgeInstalling {
  func status() -> AgentBridgeInstallationStatus
  func install(selectedClients: Set<AgentBridgeClient>) throws -> AgentBridgeInstallationStatus
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

  enum CodingKeys: String, CodingKey {
    case schema
    case bundleVersion = "bundle_version"
    case managedID = "managed_id"
    case selectedClients = "selected_clients"
    case installedPaths = "installed_paths"
    case runtimePath = "runtime_path"
    case payloadFiles = "payload_files"
    case installedAt = "installed_at"
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
    fileManager: FileManager = .default
  ) {
    self.resourceRoot = resourceRoot
    self.homeDirectory = homeDirectory
    self.fileManager = fileManager
    self.bridgeRoot =
      homeDirectory
      .appendingPathComponent(".codescribe/agent-bridge", isDirectory: true)
    self.runtimeDirectory = bridgeRoot.appendingPathComponent("runtime", isDirectory: true)
    self.receiptURL = bridgeRoot.appendingPathComponent("receipt.json")
  }

  func status() -> AgentBridgeInstallationStatus {
    let manifest: AgentBridgeBundleManifest
    do {
      manifest = try verifiedManifest()
    } catch {
      return .unavailable
    }

    guard let receipt = try? decode(AgentBridgeReceipt.self, from: receiptURL),
      receipt.schema == Self.receiptSchema
    else {
      return AgentBridgeInstallationStatus(
        payloadAvailable: true,
        bundleVersion: manifest.bundleVersion,
        installedClients: [],
        installedPaths: [],
        detail: "Ready to install after you select an agent client."
      )
    }

    let clients = receipt.selectedClients.sorted { $0.rawValue < $1.rawValue }
    let paths = clients.compactMap { receipt.installedPaths[$0.rawValue] }
    return AgentBridgeInstallationStatus(
      payloadAvailable: true,
      bundleVersion: receipt.bundleVersion,
      installedClients: clients,
      installedPaths: paths,
      detail: clients.isEmpty
        ? "No agent client is currently managed by Codescribe."
        : "Installed for \(clients.map(\.displayName).joined(separator: ", "))."
    )
  }

  func install(selectedClients: Set<AgentBridgeClient>) throws -> AgentBridgeInstallationStatus {
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

    let previousReceipt = try? decode(AgentBridgeReceipt.self, from: receiptURL)
    let managedID = previousReceipt?.managedID ?? UUID().uuidString.lowercased()
    let selected = selectedClients.sorted { $0.rawValue < $1.rawValue }
    let previouslySelected = Set(previousReceipt?.selectedClients ?? [])
    let deselected = previouslySelected.subtracting(selectedClients)

    // Conflict discovery is deliberately complete before the first rename.
    for client in selectedClients {
      let destination = client.skillDirectory(home: homeDirectory)
      if fileManager.fileExists(atPath: destination.path) {
        try requireManaged(
          destination: destination,
          client: client,
          managedID: managedID,
          receipt: previousReceipt
        )
      }
    }
    for client in deselected {
      let destination = client.skillDirectory(home: homeDirectory)
      if fileManager.fileExists(atPath: destination.path) {
        try requireManaged(
          destination: destination,
          client: client,
          managedID: managedID,
          receipt: previousReceipt
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

    do {
      try fileManager.copyItem(at: resourceRoot, to: runtimeStage)
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
      let receipt = AgentBridgeReceipt(
        schema: Self.receiptSchema,
        bundleVersion: manifest.bundleVersion,
        managedID: managedID,
        selectedClients: selected,
        installedPaths: installedPaths,
        runtimePath: runtimeDirectory.standardizedFileURL.path,
        payloadFiles: manifest.files,
        installedAt: ISO8601DateFormatter().string(from: Date())
      )
      try writeJSON(receipt, to: receiptURL)
      for record in records where record.backup != nil {
        try? fileManager.removeItem(at: record.backup!)
      }
    } catch {
      rollback(records: records)
      try? fileManager.removeItem(at: runtimeStage)
      for stage in clientStages.values {
        try? fileManager.removeItem(at: stage)
      }
      if let typed = error as? AgentBridgeInstallationError {
        throw typed
      }
      throw AgentBridgeInstallationError.transaction(error.localizedDescription)
    }

    return status()
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

  private func requireManaged(
    destination: URL,
    client: AgentBridgeClient,
    managedID: String,
    receipt: AgentBridgeReceipt?
  ) throws {
    guard let receipt,
      receipt.schema == Self.receiptSchema,
      receipt.managedID == managedID,
      receipt.installedPaths[client.rawValue] == destination.standardizedFileURL.path
    else {
      throw AgentBridgeInstallationError.conflict(
        path: destination.path,
        reason: "the existing skill folder is not present in the Codescribe receipt"
      )
    }
    let markerURL = destination.appendingPathComponent(".codescribe-managed.json")
    guard let marker = try? decode(AgentBridgeManagedMarker.self, from: markerURL),
      marker.schema == Self.markerSchema,
      marker.managedID == managedID,
      marker.client == client,
      marker.agentBridgeRoot == bridgeRoot.standardizedFileURL.path
    else {
      throw AgentBridgeInstallationError.conflict(
        path: destination.path,
        reason: "the Codescribe-managed marker is missing or does not match the receipt"
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
      if let backup {
        try? fileManager.moveItem(at: backup, to: destination)
      }
      throw error
    }
  }

  private func rollback(records: [ReplacementRecord]) {
    for record in records.reversed() {
      if record.installedReplacement, fileManager.fileExists(atPath: record.destination.path) {
        try? fileManager.removeItem(at: record.destination)
      }
      if let backup = record.backup, fileManager.fileExists(atPath: backup.path) {
        try? fileManager.moveItem(at: backup, to: record.destination)
      }
    }
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
