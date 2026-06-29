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
//   LOCAL_MODEL / STT_ENDPOINT / LLM_MODEL / LLM_ENDPOINT / LLM_ASSISTIVE_* …  free strings
// Keychain accounts (CsKeyStatus, core/config/keychain.rs::KEYCHAIN_ACCOUNTS):
//   LLM_API_KEY / STT_API_KEY / LLM_FORMATTING_API_KEY / LLM_ASSISTIVE_API_KEY / GITHUB_TOKEN

/// Subset of the codescribe config surface the Settings screen consumes.
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

    // Keychain-backed API keys — presence booleans only, secrets never read back
    func keyStatus() -> CsKeyStatus
    func keyAccounts() -> [String]
    func setApiKey(account: String, secret: String) throws
    func clearApiKey(account: String) throws

    // Editable BASE prompts
    func getFormattingPrompt() -> String
    func getAssistivePrompt() -> String
    func defaultFormattingPrompt() -> String
    func defaultAssistivePrompt() -> String
    func setFormattingPrompt(content: String) throws
    func setAssistivePrompt(content: String) throws
    func resetPromptsToDefaults() throws
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

    func updateConfig(key: String, value: String) throws {
        try config.updateConfig(key: key, value: value)
    }
    func updateConfigMany(entries: [CsConfigEntry]) throws {
        try config.updateConfigMany(entries: entries)
    }

    func keyStatus() -> CsKeyStatus { config.keyStatus() }
    func keyAccounts() -> [String] { config.keyAccounts() }
    func setApiKey(account: String, secret: String) throws {
        try config.setApiKey(account: account, secret: secret)
    }
    func clearApiKey(account: String) throws { try config.clearApiKey(account: account) }

    func getFormattingPrompt() -> String { config.getFormattingPrompt() }
    func getAssistivePrompt() -> String { config.getAssistivePrompt() }
    func defaultFormattingPrompt() -> String { config.defaultFormattingPrompt() }
    func defaultAssistivePrompt() -> String { config.defaultAssistivePrompt() }
    func setFormattingPrompt(content: String) throws {
        try config.setFormattingPrompt(content: content)
    }
    func setAssistivePrompt(content: String) throws {
        try config.setAssistivePrompt(content: content)
    }
    func resetPromptsToDefaults() throws { try config.resetPromptsToDefaults() }
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

    func loadSettings() -> CsSettings { settings }
    func configDir() -> String { dir }
    func shouldShowOnboarding() -> Bool { onboarding }
    func onboardingMode() -> String? { mode }
    func setOnboardingMode(mode: String) throws {}

    func updateConfig(key: String, value: String) throws {}
    func updateConfigMany(entries: [CsConfigEntry]) throws {}

    func keyStatus() -> CsKeyStatus { status }
    func keyAccounts() -> [String] {
        ["LLM_API_KEY", "STT_API_KEY", "LLM_FORMATTING_API_KEY", "LLM_ASSISTIVE_API_KEY", "GITHUB_TOKEN"]
    }
    func setApiKey(account: String, secret: String) throws {}
    func clearApiKey(account: String) throws {}

    func getFormattingPrompt() -> String { CsSettings.samplePrompt }
    func getAssistivePrompt() -> String { CsSettings.sampleAssistivePrompt }
    func defaultFormattingPrompt() -> String { CsSettings.samplePrompt }
    func defaultAssistivePrompt() -> String { CsSettings.sampleAssistivePrompt }
    func setFormattingPrompt(content: String) throws {}
    func setAssistivePrompt(content: String) throws {}
    func resetPromptsToDefaults() throws {}
}

// MARK: - Bridge value helpers

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
        transcriptTagTemplate: "",
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
        formattingLevel: "medium",
        whisperModel: "whisper-large-v3-turbo",
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
        githubTokenSet: false
    )

    /// Presence boolean for a canonical Keychain account name.
    func isSet(account: String) -> Bool {
        switch account {
        case "LLM_API_KEY": return llmApiKeySet
        case "STT_API_KEY": return sttApiKeySet
        case "LLM_FORMATTING_API_KEY": return llmFormattingApiKeySet
        case "LLM_ASSISTIVE_API_KEY": return llmAssistiveApiKeySet
        case "GITHUB_TOKEN": return githubTokenSet
        default: return false
        }
    }
}
