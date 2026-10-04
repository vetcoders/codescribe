import XCTest

@testable import Codescribe

private final class CredentialOnboardingEngine: OnboardingEngine {
  var provider: CsProviderOption
  var keyWrites = 0

  init(provider: CsProviderOption) { self.provider = provider }
  func shouldShowOnboarding() -> Bool { true }
  func onboardingProgress() -> UInt32 { 0 }
  func saveOnboardingProgress(step: UInt32) {}
  func markOnboardingDone() {}
  func onboardingMode() -> String? { nil }
  func setOnboardingMode(_ mode: String) throws {}
  func currentLanguage() -> CsLanguage { .auto }
  func assistiveProvider() -> String? { provider.id }
  func keyStatus() -> CsKeyStatus { .sampleAllSet }
  func availableProviders() -> [CsProviderOption] { [provider] }
  func setApiKey(account: String, secret: String) throws { keyWrites += 1 }
  func updateConfig(key: String, value: String) throws {}
}

private struct CredentialTestBridgeInstaller: AgentBridgeInstalling {
  func status() -> AgentBridgeInstallationStatus { .unavailable }
  func install(selectedClients: Set<AgentBridgeClient>) throws -> AgentBridgeInstallationStatus {
    throw AgentBridgeInstallationError.payloadUnavailable
  }
  func adoptManualSkill(client: AgentBridgeClient) throws -> AgentBridgeAdoptionResult {
    throw AgentBridgeInstallationError.payloadUnavailable
  }
}

@MainActor
final class CredentialPresentationTests: XCTestCase {
  func testSetupKeepsAccountAndProviderKeyPresenceSeparate() async {
    for (account, key) in [(true, false), (false, true), (true, true), (false, false)] {
      var provider = CsProviderOption.sampleProviders[1]
      provider.accountSignedIn = account
      provider.apiKeySet = key
      let engine = CredentialOnboardingEngine(provider: provider)
      let model = OnboardingViewModel(
        engine: engine, hotkeys: MockHotkeysEngine(), agentStatus: MockAgentStatusEngine(),
        agentBridge: CredentialTestBridgeInstaller(), probe: MockPermissionProbe(.allGranted))
      model.refreshProviderAccess()
      await awaitCondition { !model.providerAccessPending }

      XCTAssertEqual(model.selectedProviderAccountConnected, account)
      XCTAssertEqual(
        model.selectedProviderKeySet, key,
        "global key-status flags cannot impersonate this provider's credential")
      XCTAssertEqual(engine.keyWrites, 0)
      if account && !key {
        XCTAssertTrue(model.providerAccessDescription.contains("without adding one"))
        XCTAssertTrue(model.providerAccessDescription.contains("Formatting"))
      }
      engine.provider.accountSignedIn = !account
      engine.provider.apiKeySet = !key
      model.refreshProviderAccess()
      await awaitCondition { !model.providerAccessPending }
      XCTAssertEqual(model.selectedProviderAccountConnected, !account)
      XCTAssertEqual(model.selectedProviderKeySet, !key)
      XCTAssertEqual(engine.keyWrites, 0, "a passive refresh never creates a credential")
    }
  }

  func testCustomProviderKeyUsesRegistryInsteadOfVendorKeyStatusFlags() async {
    var provider = CsProviderOption.row(
      id: "custom:credential-test", kind: "custom", name: "Test Host", wire: "responses",
      endpoint: "http://localhost:8080/v1/responses", account: "LLM_CUSTOM_CREDENTIAL_TEST_API_KEY")
    provider.apiKeySet = true
    let engine = CredentialOnboardingEngine(provider: provider)
    let model = OnboardingViewModel(
      engine: engine, hotkeys: MockHotkeysEngine(), agentStatus: MockAgentStatusEngine(),
      agentBridge: CredentialTestBridgeInstaller(), probe: MockPermissionProbe(.allGranted))
    model.refreshProviderAccess()
    await awaitCondition { !model.providerAccessPending }
    XCTAssertTrue(model.selectedProviderKeySet)
    engine.provider.apiKeySet = false
    model.refreshProviderAccess()
    await awaitCondition { !model.providerAccessPending }
    XCTAssertFalse(model.selectedProviderKeySet)
    XCTAssertTrue(model.providerAccessDescription.contains("does not require an API key"))
  }

  func testAccountAccessDoesNotPretendModelDiscoveryHasAKey() {
    var provider = CsProviderOption.sampleProviders[1]
    provider.accountSignedIn = true
    provider.apiKeySet = false
    let runtime = CsRuntimeLlmLane(
      lane: .assistive, providerId: provider.id, providerDisplayName: provider.displayName,
      wire: provider.wire, endpoint: provider.endpoint, model: "current-model",
      keyAccount: provider.apiKeyAccount, keyPresent: false, accountAuth: true,
      available: true, unavailableReason: nil)
    let lane = LLMLaneModel(
      lane: .assistive, runtime: runtime, provider: provider, configuredModel: "current-model",
      discovery: CsModelDiscovery(
        providerId: provider.id, status: "no_key", message: nil, models: []))
    XCTAssertFalse(lane.usesDiscoveredPicker)
    XCTAssertTrue(lane.discoveryDescription.contains("Account sign-in"))
    XCTAssertTrue(lane.discoveryDescription.contains("Model ID"))
    let cached = LLMLaneModel(
      lane: .assistive, runtime: runtime, provider: provider, configuredModel: "current-model",
      discovery: CsModelDiscovery(
        providerId: provider.id, status: "cached", message: nil,
        models: [CsModelOption(id: "current-model", displayName: "Current Model")]))
    XCTAssertTrue(cached.usesDiscoveredPicker, "usable cached catalogs retain model selection")
  }
}
