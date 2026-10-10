import Foundation

// Seam between the Settings screen and the REAL codescribe core through the
// UniFFI bridge (CodescribeConfig). The screen NEVER instantiates the bridge
// object directly — it talks to this protocol so the view-model can be seeded
// with mock data for #Preview, while the live app injects `RealSettingsEngine`.
//
// Credential acquisition and mutations run on the shared serial executor.
// Synchronous registry/presence methods project only the existing core cache.
//
// Config-write contract (router env keys, sourced from core/config/loader.rs):
//   WHISPER_LANGUAGE      "pl" | "en"
//   AI_FORMATTING_ENABLED "1" | "0"
//   FORMATTING_LEVEL      "off" | "correction" | "smart" | "max"
//   PASTE_MODE            "safe" | "comfort" | "off"
//   USE_LOCAL_STT         "1" | "0"
//   LOCAL_MODEL / STT_{FILE,LIVE}_ENDPOINT / LLM_<LANE>_PROVIDER / LLM_<LANE>_MODEL ...  free strings
//   (no endpoint keys: endpoints belong to providers — vendors factory-pinned, custom rows CRUD)
// Keychain accounts (CsKeyStatus, core/config/keychain.rs::KEYCHAIN_ACCOUNTS): one per vendor
//   (LLM_<VENDOR>_API_KEY), STT_FILE_API_KEY, STT_LIVE_API_KEY, GITHUB_TOKEN; custom rows carry
//   LLM_CUSTOM_<ID>_API_KEY

/// One physical executor for Settings and Setup credential operations. A view
/// cancelling its task does not cancel or free a synchronous Security call.
enum ProviderCredentialIO {
  private static let queue = DispatchQueue(label: "codescribe.provider-credentials", qos: .userInitiated)

  static func perform<T: Sendable>(
    _ operation: @escaping @Sendable () throws -> T
  ) async throws -> T {
    try await withCheckedThrowingContinuation { continuation in
      queue.async {
        do { continuation.resume(returning: try operation()) }
        catch { continuation.resume(throwing: error) }
      }
    }
  }
}

/// Subset of the codescribe config surface the Settings screen consumes.
@MainActor
protocol SettingsEngine {
  // Snapshot / location
  func loadSettings() -> CsSettings
  func configDir() -> String
  func shouldShowOnboarding() -> Bool
  func onboardingMode() -> String?
  func setOnboardingMode(mode: String) throws

  // Config writes (auto-tiered by the core router)
  func updateConfig(key: String, value: String) throws
  func updateConfigMany(entries: [CsConfigEntry]) throws
  func beginNewMaxConsultation() async throws -> String
  func pendingMaxToolApprovals() async throws -> [PendingToolApproval]
  func resolveMaxToolApproval(
    _ request: PendingToolApproval, approved: Bool, remember: Bool
  ) async throws -> Bool

  // Live audio hardware truth + explicit unset for the preferred device.
  func loadAudioInputSnapshot() throws -> CsAudioInputSnapshot
  func resetAudioInputDevice() throws

  // Local Whisper model catalog + picker selection (Settings → Dictation).
  func loadWhisperModelCatalog() -> CsWhisperModelCatalog
  func selectLocalWhisperModel(reference: String) async throws -> CsWhisperModelSwitch

  // Acoustic admission: the controller's own precondition for any take
  // (measured calibration for the device + armed Silero seal lane). Reads
  // never open a stream; calibration captures ~10 s through the real recorder.
  func loadAdmissionReadiness() async throws -> CsAdmissionReadiness
  func calibrateEnergy(seconds: UInt32) async throws -> CsEnergyCalibrationReport

  // Voice Lab quality truth (JSONL stays behind the Rust bridge)
  func loadQualityRecentListing(limit: UInt64) throws -> CsQualityListing
  func loadLexiconCustomEntries() throws -> [CsLexiconEntry]
  func loadRuleCandidates(minOccurrences: UInt64) throws -> [CsRuleCandidate]
  func finalizeVoiceLabCorrection(id: String, canonical: String) throws -> CsVoiceLabSaveResult
  func teachDictionaryFromStore() throws -> CsDictionaryTeachResult
  func teachDictionaryFromStoreAsync() async throws -> CsDictionaryTeachResult
  func teachSpan(variant: String, canonical: String, kind: String) throws -> CsQualityCommitResult

  // Keychain-backed API keys — presence booleans only, secrets never read back
  func keyStatus() -> CsKeyStatus
  /// Non-provider Keychain accounts (GitHub); provider accounts ride on
  /// `CsProviderOption`, STT accounts on `CsSttLane` (atomic endpoint + key).
  func serviceKeyAccounts() -> [String]
  /// Speech-to-text lanes, always two, File then Live (stt-lanes-v1 §C).
  func sttLanes() -> [CsSttLane]
  func setApiKey(account: String, secret: String) throws
  func clearApiKey(account: String) throws
  func testApiKey(account: String) throws -> CsApiKeyProbeResult
  func testApiKeyAsync(account: String) async throws -> CsApiKeyProbeResult

  // Provider registry (vendors + custom rows), lane binding, model discovery.
  func availableProviders() -> [CsProviderOption]
  func providerAccessSnapshot() async throws -> CsProviderAccessSnapshot
  func providerAccessRevision() -> UInt64
  /// Metadata + credential revision only; nil means freshness cannot be certified.
  func composerModelCacheStamp() -> String?
  func setApiKeyAsync(account: String, secret: String) async throws
  func clearApiKeyAsync(account: String) async throws
  func addCustomProviderAsync(draft: CsCustomProviderDraft) async throws -> CsProviderOption
  func updateCustomProviderAsync(id: String, draft: CsCustomProviderDraft) async throws -> CsProviderOption
  func removeCustomProviderAsync(id: String) async throws -> CsCustomProviderRemoval
  func signOutAccountAsync(providerId: String) async throws
  func addCustomProvider(draft: CsCustomProviderDraft) throws -> CsProviderOption
  func updateCustomProvider(id: String, draft: CsCustomProviderDraft) throws -> CsProviderOption
  func removeCustomProvider(id: String) throws -> CsCustomProviderRemoval
  func setLaneProvider(lane: CsLlmLane, providerId: String) throws
  func discoverModels(providerId: String) -> CsModelDiscovery
  func discoverModelsAsync(providerId: String) async -> CsModelDiscovery
  func startAccountLogin(providerId: String) throws -> CsAccountLoginResult
  // Blocks until the in-flight login completes/fails/times out — call from a
  // background queue only. Timeout shuts the local callback server down.
  func awaitAccountLogin(providerId: String, timeoutSeconds: UInt64) throws -> CsAccountLoginResult
  func awaitAccountLoginAsync(
    providerId: String, timeoutSeconds: UInt64
  ) async throws -> CsAccountLoginResult
  func cancelAccountLogin()
  func signOutAccount(providerId: String) throws

