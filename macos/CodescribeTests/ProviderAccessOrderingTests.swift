import XCTest

@testable import Codescribe

@MainActor
private final class ControlledProviderEngine: OnboardingEngine {
  enum Failure: Error { case denied }
  var progress: UInt32 = 9
  var reads = 0
  var writes = 0
  var revision: UInt64 = 0
  var provider = CsProviderOption.sampleProviders[1]
  var read: CheckedContinuation<CsProviderAccessSnapshot, Error>?
  var write: CheckedContinuation<Void, Error>?
  func shouldShowOnboarding() -> Bool { true }
  func onboardingProgress() -> UInt32 { progress }
  func saveOnboardingProgress(step: UInt32) { progress = step }
  func markOnboardingDone() {}
  func onboardingMode() -> String? { "agentic" }
  func setOnboardingMode(_ mode: String) throws {}
  func currentLanguage() -> CsLanguage { .auto }
  func assistiveProvider() -> String? { provider.id }
  func keyStatus() -> CsKeyStatus { .sampleAllSet }
  func availableProviders() -> [CsProviderOption] { [provider] }
  func setApiKey(account: String, secret: String) throws { XCTFail("sync mutation must not run") }
  func updateConfig(key: String, value: String) throws {}
  func providerAccessRevision() -> UInt64 { revision }
  func providerAccessSnapshot() async throws -> CsProviderAccessSnapshot {
    reads += 1
    return try await withCheckedThrowingContinuation { read = $0 }
  }
  func setApiKeyAsync(account: String, secret: String) async throws {
    writes += 1
    try await withCheckedThrowingContinuation { write = $0 }
  }
  func resolveRead(revision: UInt64? = nil, accountErrors: [String: String] = [:]) {
    let continuation = read
    read = nil
    continuation?.resume(returning: CsProviderAccessSnapshot(
      providers: [provider], accountErrors: accountErrors, keyStatus: .sampleAllSet, sttLanes: [],
      revision: revision ?? self.revision))
  }
  func resolveWrite(success: Bool) {
    let continuation = write
    write = nil
    if success { revision += 1; continuation?.resume() }
    else { continuation?.resume(throwing: Failure.denied) }
  }
}

private struct OrderingBridgeInstaller: AgentBridgeInstalling {
  func status() -> AgentBridgeInstallationStatus { .unavailable }
  func install(selectedClients: Set<AgentBridgeClient>) throws -> AgentBridgeInstallationStatus {
    throw AgentBridgeInstallationError.payloadUnavailable
  }
  func adoptManualSkill(client: AgentBridgeClient) throws -> AgentBridgeAdoptionResult {
    throw AgentBridgeInstallationError.payloadUnavailable
  }
}

@MainActor
final class ProviderAccessOrderingTests: XCTestCase {
  private func makeModel(_ engine: ControlledProviderEngine) -> OnboardingViewModel {
    OnboardingViewModel(engine: engine, hotkeys: MockHotkeysEngine(),
      agentStatus: MockAgentStatusEngine(), agentBridge: OrderingBridgeInstaller(),
      probe: MockPermissionProbe(.allGranted))
  }

  private func load(_ model: OnboardingViewModel, _ engine: ControlledProviderEngine) async {
    model.refreshProviderAccess()
    await awaitCondition { engine.read != nil }
    engine.resolveRead()
    await awaitCondition { !model.providerAccessPending }
  }

  func testFocusRefreshCoalescesAndDeniedColdReadStaysUnresolved() async {
    let engine = ControlledProviderEngine()
    let model = makeModel(engine)
    model.refreshProviderAccess()
    await awaitCondition { engine.read != nil }
    for _ in 0..<20 { model.refreshProviderAccess() }
    XCTAssertEqual(engine.reads, 1)
    XCTAssertTrue(model.providerAccessPending)
    XCTAssertFalse(model.providerAccessResolved)
    let continuation = engine.read
    engine.read = nil
    continuation?.resume(throwing: ControlledProviderEngine.Failure.denied)
    await awaitCondition { !model.providerAccessPending }
    XCTAssertFalse(model.providerAccessResolved)
    XCTAssertNotNil(model.providerAccessError)
    await load(model, engine)
    XCTAssertTrue(model.providerAccessResolved)
    XCTAssertNil(model.providerAccessError)
  }

