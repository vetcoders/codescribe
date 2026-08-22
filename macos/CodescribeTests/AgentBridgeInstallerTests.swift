import CryptoKit
import Foundation
import XCTest

@testable import Codescribe

@MainActor
final class AgentBridgeInstallerTests: XCTestCase {
  private var scratch: URL!

  override func setUpWithError() throws {
    scratch = FileManager.default.temporaryDirectory
      .appendingPathComponent("codescribe-agent-bridge-tests-\(UUID().uuidString)")
    try FileManager.default.createDirectory(at: scratch, withIntermediateDirectories: true)
  }

  override func tearDownWithError() throws {
    if let scratch {
      try? FileManager.default.removeItem(at: scratch)
    }
  }

  func testInstallIsExplicitAtomicIdempotentAndSupportsIndependentClients() throws {
    let payload = try makePayload()
    let home = scratch.appendingPathComponent("home", isDirectory: true)
    let bundledHelper = payload.appendingPathComponent("bin/bus-demux.py")
    try FileManager.default.setAttributes(
      [.posixPermissions: 0o600],
      ofItemAtPath: bundledHelper.path
    )
    let installer = RealAgentBridgeInstaller(
      resourceRoot: payload,
      homeDirectory: home,
      environment: [:]
    )

    XCTAssertThrowsError(try installer.install(selectedClients: []))
    XCTAssertFalse(
      FileManager.default.fileExists(
        atPath: home.appendingPathComponent(".codescribe/agent-bridge/receipt.json").path
      )
    )

    let codexOnly = try installer.install(selectedClients: [.codex])
    XCTAssertEqual(codexOnly.installedClients, [.codex])
    let runtimeHelper = home.appendingPathComponent(
      ".codescribe/agent-bridge/runtime/bin/bus-demux.py"
    )
    let codexSkill = home.appendingPathComponent(".codex/skills/codescribe")
    let claudeSkill = home.appendingPathComponent(".claude/skills/codescribe")
    XCTAssertTrue(FileManager.default.isExecutableFile(atPath: runtimeHelper.path))
    XCTAssertTrue(
      FileManager.default.fileExists(atPath: codexSkill.appendingPathComponent("SKILL.md").path))
    XCTAssertFalse(FileManager.default.fileExists(atPath: claudeSkill.path))

    let receiptURL = home.appendingPathComponent(".codescribe/agent-bridge/receipt.json")
    let firstReceipt = try jsonObject(receiptURL)
    let firstManagedID = try XCTUnwrap(firstReceipt["managed_id"] as? String)
    XCTAssertEqual(firstReceipt["bundle_version"] as? String, "9.8.7")

    // Same selection is a content-idempotent reinstall and retains ownership.
    _ = try installer.install(selectedClients: [.codex])
    let secondManagedID = try XCTUnwrap(try jsonObject(receiptURL)["managed_id"] as? String)
    XCTAssertEqual(firstManagedID, secondManagedID)

    let both = try installer.install(selectedClients: [.codex, .claudeCode])
    XCTAssertEqual(Set(both.installedClients), [.codex, .claudeCode])
    XCTAssertTrue(
      FileManager.default.fileExists(atPath: claudeSkill.appendingPathComponent("SKILL.md").path))

    // Deselecting Codex removes only the matching managed folder.
    let claudeOnly = try installer.install(selectedClients: [.claudeCode])
    XCTAssertEqual(claudeOnly.installedClients, [.claudeCode])
    XCTAssertFalse(FileManager.default.fileExists(atPath: codexSkill.path))
    XCTAssertTrue(FileManager.default.fileExists(atPath: claudeSkill.path))
  }

  func testUnownedClientSkillIsVisibleConflictAndNeverMutated() throws {
    let payload = try makePayload()
    let home = scratch.appendingPathComponent("foreign-home", isDirectory: true)
    let destination = home.appendingPathComponent(".codex/skills/codescribe", isDirectory: true)
    try FileManager.default.createDirectory(at: destination, withIntermediateDirectories: true)
    let foreign = destination.appendingPathComponent("FOREIGN.txt")
    try Data("owned by operator\n".utf8).write(to: foreign)
    let installer = RealAgentBridgeInstaller(
      resourceRoot: payload,
      homeDirectory: home,
      environment: [:]
    )

    XCTAssertThrowsError(try installer.install(selectedClients: [.codex])) { error in
      XCTAssertTrue(
        error.localizedDescription.contains("will not overwrite"), error.localizedDescription)
      XCTAssertTrue(
        error.localizedDescription.contains(destination.path), error.localizedDescription)
    }
    XCTAssertEqual(try String(contentsOf: foreign, encoding: .utf8), "owned by operator\n")
    XCTAssertFalse(
      FileManager.default.fileExists(
        atPath: home.appendingPathComponent(".codescribe/agent-bridge/receipt.json").path
      )
    )
  }