  // Editable BASE prompts
  func getFormattingPrompt() -> String
  func getAssistivePrompt() -> String
  func formattingPromptSnapshot() -> CsPromptSnapshot
  func formattingPromptSnapshot(level: String) throws -> CsPromptSnapshot
  func assistivePromptSnapshot() -> CsPromptSnapshot
  func defaultFormattingPrompt() -> String
  func defaultAssistivePrompt() -> String
  func setFormattingPrompt(content: String) throws
  func setFormattingPrompt(level: String, content: String) throws
  func setAssistivePrompt(content: String) throws
  func restoreFormattingPromptToDefault() throws
  func restoreFormattingPromptToDefault(level: String) throws
  func restoreAssistivePromptToDefault() throws

  // Recoverable reset: preview live impact, move local data to Trash, and
  // optionally remove Keychain keys. MCP-only clear is a separate concern.
  func resetPreview() -> CsResetPreview
  func resetAppData(includeKeys: Bool, includePrompts: Bool) throws
  func resetAgentPreview() -> CsAgentResetPreview
  func resetAgentData() throws
  func clearMcpConfiguration() throws
}

extension SettingsEngine {
  func providerAccessSnapshot() async throws -> CsProviderAccessSnapshot {
    CsProviderAccessSnapshot(providers: availableProviders(), accountErrors: [:], keyStatus: keyStatus(), sttLanes: sttLanes(), revision: 0)
  }
  func providerAccessRevision() -> UInt64 { 0 }
  func composerModelCacheStamp() -> String? { nil }
  func setApiKeyAsync(account: String, secret: String) async throws { try setApiKey(account: account, secret: secret) }
  func clearApiKeyAsync(account: String) async throws { try clearApiKey(account: account) }
  func addCustomProviderAsync(draft: CsCustomProviderDraft) async throws -> CsProviderOption { try addCustomProvider(draft: draft) }
  func updateCustomProviderAsync(id: String, draft: CsCustomProviderDraft) async throws -> CsProviderOption { try updateCustomProvider(id: id, draft: draft) }
  func removeCustomProviderAsync(id: String) async throws -> CsCustomProviderRemoval { try removeCustomProvider(id: id) }
  func signOutAccountAsync(providerId: String) async throws { try signOutAccount(providerId: providerId) }

  func teachDictionaryFromStoreAsync() async throws -> CsDictionaryTeachResult {
    try teachDictionaryFromStore()
  }
  func testApiKeyAsync(account: String) async throws -> CsApiKeyProbeResult {
    try testApiKey(account: account)
  }
  func discoverModelsAsync(providerId: String) async -> CsModelDiscovery {
    discoverModels(providerId: providerId)
  }
  func awaitAccountLoginAsync(
    providerId: String, timeoutSeconds: UInt64
  ) async throws -> CsAccountLoginResult {
    try awaitAccountLogin(providerId: providerId, timeoutSeconds: timeoutSeconds)
  }
}

// MARK: - Real engine (UniFFI bridge adapter)

/// Concrete adapter over the `CodescribeConfig` bridge object. Stateless: every
/// call reloads or writes through the live core, so Swift always sees on-disk
/// truth. Injected by App.swift for the live app.
final class RealSettingsEngine: SettingsEngine {
  private let config = CodescribeConfig()
  /// Facade over the process-global controller slots; constructing it creates
  /// no listener, controller, or tap.
  private let hotkeys = CodescribeHotkeys()

  func beginNewMaxConsultation() async throws -> String {
    try await hotkeys.beginNewMaxConsultation()
  }

  func pendingMaxToolApprovals() async throws -> [PendingToolApproval] {
    try await hotkeys.pendingMaxToolApprovals().map { request in
      PendingToolApproval(
        callID: request.callId, sessionID: request.sessionId, threadID: request.threadId,
        tool: request.tool, server: request.server, risk: request.risk,
        summary: request.summary, command: request.command, cwd: request.cwd, paths: request.paths
      )
    }
  }

  func resolveMaxToolApproval(
    _ request: PendingToolApproval, approved: Bool, remember: Bool
  ) async throws -> Bool {
    try await hotkeys.resolveMaxToolApproval(
      sessionId: request.sessionID, threadId: request.threadID, callId: request.callID,
      approved: approved, remember: remember
    )
  }

  func loadSettings() -> CsSettings { config.loadSettings() }
  func configDir() -> String { config.configDir() }
  func shouldShowOnboarding() -> Bool { config.shouldShowOnboarding() }
  func onboardingMode() -> String? { config.onboardingMode() }
  func setOnboardingMode(mode: String) throws { try config.setOnboardingMode(mode: mode) }

  func updateConfig(key: String, value: String) throws {
    try config.updateConfig(key: key, value: value)
  }
  func updateConfigMany(entries: [CsConfigEntry]) throws {
    try config.updateConfigMany(entries: entries)
  }
  func loadAudioInputSnapshot() throws -> CsAudioInputSnapshot {
    try audioInputSnapshot()
  }
  func resetAudioInputDevice() throws {
    try config.resetAudioInputDevice()
  }
  func loadWhisperModelCatalog() -> CsWhisperModelCatalog {
    whisperModelCatalog()
  }
  nonisolated func selectLocalWhisperModel(reference: String) async throws -> CsWhisperModelSwitch {
    try await setLocalWhisperModel(reference: reference)
  }
  func loadAdmissionReadiness() async throws -> CsAdmissionReadiness {
    try await hotkeys.admissionReadiness()
  }
  func calibrateEnergy(seconds: UInt32) async throws -> CsEnergyCalibrationReport {
    try await hotkeys.calibrateEnergy(seconds: seconds)
  }
  func loadQualityRecentListing(limit: UInt64) throws -> CsQualityListing {
    try qualityRecentListing(limit: limit)
  }
  func loadLexiconCustomEntries() throws -> [CsLexiconEntry] {
    try lexiconCustomEntries()
  }
  func loadRuleCandidates(minOccurrences: UInt64) throws -> [CsRuleCandidate] {
    try qualityRuleCandidates(minOccurrences: minOccurrences)
  }
  func finalizeVoiceLabCorrection(id: String, canonical: String) throws -> CsVoiceLabSaveResult {
    try qualityFinalizeCorrection(correctionId: id, canonical: canonical)
  }
  func teachDictionaryFromStore() throws -> CsDictionaryTeachResult {
    try qualityTeachDictionaryFromStore()
  }
  nonisolated func teachDictionaryFromStoreAsync() async throws -> CsDictionaryTeachResult {
    try await Task.detached(priority: .userInitiated) {
      try qualityTeachDictionaryFromStore()
    }.value
  }
  func teachSpan(variant: String, canonical: String, kind: String) throws -> CsQualityCommitResult {
    try qualityTeachSpan(variant: variant, canonical: canonical, kind: kind)
  }

