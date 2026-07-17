import Foundation

// Seam between the Settings screen and the REAL codescribe core through the
// UniFFI bridge (CodescribeConfig). The screen NEVER instantiates the bridge
// object directly — it talks to this protocol so the view-model can be seeded
// with mock data for #Preview, while the live app injects `RealSettingsEngine`.
//
// All CodescribeConfig methods are synchronous and read/write on-disk truth
// (settings.json / .env / Keychain), so there are no Rust callbacks to hop onto
// the main actor here — the adapter just forwards.
//
// Config-write contract (router env keys, sourced from core/config/loader.rs):
//   WHISPER_LANGUAGE      "pl" | "en"
//   AI_FORMATTING_ENABLED "1" | "0"
//   FORMATTING_LEVEL      "raw" | "medium" | "creative"
//   USE_LOCAL_STT         "1" | "0"
//   LOCAL_MODEL / STT_ENDPOINT / LLM_MODEL / LLM_ENDPOINT / LLM_ASSISTIVE_* ...  free strings
// Keychain accounts (CsKeyStatus, core/config/keychain.rs::KEYCHAIN_ACCOUNTS):
//   LLM_API_KEY / STT_API_KEY / LLM_FORMATTING_API_KEY / LLM_ASSISTIVE_API_KEY / LLM_ANTHROPIC_API_KEY / GITHUB_TOKEN

/// Subset of the codescribe config surface the Settings screen consumes.
protocol SettingsEngine {
    // Snapshot / location
    func loadSettings() -> CsSettings
    func configDir() -> String
    func shouldShowOnboarding() -> Bool
    func onboardingMode() -> String?
    func setOnboardingMode(mode: String) throws

    /// Delegates to core lane_truth normalization (eliminates suffix-list dupe in Swift).
    func normalizeOpenaiResponsesEndpoint(_ endpoint: String) -> String

    // Config writes (auto-tiered by the core router)
    func updateConfig(key: String, value: String) throws
    func updateConfigMany(entries: [CsConfigEntry]) throws

    // Live audio hardware truth + explicit unset for the preferred device.
    func loadAudioInputSnapshot() throws -> CsAudioInputSnapshot
    func resetAudioInputDevice() throws

    // Voice Lab read-only quality truth (JSONL stays behind the Rust bridge)
    func loadQualityRecentRecords(limit: UInt64) throws -> [CsQualityRecord]
    func loadLexiconCustomEntries() throws -> [CsLexiconEntry]

    // Keychain-backed API keys — presence booleans only, secrets never read back
    func keyStatus() -> CsKeyStatus
    func keyAccounts() -> [String]
    func setApiKey(account: String, secret: String) throws
    func clearApiKey(account: String) throws
    func testApiKey(account: String) throws -> CsApiKeyProbeResult

    // Assistive/agent-lane providers and live model discovery
    func availableProviders() -> [CsProviderOption]
    func discoverModels(providerId: String) -> CsModelDiscovery
    func startAccountLogin(providerId: String) throws -> CsAccountLoginResult
    // Blocks until the in-flight login completes/fails/times out — call from a
    // background queue only. Timeout shuts the local callback server down.
    func awaitAccountLogin(providerId: String, timeoutSeconds: UInt64) throws -> CsAccountLoginResult
    func cancelAccountLogin()
    func signOutAccount(providerId: String) throws

    // Editable BASE prompts
    func getFormattingPrompt() -> String
    func getAssistivePrompt() -> String
    func formattingPromptSnapshot() -> CsPromptSnapshot
    func assistivePromptSnapshot() -> CsPromptSnapshot
    func defaultFormattingPrompt() -> String
    func defaultAssistivePrompt() -> String
    func setFormattingPrompt(content: String) throws
    func setAssistivePrompt(content: String) throws
    func restoreFormattingPromptToDefault() throws
    func restoreAssistivePromptToDefault() throws

    // Recoverable reset: preview live impact, move local data to Trash, and
    // optionally remove Keychain keys. MCP-only clear is a separate concern.
    func resetPreview() -> CsResetPreview
    func resetAppData(includeKeys: Bool, includePrompts: Bool) throws
    func clearMcpConfiguration() throws
}

// MARK: - Real engine (UniFFI bridge adapter)

