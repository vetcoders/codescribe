import CryptoKit
import Darwin
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

  func testPlaybackIdentityReadsLargeLeasesAndUsesTheirCustomBus() async throws {
    let installer = RealAgentBridgeInstaller(
      resourceRoot: nil, homeDirectory: scratch, environment: [:])
    let candidate = AgentPlaybackIdentity(
      provider: "codex", session: "large-mailbox", bus: scratch.appendingPathComponent("default.jsonl").path)
    let customBus = scratch.appendingPathComponent("custom.jsonl").path
    let lease = installer.bridgeRoot.appendingPathComponent("leases/\(candidate.leaseID).json")
    try FileManager.default.createDirectory(
      at: lease.deletingLastPathComponent(), withIntermediateDirectories: true)
    func writeLease(padding: Int) throws {
      let row: [String: Any] = [
        "schema": "codescribe.agent-bridge.lease.v1", "lease_id": candidate.leaseID,
        "provider": candidate.provider, "provider_session_id": candidate.session,
        "bus": customBus, "pending": [["text": String(repeating: "x", count: padding)]],
      ]
      try JSONSerialization.data(withJSONObject: row).write(to: lease)
    }
    try writeLease(padding: 5 << 20)
    let resolved = await RealAgentBridgeInstaller.boundPlaybackIdentities(
      for: ["2": candidate], installer: installer)
    XCTAssertEqual(resolved["2"], AgentPlaybackIdentity(
      provider: candidate.provider, session: candidate.session, bus: customBus))
    try writeLease(padding: 16 << 20)
    let oversized = await RealAgentBridgeInstaller.boundPlaybackIdentities(
      for: ["2": candidate], installer: installer)
    XCTAssertTrue(oversized.isEmpty)
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

    let none = try installer.install(selectedClients: [])
    XCTAssertTrue(none.installedClients.isEmpty)
    XCTAssertFalse(FileManager.default.fileExists(atPath: codexSkill.path))
    XCTAssertFalse(FileManager.default.fileExists(atPath: claudeSkill.path))
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

  func testOnboardingContinueInstallsChangedSelectionAndDoesNotReinstallOnReturn() {
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
    model.refreshForCurrentStep()
    XCTAssertEqual(bridge.installCalls, [])
    model.toggleAgentClient(.codex)
    XCTAssertEqual(bridge.installCalls, [])
    model.advance()
    XCTAssertEqual(bridge.installCalls, [[.codex]])
    XCTAssertEqual(model.agentBridgeStatus.installedClients, [.codex])
    XCTAssertEqual(model.step, .done)

    model.back()
    XCTAssertEqual(model.step, .agenticReadiness)
    model.advance()
    XCTAssertEqual(model.step, .done)
    XCTAssertEqual(bridge.installCalls, [[.codex]], "An unchanged selection does not write again")
  }

  func testOnboardingBackDoesNotInstallAndEmptyUnchangedSelectionCanContinue() {
    let engine = MockOnboardingEngine(progress: 11)
    engine.mode = "agentic"
    let bridge = RecordingAgentBridgeInstaller()
    let model = OnboardingViewModel(
      engine: engine, hotkeys: MockHotkeysEngine(), agentStatus: MockAgentStatusEngine(),
      agentBridge: bridge, probe: MockPermissionProbe(.allGranted))
    model.toggleAgentClient(.codex)
    model.back()
    XCTAssertEqual(model.step, .hotkeyMode)
    XCTAssertTrue(bridge.installCalls.isEmpty)

    model.advance()
    XCTAssertEqual(model.step, .agenticReadiness)
    XCTAssertTrue(bridge.installCalls.isEmpty)
    model.toggleAgentClient(.codex)
    model.advance()
    XCTAssertEqual(model.step, .done)
    XCTAssertTrue(
      bridge.installCalls.isEmpty, "Skipping all clients on a fresh setup does not write")
  }

  func testOnboardingInstallationFailureRetainsStepAndSelectionForRetry() {
    let engine = MockOnboardingEngine(progress: 11)
    engine.mode = "agentic"
    let bridge = RecordingAgentBridgeInstaller()
    bridge.failInstallation = true
    let model = OnboardingViewModel(
      engine: engine, hotkeys: MockHotkeysEngine(), agentStatus: MockAgentStatusEngine(),
      agentBridge: bridge, probe: MockPermissionProbe(.allGranted))
    model.toggleAgentClient(.claudeCode)
    model.advance()
    XCTAssertEqual(model.step, .agenticReadiness)
    XCTAssertEqual(model.selectedAgentClients, [.claudeCode])
    XCTAssertNotNil(model.agentBridgeError)
    XCTAssertEqual(bridge.installCalls, [[.claudeCode]])

    bridge.failInstallation = false
    model.advance()
    XCTAssertEqual(model.step, .done)
    XCTAssertNil(model.agentBridgeError)
    XCTAssertEqual(model.agentBridgeStatus.installedClients, [.claudeCode])
    XCTAssertEqual(bridge.installCalls, [[.claudeCode], [.claudeCode]])
  }

  func testOnboardingContinueAppliesDeselectionOfTheLastManagedClient() throws {
    let engine = MockOnboardingEngine(progress: 11)
    engine.mode = "agentic"
    let bridge = RecordingAgentBridgeInstaller()
    _ = try bridge.install(selectedClients: [.claudeCode])
    let model = OnboardingViewModel(
      engine: engine, hotkeys: MockHotkeysEngine(), agentStatus: MockAgentStatusEngine(),
      agentBridge: bridge, probe: MockPermissionProbe(.allGranted))
    XCTAssertEqual(model.selectedAgentClients, [.claudeCode])
    model.toggleAgentClient(.claudeCode)
    XCTAssertEqual(
      bridge.installCalls, [[.claudeCode]], "Selection alone does not mutate the installation")
    bridge.failInstallation = true
    model.advance()
    XCTAssertEqual(model.step, .agenticReadiness)
    XCTAssertTrue(model.agentClientShowsError(.claudeCode))
    XCTAssertFalse(
      model.agentClientShowsError(.codex), "Attribute the failure to the installed client")
    bridge.failInstallation = false
    model.advance()
    XCTAssertEqual(model.step, .done)
    XCTAssertEqual(bridge.installCalls, [[.claudeCode], [], []])
    XCTAssertTrue(model.agentBridgeStatus.installedClients.isEmpty)
  }

  func testAddingCodexFailureBelongsToCodexInsteadOfExistingClaude() throws {
    let bridge = RecordingAgentBridgeInstaller()
    _ = try bridge.install(selectedClients: [.claudeCode])
    let engine = MockOnboardingEngine(progress: 11)
    engine.mode = "agentic"
    let model = OnboardingViewModel(
      engine: engine, hotkeys: MockHotkeysEngine(), agentStatus: MockAgentStatusEngine(),
      agentBridge: bridge, probe: MockPermissionProbe(.allGranted))
    model.toggleAgentClient(.codex)
    bridge.failInstallation = true
    model.advance()
    XCTAssertEqual(model.step, .agenticReadiness)
    XCTAssertEqual(model.agentBridgeErrorClient, .codex)
    XCTAssertTrue(model.agentClientShowsError(.codex))
    XCTAssertFalse(model.agentClientShowsError(.claudeCode))
    XCTAssertFalse(model.agentClientNeedsSetup(.claudeCode))
    XCTAssertTrue(model.agentClientNeedsSetup(.codex))
  }

  func testMultipleClientFailureIsSelectionWideWithoutGuessingACard() {
    let bridge = RecordingAgentBridgeInstaller()
    bridge.failInstallation = true
    let engine = MockOnboardingEngine(progress: 11)
    engine.mode = "agentic"
    let model = OnboardingViewModel(
      engine: engine, hotkeys: MockHotkeysEngine(), agentStatus: MockAgentStatusEngine(),
      agentBridge: bridge, probe: MockPermissionProbe(.allGranted))
    model.toggleAgentClient(.codex)
    model.toggleAgentClient(.claudeCode)
    model.advance()
    XCTAssertNotNil(model.agentBridgeError)
    XCTAssertNil(model.agentBridgeErrorClient)
    XCTAssertFalse(model.agentClientShowsError(.codex))
    XCTAssertFalse(model.agentClientShowsError(.claudeCode))
  }

  func testManagedFoldersRecoverFromMissingUnreadableOrForeignReceipt() throws {
    let payload = try makePayload()
    for drift in ["missing", "unreadable", "foreign-id", "foreign-schema"] {
      let home = scratch.appendingPathComponent(drift)
      let root = scratch.appendingPathComponent("bridge-" + drift)
      let installer = RealAgentBridgeInstaller(
        resourceRoot: payload, homeDirectory: home,
        environment: ["CODESCRIBE_AGENT_BRIDGE_HOME": root.path]
      )
      _ = try installer.install(selectedClients: [.codex, .claudeCode])
      let receiptURL = root.appendingPathComponent("receipt.json")
      var receipt = try jsonObject(receiptURL)
      let oldID = try XCTUnwrap(receipt["managed_id"] as? String)
      switch drift {
      case "missing": try FileManager.default.removeItem(at: receiptURL)
      case "unreadable": try Data("invalid json".utf8).write(to: receiptURL)
      default:
        receipt[drift == "foreign-id" ? "managed_id" : "schema"] = "foreign"
        try JSONSerialization.data(withJSONObject: receipt).write(to: receiptURL)
      }
      let skills = [".codex/skills/codescribe", ".claude/skills/codescribe"]
        .map { home.appendingPathComponent($0) }
      for skill in skills {
        try Data("outdated".utf8).write(to: skill.appendingPathComponent("SKILL.md"))
      }
      let status = installer.status()
      XCTAssertEqual(Set(status.installedClients), [.codex, .claudeCode])
      XCTAssertEqual(status.clientsNeedingRepair, [.codex, .claudeCode])
      XCTAssertEqual(Set(status.installedPaths), Set(skills.map(\.path)))
      XCTAssertTrue(status.detail.contains("Update will re-adopt it."), status.detail)
      XCTAssertTrue(status.detail.contains("Claude Code: managed folder found"))
      XCTAssertTrue(status.detail.contains("Codex: managed folder found"))

      let updated = try installer.install(selectedClients: [.codex, .claudeCode])
      XCTAssertEqual(Set(updated.installedClients), [.codex, .claudeCode])
      XCTAssertTrue(updated.clientsNeedingRepair.isEmpty)
      XCTAssertTrue(updated.detail.contains("receipt and managed folder found"))
      let newID = try XCTUnwrap(try jsonObject(receiptURL)["managed_id"] as? String)
      XCTAssertNotEqual(newID, oldID)
      if drift == "foreign-id" { XCTAssertEqual(newID, "foreign") }
      for skill in skills {
        XCTAssertEqual(
          try jsonObject(skill.appendingPathComponent(".codescribe-managed.json"))["managed_id"]
            as? String, newID)
        for file in ["SKILL.md", "README.md"] {
          XCTAssertEqual(
            try Data(contentsOf: skill.appendingPathComponent(file)),
            try Data(contentsOf: payload.appendingPathComponent("skills/codescribe/" + file)))
        }
      }
    }
  }

  func testMissingReceiptDoesNotAdmitACommandWithAnotherManagedIdentity() throws {
    let home = scratch.appendingPathComponent("mismatched-command-identity")
    let root = home.appendingPathComponent(".codescribe/agent-bridge")
    let installer = RealAgentBridgeInstaller(
      resourceRoot: try makePayload(), homeDirectory: home, environment: [:])
    _ = try installer.install(selectedClients: [.codex])
    let receiptURL = root.appendingPathComponent("receipt.json")
    let managedID = try XCTUnwrap(try jsonObject(receiptURL)["managed_id"] as? String)
    let command = home.appendingPathComponent(".local/bin/cs-bus")
    let foreignBytes = Data(
      try String(contentsOf: command, encoding: .utf8)
        .replacingOccurrences(of: managedID, with: "another-installation").utf8)
    try foreignBytes.write(to: command)
    try FileManager.default.removeItem(at: receiptURL)
    let marker = home.appendingPathComponent(".codex/skills/codescribe/.codescribe-managed.json")
    let markerBytes = try Data(contentsOf: marker)
    let helper = root.appendingPathComponent("runtime/bin/bus-demux.py")
    let helperBytes = try Data(contentsOf: helper)

    XCTAssertThrowsError(try installer.install(selectedClients: [.codex]))
    XCTAssertEqual(try Data(contentsOf: command), foreignBytes)
    XCTAssertEqual(try Data(contentsOf: marker), markerBytes)
    XCTAssertEqual(try Data(contentsOf: helper), helperBytes)
    XCTAssertFalse(FileManager.default.fileExists(atPath: receiptURL.path))
  }

  func testReceiptEvidenceSurvivesMissingFoldersAndUpdateRestoresThem() throws {
    let payload = try makePayload()
    let home = scratch.appendingPathComponent("receipt-only")
    let installer = RealAgentBridgeInstaller(
      resourceRoot: payload, homeDirectory: home, environment: [:])
    _ = try installer.install(selectedClients: [.codex])
    let receiptURL = home.appendingPathComponent(".codescribe/agent-bridge/receipt.json")
    let managedID = try jsonObject(receiptURL)["managed_id"] as? String
    try FileManager.default.removeItem(at: home.appendingPathComponent(".codex/skills/codescribe"))
    XCTAssertEqual(installer.status().installedClients, [.codex])
    XCTAssertEqual(installer.status().clientsNeedingRepair, [.codex])
    XCTAssertTrue(
      installer.status().detail.contains("receipt found, managed folder missing or invalid"))
    _ = try installer.install(selectedClients: [.codex])
    XCTAssertTrue(installer.status().clientsNeedingRepair.isEmpty)
    XCTAssertEqual(try jsonObject(receiptURL)["managed_id"] as? String, managedID)
  }

  func testOnboardingContinueRepairsManagedEvidenceAndHealthyRetryDoesNotWrite() throws {
    let payload = try makePayload()
    for drift in ["missing-receipt", "unreadable-receipt", "missing-folder"] {
      let home = scratch.appendingPathComponent("onboarding-repair-" + drift)
      let installer = RealAgentBridgeInstaller(
        resourceRoot: payload, homeDirectory: home, environment: [:])
      _ = try installer.install(selectedClients: [.codex])
      let receiptURL = home.appendingPathComponent(".codescribe/agent-bridge/receipt.json")
      let folder = home.appendingPathComponent(".codex/skills/codescribe")
      switch drift {
      case "missing-receipt": try FileManager.default.removeItem(at: receiptURL)
      case "unreadable-receipt": try Data("invalid json".utf8).write(to: receiptURL)
      default: try FileManager.default.removeItem(at: folder)
      }
      let engine = MockOnboardingEngine(progress: 11)
      engine.mode = "agentic"
      let model = OnboardingViewModel(
        engine: engine, hotkeys: MockHotkeysEngine(), agentStatus: MockAgentStatusEngine(),
        agentBridge: installer, probe: MockPermissionProbe(.allGranted))
      XCTAssertEqual(model.selectedAgentClients, [.codex])
      XCTAssertEqual(model.agentBridgeStatus.clientsNeedingRepair, [.codex])
      model.advance()
      XCTAssertEqual(model.step, .done)
      XCTAssertNil(model.agentBridgeError)
      XCTAssertTrue(model.agentBridgeStatus.clientsNeedingRepair.isEmpty)
      XCTAssertTrue(
        FileManager.default.fileExists(atPath: folder.appendingPathComponent("SKILL.md").path))
      let repairedReceipt = try Data(contentsOf: receiptURL)
      let repairedInode = try fileNumber(receiptURL)
      model.back()
      model.advance()
      XCTAssertEqual(model.step, .done)
      XCTAssertEqual(try Data(contentsOf: receiptURL), repairedReceipt)
      XCTAssertEqual(
        try fileNumber(receiptURL), repairedInode, "Healthy unchanged selection is read-only")
    }
  }

  func testOnboardingRepairRefusesAnUnownedReplacementWithoutWriting() throws {
    let payload = try makePayload()
    let home = scratch.appendingPathComponent("onboarding-repair-unowned")
    let installer = RealAgentBridgeInstaller(
      resourceRoot: payload, homeDirectory: home, environment: [:])
    _ = try installer.install(selectedClients: [.codex])
    let receiptURL = home.appendingPathComponent(".codescribe/agent-bridge/receipt.json")
    let receipt = try Data(contentsOf: receiptURL)
    let folder = home.appendingPathComponent(".codex/skills/codescribe")
    try FileManager.default.removeItem(at: folder)
    try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
    let original = Data("user-maintained instructions".utf8)
    let skill = folder.appendingPathComponent("SKILL.md")
    try original.write(to: skill)
    let engine = MockOnboardingEngine(progress: 11)
    engine.mode = "agentic"
    let model = OnboardingViewModel(
      engine: engine, hotkeys: MockHotkeysEngine(), agentStatus: MockAgentStatusEngine(),
      agentBridge: installer, probe: MockPermissionProbe(.allGranted))
    model.advance()
    XCTAssertEqual(model.step, .agenticReadiness)
    XCTAssertNotNil(model.agentBridgeError)
    XCTAssertEqual(try Data(contentsOf: skill), original)
    XCTAssertEqual(try Data(contentsOf: receiptURL), receipt)
  }

  func testManagedSkillContentDamageRequiresRepairAndContinueRestoresIt() throws {
    let payload = try makePayload(extraFiles: ["skills/codescribe/scripts/fixture.py": "fixture\n"])
    for drift in ["missing-skill", "changed-readme", "missing-script", "changed-mode"] {
      let home = scratch.appendingPathComponent("content-repair-" + drift)
      let installer = RealAgentBridgeInstaller(
        resourceRoot: payload, homeDirectory: home, environment: [:])
      _ = try installer.install(selectedClients: [.codex])
      XCTAssertTrue(installer.status().clientsNeedingRepair.isEmpty)
      let folder = home.appendingPathComponent(".codex/skills/codescribe")
      switch drift {
      case "missing-skill":
        try FileManager.default.removeItem(at: folder.appendingPathComponent("SKILL.md"))
      case "changed-readme":
        try Data("corrupted instructions\n".utf8).write(
          to: folder.appendingPathComponent("README.md"))
      case "missing-script":
        try FileManager.default.removeItem(at: folder.appendingPathComponent("scripts/fixture.py"))
      default:
        try FileManager.default.setAttributes(
          [.posixPermissions: 0o600], ofItemAtPath: folder.appendingPathComponent("SKILL.md").path)
      }
      let receiptURL = home.appendingPathComponent(".codescribe/agent-bridge/receipt.json")
      let receiptBeforeInspection = try Data(contentsOf: receiptURL)
      let inodeBeforeInspection = try fileNumber(receiptURL)
      XCTAssertEqual(installer.status().clientsNeedingRepair, [.codex], drift)
      XCTAssertEqual(try Data(contentsOf: receiptURL), receiptBeforeInspection)
      XCTAssertEqual(try fileNumber(receiptURL), inodeBeforeInspection, "Inspection is passive")
      let engine = MockOnboardingEngine(progress: 11)
      engine.mode = "agentic"
      let model = OnboardingViewModel(
        engine: engine, hotkeys: MockHotkeysEngine(), agentStatus: MockAgentStatusEngine(),
        agentBridge: installer, probe: MockPermissionProbe(.allGranted))
      XCTAssertTrue(model.agentClientNeedsSetup(.codex), drift)
      model.advance()
      XCTAssertEqual(model.step, .done, drift)
      XCTAssertNil(model.agentBridgeError, drift)
      XCTAssertTrue(installer.status().clientsNeedingRepair.isEmpty, drift)
      for path in ["SKILL.md", "README.md", "scripts/fixture.py"] {
        XCTAssertEqual(
          try Data(contentsOf: folder.appendingPathComponent(path)),
          try Data(contentsOf: payload.appendingPathComponent("skills/codescribe/" + path)),
          path)
      }
      let repairedReceipt = try Data(contentsOf: receiptURL)
      let repairedInode = try fileNumber(receiptURL)
      model.back()
      model.advance()
      XCTAssertEqual(try Data(contentsOf: receiptURL), repairedReceipt)
      XCTAssertEqual(try fileNumber(receiptURL), repairedInode)
    }
  }

  func testIntactInstalledSkillUsesReceiptTruthWhenSameVersionBundleChanges() throws {
    let originalPayload = try makePayload()
    let home = scratch.appendingPathComponent("same-version-skill")
    let originalInstaller = RealAgentBridgeInstaller(
      resourceRoot: originalPayload, homeDirectory: home, environment: [:])
    _ = try originalInstaller.install(selectedClients: [.codex])
    let nextPayload = try makePayload(
      skillContent: "---\nname: codescribe\n---\nnew instructions\n")
    let nextInstaller = RealAgentBridgeInstaller(
      resourceRoot: nextPayload, homeDirectory: home, environment: [:])
    let receiptURL = home.appendingPathComponent(".codescribe/agent-bridge/receipt.json")
    let originalReceipt = try Data(contentsOf: receiptURL)
    XCTAssertTrue(nextInstaller.status().clientsNeedingRepair.isEmpty)
    XCTAssertEqual(try Data(contentsOf: receiptURL), originalReceipt)
    XCTAssertEqual(
      try Data(contentsOf: home.appendingPathComponent(".codex/skills/codescribe/SKILL.md")),
      try Data(contentsOf: originalPayload.appendingPathComponent("skills/codescribe/SKILL.md")))
  }

  func testEmptySelectionCanForgetAnAlreadyMissingManagedFolderWithoutTouchingRuntimeState() throws
  {
    let payload = try makePayload()
    for shape in ["leaf", "parents", "alias-leaf", "alias-parents"] {
      let realHome = scratch.appendingPathComponent("missing-last-client-" + shape)
      var home = realHome
      if shape.hasPrefix("alias-") {
        try FileManager.default.createDirectory(at: realHome, withIntermediateDirectories: true)
        // This witness starts from an existing managed installation; keep the
        // command-directory setup independent of the missing client path.
        try FileManager.default.createDirectory(
          at: realHome.appendingPathComponent(".local/bin", isDirectory: true),
          withIntermediateDirectories: true)
        home = scratch.appendingPathComponent("home-link-" + shape)
        try FileManager.default.createSymbolicLink(at: home, withDestinationURL: realHome)
      }
      let installer = RealAgentBridgeInstaller(
        resourceRoot: payload, homeDirectory: home, environment: [:])
      _ = try installer.install(selectedClients: [.codex])
      let folder = home.appendingPathComponent(".codex/skills/codescribe")
      let removed = shape.hasSuffix("parents") ? home.appendingPathComponent(".codex") : folder
      try FileManager.default.removeItem(at: removed)
      let runtime = home.appendingPathComponent(".codescribe/agent-bridge/runtime")
      let state = runtime.appendingPathComponent("operator-state.txt")
      let preserved = Data("retained runtime state\n".utf8)
      try preserved.write(to: state)
      let runtimeInode = try fileNumber(runtime)
      let engine = MockOnboardingEngine(progress: 11)
      engine.mode = "agentic"
      let model = OnboardingViewModel(
        engine: engine, hotkeys: MockHotkeysEngine(), agentStatus: MockAgentStatusEngine(),
        agentBridge: installer, probe: MockPermissionProbe(.allGranted))
      XCTAssertEqual(model.selectedAgentClients, [.codex], shape)
      model.toggleAgentClient(.codex)
      model.advance()
      XCTAssertEqual(model.step, .done, shape)
      XCTAssertNil(model.agentBridgeError, shape)
      XCTAssertTrue(installer.status().installedClients.isEmpty, shape)
      let receipt = try jsonObject(
        home.appendingPathComponent(".codescribe/agent-bridge/receipt.json"))
      XCTAssertEqual(receipt["selected_clients"] as? [String], [], shape)
      XCTAssertFalse(FileManager.default.fileExists(atPath: folder.path), shape)
      XCTAssertEqual(try Data(contentsOf: state), preserved, shape)
      XCTAssertEqual(try fileNumber(runtime), runtimeInode, shape)
    }
  }

  func testEmptySelectionStillRefusesAnExistingFolderWithoutItsMarker() throws {
    let payload = try makePayload()
    let home = scratch.appendingPathComponent("unmarked-last-client")
    let installer = RealAgentBridgeInstaller(
      resourceRoot: payload, homeDirectory: home, environment: [:])
    _ = try installer.install(selectedClients: [.codex])
    let folder = home.appendingPathComponent(".codex/skills/codescribe")
    try FileManager.default.removeItem(
      at: folder.appendingPathComponent(".codescribe-managed.json"))
    let personalFile = folder.appendingPathComponent("operator.txt")
    let personalBytes = Data("do not remove\n".utf8)
    try personalBytes.write(to: personalFile)
    let receiptURL = home.appendingPathComponent(".codescribe/agent-bridge/receipt.json")
    let receipt = try Data(contentsOf: receiptURL)
    XCTAssertThrowsError(try installer.install(selectedClients: []))
    XCTAssertEqual(try Data(contentsOf: receiptURL), receipt)
    XCTAssertEqual(try Data(contentsOf: personalFile), personalBytes)
  }

  func testEmptySelectionStillRefusesABrokenSymlinkAtTheMissingFolderPath() throws {
    let payload = try makePayload()
    let home = scratch.appendingPathComponent("symlink-last-client")
    let installer = RealAgentBridgeInstaller(
      resourceRoot: payload, homeDirectory: home, environment: [:])
    _ = try installer.install(selectedClients: [.codex])
    let folder = home.appendingPathComponent(".codex/skills/codescribe")
    try FileManager.default.removeItem(at: folder)
    let outside = scratch.appendingPathComponent("nonexistent-outside")
    try FileManager.default.createSymbolicLink(at: folder, withDestinationURL: outside)
    let receiptURL = home.appendingPathComponent(".codescribe/agent-bridge/receipt.json")
    let receipt = try Data(contentsOf: receiptURL)
    XCTAssertThrowsError(try installer.install(selectedClients: []))
    XCTAssertEqual(try Data(contentsOf: receiptURL), receipt)
    XCTAssertEqual(
      try FileManager.default.destinationOfSymbolicLink(atPath: folder.path), outside.path)
  }

  func testEmptySelectionDoesNotTreatAnUnreadableClientPathAsMissing() throws {
    let payload = try makePayload()
    let home = scratch.appendingPathComponent("unreadable-last-client")
    let installer = RealAgentBridgeInstaller(
      resourceRoot: payload, homeDirectory: home, environment: [:])
    _ = try installer.install(selectedClients: [.codex])
    let parent = home.appendingPathComponent(".codex/skills")
    let receiptURL = home.appendingPathComponent(".codescribe/agent-bridge/receipt.json")
    let receipt = try Data(contentsOf: receiptURL)
    try FileManager.default.setAttributes([.posixPermissions: 0o000], ofItemAtPath: parent.path)
    defer {
      try? FileManager.default.setAttributes([.posixPermissions: 0o755], ofItemAtPath: parent.path)
    }
    XCTAssertThrowsError(try installer.install(selectedClients: []))
    XCTAssertEqual(try Data(contentsOf: receiptURL), receipt)
  }

  func testEmptySelectionRefusesAMissingDestinationRedirectedThroughItsParent() throws {
    let payload = try makePayload()
    let home = scratch.appendingPathComponent("redirected-missing-client")
    let installer = RealAgentBridgeInstaller(
      resourceRoot: payload, homeDirectory: home, environment: [:])
    _ = try installer.install(selectedClients: [.codex])
    let parent = home.appendingPathComponent(".codex/skills")
    try FileManager.default.removeItem(at: parent)
    let outside = scratch.appendingPathComponent("outside-client-parent")
    try FileManager.default.createDirectory(at: outside, withIntermediateDirectories: true)
    let personalFile = outside.appendingPathComponent("operator.txt")
    let personalBytes = Data("outside content\n".utf8)
    try personalBytes.write(to: personalFile)
    try FileManager.default.createSymbolicLink(at: parent, withDestinationURL: outside)
    let receiptURL = home.appendingPathComponent(".codescribe/agent-bridge/receipt.json")
    let receipt = try Data(contentsOf: receiptURL)
    XCTAssertThrowsError(try installer.install(selectedClients: []))
    XCTAssertEqual(try Data(contentsOf: receiptURL), receipt)
    XCTAssertEqual(try Data(contentsOf: personalFile), personalBytes)
    XCTAssertEqual(
      try FileManager.default.destinationOfSymbolicLink(atPath: parent.path), outside.path)
  }

  func testLaunchSynchronizationStillRefusesAMissingSelectedFolderWithoutWriting() throws {
    let payload = try makePayload()
    let home = scratch.appendingPathComponent("missing-startup-client")
    let installer = RealAgentBridgeInstaller(
      resourceRoot: payload, homeDirectory: home, environment: [:])
    _ = try installer.install(selectedClients: [.codex])
    let folder = home.appendingPathComponent(".codex/skills/codescribe")
    try FileManager.default.removeItem(at: folder)
    let receiptURL = home.appendingPathComponent(".codescribe/agent-bridge/receipt.json")
    let receipt = try Data(contentsOf: receiptURL)
    let inode = try fileNumber(receiptURL)
    let helper = home.appendingPathComponent(".codescribe/agent-bridge/runtime/bin/bus-demux.py")
    let helperBytes = try Data(contentsOf: helper)
    XCTAssertTrue(installer.synchronizeManagedPayload().contains("skipped"))
    XCTAssertEqual(try Data(contentsOf: receiptURL), receipt)
    XCTAssertEqual(try fileNumber(receiptURL), inode)
    XCTAssertEqual(try Data(contentsOf: helper), helperBytes)
    XCTAssertFalse(FileManager.default.fileExists(atPath: folder.path))
  }

  func testLateDeselectionRefusesAForeignManagedIdentityRestoredDuringStaging() throws {
    let payload = try makePayload()
    let home = scratch.appendingPathComponent("late-foreign-client")
    let initialInstaller = RealAgentBridgeInstaller(
      resourceRoot: payload, homeDirectory: home, environment: [:])
    _ = try initialInstaller.install(selectedClients: [.codex])
    let folder = home.appendingPathComponent(".codex/skills/codescribe")
    let markerURL = folder.appendingPathComponent(".codescribe-managed.json")
    var restoredMarker = try jsonObject(markerURL)
    restoredMarker["managed_id"] = "another-managed-generation"
    let restoredMarkerBytes = try JSONSerialization.data(withJSONObject: restoredMarker)
    try FileManager.default.removeItem(at: folder)
    let receiptURL = home.appendingPathComponent(".codescribe/agent-bridge/receipt.json")
    let receipt = try Data(contentsOf: receiptURL)
    let inode = try fileNumber(receiptURL)
    let fileManager = RestoringForeignFolderFileManager(
      folder: folder, marker: restoredMarkerBytes)
    let installer = RealAgentBridgeInstaller(
      resourceRoot: payload, homeDirectory: home, fileManager: fileManager, environment: [:])
    XCTAssertThrowsError(try installer.install(selectedClients: []))
    XCTAssertTrue(fileManager.didRestore, "The foreign folder appears only after preflight")
    XCTAssertEqual(try Data(contentsOf: receiptURL), receipt)
    XCTAssertEqual(try fileNumber(receiptURL), inode)
    XCTAssertEqual(try Data(contentsOf: markerURL), restoredMarkerBytes)
    XCTAssertEqual(
      try Data(contentsOf: folder.appendingPathComponent("operator.txt")),
      Data("restored foreign content\n".utf8))
  }

  func testInvalidOwnershipMarkersRefuseUpdateWithoutMutation() throws {
    let payload = try makePayload()
    for field in ["schema", "client", "agent_bridge_root"] {
      let home = scratch.appendingPathComponent(field)
      let installer = RealAgentBridgeInstaller(
        resourceRoot: payload, homeDirectory: home, environment: [:])
      _ = try installer.install(selectedClients: [.codex])
      let destination = home.appendingPathComponent(".codex/skills/codescribe")
      let markerURL = destination.appendingPathComponent(".codescribe-managed.json")
      var marker = try jsonObject(markerURL)
      marker[field] = field == "client" ? "claude-code" : "foreign"
      let markerData = try JSONSerialization.data(withJSONObject: marker)
      try markerData.write(to: markerURL)
      let receiptURL = home.appendingPathComponent(".codescribe/agent-bridge/receipt.json")
      try FileManager.default.removeItem(at: receiptURL)
      let skillURL = destination.appendingPathComponent("SKILL.md")
      let skillData = try Data(contentsOf: skillURL)
      XCTAssertTrue(installer.status().installedClients.isEmpty)
      XCTAssertThrowsError(try installer.install(selectedClients: [.codex]))
      XCTAssertEqual(try Data(contentsOf: markerURL), markerData)
      XCTAssertEqual(try Data(contentsOf: skillURL), skillData)
      XCTAssertFalse(FileManager.default.fileExists(atPath: receiptURL.path))
    }
  }

  func testDirectDiagnosticsRefreshesInstalledStateAndLaunchChangesWithoutWriting() throws {
    let payload = try makePayload()
    let home = scratch.appendingPathComponent("diagnostics-home")
    let installer = RealAgentBridgeInstaller(
      resourceRoot: payload, homeDirectory: home, environment: [:])
    _ = try installer.install(selectedClients: [.codex])
    let receiptURL = home.appendingPathComponent(".codescribe/agent-bridge/receipt.json")
    let initialReceipt = try Data(contentsOf: receiptURL)
    let model = SettingsViewModel(
      creatorAgentBridge: installer,
      permissionProbe: MockPermissionProbe(.allGranted),
      servingStatusProvider: { nil }
    )
    model.select(SettingsTab.agentStatus)
    model.refresh()
    XCTAssertEqual(model.currentTab, .agentStatus)
    XCTAssertEqual(model.creatorAgentBridgeStatus.installedClients, [.codex])
    XCTAssertFalse(model.creatorAgentBridgeStatus.installedPaths.isEmpty)
    XCTAssertEqual(try Data(contentsOf: receiptURL), initialReceipt)

    // A direct link must refresh even when Diagnostics is already selected.
    _ = try installer.install(selectedClients: [.codex, .claudeCode])
    let revisitedReceipt = try Data(contentsOf: receiptURL)
    XCTAssertEqual(model.creatorAgentBridgeStatus.installedClients, [.codex])
    model.select(SettingsTab.agentStatus)
    XCTAssertEqual(Set(model.creatorAgentBridgeStatus.installedClients), [.codex, .claudeCode])
    XCTAssertEqual(try Data(contentsOf: receiptURL), revisitedReceipt)

    // Launch synchronization changes disk state without visiting Creator.
    _ = try installer.install(selectedClients: [.claudeCode])
    let synchronizedReceipt = try Data(contentsOf: receiptURL)
    NotificationCenter.default.post(
      name: SettingsViewModel.agentBridgeLaunchSynchronizationDidFinish, object: nil)
    XCTAssertEqual(model.creatorAgentBridgeStatus.installedClients, [.claudeCode])
    XCTAssertEqual(try Data(contentsOf: receiptURL), synchronizedReceipt)
  }

  func testCreatorInspectionIsPassiveAndInstallingOneClientPreservesTheOther() throws {
    let payload = try makePayload()
    let home = scratch.appendingPathComponent("creator-home")
    let installer = RealAgentBridgeInstaller(
      resourceRoot: payload, homeDirectory: home, environment: [:])
    let model = SettingsViewModel(
      creatorAgentBridge: installer,
      permissionProbe: MockPermissionProbe(.allGranted),
      servingStatusProvider: { nil }
    )
    model.refreshCreatorAgentBridge()
    XCTAssertTrue(model.creatorAgentBridgeStatus.payloadAvailable)
    XCTAssertFalse(FileManager.default.fileExists(atPath: home.path))
    XCTAssertNil(model.creatorAgentBridgeNotice)

    // Another surface installs Claude after Creator's last inspection.
    _ = try installer.install(selectedClients: [.claudeCode])
    model.installCreatorAgentBridge(for: .codex)
    XCTAssertNil(model.creatorAgentBridgeError)
    XCTAssertEqual(Set(model.creatorAgentBridgeStatus.installedClients), [.codex, .claudeCode])
    XCTAssertTrue(
      FileManager.default.fileExists(
        atPath: home.appendingPathComponent(".claude/skills/codescribe/SKILL.md").path))
    XCTAssertTrue(
      FileManager.default.fileExists(
        atPath: home.appendingPathComponent(".codex/skills/codescribe/SKILL.md").path))
    XCTAssertTrue(model.creatorAgentBridgeNotice?.contains("does not attach a listener") == true)
    model.installCreatorAgentBridge(for: .codex)
    XCTAssertEqual(Set(model.creatorAgentBridgeStatus.installedClients), [.codex, .claudeCode])
  }

  func testLaunchSynchronizationDetailStaysOutOfTheCreatorNotice() throws {
    let payload = try makePayload()
    let home = scratch.appendingPathComponent("creator-launch-detail")
    let detail = "Agent bridge synchronization retained the installed runtime: test detail"
    defer { SettingsViewModel.recordAgentBridgeLaunchSynchronization(nil) }
    SettingsViewModel.recordAgentBridgeLaunchSynchronization(detail)

    // A Settings instance opened after launch reads the recorded detail.
    let model = SettingsViewModel(
      creatorAgentBridge: RealAgentBridgeInstaller(
        resourceRoot: payload, homeDirectory: home, environment: [:]),
      permissionProbe: MockPermissionProbe(.allGranted),
      servingStatusProvider: { nil }
    )
    model.refreshCreatorAgentBridge()
    XCTAssertEqual(model.creatorAgentBridgeLaunchDetail, detail)
    XCTAssertNil(model.creatorAgentBridgeNotice)
    XCTAssertNil(model.creatorAgentBridgeError)

    // A later launch result reaches an already open instance and clears.
    SettingsViewModel.recordAgentBridgeLaunchSynchronization(nil)
    XCTAssertNil(model.creatorAgentBridgeLaunchDetail)
    XCTAssertNil(model.creatorAgentBridgeNotice)
  }

  func testCreatorShowsUnownedSkillConflictWithoutReplacingUserContent() throws {
    let payload = try makePayload()
    let home = scratch.appendingPathComponent("creator-unowned")
    let skill = home.appendingPathComponent(".codex/skills/codescribe")
    try FileManager.default.createDirectory(at: skill, withIntermediateDirectories: true)
    let source = skill.appendingPathComponent("SKILL.md")
    let original = Data("user-maintained instructions".utf8)
    try original.write(to: source)
    let model = SettingsViewModel(
      creatorAgentBridge: RealAgentBridgeInstaller(
        resourceRoot: payload, homeDirectory: home, environment: [:]),
      permissionProbe: MockPermissionProbe(.allGranted),
      servingStatusProvider: { nil }
    )
    model.installCreatorAgentBridge(for: .codex)
    XCTAssertNotNil(model.creatorAgentBridgeError)
    XCTAssertNil(model.creatorAgentBridgeNotice)
    XCTAssertEqual(try Data(contentsOf: source), original)
    XCTAssertFalse(
      FileManager.default.fileExists(
        atPath: home.appendingPathComponent(".codescribe/agent-bridge/receipt.json").path))
  }

  func testExplicitManualAdoptionRetainsOriginalAcrossLaterUpdates() throws {
    let payload = try makePayload()
    let home = scratch.appendingPathComponent("manual-adoption")
    let destination = home.appendingPathComponent(".codex/skills/codescribe")
    try FileManager.default.createDirectory(at: destination, withIntermediateDirectories: true)
    let original = Data("manual instructions with personal changes".utf8)
    try original.write(to: destination.appendingPathComponent("SKILL.md"))
    try Data("extra notes".utf8).write(to: destination.appendingPathComponent("notes.txt"))
    let installer = RealAgentBridgeInstaller(
      resourceRoot: payload, homeDirectory: home, environment: [:])
    _ = try installer.install(selectedClients: [.claudeCode])
    XCTAssertThrowsError(try installer.install(selectedClients: [.codex, .claudeCode]))
    let result = try installer.adoptManualSkill(client: .codex)
    XCTAssertEqual(result.backupPaths.count, 1)
    XCTAssertEqual(Set(result.status.installedClients), [.codex, .claudeCode])
    let backup = URL(fileURLWithPath: try XCTUnwrap(result.backupPaths.first))
    XCTAssertEqual(try Data(contentsOf: backup.appendingPathComponent("SKILL.md")), original)
    XCTAssertTrue(
      FileManager.default.fileExists(atPath: backup.appendingPathComponent("notes.txt").path))
    XCTAssertEqual(
      try Data(contentsOf: destination.appendingPathComponent("SKILL.md")),
      try Data(contentsOf: payload.appendingPathComponent("skills/codescribe/SKILL.md")))
    _ = try installer.install(selectedClients: [.codex, .claudeCode])
    XCTAssertEqual(try Data(contentsOf: backup.appendingPathComponent("SKILL.md")), original)
    let receipt = try jsonObject(
      home.appendingPathComponent(".codescribe/agent-bridge/receipt.json"))
    XCTAssertEqual(receipt["preserved_manual_backups"] as? [String], result.backupPaths)
    XCTAssertThrowsError(
      try installer.adoptManualSkill(client: .codex), "managed folders are not manual copies")
  }

  func testManualAdoptionReceiptFailureRestoresOriginalFolder() throws {
    let payload = try makePayload()
    let home = scratch.appendingPathComponent("manual-failure")
    let destination = home.appendingPathComponent(".codex/skills/codescribe")
    try FileManager.default.createDirectory(at: destination, withIntermediateDirectories: true)
    let original = Data("preserve on failure".utf8)
    try original.write(to: destination.appendingPathComponent("SKILL.md"))
    // A directory at the receipt path forces the final write to fail after replacements.
    try FileManager.default.createDirectory(
      at: home.appendingPathComponent(".codescribe/agent-bridge/receipt.json"),
      withIntermediateDirectories: true)
    let installer = RealAgentBridgeInstaller(
      resourceRoot: payload, homeDirectory: home, environment: [:])
    XCTAssertThrowsError(try installer.adoptManualSkill(client: .codex))
    XCTAssertEqual(try Data(contentsOf: destination.appendingPathComponent("SKILL.md")), original)
    XCTAssertFalse(
      FileManager.default.fileExists(
        atPath: destination.appendingPathComponent(".codescribe-managed.json").path))
    XCTAssertFalse(
      FileManager.default.fileExists(
        atPath: home.appendingPathComponent(".codescribe/agent-bridge/runtime").path))
  }

  func testManualAdoptionRefusesRedirectedFolder() throws {
    let payload = try makePayload()
    let home = scratch.appendingPathComponent("manual-symlink")
    let outside = scratch.appendingPathComponent("outside-skill")
    try FileManager.default.createDirectory(at: outside, withIntermediateDirectories: true)
    let original = Data("outside instructions".utf8)
    try original.write(to: outside.appendingPathComponent("SKILL.md"))
    let destination = home.appendingPathComponent(".codex/skills/codescribe")
    try FileManager.default.createDirectory(
      at: destination.deletingLastPathComponent(), withIntermediateDirectories: true)
    try FileManager.default.createSymbolicLink(at: destination, withDestinationURL: outside)
    let installer = RealAgentBridgeInstaller(
      resourceRoot: payload, homeDirectory: home, environment: [:])
    XCTAssertThrowsError(try installer.adoptManualSkill(client: .codex))
    XCTAssertEqual(try Data(contentsOf: outside.appendingPathComponent("SKILL.md")), original)
    XCTAssertFalse(
      FileManager.default.fileExists(
        atPath: home.appendingPathComponent(".codescribe/agent-bridge/receipt.json").path))
  }

  func testInstallationLeaseRefusesAnotherWriterAndReleasesAfterFailure() throws {
    let payload = try makePayload()
    let home = scratch.appendingPathComponent("install-lease")
    let root = home.appendingPathComponent(".codescribe/agent-bridge")
    try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
    let lockPath = root.appendingPathComponent("installation.lock").path
    let descriptor = Darwin.open(lockPath, O_CREAT | O_RDWR, mode_t(0o600))
    XCTAssertGreaterThanOrEqual(descriptor, 0)
    guard descriptor >= 0 else { return }
    defer { _ = Darwin.close(descriptor) }
    XCTAssertEqual(flock(descriptor, LOCK_EX | LOCK_NB), 0)
    let installer = RealAgentBridgeInstaller(
      resourceRoot: payload, homeDirectory: home, environment: [:])
    XCTAssertThrowsError(try installer.install(selectedClients: [.codex]))
    XCTAssertFalse(
      FileManager.default.fileExists(atPath: root.appendingPathComponent("runtime").path))
    XCTAssertFalse(
      FileManager.default.fileExists(atPath: root.appendingPathComponent("receipt.json").path))
    XCTAssertEqual(flock(descriptor, LOCK_UN), 0)
    // Missing manual folder fails after the installer acquires the lock.
    XCTAssertThrowsError(try installer.adoptManualSkill(client: .codex))
    XCTAssertEqual(flock(descriptor, LOCK_EX | LOCK_NB), 0, "failure released ownership")
    XCTAssertEqual(flock(descriptor, LOCK_UN), 0)
    _ = try installer.install(selectedClients: [.codex])
    XCTAssertEqual(flock(descriptor, LOCK_EX | LOCK_NB), 0, "success released ownership")
    XCTAssertTrue(FileManager.default.fileExists(atPath: lockPath))
    XCTAssertEqual(flock(descriptor, LOCK_UN), 0)
  }

  func testLanguageRestartUsesCanonicalIdleGuardAndHoldsTurnLease() async throws {
    let home = scratch.appendingPathComponent("language-restart")
    let leasePath = home.appendingPathComponent("turn.lock")
    let busyPath = home.appendingPathComponent("busy")
    let helper = """
      #!/usr/bin/env python3
      import pathlib, sys
      root = pathlib.Path(__file__).resolve().parents[2]
      if sys.argv[1:] == ['--print-agent-turn-lease-path']:
          print(root / 'turn.lock')
      elif sys.argv[1:] == ['--assert-install-idle']:
          sys.exit(2 if (root / 'busy').exists() else 0)
      else:
          sys.exit(9)
      """
    let payload = try makePayload(helperContent: helper)
    let installer = RealAgentBridgeInstaller(
      resourceRoot: payload, homeDirectory: home, environment: [:])
    _ = try installer.install(selectedClients: [.codex])
    try Data().write(to: busyPath)
    do {
      _ = try await RealAgentBridgeInstaller.acquireIdleLanguageRestartLease(installer: installer)
      XCTFail("A live or unreadable bus must refuse restart")
    } catch {
      XCTAssertEqual(error as? InterfaceLanguageRestartError, .busy)
    }
    try FileManager.default.removeItem(at: busyPath)
    let lease = try await RealAgentBridgeInstaller.acquireIdleLanguageRestartLease(
      installer: installer)
    let probe = Darwin.open(leasePath.path, O_RDWR | O_CLOEXEC)
    XCTAssertGreaterThanOrEqual(probe, 0)
    guard probe >= 0 else { return }
    defer { _ = Darwin.close(probe) }
    XCTAssertNotEqual(flock(probe, LOCK_SH | LOCK_NB), 0, "Restart prevents a new agent turn")
    try lease.close()
    XCTAssertEqual(
      flock(probe, LOCK_SH | LOCK_NB), 0, "Closing the admitted lease releases ownership")
    XCTAssertEqual(flock(probe, LOCK_UN), 0)
    XCTAssertEqual(flock(probe, LOCK_SH | LOCK_NB), 0)
    do {
      _ = try await RealAgentBridgeInstaller.acquireIdleLanguageRestartLease(installer: installer)
      XCTFail("An active agent turn must refuse restart")
    } catch {
      XCTAssertEqual(error as? InterfaceLanguageRestartError, .busy)
    }
  }

  func testInstallationRefusesSymlinkedLeaseWithoutTouchingItsTarget() throws {
    let payload = try makePayload()
    let home = scratch.appendingPathComponent("install-lock-symlink")
    let root = home.appendingPathComponent(".codescribe/agent-bridge")
    try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
    let outside = scratch.appendingPathComponent("outside-lock")
    let original = Data("unrelated file".utf8)
    try original.write(to: outside)
    try FileManager.default.createSymbolicLink(
      at: root.appendingPathComponent("installation.lock"), withDestinationURL: outside)
    let installer = RealAgentBridgeInstaller(
      resourceRoot: payload, homeDirectory: home, environment: [:])
    XCTAssertThrowsError(try installer.install(selectedClients: [.codex]))
    XCTAssertEqual(try Data(contentsOf: outside), original)
    XCTAssertFalse(
      FileManager.default.fileExists(atPath: root.appendingPathComponent("runtime").path))
    XCTAssertFalse(
      FileManager.default.fileExists(atPath: root.appendingPathComponent("receipt.json").path))
  }

  func testIncompleteRollbackReportsAndPreservesTheOriginalBackup() throws {
    let payload = try makePayload()
    for blockRemoval in [true, false] {
      let home = scratch.appendingPathComponent("rollback-\(blockRemoval)")
      let destination = home.appendingPathComponent(".codex/skills/codescribe")
      try FileManager.default.createDirectory(at: destination, withIntermediateDirectories: true)
      let original = Data("original user instructions".utf8)
      try original.write(to: destination.appendingPathComponent("SKILL.md"))
      try FileManager.default.createDirectory(
        at: home.appendingPathComponent(".codescribe/agent-bridge/receipt.json"),
        withIntermediateDirectories: true)
      let manager = RefusingRollbackFileManager(
        destination: destination, blockRemoval: blockRemoval)
      let installer = RealAgentBridgeInstaller(
        resourceRoot: payload, homeDirectory: home, fileManager: manager, environment: [:])
      var failure: String?
      XCTAssertThrowsError(try installer.adoptManualSkill(client: .codex)) { error in
        failure = error.localizedDescription
      }
      XCTAssertEqual(manager.injectedRefusals, 1, "the fixture must reach the rollback operation")
      let backups = try FileManager.default.contentsOfDirectory(
        at: destination.deletingLastPathComponent(), includingPropertiesForKeys: nil
      ).filter { $0.lastPathComponent.hasPrefix(".codescribe.backup-") }
      XCTAssertEqual(backups.count, 1)
      let backup = try XCTUnwrap(backups.first)
      XCTAssertEqual(try Data(contentsOf: backup.appendingPathComponent("SKILL.md")), original)
      XCTAssertTrue(failure?.contains("Rollback incomplete") == true)
      // Directory enumeration may canonicalize the temporary-root spelling.
      // The diagnostic uses the destination spelling supplied to the installer.
      let reportedBackup = destination.deletingLastPathComponent()
        .appendingPathComponent(backup.lastPathComponent, isDirectory: true)
      XCTAssertEqual(reportedBackup.resolvingSymlinksInPath(), backup.resolvingSymlinksInPath())
      XCTAssertEqual(
        try Data(contentsOf: reportedBackup.appendingPathComponent("SKILL.md")), original)
      XCTAssertTrue(failure?.contains(reportedBackup.path) == true, failure ?? "missing diagnostic")
      XCTAssertTrue(failure?.contains(destination.path) == true)
      XCTAssertEqual(FileManager.default.fileExists(atPath: destination.path), blockRemoval)
    }
  }

  // Integrator (2026-10-01): build 1643 bundled the speech guard, but the
  // managed runtime kept the earlier helper until someone clicked Install.
  func testLaunchSynchronizationUpdatesAManagedRuntimeAndKeepsItsState() throws {
    let home = scratch.appendingPathComponent("sync-home", isDirectory: true)
    let previous = try makePayload(helperContent: "#!/usr/bin/env python3\nprint('old')\n")
    _ = try RealAgentBridgeInstaller(
      resourceRoot: previous, homeDirectory: home, environment: [:]
    ).install(selectedClients: [.claudeCode, .codex])
    let runtime = home.appendingPathComponent(".codescribe/agent-bridge/runtime", isDirectory: true)
    let followers = runtime.appendingPathComponent("followers", isDirectory: true)
    try FileManager.default.createDirectory(at: followers, withIntermediateDirectories: true)
    let log = followers.appendingPathComponent("filip.log")
    let logBytes = Data("follower filip attached\n".utf8)
    try logBytes.write(to: log)
    let cursor = runtime.appendingPathComponent("agent-ack-cursor.json")
    let cursorBytes = Data("{\"cursor\": 41}\n".utf8)
    try cursorBytes.write(to: cursor)
    let cursorInode = try fileNumber(cursor)

    let bundled = try makePayload(helperContent: "#!/usr/bin/env python3\nprint('guard')\n")
    let installer = RealAgentBridgeInstaller(
      resourceRoot: bundled, homeDirectory: home, environment: [:])
    let detail = installer.synchronizeManagedPayload()

    XCTAssertTrue(detail.contains("synchronized"), detail)
    XCTAssertEqual(
      try String(contentsOf: runtime.appendingPathComponent("bin/bus-demux.py"), encoding: .utf8),
      "#!/usr/bin/env python3\nprint('guard')\n")
    let receipt = try jsonObject(
      home.appendingPathComponent(".codescribe/agent-bridge/receipt.json"))
    XCTAssertEqual(
      Set(try XCTUnwrap(receipt["selected_clients"] as? [String])),
      Set([AgentBridgeClient.claudeCode.rawValue, AgentBridgeClient.codex.rawValue]),
      "startup keeps the recorded selection")
    XCTAssertEqual(try Data(contentsOf: log), logBytes)
    XCTAssertEqual(try Data(contentsOf: cursor), cursorBytes)
    XCTAssertEqual(try fileNumber(cursor), cursorInode, "ack cursor keeps its inode")

    let again = installer.synchronizeManagedPayload()
    XCTAssertTrue(again.contains("unchanged"), again)
  }

  func testLaunchInstallsRuntimeCommandsWithoutSelectingClientSkills() throws {
    let home = scratch.appendingPathComponent("sync-fresh-home", isDirectory: true)
    let installer = RealAgentBridgeInstaller(
      resourceRoot: try makePayload(), homeDirectory: home, environment: [:])

    let detail = installer.synchronizeManagedPayload()

    XCTAssertTrue(detail.contains("runtime installed"), detail)
    XCTAssertTrue(
      FileManager.default.fileExists(
        atPath: home.appendingPathComponent(".codescribe/agent-bridge/receipt.json").path))
    XCTAssertTrue(
      FileManager.default.fileExists(
        atPath: home.appendingPathComponent(".codescribe/agent-bridge/runtime").path))
    for folder in [".codex/skills/codescribe", ".claude/skills/codescribe"] {
      XCTAssertFalse(
        FileManager.default.fileExists(atPath: home.appendingPathComponent(folder).path),
        "no client folder appears without an explicit installation")
    }
    for command in ["cs-bus", "cs-say"] {
      let path = home.appendingPathComponent(".local/bin/" + command).path
      XCTAssertTrue(FileManager.default.isExecutableFile(atPath: path))
      let values = try URL(fileURLWithPath: path).resourceValues(
        forKeys: [.isRegularFileKey, .isSymbolicLinkKey])
      XCTAssertTrue(values.isRegularFile == true && values.isSymbolicLink != true)
    }
    XCTAssertTrue(installer.synchronizeManagedPayload().contains("unchanged"))
    try FileManager.default.removeItem(at: home.appendingPathComponent(".local/bin/cs-bus"))
    XCTAssertTrue(installer.synchronizeManagedPayload().contains("unchanged"))
    XCTAssertTrue(
      FileManager.default.isExecutableFile(
        atPath: home.appendingPathComponent(".local/bin/cs-bus").path))
  }

  func testForeignCommandRefusesBeforePayloadMutation() throws {
    let home = scratch.appendingPathComponent("foreign-command-home")
    let foreign = home.appendingPathComponent(".local/bin/cs-say")
    try FileManager.default.createDirectory(
      at: foreign.deletingLastPathComponent(), withIntermediateDirectories: true)
    try Data("my command".utf8).write(to: foreign)
    let installer = RealAgentBridgeInstaller(
      resourceRoot: try makePayload(), homeDirectory: home, environment: [:])
    XCTAssertThrowsError(try installer.install(selectedClients: [.codex]))
    XCTAssertEqual(try String(contentsOf: foreign, encoding: .utf8), "my command")
    XCTAssertFalse(
      FileManager.default.fileExists(
        atPath: home.appendingPathComponent(".codescribe/agent-bridge/runtime").path))
  }

  func testExplicitRuntimeInstallUpdatesHandEditedManagedHelperWithoutChangingSelection() throws {
    let home = scratch.appendingPathComponent("explicit-runtime-home")
    let installer = RealAgentBridgeInstaller(
      resourceRoot: try makePayload(), homeDirectory: home, environment: [:])
    _ = try installer.install(selectedClients: [.codex, .claudeCode])
    let helper = home.appendingPathComponent(".codescribe/agent-bridge/runtime/bin/bus-demux.py")
    try Data("experiment".utf8).write(to: helper)
    XCTAssertTrue(installer.synchronizeManagedPayload().contains("skipped"))
    _ = try installer.installRuntime()
    XCTAssertEqual(Set(installer.status().installedClients), [.codex, .claudeCode])
    XCTAssertTrue(try String(contentsOf: helper, encoding: .utf8).contains("print('bridge')"))
  }

  func testLaunchSynchronizationLeavesAHandEditedRuntimeAlone() throws {
    let home = scratch.appendingPathComponent("sync-edited-home", isDirectory: true)
    _ = try RealAgentBridgeInstaller(
      resourceRoot: try makePayload(helperContent: "#!/usr/bin/env python3\nprint('old')\n"),
      homeDirectory: home, environment: [:]
    ).install(selectedClients: [.codex])
    let helper = home.appendingPathComponent(".codescribe/agent-bridge/runtime/bin/bus-demux.py")
    let edited = "#!/usr/bin/env python3\nprint('hand edited')\n"
    try Data(edited.utf8).write(to: helper)

    let detail = RealAgentBridgeInstaller(
      resourceRoot: try makePayload(helperContent: "#!/usr/bin/env python3\nprint('guard')\n"),
      homeDirectory: home, environment: [:]
    ).synchronizeManagedPayload()

    XCTAssertTrue(detail.contains("skipped"), detail)
    XCTAssertEqual(try String(contentsOf: helper, encoding: .utf8), edited)
  }

  func testInstallPreservesRuntimeStateByteForByteAtTheSamePath() throws {
    let payload = try makePayload()
    let home = scratch.appendingPathComponent("state-preserved")
    let runtime = home.appendingPathComponent(".codescribe/agent-bridge/runtime", isDirectory: true)
    let followers = runtime.appendingPathComponent("followers", isDirectory: true)
    try FileManager.default.createDirectory(at: followers, withIntermediateDirectories: true)
    let log = followers.appendingPathComponent("filip.log")
    let logBytes = Data("follower filip attached\n".utf8)
    try logBytes.write(to: log)
    let cursor = runtime.appendingPathComponent("agent-ack-cursor.json")
    let cursorBytes = Data("{\"cursor\": 41}\n".utf8)
    try cursorBytes.write(to: cursor)
    let overlayCursor = runtime.appendingPathComponent("overlay-delivery-cursor.v1.json")
    let overlayBytes = Data("{\"sealed\": 7}\n".utf8)
    try overlayBytes.write(to: overlayCursor)
    let installer = RealAgentBridgeInstaller(
      resourceRoot: payload, homeDirectory: home, environment: [:])

    _ = try installer.install(selectedClients: [.codex])

    XCTAssertEqual(try Data(contentsOf: log), logBytes)
    XCTAssertEqual(try Data(contentsOf: cursor), cursorBytes)
    XCTAssertEqual(try Data(contentsOf: overlayCursor), overlayBytes)
    XCTAssertTrue(
      FileManager.default.fileExists(
        atPath: runtime.appendingPathComponent("bin/bus-demux.py").path))
    let receipt = try jsonObject(
      home.appendingPathComponent(".codescribe/agent-bridge/receipt.json"))
    let preserved = try XCTUnwrap(receipt["preserved_entries"] as? [String])
    XCTAssertTrue(preserved.contains("followers"), preserved.joined(separator: ","))
    XCTAssertTrue(preserved.contains("followers/filip.log"), preserved.joined(separator: ","))
    XCTAssertTrue(preserved.contains("agent-ack-cursor.json"), preserved.joined(separator: ","))
    XCTAssertTrue(
      preserved.contains("overlay-delivery-cursor.v1.json"), preserved.joined(separator: ","))

    // A content-identical reinstall preserves the same state again.
    _ = try installer.install(selectedClients: [.codex])
    XCTAssertEqual(try Data(contentsOf: log), logBytes)
    XCTAssertEqual(try Data(contentsOf: cursor), cursorBytes)
    XCTAssertEqual(try Data(contentsOf: overlayCursor), overlayBytes)
  }

  func testOpenFollowerLogKeepsItsInodeAndStaysWritableAcrossInstall() throws {
    let payload = try makePayload()
    let home = scratch.appendingPathComponent("state-inode")
    let followers = home.appendingPathComponent(
      ".codescribe/agent-bridge/runtime/followers", isDirectory: true)
    try FileManager.default.createDirectory(at: followers, withIntermediateDirectories: true)
    let log = followers.appendingPathComponent("ksawery.log")
    try Data("before install\n".utf8).write(to: log)
    // The incident: a running follower holds this descriptor across the update.
    let handle = try FileHandle(forUpdating: log)
    defer { try? handle.close() }
    var descriptorBefore = stat()
    XCTAssertEqual(Darwin.fstat(handle.fileDescriptor, &descriptorBefore), 0)
    let pathInodeBefore = try XCTUnwrap(
      FileManager.default.attributesOfItem(atPath: log.path)[.systemFileNumber] as? NSNumber)
    let installer = RealAgentBridgeInstaller(
      resourceRoot: payload, homeDirectory: home, environment: [:])

    _ = try installer.install(selectedClients: [.codex])

    var descriptorAfter = stat()
    XCTAssertEqual(Darwin.fstat(handle.fileDescriptor, &descriptorAfter), 0)
    XCTAssertEqual(descriptorBefore.st_ino, descriptorAfter.st_ino)
    let pathInodeAfter = try XCTUnwrap(
      FileManager.default.attributesOfItem(atPath: log.path)[.systemFileNumber] as? NSNumber)
    XCTAssertEqual(pathInodeBefore, pathInodeAfter)
    XCTAssertEqual(descriptorAfter.st_ino, pathInodeAfter.uint64Value)
    _ = try handle.seekToEnd()
    try handle.write(contentsOf: Data("after install\n".utf8))
    try handle.synchronize()
    XCTAssertEqual(
      try String(contentsOf: log, encoding: .utf8),
      "before install\nafter install\n")
  }

  func testRollbackAfterSyntheticFailureLeavesRuntimeStateUntouched() throws {
    let helperV1 = "#!/usr/bin/env python3\nprint('v1')\n"
    let payloadV1 = try makePayload(bundleVersion: "9.8.7", helperContent: helperV1)
    let home = scratch.appendingPathComponent("state-rollback")
    let installerV1 = RealAgentBridgeInstaller(
      resourceRoot: payloadV1, homeDirectory: home, environment: [:])
    _ = try installerV1.install(selectedClients: [.codex])
    let runtime = home.appendingPathComponent(".codescribe/agent-bridge/runtime", isDirectory: true)
    let followers = runtime.appendingPathComponent("followers", isDirectory: true)
    try FileManager.default.createDirectory(at: followers, withIntermediateDirectories: true)
    let log = followers.appendingPathComponent("filip.log")
    let logBytes = Data("follower mid-session\n".utf8)
    try logBytes.write(to: log)
    let cursor = runtime.appendingPathComponent("agent-ack-cursor.json")
    let cursorBytes = Data("{\"cursor\": 42}\n".utf8)
    try cursorBytes.write(to: cursor)

    let payloadV2 = try makePayload(
      bundleVersion: "9.8.8",
      helperContent: "#!/usr/bin/env python3\nprint('v2')\n")
    let installerV2 = RealAgentBridgeInstaller(
      resourceRoot: payloadV2, homeDirectory: home, environment: [:])
    // A directory at the receipt path forces the commit write to fail after
    // every replacement, exercising the full rollback path.
    let receiptURL = home.appendingPathComponent(".codescribe/agent-bridge/receipt.json")
    try FileManager.default.removeItem(at: receiptURL)
    try FileManager.default.createDirectory(at: receiptURL, withIntermediateDirectories: true)

    XCTAssertThrowsError(try installerV2.install(selectedClients: [.codex]))

    XCTAssertEqual(try Data(contentsOf: log), logBytes)
    XCTAssertEqual(try Data(contentsOf: cursor), cursorBytes)
    XCTAssertEqual(
      try Data(contentsOf: runtime.appendingPathComponent("bin/bus-demux.py")),
      Data(helperV1.utf8))
    XCTAssertTrue(
      FileManager.default.fileExists(
        atPath: runtime.appendingPathComponent("skills/codescribe/SKILL.md").path))
    let leftovers = try FileManager.default.contentsOfDirectory(
      atPath: home.appendingPathComponent(".codescribe/agent-bridge").path
    ).filter { $0.hasPrefix(".runtime-stage") || $0.contains(".backup-") }
    XCTAssertTrue(leftovers.isEmpty, leftovers.joined(separator: ","))
  }

  func testStalePayloadFromPreviousManifestIsRemovedButStateIsNot() throws {
    let payloadV1 = try makePayload(extraFiles: ["docs/legacy.txt": "legacy docs\n"])
    let home = scratch.appendingPathComponent("state-stale")
    let installerV1 = RealAgentBridgeInstaller(
      resourceRoot: payloadV1, homeDirectory: home, environment: [:])
    _ = try installerV1.install(selectedClients: [.codex])
    let runtime = home.appendingPathComponent(".codescribe/agent-bridge/runtime", isDirectory: true)
    XCTAssertTrue(
      FileManager.default.fileExists(
        atPath: runtime.appendingPathComponent("docs/legacy.txt").path))
    let followers = runtime.appendingPathComponent("followers", isDirectory: true)
    try FileManager.default.createDirectory(at: followers, withIntermediateDirectories: true)
    let log = followers.appendingPathComponent("filip.log")
    let logBytes = Data("still attached\n".utf8)
    try logBytes.write(to: log)

    let payloadV2 = try makePayload(bundleVersion: "9.8.8")
    let installerV2 = RealAgentBridgeInstaller(
      resourceRoot: payloadV2, homeDirectory: home, environment: [:])
    _ = try installerV2.install(selectedClients: [.codex])

    XCTAssertFalse(
      FileManager.default.fileExists(
        atPath: runtime.appendingPathComponent("docs/legacy.txt").path),
      "stale payload committed by the previous receipt must be removed")
    XCTAssertFalse(
      FileManager.default.fileExists(atPath: runtime.appendingPathComponent("docs").path),
      "payload directories left empty by stale removal are pruned")
    XCTAssertEqual(try Data(contentsOf: log), logBytes)
    XCTAssertTrue(
      FileManager.default.fileExists(
        atPath: runtime.appendingPathComponent("bin/bus-demux.py").path))
    let receipt = try jsonObject(
      home.appendingPathComponent(".codescribe/agent-bridge/receipt.json"))
    let preserved = try XCTUnwrap(receipt["preserved_entries"] as? [String])
    XCTAssertTrue(preserved.contains("followers/filip.log"), preserved.joined(separator: ","))
    let payloadPaths = try XCTUnwrap(receipt["payload_files"] as? [[String: Any]])
    XCTAssertFalse(payloadPaths.contains { $0["path"] as? String == "docs/legacy.txt" })
  }

  private func makePayload(
    bundleVersion: String = "9.8.7",
    helperContent: String = "#!/usr/bin/env python3\nprint('bridge')\n",
    skillContent: String = "---\nname: codescribe\n---\n",
    extraFiles: [String: String] = [:]
  ) throws -> URL {
    let payload = scratch.appendingPathComponent("payload-\(UUID().uuidString)", isDirectory: true)
    let helper = payload.appendingPathComponent("bin/bus-demux.py")
    let skill = payload.appendingPathComponent("skills/codescribe", isDirectory: true)
    try FileManager.default.createDirectory(
      at: helper.deletingLastPathComponent(),
      withIntermediateDirectories: true
    )
    try FileManager.default.createDirectory(at: skill, withIntermediateDirectories: true)
    try Data(helperContent.utf8).write(to: helper)
    try FileManager.default.setAttributes([.posixPermissions: 0o755], ofItemAtPath: helper.path)
    try Data(skillContent.utf8).write(
      to: skill.appendingPathComponent("SKILL.md")
    )
    try Data("reference\n".utf8).write(to: skill.appendingPathComponent("README.md"))

    var relativeFiles = [
      "bin/bus-demux.py",
      "skills/codescribe/README.md",
      "skills/codescribe/SKILL.md",
    ]
    for name in ["cs-bus", "cs-say"] {
      let url = payload.appendingPathComponent("bin/" + name)
      try Data("#!/usr/bin/env python3\nprint('command')\n".utf8).write(to: url)
      try FileManager.default.setAttributes([.posixPermissions: 0o755], ofItemAtPath: url.path)
      relativeFiles.append("bin/" + name)
    }
    for (path, content) in extraFiles {
      let url = payload.appendingPathComponent(path)
      try FileManager.default.createDirectory(
        at: url.deletingLastPathComponent(),
        withIntermediateDirectories: true
      )
      try Data(content.utf8).write(to: url)
      relativeFiles.append(path)
    }
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
      "bundle_version": bundleVersion,
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

  private func fileNumber(_ url: URL) throws -> UInt64 {
    let attributes = try FileManager.default.attributesOfItem(atPath: url.path)
    return try XCTUnwrap((attributes[.systemFileNumber] as? NSNumber)?.uint64Value)
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

/// Test-only I/O fault injection. Immutable configuration retains FileManager's
/// cross-thread contract; it does not add a production suppression or error path.
private final class RefusingRollbackFileManager: FileManager, @unchecked Sendable {
  private let destination: URL
  private let blockRemoval: Bool
  private(set) var injectedRefusals = 0

  init(destination: URL, blockRemoval: Bool) {
    self.destination = destination
    self.blockRemoval = blockRemoval
    super.init()
  }

  override func removeItem(at URL: URL) throws {
    if blockRemoval, URL.standardizedFileURL.path == destination.standardizedFileURL.path {
      injectedRefusals += 1
      throw CocoaError(.fileWriteNoPermission)
    }
    try super.removeItem(at: URL)
  }

  override func moveItem(at source: URL, to target: URL) throws {
    if !blockRemoval, target.standardizedFileURL.path == destination.standardizedFileURL.path,
      source.lastPathComponent.hasPrefix(".codescribe.backup-")
    {
      injectedRefusals += 1
      throw CocoaError(.fileWriteNoPermission)
    }
    try super.moveItem(at: source, to: target)
  }
}

private final class RestoringForeignFolderFileManager: FileManager, @unchecked Sendable {
  private let folder: URL
  private let marker: Data
  private(set) var didRestore = false

  init(folder: URL, marker: Data) {
    self.folder = folder
    self.marker = marker
    super.init()
  }

  override func copyItem(at source: URL, to destination: URL) throws {
    if !didRestore {
      didRestore = true
      try super.createDirectory(at: folder, withIntermediateDirectories: true)
      try marker.write(to: folder.appendingPathComponent(".codescribe-managed.json"))
      try Data("restored foreign content\n".utf8).write(
        to: folder.appendingPathComponent("operator.txt"))
    }
    try super.copyItem(at: source, to: destination)
  }
}

private final class RecordingAgentBridgeInstaller: AgentBridgeInstalling {
  func adoptManualSkill(client: AgentBridgeClient) throws -> AgentBridgeAdoptionResult {
    throw AgentBridgeInstallationError.transaction("manual adoption not configured in this fixture")
  }
  private(set) var installCalls: [Set<AgentBridgeClient>] = []
  var failInstallation = false
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
    if failInstallation {
      throw AgentBridgeInstallationError.transaction("Installation refused by the test fixture")
    }
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