  func keyStatus() -> CsKeyStatus { config.keyStatus() }
  func serviceKeyAccounts() -> [String] { config.serviceKeyAccounts() }
  func sttLanes() -> [CsSttLane] { config.sttLanes() }
  func setApiKey(account: String, secret: String) throws {
    try config.setApiKey(account: account, secret: secret)
  }
  func clearApiKey(account: String) throws { try config.clearApiKey(account: account) }
  func testApiKey(account: String) throws -> CsApiKeyProbeResult {
    try config.testApiKey(account: account)
  }
  nonisolated func testApiKeyAsync(account: String) async throws -> CsApiKeyProbeResult {
    try await Task.detached(priority: .userInitiated) {
      try CodescribeConfig().testApiKey(account: account)
    }.value
  }

  func providerAccessSnapshot() async throws -> CsProviderAccessSnapshot {
    try await ProviderCredentialIO.perform { try CodescribeConfig().providerAccessSnapshot() }
  }
  func providerAccessRevision() -> UInt64 { config.providerAccessRevision() }
  func composerModelCacheStamp() -> String? { config.composerModelCacheStamp() }
  func setApiKeyAsync(account: String, secret: String) async throws {
    try await ProviderCredentialIO.perform { try CodescribeConfig().setApiKey(account: account, secret: secret) }
  }
  func clearApiKeyAsync(account: String) async throws {
    try await ProviderCredentialIO.perform { try CodescribeConfig().clearApiKey(account: account) }
  }
  func addCustomProviderAsync(draft: CsCustomProviderDraft) async throws -> CsProviderOption {
    try await ProviderCredentialIO.perform { try CodescribeConfig().addCustomProvider(draft: draft) }
  }
  func updateCustomProviderAsync(id: String, draft: CsCustomProviderDraft) async throws -> CsProviderOption {
    try await ProviderCredentialIO.perform { try CodescribeConfig().updateCustomProvider(id: id, draft: draft) }
  }
  func removeCustomProviderAsync(id: String) async throws -> CsCustomProviderRemoval {
    try await ProviderCredentialIO.perform { try CodescribeConfig().removeCustomProvider(id: id) }
  }
  func signOutAccountAsync(providerId: String) async throws {
    try await ProviderCredentialIO.perform { try CodescribeConfig().signOutAccount(providerId: providerId) }
  }
  func availableProviders() -> [CsProviderOption] { config.availableProviders() }
  func addCustomProvider(draft: CsCustomProviderDraft) throws -> CsProviderOption {
    try config.addCustomProvider(draft: draft)
  }
  func updateCustomProvider(id: String, draft: CsCustomProviderDraft) throws -> CsProviderOption {
    try config.updateCustomProvider(id: id, draft: draft)
  }
  func removeCustomProvider(id: String) throws -> CsCustomProviderRemoval {
    try config.removeCustomProvider(id: id)
  }
  func setLaneProvider(lane: CsLlmLane, providerId: String) throws {
    try config.setLaneProvider(lane: lane, providerId: providerId)
  }
  func discoverModels(providerId: String) -> CsModelDiscovery {
    config.discoverModels(providerId: providerId)
  }
  nonisolated func discoverModelsAsync(providerId: String) async -> CsModelDiscovery {
    await Task.detached(priority: .userInitiated) {
      CodescribeConfig().discoverModels(providerId: providerId)
    }.value
  }
  func startAccountLogin(providerId: String) throws -> CsAccountLoginResult {
    try config.startAccountLogin(providerId: providerId)
  }
  func awaitAccountLogin(providerId: String, timeoutSeconds: UInt64) throws -> CsAccountLoginResult
  {
    try config.awaitAccountLogin(providerId: providerId, timeoutSeconds: timeoutSeconds)
  }
  nonisolated func awaitAccountLoginAsync(
    providerId: String, timeoutSeconds: UInt64
  ) async throws -> CsAccountLoginResult {
    try await Task.detached(priority: .userInitiated) {
      try CodescribeConfig().awaitAccountLogin(
        providerId: providerId,
        timeoutSeconds: timeoutSeconds
      )
    }.value
  }
  func cancelAccountLogin() { config.cancelAccountLogin() }
  func signOutAccount(providerId: String) throws {
    try config.signOutAccount(providerId: providerId)
  }

  func getFormattingPrompt() -> String { config.getFormattingPrompt() }
  func getAssistivePrompt() -> String { config.getAssistivePrompt() }
  func formattingPromptSnapshot() -> CsPromptSnapshot { config.formattingPromptSnapshot() }
  func formattingPromptSnapshot(level: String) throws -> CsPromptSnapshot {
    try config.formattingPromptSnapshotForLevel(level: level)
  }
  func assistivePromptSnapshot() -> CsPromptSnapshot { config.assistivePromptSnapshot() }
  func defaultFormattingPrompt() -> String { config.defaultFormattingPrompt() }
  func defaultAssistivePrompt() -> String { config.defaultAssistivePrompt() }
  func setFormattingPrompt(content: String) throws {
    try config.setFormattingPrompt(content: content)
  }
  func setFormattingPrompt(level: String, content: String) throws {
    try config.setFormattingPromptForLevel(level: level, content: content)
  }
  func setAssistivePrompt(content: String) throws {
    try config.setAssistivePrompt(content: content)
  }
  func restoreFormattingPromptToDefault() throws {
    try config.restoreFormattingPromptToDefault()
  }
  func restoreFormattingPromptToDefault(level: String) throws {
    try config.restoreFormattingPromptForLevelToDefault(level: level)
  }
  func restoreAssistivePromptToDefault() throws {
    try config.restoreAssistivePromptToDefault()
  }