  func testDeselectionRefusesFolderWhoseManagedMarkerWasReplaced() throws {
    let payload = try makePayload()
    let home = scratch.appendingPathComponent("marker-home", isDirectory: true)
    let installer = RealAgentBridgeInstaller(
      resourceRoot: payload,
      homeDirectory: home,
      environment: [:]
    )
    _ = try installer.install(selectedClients: [.codex, .claudeCode])
    let codexSkill = home.appendingPathComponent(".codex/skills/codescribe", isDirectory: true)
    let marker = codexSkill.appendingPathComponent(".codescribe-managed.json")
    try FileManager.default.removeItem(at: marker)
    let foreign = codexSkill.appendingPathComponent("FOREIGN.txt")
    try Data("do not delete\n".utf8).write(to: foreign)

    XCTAssertThrowsError(try installer.install(selectedClients: [.claudeCode])) { error in
      XCTAssertTrue(error.localizedDescription.contains("marker"), error.localizedDescription)
    }
    XCTAssertEqual(try String(contentsOf: foreign, encoding: .utf8), "do not delete\n")
    XCTAssertTrue(FileManager.default.fileExists(atPath: codexSkill.path))
  }

  func testChecksumMismatchRefusesBeforeHomeMutation() throws {
    let payload = try makePayload()
    let helper = payload.appendingPathComponent("bin/bus-demux.py")
    try Data("tamper\n".utf8).append(to: helper)
    let home = scratch.appendingPathComponent("tamper-home", isDirectory: true)
    let installer = RealAgentBridgeInstaller(
      resourceRoot: payload,
      homeDirectory: home,
      environment: [:]
    )

    XCTAssertThrowsError(try installer.install(selectedClients: [.codex])) { error in
      XCTAssertTrue(error.localizedDescription.contains("checksum"), error.localizedDescription)
    }
    XCTAssertFalse(FileManager.default.fileExists(atPath: home.path))
  }

  func testBridgeHomeOverrideMatchesTheInstalledFollowerResolver() throws {
    let payload = try makePayload()
    let home = scratch.appendingPathComponent("override-home", isDirectory: true)
    let override = scratch.appendingPathComponent("custom-agent-bridge", isDirectory: true)
    let installer = RealAgentBridgeInstaller(
      resourceRoot: payload,
      homeDirectory: home,
      environment: ["CODESCRIBE_AGENT_BRIDGE_HOME": override.path]
    )

    _ = try installer.install(selectedClients: [.codex])

    XCTAssertTrue(
      FileManager.default.fileExists(atPath: override.appendingPathComponent("receipt.json").path)
    )
    XCTAssertTrue(
      FileManager.default.fileExists(
        atPath: override.appendingPathComponent("runtime/bin/bus-demux.py").path
      )
    )
    XCTAssertFalse(
      FileManager.default.fileExists(
        atPath: home.appendingPathComponent(".codescribe/agent-bridge/receipt.json").path
      )
    )
  }

  func testOnboardingUsesPolishCopyAndNeverInstallsUntilSelectionAndClick() {
    let engine = MockOnboardingEngine(progress: 11)
    engine.mode = "agentic"
    engine.language = .polish
    let bridge = RecordingAgentBridgeInstaller()
    let model = OnboardingViewModel(
      engine: engine,
      hotkeys: MockHotkeysEngine(),
      agentStatus: MockAgentStatusEngine(),
      agentBridge: bridge,
      probe: MockPermissionProbe(.allGranted)
    )

    XCTAssertEqual(bridge.installCalls, [])
    XCTAssertTrue(model.selectedAgentClients.isEmpty)
    XCTAssertTrue(model.agentBridgeUsesPolishCopy)
    XCTAssertTrue(model.agentBridgeExplanation.contains("szkice na żywo"))
    XCTAssertTrue(model.agentBridgeExplanation.contains("transcript_sealed"))

    let fallbackEngine = MockOnboardingEngine(progress: 11)
    fallbackEngine.mode = "agentic"
    fallbackEngine.language = .auto
    let fallbackModel = OnboardingViewModel(
      engine: fallbackEngine,
      hotkeys: MockHotkeysEngine(),
      agentStatus: MockAgentStatusEngine(),
      agentBridge: RecordingAgentBridgeInstaller(),
      probe: MockPermissionProbe(.allGranted)
    )
    XCTAssertFalse(fallbackModel.agentBridgeUsesPolishCopy)
    XCTAssertTrue(fallbackModel.agentBridgeExplanation.contains("live drafts"))
    XCTAssertTrue(fallbackModel.agentBridgeExplanation.contains("transcript_sealed"))

    model.refreshForCurrentStep()
    XCTAssertEqual(bridge.installCalls, [])

    model.toggleAgentClient(.codex)
    model.installAgentBridge()
    XCTAssertEqual(bridge.installCalls, [[.codex]])
    XCTAssertEqual(model.agentBridgeStatus.installedClients, [.codex])
  }