  func testContinueWaitsForDurableSaveAndFailureRetainsStepAndDraft() async {
    let engine = ControlledProviderEngine()
    let model = makeModel(engine)
    await load(model, engine)
    XCTAssertEqual(model.step, .apiKey)
    model.apiKeyDraft = "synthetic-key"
    model.advance()
    await awaitCondition { engine.write != nil }
    XCTAssertTrue(model.providerMutationPending)
    XCTAssertEqual(model.step, .apiKey)
    model.advance()
    model.back()
    XCTAssertEqual(engine.writes, 1)
    XCTAssertEqual(model.step, .apiKey)
    engine.resolveWrite(success: false)
    await awaitCondition { !model.providerMutationPending && engine.read != nil }
    engine.resolveRead()
    await awaitCondition { !model.providerAccessPending }
    XCTAssertEqual(model.apiKeyDraft, "synthetic-key")
    XCTAssertEqual(model.step, .apiKey)
    XCTAssertNotNil(model.lastError)
    model.advance()
    await awaitCondition { engine.write != nil }
    engine.resolveWrite(success: true)
    await awaitCondition { !model.providerMutationPending && engine.read != nil }
    engine.resolveRead()
    await awaitCondition { !model.providerAccessPending }
    XCTAssertEqual(model.step, .hotkeyMode)
    XCTAssertEqual(model.apiKeyDraft, "")
    XCTAssertNil(model.lastError)
  }

  func testSuccessfulSaveDoesNotClearNewerDraft() async {
    let engine = ControlledProviderEngine()
    let model = makeModel(engine)
    await load(model, engine)
    model.apiKeyDraft = "submitted"
    model.saveApiKey()
    await awaitCondition { engine.write != nil }
    model.apiKeyDraft = "newer draft"
    engine.resolveWrite(success: true)
    await awaitCondition { !model.providerMutationPending && engine.read != nil }
    engine.resolveRead()
    await awaitCondition { !model.providerAccessPending }
    XCTAssertEqual(model.apiKeyDraft, "newer draft")
  }

  func testOldSnapshotAfterMutationIsRejectedAndFollowUpPublishesNewRevision() async {
    let engine = ControlledProviderEngine()
    let model = makeModel(engine)
    await load(model, engine)
    let oldRevision = engine.revision
    model.refreshProviderAccess()
    await awaitCondition { engine.read != nil }
    model.apiKeyDraft = "synthetic-key"
    model.saveApiKey()
    await awaitCondition { engine.write != nil }
    engine.resolveWrite(success: true)
    await awaitCondition { !model.providerMutationPending }
    engine.provider.apiKeySet = false
    engine.resolveRead(revision: oldRevision)
    await awaitCondition { engine.read != nil }
    XCTAssertEqual(engine.reads, 3, "one follow-up replaces the invalidated read")
    engine.resolveRead()
    await awaitCondition { !model.providerAccessPending }
    XCTAssertFalse(model.selectedProviderKeySet)
    XCTAssertNil(model.providerAccessError)
  }
  func testContinueWithNewerDraftKeepsApiKeyStepAfterEarlierSaveSucceeds() async {
    let engine = ControlledProviderEngine()
    let model = makeModel(engine)
    await load(model, engine)
    model.apiKeyDraft = "submitted"
    model.advance()
    await awaitCondition { engine.write != nil }
    let selected = model.selectedProviderId
    model.selectProvider("anthropic-messages")
    XCTAssertEqual(model.selectedProviderId, selected, "provider cannot change under an active save")
    model.apiKeyDraft = "newer draft"
    engine.resolveWrite(success: true)
    await awaitCondition { !model.providerMutationPending && engine.read != nil }
    engine.resolveRead()
    await awaitCondition { !model.providerAccessPending }
    XCTAssertEqual(model.apiKeyDraft, "newer draft")
    XCTAssertEqual(model.step, .apiKey, "Continue must not skip the new unsaved draft")
  }