  func resetPreview() -> CsResetPreview { config.resetPreview() }
  func resetAppData(includeKeys: Bool, includePrompts: Bool) throws {
    try config.resetAppData(includeKeys: includeKeys, includePrompts: includePrompts)
  }
  func resetAgentPreview() -> CsAgentResetPreview { config.resetAgentPreview() }
  func resetAgentData() throws { try config.resetAgentData() }
  func clearMcpConfiguration() throws { try config.clearMcpConfiguration() }
}

// MARK: - Mock engine (previews)

/// In-memory stand-in for #Preview and standalone rendering. Writes are no-ops;
/// the view-model also updates its own snapshot optimistically so the controls
/// still feel live in previews.
struct MockSettingsEngine: SettingsEngine {
  var settings: CsSettings = .sample
  var status: CsKeyStatus = .sampleAllSet
  var dir: String = "~/.codescribe"
  var onboarding: Bool = false
  var mode: String? = "agentic"
  var qualityRecords: [CsQualityRecord] = []
  var lexiconEntries: [CsLexiconEntry] = []
  var unchangedQualityTakes: UInt64 = 0
  var qualityRecordsLoader: (() throws -> [CsQualityRecord])?
  var lexiconEntriesLoader: (() throws -> [CsLexiconEntry])?
  var audioSnapshot: CsAudioInputSnapshot = .sample
  var admissionReadiness: CsAdmissionReadiness = .sampleGranted
  var calibrationReport: CsEnergyCalibrationReport = .sample
  var calibrateEnergyObserver: ((UInt32) throws -> CsEnergyCalibrationReport)?
  var beginNewMaxConsultationObserver: (() async throws -> String)?
  var pendingMaxApprovalsObserver: (() async throws -> [PendingToolApproval])?
  var resolveMaxApprovalObserver: ((PendingToolApproval, Bool, Bool) async throws -> Bool)?
  var resetPreviewValue: CsResetPreview = .sample
  var agentResetPreviewValue: CsAgentResetPreview = .sample
  var formattingSnapshot: CsPromptSnapshot = .sampleFormatting
  var assistiveSnapshot: CsPromptSnapshot = .sampleAssistive
  /// Prompt restores land here (reference-typed): after a restore the mock
  /// reports the built-in fallback, as the real engine does once the custom
  /// file is removed.
  var promptStore: MockPromptStore = MockPromptStore()
  var promptSaveObserver: ((String, String) throws -> Void)?
  var promptRestoreObserver: ((String) throws -> Void)?
  var resetAppDataObserver: ((Bool, Bool) throws -> Void)?
  var resetAgentDataObserver: (() throws -> Void)?
  var clearMcpConfigurationObserver: (() throws -> Void)?
  var settingsLoader: (() -> CsSettings)?
  /// Custom rows + lane bindings persist across calls (reference-typed, like settings.json).
  var providerStore: MockProviderStore = MockProviderStore()
  var updateConfigManyObserver: (([CsConfigEntry]) throws -> Void)?
  var resetAudioInputDeviceObserver: (() throws -> Void)?
  /// Preview seed for the local-Whisper picker; the observer records selections.
  var whisperCatalog: CsWhisperModelCatalog = .sample
  var selectLocalWhisperModelObserver: ((String) async throws -> CsWhisperModelSwitch)?
  var voiceLabEditObserver: ((String, String) throws -> CsVoiceLabSaveResult)?
  var ruleCandidates: [CsRuleCandidate] = []
  var ruleCandidatesLoader: (() throws -> [CsRuleCandidate])?
  var teachSpanObserver: ((String, String, String) throws -> CsQualityCommitResult)?
  /// Controlled acquisition for previews and integrator ordering witnesses.
  var providerAccessSnapshotLoader: (() async throws -> CsProviderAccessSnapshot)?
  var composerStampProvider: (() -> String?)?
  var providersReader: (() -> [CsProviderOption])?
  var discoveryLoader: ((String) -> CsModelDiscovery)?
  // Keep the long-standing config observer last so existing trailing-closure
  // call sites continue to bind to config writes, not Voice Lab edits.
  var updateConfigObserver: ((String, String) throws -> Void)?

  func loadSettings() -> CsSettings { settingsLoader?() ?? settings }
  func pendingMaxToolApprovals() async throws -> [PendingToolApproval] {
    try await pendingMaxApprovalsObserver?() ?? []
  }
  func resolveMaxToolApproval(
    _ request: PendingToolApproval, approved: Bool, remember: Bool
  ) async throws -> Bool {
    try await resolveMaxApprovalObserver?(request, approved, remember) ?? false
  }
  func beginNewMaxConsultation() async throws -> String {
    guard let beginNewMaxConsultationObserver else {
      throw NSError(
        domain: "Codescribe.Preview", code: 1,
        userInfo: [NSLocalizedDescriptionKey: "Consultation reset is unavailable in this preview."]
      )
    }
    return try await beginNewMaxConsultationObserver()
  }
  func configDir() -> String { dir }
  func shouldShowOnboarding() -> Bool { onboarding }
  func onboardingMode() -> String? { mode }
  func setOnboardingMode(mode: String) throws {}