  private func makePayload() throws -> URL {
    let payload = scratch.appendingPathComponent("payload-\(UUID().uuidString)", isDirectory: true)
    let helper = payload.appendingPathComponent("bin/bus-demux.py")
    let skill = payload.appendingPathComponent("skills/codescribe", isDirectory: true)
    try FileManager.default.createDirectory(
      at: helper.deletingLastPathComponent(),
      withIntermediateDirectories: true
    )
    try FileManager.default.createDirectory(at: skill, withIntermediateDirectories: true)
    try Data("#!/usr/bin/env python3\nprint('bridge')\n".utf8).write(to: helper)
    try FileManager.default.setAttributes([.posixPermissions: 0o755], ofItemAtPath: helper.path)
    try Data("---\nname: codescribe\n---\n".utf8).write(
      to: skill.appendingPathComponent("SKILL.md")
    )
    try Data("reference\n".utf8).write(to: skill.appendingPathComponent("README.md"))

    let relativeFiles = [
      "bin/bus-demux.py",
      "skills/codescribe/README.md",
      "skills/codescribe/SKILL.md",
    ]
    let files: [[String: Any]] = try relativeFiles.map { relative in
      let url = payload.appendingPathComponent(relative)
      let data = try Data(contentsOf: url)
      let permissions =
        try FileManager.default.attributesOfItem(atPath: url.path)[.posixPermissions]
        as? NSNumber
      return [
        "path": relative,
        "sha256": sha256(data),
        "bytes": data.count,
        "mode": String(format: "%04o", permissions?.intValue ?? 0o644),
      ]
    }
    let manifest: [String: Any] = [
      "schema": "codescribe.agent-bridge.bundle.v1",
      "bundle_version": "9.8.7",
      "helper": "bin/bus-demux.py",
      "skill": "skills/codescribe",
      "files": files,
    ]
    let manifestData =
      try JSONSerialization.data(
        withJSONObject: manifest,
        options: [.prettyPrinted, .sortedKeys]
      ) + Data([0x0A])
    try manifestData.write(to: payload.appendingPathComponent("manifest.json"))
    return payload
  }

  private func jsonObject(_ url: URL) throws -> [String: Any] {
    try XCTUnwrap(
      try JSONSerialization.jsonObject(with: Data(contentsOf: url)) as? [String: Any]
    )
  }

  private func sha256(_ data: Data) -> String {
    SHA256.hash(data: data).map { String(format: "%02x", $0) }.joined()
  }
}

private final class RecordingAgentBridgeInstaller: AgentBridgeInstalling {
  private(set) var installCalls: [Set<AgentBridgeClient>] = []
  private var current = AgentBridgeInstallationStatus(
    payloadAvailable: true,
    bundleVersion: "9.8.7",
    installedClients: [],
    installedPaths: [],
    detail: "Ready"
  )

  func status() -> AgentBridgeInstallationStatus { current }

  func install(selectedClients: Set<AgentBridgeClient>) throws -> AgentBridgeInstallationStatus {
    installCalls.append(selectedClients)
    let clients = selectedClients.sorted { $0.rawValue < $1.rawValue }
    current = AgentBridgeInstallationStatus(
      payloadAvailable: true,
      bundleVersion: "9.8.7",
      installedClients: clients,
      installedPaths: clients.map { "/tmp/\($0.rawValue)" },
      detail: "Installed"
    )
    return current
  }
}

extension Data {
  fileprivate func append(to url: URL) throws {
    let handle = try FileHandle(forWritingTo: url)
    defer { try? handle.close() }
    try handle.seekToEnd()
    try handle.write(contentsOf: self)
  }
}