/// Concrete adapter over the `CodescribeConfig` bridge object. Stateless: every
/// call reloads or writes through the live core, so Swift always sees on-disk
/// truth. Injected by App.swift for the live app.
final class RealSettingsEngine: SettingsEngine {
    private let config = CodescribeConfig()

    func loadSettings() -> CsSettings { config.loadSettings() }
    func configDir() -> String { config.configDir() }
    func shouldShowOnboarding() -> Bool { config.shouldShowOnboarding() }
    func onboardingMode() -> String? { config.onboardingMode() }
    func setOnboardingMode(mode: String) throws { try config.setOnboardingMode(mode: mode) }

    func normalizeOpenaiResponsesEndpoint(_ endpoint: String) -> String {
        config.normalizeOpenaiResponsesEndpoint(endpoint: endpoint)
    }

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
    func loadQualityRecentRecords(limit: UInt64) throws -> [CsQualityRecord] {
        try qualityRecentRecords(limit: limit)
    }
    func loadLexiconCustomEntries() throws -> [CsLexiconEntry] {
        try lexiconCustomEntries()
    }

    func keyStatus() -> CsKeyStatus { config.keyStatus() }
    func keyAccounts() -> [String] { config.keyAccounts() }
    func setApiKey(account: String, secret: String) throws {
        try config.setApiKey(account: account, secret: secret)
    }
    func clearApiKey(account: String) throws { try config.clearApiKey(account: account) }
    func testApiKey(account: String) throws -> CsApiKeyProbeResult {
        try config.testApiKey(account: account)
    }

    func availableProviders() -> [CsProviderOption] { config.availableProviders() }
    func discoverModels(providerId: String) -> CsModelDiscovery {
        config.discoverModels(providerId: providerId)
    }
    func startAccountLogin(providerId: String) throws -> CsAccountLoginResult {
        try config.startAccountLogin(providerId: providerId)
    }
    func awaitAccountLogin(providerId: String, timeoutSeconds: UInt64) throws -> CsAccountLoginResult {
        try config.awaitAccountLogin(providerId: providerId, timeoutSeconds: timeoutSeconds)
    }
    func cancelAccountLogin() { config.cancelAccountLogin() }
    func signOutAccount(providerId: String) throws {
        try config.signOutAccount(providerId: providerId)
    }

    func getFormattingPrompt() -> String { config.getFormattingPrompt() }
    func getAssistivePrompt() -> String { config.getAssistivePrompt() }
    func formattingPromptSnapshot() -> CsPromptSnapshot { config.formattingPromptSnapshot() }
    func assistivePromptSnapshot() -> CsPromptSnapshot { config.assistivePromptSnapshot() }
    func defaultFormattingPrompt() -> String { config.defaultFormattingPrompt() }
    func defaultAssistivePrompt() -> String { config.defaultAssistivePrompt() }
    func setFormattingPrompt(content: String) throws {
        try config.setFormattingPrompt(content: content)
    }
    func setAssistivePrompt(content: String) throws {
        try config.setAssistivePrompt(content: content)
    }
    func restoreFormattingPromptToDefault() throws {
        try config.restoreFormattingPromptToDefault()
    }
    func restoreAssistivePromptToDefault() throws {
        try config.restoreAssistivePromptToDefault()
    }

    func resetPreview() -> CsResetPreview { config.resetPreview() }
    func resetAppData(includeKeys: Bool, includePrompts: Bool) throws {
        try config.resetAppData(includeKeys: includeKeys, includePrompts: includePrompts)
    }
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
    var audioSnapshot: CsAudioInputSnapshot = .sample
    var resetPreviewValue: CsResetPreview = .sample
    var formattingSnapshot: CsPromptSnapshot = .sampleFormatting
    var assistiveSnapshot: CsPromptSnapshot = .sampleAssistive
    var promptSaveObserver: ((String, String) throws -> Void)?
    var promptRestoreObserver: ((String) throws -> Void)?
    var resetAppDataObserver: ((Bool, Bool) throws -> Void)?
    var clearMcpConfigurationObserver: (() throws -> Void)?
    var updateConfigManyObserver: (([CsConfigEntry]) throws -> Void)?
    var resetAudioInputDeviceObserver: (() throws -> Void)?
    var updateConfigObserver: ((String, String) throws -> Void)?