  func updateConfig(key: String, value: String) throws {
    try updateConfigObserver?(key, value)
  }
  func updateConfigMany(entries: [CsConfigEntry]) throws {
    try updateConfigManyObserver?(entries)
  }
  func loadAudioInputSnapshot() throws -> CsAudioInputSnapshot { audioSnapshot }
  func resetAudioInputDevice() throws {
    try resetAudioInputDeviceObserver?()
  }
  func loadWhisperModelCatalog() -> CsWhisperModelCatalog { whisperCatalog }
  func selectLocalWhisperModel(reference: String) async throws -> CsWhisperModelSwitch {
    if let selectLocalWhisperModelObserver {
      return try await selectLocalWhisperModelObserver(reference)
    }
    return CsWhisperModelSwitch(applied: true, pending: false)
  }
  func loadAdmissionReadiness() async throws -> CsAdmissionReadiness { admissionReadiness }
  func calibrateEnergy(seconds: UInt32) async throws -> CsEnergyCalibrationReport {
    if let calibrateEnergyObserver {
      return try calibrateEnergyObserver(seconds)
    }
    return calibrationReport
  }
  func loadQualityRecentListing(limit: UInt64) throws -> CsQualityListing {
    let records = try qualityRecordsLoader?() ?? qualityRecords
    return CsQualityListing(
      records: Array(records.prefix(Int(clamping: limit))),
      unchangedTakes: unchangedQualityTakes,
      totalCorrections: UInt64(records.count)
    )
  }
  func loadLexiconCustomEntries() throws -> [CsLexiconEntry] {
    try lexiconEntriesLoader?() ?? lexiconEntries
  }
  func loadRuleCandidates(minOccurrences: UInt64) throws -> [CsRuleCandidate] {
    try ruleCandidatesLoader?() ?? ruleCandidates
  }
  func finalizeVoiceLabCorrection(id: String, canonical: String) throws -> CsVoiceLabSaveResult {
    if let voiceLabEditObserver {
      return try voiceLabEditObserver(id, canonical)
    }
    guard let record = qualityRecords.first(where: { $0.id == id }) else {
      throw NSError(domain: "VoiceLab", code: 404)
    }
    return CsVoiceLabSaveResult(
      record: CsQualityRecord(
        id: record.id,
        revision: record.revision + 1,
        rawText: record.rawText,
        variant: record.variant,
        editedText: canonical,
        action: "edit",
        editProvenance: "manual_human",
        timestampMs: record.timestampMs,
        avgLogprob: nil,
        speechPct: nil,
        confidenceFlags: []
      ),
      pairsLearned: 0,
      lexiconError: nil
    )
  }
  func teachDictionaryFromStore() throws -> CsDictionaryTeachResult {
    // Preview / mock: treat current lexicon as already taught.
    let total = UInt32(lexiconEntries.count)
    let fromCorrection = UInt32(lexiconEntries.filter { $0.source == "correction" }.count)
    return CsDictionaryTeachResult(
      fromCorrections: 0,
      fromProposed: 0,
      totalRules: total,
      rulesFromCorrectionSource: fromCorrection
    )
  }
  func teachSpan(variant: String, canonical: String, kind: String) throws -> CsQualityCommitResult {
    if let teachSpanObserver {
      return try teachSpanObserver(variant, canonical, kind)
    }
    return CsQualityCommitResult(
      pairsLearned: 1,
      evidenceOnly: false,
      acknowledgement: "Saved — 1 rule learned",
      teachSeen: nil,
      teachRequired: nil
    )
  }

  func providerAccessSnapshot() async throws -> CsProviderAccessSnapshot {
    if let providerAccessSnapshotLoader { return try await providerAccessSnapshotLoader() }
    return CsProviderAccessSnapshot(
      providers: availableProviders(), accountErrors: [:], keyStatus: keyStatus(),
      sttLanes: sttLanes(), revision: 0)
  }
  func composerModelCacheStamp() -> String? { composerStampProvider?() }
  func keyStatus() -> CsKeyStatus { status }
  func serviceKeyAccounts() -> [String] { ["GITHUB_TOKEN"] }
  /// Two sample lanes whose endpoint and key presence follow the mock's
  /// settings / status, so a persisted `STT_*_ENDPOINT` write is witnessable.
  func sttLanes() -> [CsSttLane] {
    let loaded = loadSettings()
    var file = CsSttLane.sampleFile
    file.endpoint = loaded.sttFileEndpoint
    file.apiKeySet = status.sttFileApiKeySet
    var live = CsSttLane.sampleLive
    live.endpoint = loaded.sttLiveEndpoint
    live.apiKeySet = status.sttLiveApiKeySet
    return [file, live]
  }
  func setApiKey(account: String, secret: String) throws {}
  func clearApiKey(account: String) throws {}
  func testApiKey(account: String) throws -> CsApiKeyProbeResult {
    CsApiKeyProbeResult.sample(account: account)
  }

  func availableProviders() -> [CsProviderOption] {
    providersReader?() ?? (CsProviderOption.sampleProviders + providerStore.custom)
  }
  func addCustomProvider(draft: CsCustomProviderDraft) throws -> CsProviderOption {
    try providerStore.add(draft)
  }
  func updateCustomProvider(id: String, draft: CsCustomProviderDraft) throws -> CsProviderOption {
    try providerStore.update(id: id, draft: draft)
  }
  func removeCustomProvider(id: String) throws -> CsCustomProviderRemoval {
    try providerStore.remove(id: id)
  }
  func setLaneProvider(lane: CsLlmLane, providerId: String) throws {
    try providerStore.setLane(lane, providerId: providerId, known: availableProviders())
  }
  func discoverModels(providerId: String) -> CsModelDiscovery {
    discoveryLoader?(providerId) ?? CsModelDiscovery.sample(for: providerId)
  }
  func startAccountLogin(providerId: String) throws -> CsAccountLoginResult {
    CsAccountLoginResult(
      providerId: providerId,
      status: "blocked",
      message: "awaiting app registration",
      authUrl: nil,
      signedIn: false,
      clientIdConfigured: false
    )
  }

  func awaitAccountLogin(providerId: String, timeoutSeconds: UInt64) throws -> CsAccountLoginResult
  {
    CsAccountLoginResult(
      providerId: providerId,
      status: "idle",
      message: "no sign-in in progress",
      authUrl: nil,
      signedIn: false,
      clientIdConfigured: false
    )
  }
  func cancelAccountLogin() {}
  func signOutAccount(providerId: String) throws {}

  func getFormattingPrompt() -> String { CsSettings.samplePrompt }
  func getAssistivePrompt() -> String { CsSettings.sampleAssistivePrompt }
  func formattingPromptSnapshot() -> CsPromptSnapshot {
    promptStore.restored["correction"] ?? formattingSnapshot
  }
  func formattingPromptSnapshot(level: String) throws -> CsPromptSnapshot {
    if let restored = promptStore.restored[level] { return restored }
    switch level {
    case "correction": return formattingSnapshot
    case "smart": return .sampleFormattingSmart
    case "max": return .sampleFormattingMax
    default: throw NSError(domain: "FormattingPolicy", code: 1)
    }
  }
  func assistivePromptSnapshot() -> CsPromptSnapshot {
    promptStore.restored["assistive"] ?? assistiveSnapshot
  }
  func defaultFormattingPrompt() -> String { CsSettings.samplePrompt }
  func defaultAssistivePrompt() -> String { CsSettings.sampleAssistivePrompt }
  func setFormattingPrompt(content: String) throws {
    try promptSaveObserver?("formatting", content)
  }
  func setFormattingPrompt(level: String, content: String) throws {
    try promptSaveObserver?(level, content)
  }
  func setAssistivePrompt(content: String) throws {
    try promptSaveObserver?("assistive", content)
  }
  func restoreFormattingPromptToDefault() throws {
    try promptRestoreObserver?("formatting")
    promptStore.restored["correction"] = .builtIn(
      formattingSnapshot, content: CsSettings.samplePrompt)
  }
  func restoreFormattingPromptToDefault(level: String) throws {
    try promptRestoreObserver?(level)
    let current = try formattingPromptSnapshot(level: level)
    promptStore.restored[level] = .builtIn(current, content: current.content)
  }
  func restoreAssistivePromptToDefault() throws {
    try promptRestoreObserver?("assistive")
    promptStore.restored["assistive"] = .builtIn(
      assistiveSnapshot, content: CsSettings.sampleAssistivePrompt)
  }
  func resetPreview() -> CsResetPreview { resetPreviewValue }
  func resetAppData(includeKeys: Bool, includePrompts: Bool) throws {
    try resetAppDataObserver?(includeKeys, includePrompts)
  }
  func resetAgentPreview() -> CsAgentResetPreview { agentResetPreviewValue }
  func resetAgentData() throws { try resetAgentDataObserver?() }
  func clearMcpConfiguration() throws {
    try clearMcpConfigurationObserver?()
  }
}

