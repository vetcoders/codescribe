import XCTest

@testable import Codescribe

private final class CredentialOnboardingEngine: OnboardingEngine {
  var provider: CsProviderOption
  var keyWrites = 0

  init(provider: CsProviderOption) { self.provider = provider }
  func shouldShowOnboarding() -> Bool { true }
  func onboardingProgress() -> UInt32 {
    UInt32(OnboardingStep.flow.firstIndex(of: .apiKey)!)
  }
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
      XCTAssertEqual(
        model.step, .apiKey, "Credential presence belongs to the explained provider step")
      model.refreshProviderAccess()
      await awaitCondition { model.providerAccessResolved && !model.providerAccessPending }

      XCTAssertEqual(model.selectedProviderAccountConnected, account)
      XCTAssertEqual(
        model.selectedProviderKeySet, key,
        "global key-status flags cannot impersonate this provider's credential")
      XCTAssertEqual(engine.keyWrites, 0)
      if account && !key {
        XCTAssertEqual(model.selectedProviderAccountStatus, "Connected")
        XCTAssertEqual(model.selectedProviderKeyStatus, "Not set")
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
    XCTAssertEqual(model.step, .apiKey)
    model.refreshProviderAccess()
    await awaitCondition { model.providerAccessResolved && !model.providerAccessPending }
    XCTAssertTrue(model.selectedProviderKeySet)
    engine.provider.apiKeySet = false
    model.refreshProviderAccess()
    await awaitCondition { !model.providerAccessPending }
    XCTAssertFalse(model.selectedProviderKeySet)
    XCTAssertFalse(model.selectedProviderRequiresApiKey)
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
    XCTAssertTrue(lane.discoveryDescription.contains("connected account"))
    XCTAssertTrue(lane.discoveryDescription.contains("model ID"))
    XCTAssertEqual(lane.availabilityDescription, "Connected account")
    XCTAssertFalse(lane.discoveryFailed, "a missing key is not a failed fetch")
    XCTAssertNil(lane.discoveryErrorDetails)
    let cached = LLMLaneModel(
      lane: .assistive, runtime: runtime, provider: provider, configuredModel: "current-model",
      discovery: CsModelDiscovery(
        providerId: provider.id, status: "cached", message: nil,
        models: [CsModelOption(id: "current-model", displayName: "Current Model")]))
    XCTAssertTrue(cached.usesDiscoveredPicker, "usable cached catalogs retain model selection")
  }

  func testFormattingRespectsResponsesAccountWithoutInventingAnAPIKey() {
    var provider = CsProviderOption.sampleProviders[1]
    provider.accountSignedIn = true
    provider.apiKeySet = false
    let runtime = CsRuntimeLlmLane(
      lane: .formatting, providerId: provider.id, providerDisplayName: provider.displayName,
      wire: provider.wire, endpoint: provider.endpoint, model: "formatter-model",
      keyAccount: provider.apiKeyAccount, keyPresent: false, accountAuth: true,
      available: true, unavailableReason: nil)
    let lane = LLMLaneModel(
      lane: .formatting, runtime: runtime, provider: provider, configuredModel: "formatter-model",
      discovery: CsModelDiscovery(
        providerId: provider.id, status: "no_key", message: nil, models: []))
    XCTAssertEqual(lane.availabilityDescription, "Connected account")
    XCTAssertEqual(lane.availabilityTint, CSColor.oliveLight)
    XCTAssertEqual(
      SettingsViewModel.availabilityTint(for: provider, lane: .formatting), CSColor.oliveLight)
    XCTAssertTrue(lane.discoveryDescription.contains("connected account covers model requests"))
    XCTAssertTrue(lane.discoveryDescription.contains("model list needs"))
    XCTAssertFalse(lane.discoveryDescription.contains("Formatting needs"))
    XCTAssertFalse(lane.usesDiscoveredPicker)
    provider.accountSignedIn = false
    XCTAssertEqual(
      SettingsViewModel.availabilityTint(for: provider, lane: .formatting), CSColor.terracotta)
  }

