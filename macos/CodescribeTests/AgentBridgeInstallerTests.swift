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

  func testOnboardingUsesEnglishCopyWithPolishDictationAndNeverInstallsUntilSelectionAndClick() {
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
    XCTAssertEqual(model.agentBridgeTitle, "Connect Codescribe to your agent.")
    XCTAssertEqual(model.agentBridgeButtonTitle, "Install selected")
    XCTAssertTrue(model.agentBridgeExplanation.contains("live drafts"))
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
    XCTAssertEqual(fallbackModel.agentBridgeTitle, model.agentBridgeTitle)
    XCTAssertTrue(fallbackModel.agentBridgeExplanation.contains("live drafts"))
    XCTAssertTrue(fallbackModel.agentBridgeExplanation.contains("transcript_sealed"))

    model.refreshForCurrentStep()
    XCTAssertEqual(bridge.installCalls, [])

    model.toggleAgentClient(.codex)
    XCTAssertEqual(bridge.installCalls, [])
    model.installAgentBridge()
    XCTAssertEqual(model.agentBridgeButtonTitle, "Update selected")
    XCTAssertEqual(bridge.installCalls, [[.codex]])
    XCTAssertEqual(model.agentBridgeStatus.installedClients, [.codex])
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
      XCTAssertEqual(Set(status.installedPaths), Set(skills.map(\.path)))
      XCTAssertTrue(status.detail.contains("Update will re-adopt it."), status.detail)
      XCTAssertTrue(status.detail.contains("Claude Code: managed folder found"))
      XCTAssertTrue(status.detail.contains("Codex: managed folder found"))

      let updated = try installer.install(selectedClients: [.codex, .claudeCode])
      XCTAssertEqual(Set(updated.installedClients), [.codex, .claudeCode])
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
    XCTAssertTrue(
      installer.status().detail.contains("receipt found, managed folder missing or invalid"))
    _ = try installer.install(selectedClients: [.codex])
    XCTAssertEqual(try jsonObject(receiptURL)["managed_id"] as? String, managedID)
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

private final class RecordingAgentBridgeInstaller: AgentBridgeInstalling {
  func adoptManualSkill(client: AgentBridgeClient) throws -> AgentBridgeAdoptionResult {
    throw AgentBridgeInstallationError.transaction("manual adoption not configured in this fixture")
  }
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