/// Mock-side stand-in for the core registry's custom CRUD + lane bindings —
/// reference-typed so the value-typed engine observes its own writes. Validation
/// covers only what tests read; the real rules live in `core/llm/provider.rs`.
@MainActor
/// Snapshots the mock reports after a prompt restore, keyed by level or
/// "assistive". Reference-typed so the struct engine can record them.
final class MockPromptStore {
  var restored: [String: CsPromptSnapshot] = [:]
}

final class MockProviderStore {
  enum Failure: Error, Equatable {
    case emptyName
    case invalidEndpoint(String)
    case unknownProvider(String)
  }

  var custom: [CsProviderOption] = []
  var laneProviders: [CsLlmLane: String] = [:]

  /// Slug + scheme check in the core's shape; the row id is `custom:<slug>`.
  private func row(_ draft: CsCustomProviderDraft, keySet: Bool) throws -> CsProviderOption {
    let slug = String(draft.name.lowercased().map { $0.isLetter || $0.isNumber ? $0 : "-" })
      .trimmingCharacters(in: CharacterSet(charactersIn: "-"))
    guard !slug.isEmpty else { throw Failure.emptyName }
    guard draft.endpoint.hasPrefix("http://") || draft.endpoint.hasPrefix("https://") else {
      throw Failure.invalidEndpoint(draft.endpoint)
    }
    let account = slug.uppercased().replacingOccurrences(of: "-", with: "_")
    return .row(
      id: "custom:\(slug)", kind: "custom", name: draft.name, wire: draft.wire,
      endpoint: draft.endpoint, account: "LLM_CUSTOM_\(account)_API_KEY", keySet: keySet)
  }

  /// Bridge rows are addressed by the bare slug (§D 17:55Z); the picker id has the prefix.
  private func index(of slug: String) throws -> Int {
    guard let index = custom.firstIndex(where: { $0.id == "custom:\(slug)" }) else {
      throw Failure.unknownProvider(slug)
    }
    return index
  }

  func add(_ draft: CsCustomProviderDraft) throws -> CsProviderOption {
    let option = try row(draft, keySet: draft.apiKey != nil)
    custom.append(option)
    return option
  }

  /// Id (and therefore the key account) is immutable; only name/wire/endpoint move.
  func update(id: String, draft: CsCustomProviderDraft) throws -> CsProviderOption {
    let index = try index(of: id)
    var option = try row(draft, keySet: custom[index].apiKeySet || draft.apiKey != nil)
    option.id = custom[index].id
    option.apiKeyAccount = custom[index].apiKeyAccount
    custom[index] = option
    return option
  }

  func remove(id: String) throws -> CsCustomProviderRemoval {
    let removed = try custom.remove(at: index(of: id))
    let reset = [CsLlmLane.formatting, .assistive].filter { laneProviders[$0] == removed.id }
    for lane in reset { laneProviders[lane] = "openai-responses" }
    return CsCustomProviderRemoval(id: id, lanesReset: reset)
  }

  func setLane(_ lane: CsLlmLane, providerId: String, known: [CsProviderOption]) throws {
    guard known.contains(where: { $0.id == providerId }) else {
      throw Failure.unknownProvider(providerId)
    }
    laneProviders[lane] = providerId
  }

  /// Resolved-lane projection the way the loader would seal it.
  func runtimeLane(_ lane: CsLlmLane, model: String = "gpt-5.2") -> CsRuntimeLlmLane {
    let providerId = laneProviders[lane] ?? "openai-responses"
    let provider = (CsProviderOption.sampleProviders + custom).first { $0.id == providerId }
    return CsRuntimeLlmLane(
      lane: lane,
      providerId: providerId,
      providerDisplayName: provider?.displayName ?? providerId,
      wire: provider?.wire ?? "responses",
      endpoint: provider?.endpoint ?? "",
      model: model,
      keyAccount: provider?.apiKeyAccount ?? "",
      keyPresent: provider?.apiKeySet ?? false,
      accountAuth: provider?.accountSignedIn ?? false,
      available: provider.map { $0.apiKeySet || $0.accountSignedIn || !$0.keyRequired } ?? false,
      unavailableReason: nil
    )
  }
}

// MARK: - Bridge value helpers

extension CsAudioInputSnapshot {
  static let sample = CsAudioInputSnapshot(
    devices: ["MacBook Pro Microphone", "USB Studio Mic"],
    configuredDevice: nil,
    runtimeDevice: "MacBook Pro Microphone",
    configuredDeviceAvailable: true,
    fallbackToDefault: false,
    runtimeConfigurationMatches: true
  )
}

extension CsAdmissionReadiness {
  static let sampleGranted = CsAdmissionReadiness(
    ready: true,
    code: "admission_granted",
    message: "",
    deviceName: "MacBook Pro Microphone",
    sampleRate: 48_000,
    calibrationVersion: "cal1-macbook-pro-microphone-1@48000hz",
    calibrationStatus: "sealed",
    calibrationPath: "~/Library/Application Support/Codescribe/energy-calibration.json",
    calibratedDevices: ["MacBook Pro Microphone"],
    sealLaneArmed: true,
    sealLaneSettingArmed: true,
    sealLaneSource: "settings",
    sealLaneEnv: "CODESCRIBE_SILERO_FUSION"
  )

