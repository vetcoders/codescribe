import XCTest

@testable import Codescribe

@MainActor
private final class ControlledProviderEngine: OnboardingEngine {
  enum Failure: Error { case denied }
  var progress: UInt32 = 9
  var reads = 0
  var writes = 0
  var keySaveAccounts: [String] = []
  var revision: UInt64 = 0
  var failProviderSelection = false
  var providerSelectionAttempts: [String] = []
  var configuredProviderId: String?
  var provider = CsProviderOption.sampleProviders[1]
  var additionalProviders: [CsProviderOption] = []
  var read: CheckedContinuation<CsProviderAccessSnapshot, Error>?
  var write: CheckedContinuation<Void, Error>?
  func shouldShowOnboarding() -> Bool { true }
  func onboardingProgress() -> UInt32 { progress }
  func saveOnboardingProgress(step: UInt32) { progress = step }
  func markOnboardingDone() {}
  func onboardingMode() -> String? { "agentic" }
  func setOnboardingMode(_ mode: String) throws {}
  func currentLanguage() -> CsLanguage { .auto }
  func assistiveProvider() -> String? { configuredProviderId ?? provider.id }
  func keyStatus() -> CsKeyStatus { .sampleAllSet }
  func availableProviders() -> [CsProviderOption] { [provider] }
  func setApiKey(account: String, secret: String) throws { XCTFail("sync mutation must not run") }
  func updateConfig(key: String, value: String) throws {
    if key == "LLM_ASSISTIVE_PROVIDER" {
      providerSelectionAttempts.append(value)
      if failProviderSelection { throw Failure.denied }
      configuredProviderId = value
    }
  }
  func providerAccessRevision() -> UInt64 { revision }
  func providerAccessSnapshot() async throws -> CsProviderAccessSnapshot {
    reads += 1
    return try await withCheckedThrowingContinuation { read = $0 }
  }
  func setApiKeyAsync(account: String, secret: String) async throws {
    writes += 1
    keySaveAccounts.append(account)
    try await withCheckedThrowingContinuation { write = $0 }
  }
  func resolveRead(revision: UInt64? = nil, accountErrors: [String: String] = [:]) {
    let continuation = read
    read = nil
    continuation?.resume(
      returning: CsProviderAccessSnapshot(
        providers: [provider] + additionalProviders, accountErrors: accountErrors,
        keyStatus: .sampleAllSet, sttLanes: [],
        revision: revision ?? self.revision))
  }
  func resolveWrite(success: Bool) {
    let continuation = write
    write = nil
    if success {
      revision += 1
      continuation?.resume()
    } else {
      continuation?.resume(throwing: Failure.denied)
    }
  }
}

private struct OrderingBridgeInstaller: AgentBridgeInstalling {
  var value: AgentBridgeInstallationStatus = .unavailable
  func status() -> AgentBridgeInstallationStatus { value }
  func install(selectedClients: Set<AgentBridgeClient>) throws -> AgentBridgeInstallationStatus {
    throw AgentBridgeInstallationError.payloadUnavailable
  }
  func adoptManualSkill(client: AgentBridgeClient) throws -> AgentBridgeAdoptionResult {
    throw AgentBridgeInstallationError.payloadUnavailable
  }
}