  func testUnavailableAccountKeepsResolvedRegistryAndDoesNotClaimReadiness() async {
    let engine = ControlledProviderEngine()
    engine.progress = 11
    engine.provider.apiKeySet = false
    engine.provider.accountSignedIn = false
    let model = makeModel(engine)
    model.refreshProviderAccess()
    await awaitCondition { engine.read != nil }
    engine.resolveRead(accountErrors: [engine.provider.id: "Account access unavailable"])
    await awaitCondition { !model.providerAccessPending }
    XCTAssertTrue(model.providerAccessResolved)
    XCTAssertNil(model.providerAccessError, "one account must not fail the whole snapshot")
    XCTAssertEqual(model.providers.map(\.id), [engine.provider.id])
    XCTAssertNotNil(model.selectedProviderAccountError)
    XCTAssertFalse(model.selectedProviderAccountConnected)
    XCTAssertFalse(model.selectedProviderKeySet)
    XCTAssertNil(model.readiness, "unknown account access cannot claim a ready agent")
  }

  func testUnavailableAccountPreservesIndependentApiKeyAndRecoversOnNextRead() async {
    let engine = ControlledProviderEngine()
    engine.progress = 11
    engine.provider.apiKeySet = true
    engine.provider.accountSignedIn = false
    let model = makeModel(engine)
    model.refreshProviderAccess()
    await awaitCondition { engine.read != nil }
    engine.resolveRead(accountErrors: [engine.provider.id: "Account access unavailable"])
    await awaitCondition { !model.providerAccessPending }
    XCTAssertTrue(model.selectedProviderKeySet)
    XCTAssertNotNil(model.selectedProviderAccountError)
    XCTAssertNotNil(model.readiness, "an independent API key remains usable")
    model.refreshProviderAccess()
    await awaitCondition { engine.read != nil }
    engine.resolveRead()
    await awaitCondition { !model.providerAccessPending }
    XCTAssertNil(model.selectedProviderAccountError)
    XCTAssertTrue(model.selectedProviderKeySet)
  }

  func testContinueWithDraftDuringColdProviderReadHasAnExplicitOutcome() async {
    let engine = ControlledProviderEngine()
    let model = makeModel(engine)
    model.refreshProviderAccess()
    await awaitCondition { engine.read != nil }
    XCTAssertFalse(model.providerAccessResolved)
    XCTAssertNil(model.selectedProvider)
    model.apiKeyDraft = "synthetic-unresolved-draft"
    model.saveApiKey()
    XCTAssertEqual(engine.writes, 0, "explicit Save cannot target unresolved credentials")
    model.advance()
    XCTAssertEqual(engine.writes, 0, "unknown provider account cannot accept a key write")
    XCTAssertEqual(model.apiKeyDraft, "synthetic-unresolved-draft")
    XCTAssertNotEqual(model.step, .apiKey)
    model.back()
    XCTAssertEqual(model.step, .apiKey)
    XCTAssertEqual(model.apiKeyDraft, "synthetic-unresolved-draft")
    engine.resolveRead()
    await awaitCondition { !model.providerAccessPending }
  }

  func testContinueWithDraftAfterFailedProviderReadHasAnExplicitOutcome() async {
    let engine = ControlledProviderEngine()
    let model = makeModel(engine)
    model.refreshProviderAccess()
    await awaitCondition { engine.read != nil }
    let pending = engine.read
    engine.read = nil
    pending?.resume(throwing: ControlledProviderEngine.Failure.denied)
    await awaitCondition { !model.providerAccessPending }
    XCTAssertFalse(model.providerAccessResolved)
    model.apiKeyDraft = "synthetic-denied-draft"
    model.saveApiKey()
    XCTAssertEqual(engine.writes, 0, "explicit Save cannot target unresolved credentials")
    model.advance()
    XCTAssertEqual(engine.writes, 0)
    XCTAssertEqual(model.apiKeyDraft, "synthetic-denied-draft")
    XCTAssertNotEqual(model.step, .apiKey)
    model.back()
    XCTAssertEqual(model.step, .apiKey)
    XCTAssertEqual(model.apiKeyDraft, "synthetic-denied-draft")
  }

}