  static let sampleMissing = CsAdmissionReadiness(
    ready: false,
    code: "admission_calibration_missing",
    message:
      "no acoustic calibration measured yet — Run Calibrate microphone in Settings › Audio (about 10 seconds of normal speech).",
    deviceName: nil,
    sampleRate: nil,
    calibrationVersion: nil,
    calibrationStatus: "missing",
    calibrationPath: "~/Library/Application Support/Codescribe/energy-calibration.json",
    calibratedDevices: [],
    sealLaneArmed: true,
    sealLaneSettingArmed: true,
    sealLaneSource: "settings",
    sealLaneEnv: "CODESCRIBE_SILERO_FUSION"
  )
}

extension CsEnergyCalibrationReport {
  static let sample = CsEnergyCalibrationReport(
    deviceName: "MacBook Pro Microphone",
    sampleRate: 48_000,
    measuredSeconds: 6.2,
    activeSpeechMedianDbfs: -38.4,
    noiseFloorDbfs: nil,
    peakDbfs: -12.5,
    existenceThresholdDbfs: -54.3,
    version: "cal1-macbook-pro-microphone-1",
    path: "~/Library/Application Support/Codescribe/energy-calibration.json"
  )
}

extension CsResetPreview {
  static let sample = CsResetPreview(
    audioFiles: 98,
    transcriptDays: 6,
    threads: 12,
    totalBytes: 31_981_568
  )
}

extension CsAgentResetPreview {
  static let sample = CsAgentResetPreview(
    threads: 3,
    files: 8,
    totalBytes: 12_288,
    secretsPresent: true
  )
}

extension CsPromptSnapshot {
  static let sampleFormatting = CsPromptSnapshot(
    content: CsSettings.samplePrompt,
    path: "~/.codescribe/prompts/formatting.txt",
    source: "custom_file",
    readError: nil
  )

  static let sampleFormattingSmart = CsPromptSnapshot(
    content: "Smart formatting preview prompt.",
    path: "~/.codescribe/prompts/formatting-smart.txt",
    source: "built_in_fallback",
    readError: nil
  )

  static let sampleFormattingMax = CsPromptSnapshot(
    content: "Max formatting preview prompt.",
    path: "~/.codescribe/prompts/formatting-max.txt",
    source: "built_in_fallback",
    readError: nil
  )

  static let sampleAssistive = CsPromptSnapshot(
    content: CsSettings.sampleAssistivePrompt,
    path: "~/.codescribe/prompts/assistive.txt",
    source: "custom_file",
    readError: nil
  )

  /// What the engine reports once the custom file is gone: same path, the
  /// built-in text, source `built_in_fallback`, no read error.
  static func builtIn(_ previous: CsPromptSnapshot, content: String) -> CsPromptSnapshot {
    CsPromptSnapshot(
      content: content, path: previous.path, source: "built_in_fallback", readError: nil)
  }
}

extension CsWhisperModelCatalog {
  /// Preview seed: one active models-dir row, one HF-cache row, one refused
  /// quantized row — the three states the picker must render.
  static let sample = CsWhisperModelCatalog(
    options: [
      CsWhisperModelOption(
        id: "whisper-large-v3-turbo",
        label: "Large v3 Turbo · FP16",
        path: "~/.codescribe/models/whisper-large-v3-turbo",
        source: "models_dir",
        usable: true,
        reason: nil,
        active: true
      ),
      CsWhisperModelOption(
        id: "mlx-community/whisper-large-v3",
        label: "Large v3 · FP16",
        path: "~/.cache/huggingface/hub/models--mlx-community--whisper-large-v3",
        source: "hf_cache",
        usable: true,
        reason: nil,
        active: false
      ),
      CsWhisperModelOption(
        id: "whisper-large-v3-turbo-q8",
        label: "Large v3 Turbo Q8",
        path: "~/.codescribe/models/whisper-large-v3-turbo-q8",
        source: "models_dir",
        usable: false,
        reason: "Quantized weights are not supported by the local engine",
        active: false
      ),
    ],
    configured: "whisper-large-v3-turbo",
    resolvedPath: "~/.codescribe/models/whisper-large-v3-turbo",
    loaded: "~/.codescribe/models/whisper-large-v3-turbo",
    overrideKind: nil
  )
}

extension CsLanguage {
  /// Two-letter code shown in the UI and written to `WHISPER_LANGUAGE`.
  var shortCode: String {
    switch self {
    case .auto: return "auto"
    case .polish: return "pl"
    case .english: return "en"
    }
  }

  /// Human-readable label for the language picker.
  var displayName: String {
    switch self {
    case .auto:
      return String(
        localized: "Auto",
        comment: "Dictation language picker: detect the spoken language automatically"
      )
    case .polish: return String(localized: "Polish", comment: "Dictation language name")
    case .english: return String(localized: "English", comment: "Dictation language name")
    }
  }
}

extension CsSettings {
  /// Sample config matching the mock (Polish whisper, local STT final-verdict).
  static let sample = CsSettings(
    holdExclusive: true,
    holdArmModifier: "shift",
    holdStartDelayMs: 250,
    doubleTapIntervalMs: 320,
    toggleSilenceSec: 1.5,
    whisperContextWindowSec: 8,
    whisperAdaptiveBuffer: false,
    formatOnDevice: false,
    lightPlusSentencePauseSec: 0.7,
    deferredInsertShortcut: "disabled",
    channelModifier: "ctrl",
    fnTapTogglesDictation: false,
    middleMouseActsAsFn: false,
    whisperLanguage: .polish,
    aiFormattingEnabled: true,
    pasteMode: .safe,
    transcriptSendMode: "end_of_utterance",
    transcriptTaggingEnabled: false,
    transcriptTagTemplate: "<codescribe mode=\"{mode}\" lang=\"{lang}\">\n{text}\n</codescribe>",
    aiMaxTokens: 1024,
    aiAssistiveMaxTokens: 2048,
    showTrayGlyph: true,
    showDockIcon: false,
    transcriptionOverlayEnabled: true,
    holdIndicator: true,
    holdBadgeSize: 12,
    holdBadgeOffsetX: 0,
    holdBadgeOffsetY: 0,
    overlayPositionMode: "snapped_top_right",
    overlayCustomX: nil,
    overlayCustomY: nil,
    beepOnStart: true,
    soundName: "Tink",
    soundVolume: 0.6,
    audioInputDevice: nil,
    audioRetention: "forever",
    historyEnabled: true,
    quickNotesEnabled: true,
    quickNotesSaveOnly: false,
    useLocalStt: true,
    localModel: "whisper-large-v3-turbo",
    sttFileEndpoint: nil,
    sttLiveEndpoint: nil,
    restoreClipboard: true,
    restoreClipboardDelayMs: 200,
    startAtLogin: false,
    agentEnterSends: true,
    agentAutoSend: false,
    dumpAudioLogs: false,
    // Contract §C: lane = provider ref + model; no endpoint fields on CsSettings.
    llmFormattingProvider: "openai-responses",
    llmFormattingModel: "gpt-4o-mini",
    llmAssistiveProvider: "openai-responses",
    llmAssistiveModel: "gpt-4o",
    formattingLevel: "correction",
    whisperModel: "whisper-large-v3-turbo",
    layeredTranscription: nil,
    agentWorkspaceRoots: ["~/.codescribe"],
    bufferDelayMs: nil,
    typingCps: nil,
    emitWordsMax: nil,
    bufferedInterimSec: nil,
    backendMaxUploadMb: nil,
    asrMode: nil,
    cloudConsent: nil,
    asrGatewayUrl: nil
  )