@MainActor
final class ProviderAccessOrderingTests: XCTestCase {
  private func makeModel(
    _ engine: ControlledProviderEngine,
    bridge: AgentBridgeInstalling = OrderingBridgeInstaller(),
    readiness: CsAgenticReadiness = .sample
  ) -> OnboardingViewModel {
    OnboardingViewModel(
      engine: engine, hotkeys: MockHotkeysEngine(),
      agentStatus: MockAgentStatusEngine(readiness: readiness), agentBridge: bridge,
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
    model.beginApiKeyEditing()
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
    XCTAssertTrue(model.apiKeyEditorExpanded)
    XCTAssertEqual(model.step, .apiKey)
    XCTAssertNotNil(model.apiKeySaveError)
    model.advance()
    await awaitCondition { engine.write != nil }
    engine.resolveWrite(success: true)
    await awaitCondition { !model.providerMutationPending && engine.read != nil }
    engine.resolveRead()
    await awaitCondition { !model.providerAccessPending }
    XCTAssertEqual(model.step, .hotkeyMode)
    XCTAssertEqual(model.apiKeyDraft, "")
    XCTAssertFalse(model.apiKeyEditorExpanded)
    XCTAssertNil(model.apiKeySaveError)
  }

  func testSuccessfulSaveDoesNotClearNewerDraft() async {
    let engine = ControlledProviderEngine()
    let model = makeModel(engine)
    await load(model, engine)
    model.beginApiKeyEditing()
    model.apiKeyDraft = "submitted"
    model.saveApiKey()
    await awaitCondition { engine.write != nil }
    model.apiKeyDraft = "newer draft"
    engine.resolveWrite(success: true)
    await awaitCondition { !model.providerMutationPending && engine.read != nil }
    engine.resolveRead()
    await awaitCondition { !model.providerAccessPending }
    XCTAssertEqual(model.apiKeyDraft, "newer draft")
    XCTAssertTrue(model.apiKeyEditorExpanded)
  }

  func testApiKeyEditorOpensExplicitlyAndCollapsesAfterSuccessfulSave() async {
    let engine = ControlledProviderEngine()
    let model = makeModel(engine)
    await load(model, engine)
    XCTAssertFalse(model.apiKeyEditorExpanded)
    model.beginApiKeyEditing()
    XCTAssertTrue(model.apiKeyEditorExpanded)
    model.apiKeyDraft = "submitted"
    model.saveApiKey()
    await awaitCondition { engine.write != nil }
    engine.resolveWrite(success: true)
    await awaitCondition { !model.providerMutationPending && engine.read != nil }
    engine.resolveRead()
    await awaitCondition { !model.providerAccessPending }
    XCTAssertFalse(model.apiKeyEditorExpanded)
    XCTAssertEqual(model.apiKeyDraft, "")
    XCTAssertEqual(model.step, .apiKey, "Save alone does not advance the wizard")
  }

  func testOptionalCustomProviderKeyCanBeEditedSavedAndRetriedWithoutChangingProvider() async {
    let engine = ControlledProviderEngine()
    engine.provider.id = "custom:optional-fixture"
    engine.provider.kind = "custom"
    engine.provider.apiKeyAccount = "LLM_CUSTOM_OPTIONAL_FIXTURE_API_KEY"
    engine.provider.keyRequired = false
    engine.provider.apiKeySet = false
    engine.provider.accountSignedIn = false
    engine.provider.accountLoginEnabled = false
    let model = makeModel(engine)
    await load(model, engine)
    guard model.apiKeySaveAvailable else {
      XCTFail("An optional custom-provider key must remain editable and saveable")
      return
    }
    XCTAssertFalse(model.selectedProviderKeySet)
    model.beginApiKeyEditing()
    XCTAssertTrue(model.apiKeyEditorExpanded)
    model.apiKeyDraft = "synthetic-optional-key"
    model.saveApiKey()
    await awaitCondition { engine.write != nil }
    engine.resolveWrite(success: false)
    await awaitCondition { !model.providerMutationPending && engine.read != nil }
    engine.resolveRead()
    await awaitCondition { !model.providerAccessPending }
    XCTAssertEqual(model.apiKeyDraft, "synthetic-optional-key")
    XCTAssertTrue(model.apiKeyEditorExpanded)
    XCTAssertNotNil(model.apiKeySaveError)
    XCTAssertEqual(engine.keySaveAccounts, [engine.provider.apiKeyAccount])
    model.saveApiKey()
    await awaitCondition { engine.write != nil }
    engine.resolveWrite(success: true)
    engine.provider.apiKeySet = true
    await awaitCondition { !model.providerMutationPending && engine.read != nil }
    engine.resolveRead()
    await awaitCondition { !model.providerAccessPending }
    XCTAssertEqual(
      engine.keySaveAccounts, [engine.provider.apiKeyAccount, engine.provider.apiKeyAccount])
    XCTAssertEqual(model.apiKeyDraft, "")
    XCTAssertFalse(model.apiKeyEditorExpanded)
    XCTAssertTrue(model.selectedProviderKeySet)
    XCTAssertNil(model.apiKeySaveError)
    XCTAssertEqual(model.selectedProviderId, engine.provider.id)
    XCTAssertTrue(engine.providerSelectionAttempts.isEmpty)
    XCTAssertEqual(model.step, .apiKey)
  }

  func testProviderWithoutAnApiKeyAccountCannotOpenOrSaveAKey() async {
    let engine = ControlledProviderEngine()
    engine.provider.apiKeyAccount = ""
    engine.provider.keyRequired = false
    let model = makeModel(engine)
    await load(model, engine)
    XCTAssertFalse(model.apiKeySaveAvailable)
    model.beginApiKeyEditing()
    XCTAssertFalse(model.apiKeyEditorExpanded)
    model.apiKeyDraft = "synthetic-unsupported-key"
    model.saveApiKey()
    XCTAssertEqual(engine.writes, 0)
    XCTAssertTrue(engine.keySaveAccounts.isEmpty)
  }

  func testProviderSwitchKeepsDraftsSeparateAndReturnsToCollapsedEditor() async throws {
    let engine = ControlledProviderEngine()
    let second = try XCTUnwrap(
      CsProviderOption.sampleProviders.first { $0.id != engine.provider.id && $0.keyRequired })
    engine.additionalProviders = [second]
    let model = makeModel(engine)
    await load(model, engine)
    let firstID = model.selectedProviderId
    model.beginApiKeyEditing()
    model.apiKeyDraft = "first-provider-draft"
    model.selectProvider(second.id)
    XCTAssertEqual(model.apiKeyDraft, "", "A draft cannot become another provider's credential")
    XCTAssertFalse(model.apiKeyEditorExpanded)
    model.beginApiKeyEditing()
    model.apiKeyDraft = "second-provider-draft"
    model.selectProvider(firstID)
    XCTAssertEqual(model.apiKeyDraft, "first-provider-draft")
    XCTAssertFalse(model.apiKeyEditorExpanded)
    model.selectProvider(second.id)
    XCTAssertEqual(model.apiKeyDraft, "second-provider-draft")
    XCTAssertEqual(engine.writes, 0, "Changing the picker never writes a secret")
  }

  func testProviderSelectionFailureKeepsCurrentProviderAndRetriesOnlySelection() async throws {
    let engine = ControlledProviderEngine()
    let second = try XCTUnwrap(
      CsProviderOption.sampleProviders.first { $0.id != engine.provider.id && $0.keyRequired })
    engine.additionalProviders = [second]
    let model = makeModel(engine)
    await load(model, engine)
    let firstID = model.selectedProviderId
    model.beginApiKeyEditing()
    model.apiKeyDraft = "draft-a"
    model.selectProvider(second.id)
    model.beginApiKeyEditing()
    model.apiKeyDraft = "draft-b"
    model.selectProvider(firstID)
    engine.failProviderSelection = true
    model.selectProvider(second.id)
    XCTAssertEqual(model.selectedProviderId, firstID)
    XCTAssertEqual(model.apiKeyDraft, "draft-a")
    XCTAssertFalse(model.apiKeyEditorExpanded)
    XCTAssertNotNil(model.providerSelectionError)
    XCTAssertNil(model.apiKeySaveError)
    model.beginApiKeyEditing()
    XCTAssertNil(model.apiKeySaveError, "Opening the key editor cannot reclassify another error")
    model.retryProviderSelection()
    XCTAssertEqual(engine.writes, 0)
    XCTAssertEqual(model.selectedProviderId, firstID)
    engine.failProviderSelection = false
    model.retryProviderSelection()
    XCTAssertEqual(model.selectedProviderId, second.id)
    XCTAssertEqual(model.apiKeyDraft, "draft-b")
    XCTAssertFalse(model.apiKeyEditorExpanded)
    XCTAssertNil(model.providerSelectionError)
    XCTAssertEqual(
      Array(engine.providerSelectionAttempts.suffix(3)), [second.id, second.id, second.id])
    XCTAssertEqual(engine.writes, 0)
  }

  func testDisplayedFallbackRequiresExplicitPersistenceAndRetainsFailedDraftForRetry() async {
    let engine = ControlledProviderEngine()
    engine.configuredProviderId = "custom:removed"
    let model = makeModel(engine)
    await load(model, engine)
    let fallback = engine.provider.id
    XCTAssertEqual(model.selectedProviderId, fallback)
    XCTAssertEqual(engine.assistiveProvider(), "custom:removed")
    XCTAssertTrue(engine.providerSelectionAttempts.isEmpty, "Registry refresh is read-only")
    model.advance()
    model.back()
    await awaitCondition { engine.read != nil }
    engine.resolveRead()
    await awaitCondition { !model.providerAccessPending }
    XCTAssertTrue(engine.providerSelectionAttempts.isEmpty, "Continue and Back do not normalize")

    model.beginApiKeyEditing()
    model.apiKeyDraft = "fallback-provider-draft"
    engine.failProviderSelection = true
    model.selectProvider(fallback)
    XCTAssertEqual(engine.providerSelectionAttempts, [fallback])
    XCTAssertEqual(engine.assistiveProvider(), "custom:removed")
    XCTAssertEqual(model.selectedProviderId, fallback)
    XCTAssertEqual(model.apiKeyDraft, "fallback-provider-draft")
    XCTAssertNotNil(model.providerSelectionError)
    XCTAssertNil(model.apiKeySaveError)

    engine.failProviderSelection = false
    model.retryProviderSelection()
    XCTAssertEqual(engine.providerSelectionAttempts, [fallback, fallback])
    XCTAssertEqual(engine.assistiveProvider(), fallback)
    XCTAssertEqual(model.apiKeyDraft, "fallback-provider-draft")
    XCTAssertNil(model.providerSelectionError)
    XCTAssertEqual(engine.writes, 0, "Selection retry never submits the key draft")
    model.selectProvider(fallback)
    XCTAssertEqual(
      engine.providerSelectionAttempts, [fallback, fallback], "Persisted choice is a no-op")
  }

  func testKeySaveFailureAndRetryDoNotChangeProviderSelectionOrGeneralError() async {
    let engine = ControlledProviderEngine()
    let model = makeModel(engine)
    await load(model, engine)
    model.lastError = "earlier setup failure"
    model.beginApiKeyEditing()
    model.apiKeyDraft = "key-draft"
    model.saveApiKey()
    await awaitCondition { engine.write != nil }
    engine.resolveWrite(success: false)
    await awaitCondition { !model.providerMutationPending && engine.read != nil }
    engine.resolveRead()
    await awaitCondition { !model.providerAccessPending }
    XCTAssertNotNil(model.apiKeySaveError)
    XCTAssertNil(model.providerSelectionError)
    XCTAssertEqual(model.lastError, "earlier setup failure")
    XCTAssertTrue(engine.providerSelectionAttempts.isEmpty)
    model.saveApiKey()
    await awaitCondition { engine.write != nil }
    XCTAssertEqual(engine.writes, 2)
    engine.resolveWrite(success: true)
    await awaitCondition { !model.providerMutationPending && engine.read != nil }
    engine.resolveRead()
    await awaitCondition { !model.providerAccessPending }
    XCTAssertNil(model.apiKeySaveError)
    XCTAssertEqual(model.lastError, "earlier setup failure")
    XCTAssertTrue(engine.providerSelectionAttempts.isEmpty)
  }

  func testGlobalProviderAndNativeFailuresDoNotClaimClientInstallationProblems() async {
    let installed = AgentBridgeInstallationStatus(
      payloadAvailable: true, bundleVersion: "fixture", installedClients: [.codex],
      installedPaths: ["/fixture/codex"], detail: "Installed")
    for nativeReady in [true, false] {
      let engine = ControlledProviderEngine()
      engine.progress = 11
      let model = makeModel(
        engine, bridge: OrderingBridgeInstaller(value: installed),
        readiness: CsAgenticReadiness(configPathDisplay: "", ready: nativeReady, rows: []))
      await load(model, engine)
      XCTAssertFalse(model.agentClientNeedsSetup(.codex), "Healthy selected client")
      XCTAssertFalse(model.agentClientNeedsSetup(.claudeCode), "Unselected client")
      if nativeReady {
        model.refreshProviderAccess()
        await awaitCondition { engine.read != nil }
        let pending = engine.read
        engine.read = nil
        pending?.resume(throwing: ControlledProviderEngine.Failure.denied)
        await awaitCondition { !model.providerAccessPending }
      }
      XCTAssertTrue(model.agentNeedsGlobalSetup)
      XCTAssertFalse(model.agentClientNeedsSetup(.codex))
      XCTAssertFalse(model.agentClientNeedsSetup(.claudeCode))
      model.toggleAgentClient(.claudeCode)
      XCTAssertTrue(model.agentClientNeedsSetup(.claudeCode), "Only the selected missing client")
      XCTAssertFalse(model.agentClientNeedsSetup(.codex))
    }
  }

  func testContinueDoesNotSaveRestoredDraftBehindCollapsedEditor() async throws {
    let engine = ControlledProviderEngine()
    let second = try XCTUnwrap(
      CsProviderOption.sampleProviders.first { $0.id != engine.provider.id && $0.keyRequired })
    engine.additionalProviders = [second]
    let model = makeModel(engine)
    await load(model, engine)
    let firstID = model.selectedProviderId
    model.beginApiKeyEditing()
    model.apiKeyDraft = "unsaved-provider-draft"
    model.selectProvider(second.id)
    model.selectProvider(firstID)
    XCTAssertFalse(model.apiKeyEditorExpanded)
    XCTAssertEqual(model.apiKeyDraft, "unsaved-provider-draft")

    model.advance()
    XCTAssertEqual(model.step, .hotkeyMode)
    XCTAssertEqual(engine.writes, 0, "Continue and Skip must not submit a hidden field")
    XCTAssertEqual(model.apiKeyDraft, "unsaved-provider-draft")
    await load(model, engine)
    model.back()
    model.beginApiKeyEditing()
    XCTAssertEqual(model.apiKeyDraft, "unsaved-provider-draft")
    await load(model, engine)
  }

  func testReadyStateRequiresCurrentSuccessfulProviderRead() async {
    let engine = ControlledProviderEngine()
    engine.progress = 11
    let model = makeModel(engine)
    await load(model, engine)
    XCTAssertTrue(model.agentBridgeReadyToGo)
    XCTAssertFalse(model.agentClientNeedsSetup(.codex))
    model.refreshProviderAccess()
    await awaitCondition { engine.read != nil }
    XCTAssertFalse(model.agentBridgeReadyToGo, "Pending credentials cannot claim ready")
    let pending = engine.read
    engine.read = nil
    pending?.resume(throwing: ControlledProviderEngine.Failure.denied)
    await awaitCondition { !model.providerAccessPending }
    XCTAssertNotNil(model.providerAccessError)
    XCTAssertFalse(model.agentBridgeReadyToGo, "A prior ready snapshot must not hide a failed read")
    XCTAssertTrue(model.agentNeedsGlobalSetup)
    await load(model, engine)
    XCTAssertTrue(model.agentBridgeReadyToGo)
    XCTAssertFalse(model.agentClientNeedsSetup(.codex))
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
    model.beginApiKeyEditing()
    model.apiKeyDraft = "submitted"
    model.advance()
    await awaitCondition { engine.write != nil }
    let selected = model.selectedProviderId
    model.selectProvider("anthropic-messages")
    XCTAssertEqual(
      model.selectedProviderId, selected, "provider cannot change under an active save")
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