    func loadSettings() -> CsSettings { settings }
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
    func loadQualityRecentRecords(limit: UInt64) throws -> [CsQualityRecord] {
        Array(qualityRecords.prefix(Int(clamping: limit)))
    }
    func loadLexiconCustomEntries() throws -> [CsLexiconEntry] { lexiconEntries }

    func keyStatus() -> CsKeyStatus { status }
    func keyAccounts() -> [String] {
        [
            "LLM_API_KEY", "STT_API_KEY", "LLM_FORMATTING_API_KEY",
            "LLM_ASSISTIVE_API_KEY", "LLM_ANTHROPIC_API_KEY", "GITHUB_TOKEN",
        ]
    }
    func setApiKey(account: String, secret: String) throws {}
    func clearApiKey(account: String) throws {}
    func testApiKey(account: String) throws -> CsApiKeyProbeResult {
        CsApiKeyProbeResult.sample(account: account)
    }

    func availableProviders() -> [CsProviderOption] { CsProviderOption.sampleProviders }
    func discoverModels(providerId: String) -> CsModelDiscovery {
        CsModelDiscovery.sample(for: providerId)
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

    func normalizeOpenaiResponsesEndpoint(_ endpoint: String) -> String {
        // Mock: pass-through or minimal normalize for preview stability.
        var base = endpoint.trimmingCharacters(in: .whitespacesAndNewlines.union(.init(charactersIn: "/")))
        for s in ["/v1/responses", "/v1/chat/completions", "/v1/completions"] where base.hasSuffix(s) {
            base.removeLast(s.count)
            return base + "/v1/responses"
        }
        if base.hasSuffix("/v1") { base.removeLast(3) }
        return base + "/v1/responses"
    }
    func awaitAccountLogin(providerId: String, timeoutSeconds: UInt64) throws -> CsAccountLoginResult {
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
    func formattingPromptSnapshot() -> CsPromptSnapshot { formattingSnapshot }
    func assistivePromptSnapshot() -> CsPromptSnapshot { assistiveSnapshot }
    func defaultFormattingPrompt() -> String { CsSettings.samplePrompt }
    func defaultAssistivePrompt() -> String { CsSettings.sampleAssistivePrompt }
    func setFormattingPrompt(content: String) throws {
        try promptSaveObserver?("formatting", content)
    }
    func setAssistivePrompt(content: String) throws {
        try promptSaveObserver?("assistive", content)
    }
    func restoreFormattingPromptToDefault() throws {
        try promptRestoreObserver?("formatting")
    }
    func restoreAssistivePromptToDefault() throws {
        try promptRestoreObserver?("assistive")
    }
    func resetPreview() -> CsResetPreview { resetPreviewValue }
    func resetAppData(includeKeys: Bool, includePrompts: Bool) throws {
        try resetAppDataObserver?(includeKeys, includePrompts)
    }
    func clearMcpConfiguration() throws {
        try clearMcpConfigurationObserver?()
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

extension CsResetPreview {
    static let sample = CsResetPreview(
        audioFiles: 98,
        transcriptDays: 6,
        threads: 12,
        totalBytes: 31_981_568
    )
}

extension CsPromptSnapshot {
    static let sampleFormatting = CsPromptSnapshot(
        content: CsSettings.samplePrompt,
        path: "~/.codescribe/prompts/formatting.txt",
        source: "custom_file",
        readError: nil
    )

    static let sampleAssistive = CsPromptSnapshot(
        content: CsSettings.sampleAssistivePrompt,
        path: "~/.codescribe/prompts/assistive.txt",
        source: "custom_file",
        readError: nil
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
        case .auto: return "Auto"
        case .polish: return "Polish"
        case .english: return "English"
        }
    }
}

extension CsSettings {
    /// Sample config matching the mock (Polish whisper, local STT final-verdict).
    static let sample = CsSettings(
        holdExclusive: true,
        holdStartDelayMs: 250,
        doubleTapIntervalMs: 320,
        toggleSilenceSec: 1.5,
        whisperLanguage: .polish,
        aiFormattingEnabled: true,
        transcriptSendMode: "end_of_utterance",
        transcriptTaggingEnabled: false,
        transcriptTagTemplate: "<codescribe mode=\"{mode}\" lang=\"{lang}\">\n{text}\n</codescribe>",
        aiMaxTokens: 1024,
        aiAssistiveMaxTokens: 2048,
        showTrayGlyph: true,
        showDockIcon: false,
        transcriptionOverlayEnabled: true,
        holdIndicator: true,
        holdBadgeSize: 28,
        holdBadgeOffsetX: 0,
        holdBadgeOffsetY: 0,
        overlayPositionMode: "snapped_top_right",
        overlayCustomX: nil,
        overlayCustomY: nil,
        beepOnStart: true,
        soundName: "Tink",
        soundVolume: 0.6,
        audioInputDevice: nil,
        historyEnabled: true,
        quickNotesEnabled: true,
        quickNotesSaveOnly: false,
        useLocalStt: true,
        localModel: "whisper-large-v3-turbo",
        sttEndpoint: nil,
        sttEngine: nil,
        llmEndpoint: "https://api.openai.com/v1/responses",
        restoreClipboard: true,
        restoreClipboardDelayMs: 200,
        startAtLogin: false,
        agentEnterSends: true,
        dumpAudioLogs: false,
        llmModel: "gpt-4o-mini",
        llmFormattingEndpoint: "https://api.openai.com/v1/responses",
        llmFormattingModel: "gpt-4o-mini",
        llmAssistiveEndpoint: "https://api.openai.com/v1/responses",
        llmAssistiveModel: "gpt-4o",
        llmAssistiveProvider: "openai-responses",
        formattingLevel: "medium",
        whisperModel: "whisper-large-v3-turbo",
        layeredTranscription: nil,
        agentWorkspaceRoots: ["~/Git"],
        bufferDelayMs: nil,
        typingCps: nil,
        emitWordsMax: nil,
        bufferedInterimSec: nil,
        backendMaxUploadMb: nil
    )

    static let samplePrompt =
        "Clean up the dictated text: fix punctuation and casing, drop filler words, keep the speaker's meaning intact."
    static let sampleAssistivePrompt =
        "You are a concise voice assistant. Answer the user's spoken request directly and act on it using the available tools."
}

extension CsKeyStatus {
    /// All providers configured — used by the preview seed.
    static let sampleAllSet = CsKeyStatus(
        llmApiKeySet: true,
        sttApiKeySet: true,
        llmFormattingApiKeySet: true,
        llmAssistiveApiKeySet: true,
        llmAnthropicApiKeySet: false,
        githubTokenSet: false
    )

    /// Presence boolean for a canonical Keychain account name.
    func isSet(account: String) -> Bool {
        switch account {
        case "LLM_API_KEY": return llmApiKeySet
        case "STT_API_KEY": return sttApiKeySet
        case "LLM_FORMATTING_API_KEY": return llmFormattingApiKeySet
        case "LLM_ASSISTIVE_API_KEY": return llmAssistiveApiKeySet
        case "LLM_ANTHROPIC_API_KEY": return llmAnthropicApiKeySet
        case "GITHUB_TOKEN": return githubTokenSet
        default: return false
        }
    }
}

extension CsApiKeyProbeResult {
    static func sample(account: String) -> CsApiKeyProbeResult {
        CsApiKeyProbeResult(
            account: account,
            status: account == "STT_API_KEY" ? .unsupported : .ok,
            message: account == "STT_API_KEY"
                ? "no cheap liveness probe is available for this STT key"
                : "key accepted and quota available",
            probedEndpoint: nil
        )
    }
}

extension CsProviderOption {
    /// Preview seed mirroring the core provider identities (OpenAI + Anthropic).
    static let sampleProviders: [CsProviderOption] = [
        CsProviderOption(
            id: "openai-responses",
            displayName: "OpenAI (Responses)",
            apiKeyAccount: "LLM_ASSISTIVE_API_KEY",
            apiKeySet: true,
            accountSignedIn: false,
            accountLoginEnabled: false,
            accountStatusMessage: "awaiting app registration",
            oauthClientId: nil,
            models: []
        ),
        CsProviderOption(
            id: "anthropic-messages",
            displayName: "Anthropic (Messages)",
            apiKeyAccount: "LLM_ANTHROPIC_API_KEY",
            apiKeySet: false,
            accountSignedIn: false,
            accountLoginEnabled: false,
            accountStatusMessage: "provider account login unavailable",
            oauthClientId: nil,
            models: []
        ),
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
