import XCTest

@testable import Codescribe

@MainActor
final class OnboardingConsentTests: XCTestCase {
  func testWelcomeAndModeNeverReadProviderCredentials() async throws {
    let engine = ConsentRecordingEngine()
    let model = makeModel(engine)
    XCTAssertEqual(engine.keyReads, 0, "Showing the first screen cannot read credentials")
    model.refreshForCurrentStep()
    model.refreshProviderAccess()
    await Task.yield()
    model.advance()
    model.refreshForCurrentStep()
    await Task.yield()
    XCTAssertEqual(model.step, .mode)
    XCTAssertEqual(engine.snapshotReads, 0, "Window activation and navigation are not auth consent")
    XCTAssertEqual(engine.keyReads, 0)
  }

  func testSetupHasOnePermissionChapterAndAnOptionalLocalModelChapter() {
    XCTAssertEqual(
      OnboardingStep.count, 9,
      "Language, mode, permissions, dictation language, local model, provider, hotkeys, agent, done"
    )
  }

  func testPermissionAndLocalModelNavigationDoesNotReadProviderCredentials() async throws {
    let providerIndex = try XCTUnwrap(OnboardingStep.flow.firstIndex(of: .apiKey))
    for index in 0..<providerIndex {
      let engine = ConsentRecordingEngine()
      engine.fixture.progress = UInt32(index)
      let model = makeModel(engine)
      model.refreshForCurrentStep()
      model.refreshProviderAccess()
      for _ in 0..<8 { await Task.yield() }
      XCTAssertEqual(engine.keyReads, 0, "Chapter \(index) has no credential consent")
      XCTAssertEqual(engine.snapshotReads, 0, "Chapter \(index) has no credential consent")
    }
  }

  func testProviderChapterStillLoadsCredentialState() async throws {
    let engine = ConsentRecordingEngine()
    engine.fixture.progress = UInt32(try XCTUnwrap(OnboardingStep.flow.firstIndex(of: .apiKey)))
    let model = makeModel(engine)
    XCTAssertEqual(engine.snapshotReads, 0)
    model.refreshForCurrentStep()
    for _ in 0..<8 { await Task.yield() }
    XCTAssertEqual(engine.snapshotReads, 1, "The explained provider step owns the credential read")
    XCTAssertTrue(model.providerAccessResolved)
  }

  private func makeModel(_ engine: ConsentRecordingEngine) -> OnboardingViewModel {
    OnboardingViewModel(
      engine: engine, hotkeys: MockHotkeysEngine(), agentStatus: MockAgentStatusEngine(),
      agentBridge: ConsentTestBridge(), probe: MockPermissionProbe(.allGranted),
      preferredLanguages: ["en"], processInterfaceLanguage: .english)
  }
}

@MainActor
private final class ConsentRecordingEngine: OnboardingEngine {
  let fixture = MockOnboardingEngine()
  private(set) var keyReads = 0
  private(set) var snapshotReads = 0
  func shouldShowOnboarding() -> Bool { fixture.shouldShowOnboarding() }
  func onboardingProgress() -> UInt32 { fixture.onboardingProgress() }
  func saveOnboardingProgress(step: UInt32) { fixture.saveOnboardingProgress(step: step) }
  func markOnboardingDone() { fixture.markOnboardingDone() }
  func onboardingMode() -> String? { fixture.onboardingMode() }
  func setOnboardingMode(_ mode: String) throws { try fixture.setOnboardingMode(mode) }
  func currentLanguage() -> CsLanguage { fixture.currentLanguage() }
  func assistiveProvider() -> String? { fixture.assistiveProvider() }
  func keyStatus() -> CsKeyStatus {
    keyReads += 1
    return fixture.keyStatus()
  }
  func availableProviders() -> [CsProviderOption] { fixture.availableProviders() }
  func providerAccessSnapshot() async throws -> CsProviderAccessSnapshot {
    snapshotReads += 1
    return CsProviderAccessSnapshot(
      providers: fixture.availableProviders(), accountErrors: [:],
      keyStatus: fixture.keyStatus(), sttLanes: [], revision: 0)
  }
  func setApiKey(account: String, secret: String) throws {}
  func updateConfig(key: String, value: String) throws {}
}

private struct ConsentTestBridge: AgentBridgeInstalling {
  func status() -> AgentBridgeInstallationStatus {
    .init(
      payloadAvailable: false, bundleVersion: nil, installedClients: [], installedPaths: [],
      detail: "")
  }
  func install(selectedClients: Set<AgentBridgeClient>) throws -> AgentBridgeInstallationStatus {
    throw AgentBridgeInstallationError.payloadUnavailable
  }
  func adoptManualSkill(client: AgentBridgeClient) throws -> AgentBridgeAdoptionResult {
    throw AgentBridgeInstallationError.payloadUnavailable
  }
}