  static let samplePrompt =
    "Clean up the dictated text: fix punctuation and casing, drop filler words, keep the speaker's meaning intact."
  static let sampleAssistivePrompt =
    "You are a concise voice assistant. Answer the user's spoken request directly and act on it using the available tools."
}

extension CsKeyStatus {
  /// OpenAI + STT configured — used by the preview seed. Field order follows
  /// `KEYCHAIN_ACCOUNTS` (§B.3: Libraxis first).
  static let sampleAllSet = CsKeyStatus(
    llmLibraxisApiKeySet: false,
    llmOpenaiApiKeySet: true,
    llmXaiApiKeySet: false,
    llmAnthropicApiKeySet: false,
    sttFileApiKeySet: true,
    sttLiveApiKeySet: true,
    githubTokenSet: false
  )

  /// Presence boolean for a static Keychain account (`KEYCHAIN_ACCOUNTS`).
  /// Custom-provider accounts are not here: read `CsProviderOption.apiKeySet`.
  func isSet(account: String) -> Bool {
    switch account {
    case "LLM_LIBRAXIS_API_KEY": return llmLibraxisApiKeySet
    case "LLM_OPENAI_API_KEY": return llmOpenaiApiKeySet
    case "LLM_XAI_API_KEY": return llmXaiApiKeySet
    case "LLM_ANTHROPIC_API_KEY": return llmAnthropicApiKeySet
    case "STT_FILE_API_KEY": return sttFileApiKeySet
    case "STT_LIVE_API_KEY": return sttLiveApiKeySet
    case "GITHUB_TOKEN": return githubTokenSet
    default: return false
    }
  }
}

extension CsApiKeyProbeResult {
  static func sample(account: String) -> CsApiKeyProbeResult {
    CsApiKeyProbeResult(
      account: account,
      status: .ok,
      message: "key accepted and quota available",
      probedEndpoint: nil
    )
  }
}

extension CsSttLane {
  /// Mirrors `SttLane::File` (§B.0): https multipart or NDJSON `:stream`.
  static let sampleFile = CsSttLane(
    id: "file",
    title: "File transcription",
    accepts: "multipart /v1/audio/transcriptions · NDJSON …:stream",
    placeholder: "https://…/v1/audio/transcriptions",
    endpoint: nil,
    endpointWireKey: "STT_FILE_ENDPOINT",
    keyAccount: "STT_FILE_API_KEY",
    apiKeySet: false
  )
  /// Mirrors `SttLane::Live` (§B.0): the wss protocol ids.
  static let sampleLive = CsSttLane(
    id: "live",
    title: "Live transcript",
    accepts: "stt-ws-v1 · xAI /v1/stt",
    placeholder: "wss://…/v1/audio/transcribe",
    endpoint: nil,
    endpointWireKey: "STT_LIVE_ENDPOINT",
    keyAccount: "STT_LIVE_API_KEY",
    apiKeySet: false
  )
}

extension CsProviderOption {
  /// Row shape shared by the vendor seed and the mock custom store. Vendors
  /// require a key; custom hosts are key-optional; `login` marks OAuth vendors.
  static func row(
    id: String, kind: String, name: String, wire: String, endpoint: String, account: String,
    keySet: Bool = false, login: Bool = false
  ) -> CsProviderOption {
    CsProviderOption(
      id: id, kind: kind, displayName: name, wire: wire, endpoint: endpoint,
      apiKeyAccount: account, apiKeySet: keySet, keyRequired: kind == "vendor",
      accountSignedIn: false, accountLoginEnabled: login,
      accountStatusMessage: login ? "not signed in" : "provider account login unavailable",
      accountIdentity: nil, oauthClientId: nil)
  }

  /// Preview seed mirroring `ALL_PROVIDERS` with factory endpoints; the mock
  /// order is §B.3's (Libraxis, OpenAI, xAI, Anthropic) — I1 owns the real one.
  static let sampleProviders: [CsProviderOption] = [
    .row(
      id: "libraxis-responses", kind: "vendor", name: "Libraxis", wire: "responses",
      endpoint: "https://api.libraxis.com/v1/responses", account: "LLM_LIBRAXIS_API_KEY"),
    .row(
      id: "openai-responses", kind: "vendor", name: "OpenAI", wire: "responses",
      endpoint: "https://api.openai.com/v1/responses", account: "LLM_OPENAI_API_KEY",
      keySet: true, login: true),
    .row(
      id: "xai-responses", kind: "vendor", name: "xAI", wire: "responses",
      endpoint: "https://api.x.ai/v1/responses", account: "LLM_XAI_API_KEY", login: true),
    .row(
      id: "anthropic-messages", kind: "vendor", name: "Anthropic", wire: "messages",
      endpoint: "https://api.anthropic.com/v1/messages", account: "LLM_ANTHROPIC_API_KEY"),
  ]
}

extension CsModelDiscovery {
  static func sample(for providerId: String) -> CsModelDiscovery {
    switch providerId {
    case "anthropic-messages":
      return CsModelDiscovery(
        providerId: providerId,
        status: "no_key",
        message: "Add API key to discover models",
        models: []
      )
    default:
      let models = [CsSettings.sample.llmAssistiveModel, CsSettings.sample.llmFormattingModel]
        .compactMap { $0 }
        .map { CsModelOption(id: $0, displayName: $0) }
      return CsModelDiscovery(
        providerId: "openai-responses",
        status: "fresh",
        message: nil,
        models: models
      )
    }
  }
}