  /// A stored key is presence, not validity: the lane says "Stored API key"
  /// while discovery, independently, may still report the key as rejected.
  func testStoredKeyLabelDoesNotClaimValidity() {
    var provider = CsProviderOption.sampleProviders[1]
    provider.accountSignedIn = false
    provider.apiKeySet = true
    let runtime = CsRuntimeLlmLane(
      lane: .formatting, providerId: provider.id, providerDisplayName: "xAI",
      wire: provider.wire, endpoint: provider.endpoint, model: "grok-4",
      keyAccount: provider.apiKeyAccount, keyPresent: true, accountAuth: false,
      available: true, unavailableReason: nil)
    let rejected = LLMLaneModel(
      lane: .formatting, runtime: runtime, provider: provider, configuredModel: "",
      discovery: CsModelDiscovery(
        providerId: provider.id, status: "key_rejected",
        message: "{\"error\":\"Incorrect API key provided\"}", models: []))
    XCTAssertEqual(rejected.availabilityDescription, "Stored API key")
    XCTAssertTrue(rejected.discoveryFailed)
    XCTAssertEqual(
      rejected.discoveryDescription,
      "Could not fetch \(provider.displayName) models. The API key was rejected. Check it under Providers."
    )
    XCTAssertEqual(rejected.discoveryErrorDetails, "{\"error\":\"Incorrect API key provided\"}")
    XCTAssertFalse(
      rejected.discoveryDescription.contains("Incorrect"), "the raw body stays under Error details")

    // Any other failure is a plain fetch error: the key is never blamed on a guess.
    let outage = LLMLaneModel(
      lane: .formatting, runtime: runtime, provider: provider, configuredModel: "",
      discovery: CsModelDiscovery(
        providerId: provider.id, status: "error", message: "connection refused", models: []))
    XCTAssertEqual(
      outage.discoveryDescription,
      "Could not fetch \(provider.displayName) models. Check the provider under Providers."
    )
    XCTAssertEqual(outage.discoveryErrorDetails, "connection refused")
    let bare = LLMLaneModel(
      lane: .formatting, runtime: runtime, provider: provider, configuredModel: "",
      discovery: CsModelDiscovery(
        providerId: provider.id, status: "error", message: nil, models: []))
    XCTAssertNil(bare.discoveryErrorDetails, "no details row without a message")

    // Access still being checked wins over a stale failure.
    var pending = rejected
    pending.credentialAccessResolved = false
    XCTAssertFalse(pending.discoveryFailed)
    XCTAssertNil(pending.discoveryErrorDetails)
  }

  /// Both lanes on one provider share one discovery record; only Formatting
  /// defers to the Agent's line, and only while both show the same failure.
  func testFormattingDefersASharedDiscoveryFailureToTheAgentLane() {
    let provider = CsProviderOption.sampleProviders[1]
    func runtime(_ lane: CsLlmLane, providerId: String) -> CsRuntimeLlmLane {
      CsRuntimeLlmLane(
        lane: lane, providerId: providerId, providerDisplayName: provider.displayName,
        wire: provider.wire, endpoint: provider.endpoint, model: "m",
        keyAccount: provider.apiKeyAccount, keyPresent: true, accountAuth: false,
        available: true, unavailableReason: nil)
    }
    func model(_ lane: LLMLane, providerId: String, status: String) -> LLMLaneModel {
      LLMLaneModel(
        lane: lane, runtime: runtime(lane.bridgeLane, providerId: providerId), provider: provider,
        configuredModel: "",
        discovery: CsModelDiscovery(
          providerId: providerId, status: status, message: "x", models: []))
    }
    let agentFailed = model(.assistive, providerId: provider.id, status: "key_rejected")
    let formattingFailed = model(.formatting, providerId: provider.id, status: "key_rejected")
    XCTAssertTrue(formattingFailed.repeatsDiscoveryFailure(of: agentFailed))
    XCTAssertFalse(
      agentFailed.repeatsDiscoveryFailure(of: formattingFailed), "the Agent line always prints")
    XCTAssertFalse(
      model(.formatting, providerId: "custom:other", status: "error")
        .repeatsDiscoveryFailure(of: agentFailed),
      "a different provider has its own failure")
    XCTAssertFalse(
      model(.formatting, providerId: provider.id, status: "fresh").repeatsDiscoveryFailure(
        of: agentFailed))
    XCTAssertFalse(
      formattingFailed.repeatsDiscoveryFailure(
        of: model(.assistive, providerId: provider.id, status: "no_key")),
      "a missing key is explained per lane, never folded")
  }
}
