import SwiftUI

// Rail sections. Creator · Keys · Prompts · Engine are interactive; Audio · Voice
// Lab · User render but are inert (present-but-disabled), matching the mock.
enum SettingsSection: String, CaseIterable, Identifiable {
    case creator = "Creator"
    case keys = "Keys"
    case prompts = "Prompts"
    case engine = "Engine"
    case audio = "Audio"
    case voiceLab = "Voice Lab"
    case user = "User"

    var id: String { rawValue }
    var isInteractive: Bool {
        switch self {
        case .creator, .keys, .prompts, .engine: return true
        case .audio, .voiceLab, .user: return false
        }
    }
}

/// View-model owning the Settings screen state. Seeded with mock data so the
/// #Preview renders standalone; the live app injects `RealSettingsEngine`
/// (over the `CodescribeConfig` bridge) + the native permission probe.
@MainActor
final class SettingsViewModel: ObservableObject {
    @Published var section: SettingsSection = .creator

    @Published private(set) var permissions: PermissionSnapshot
    @Published private(set) var settings: CsSettings
    @Published private(set) var keyStatus: CsKeyStatus
    @Published private(set) var configDir: String
    @Published private(set) var needsOnboarding: Bool
    @Published private(set) var agentReadiness: CsAgenticReadiness
    @Published private(set) var mcpStatus: CsMcpStatusReport
    @Published var lastError: String?

    /// Version label. No FFI surface exposes the running version, so this stays a
    /// build-time constant (tracked gap).
    let appVersion: String = "0.8.0"

    private let engine: SettingsEngine?
    private let permissionProbe: PermissionProbing
    private let agentStatus: AgentStatusEngine?

    init(
        engine: SettingsEngine? = nil,
        permissionProbe: PermissionProbing = NativePermissionProbe(),
        agentStatus: AgentStatusEngine? = nil
    ) {
        self.engine = engine
        self.permissionProbe = permissionProbe
        self.agentStatus = agentStatus

        // Keep construction side-effect free. SwiftUI may instantiate the
        // Settings scene at app launch; live config/keychain reads happen in
        // `refresh()` when the Settings window actually appears.
        self.permissions = permissionProbe.snapshot()
        self.settings = .sample
        self.keyStatus = .sampleAllSet
        self.configDir = ""
        self.needsOnboarding = false
        self.agentReadiness = .sample
        self.mcpStatus = .sample
    }

    /// Re-read live state (permissions can change while the window is open).
    func refresh() {
        permissions = permissionProbe.snapshot()
        if let engine {
            settings = engine.loadSettings()
            keyStatus = engine.keyStatus()
            configDir = engine.configDir()
            needsOnboarding = engine.shouldShowOnboarding()
        }
        refreshAgentStatus()
    }

    /// Re-probe just the agent substrate (readiness + MCP status). Cheap on-disk
    /// reads; used by the Engine panel's "Refresh" action so re-checking MCP does
    /// not disturb the rest of the panel.
    func refreshAgentStatus() {
        guard let agentStatus else { return }
        agentReadiness = agentStatus.agenticReadiness()
        mcpStatus = agentStatus.mcpStatus()
    }

    func select(_ target: SettingsSection) {
        guard target.isInteractive else { return }
        section = target
    }

    // MARK: - Engine-panel derived values (runtime truth)

    var activeSTT: String {
        settings.useLocalStt ? "Local · final verdict" : "Cloud · streaming"
    }

    /// STT is "healthy" (olive dot) when a local model is configured, or when a
    /// cloud endpoint is set. We can't probe live Whisper load state from the
    /// config engine alone — that lives on `CodescribeDictation` (tracked gap).
    var sttHealthy: Bool {
        settings.useLocalStt ? !settings.localModel.isEmpty
                             : (settings.sttEndpoint?.isEmpty == false)
    }

    var whisperLanguageCode: String { settings.whisperLanguage.shortCode }

    var sttModelDescription: String {
        settings.useLocalStt ? settings.localModel
                             : (settings.sttEndpoint ?? "cloud default")
    }

    var llmModelDescription: String { settings.llmModel ?? "default" }
    var llmEndpointDescription: String { settings.llmEndpoint ?? "default" }

    var formattingDescription: String {
        guard settings.aiFormattingEnabled else { return "off" }
        return "on · \(settings.formattingLevel ?? "medium")"
    }

    /// Any LLM/STT provider key present (GitHub token is shown separately).
    var apiKeysStored: Bool {
        keyStatus.llmApiKeySet || keyStatus.llmAssistiveApiKeySet
            || keyStatus.llmFormattingApiKeySet || keyStatus.sttApiKeySet
    }

    var apiKeysDescription: String {
        apiKeysStored ? "Stored in Keychain" : "Not configured"
    }

    // MARK: - Creator mutations (write through the core router)

    func setLanguage(_ lang: CsLanguage) {
        settings.whisperLanguage = lang
        persist("WHISPER_LANGUAGE", lang.shortCode)
    }

    func setFormattingEnabled(_ on: Bool) {
        settings.aiFormattingEnabled = on
        persist("AI_FORMATTING_ENABLED", on ? "1" : "0")
    }

    func setFormattingLevel(_ level: String) {
        settings.formattingLevel = level
        persist("FORMATTING_LEVEL", level)
    }

    func setUseLocalStt(_ on: Bool) {
        settings.useLocalStt = on
        persist("USE_LOCAL_STT", on ? "1" : "0")
    }

    private func persist(_ key: String, _ value: String) {
        guard let engine else { return }
        do {
            try engine.updateConfig(key: key, value: value)
            settings = engine.loadSettings()
        } catch {
            lastError = String(describing: error)
        }
    }

    // MARK: - Keys (Keychain-backed; secrets never read back)

    /// Friendly labels for the canonical Keychain accounts.
    static func keyLabel(for account: String) -> String {
        switch account {
        case "LLM_API_KEY": return "LLM API key"
        case "STT_API_KEY": return "Speech-to-text API key"
        case "LLM_FORMATTING_API_KEY": return "Formatting API key"
        case "LLM_ASSISTIVE_API_KEY": return "Assistive API key"
        case "GITHUB_TOKEN": return "GitHub token"
        default: return account
        }
    }

    var keyAccounts: [String] { engine?.keyAccounts() ?? [] }

    func saveKey(account: String, secret: String) {
        let trimmed = secret.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty, let engine else { return }
        do {
            try engine.setApiKey(account: account, secret: trimmed)
            keyStatus = engine.keyStatus()
        } catch {
            lastError = String(describing: error)
        }
    }

    func clearKey(account: String) {
        guard let engine else { return }
        do {
            try engine.clearApiKey(account: account)
            keyStatus = engine.keyStatus()
        } catch {
            lastError = String(describing: error)
        }
    }

    // MARK: - Prompts (editable BASE prompts)

    func formattingPrompt() -> String { engine?.getFormattingPrompt() ?? CsSettings.samplePrompt }
    func assistivePrompt() -> String { engine?.getAssistivePrompt() ?? CsSettings.sampleAssistivePrompt }
    func defaultFormattingPrompt() -> String {
        engine?.defaultFormattingPrompt() ?? CsSettings.samplePrompt
    }
    func defaultAssistivePrompt() -> String {
        engine?.defaultAssistivePrompt() ?? CsSettings.sampleAssistivePrompt
    }

    func saveFormattingPrompt(_ content: String) {
        do { try engine?.setFormattingPrompt(content: content) }
        catch { lastError = String(describing: error) }
    }

    func saveAssistivePrompt(_ content: String) {
        do { try engine?.setAssistivePrompt(content: content) }
        catch { lastError = String(describing: error) }
    }

    func resetPromptsToDefaults() {
        do { try engine?.resetPromptsToDefaults() }
        catch { lastError = String(describing: error) }
    }

    // MARK: - Preview seed

    static var preview: SettingsViewModel { preview(.creator) }

    static func preview(_ section: SettingsSection) -> SettingsViewModel {
        let model = SettingsViewModel(
            engine: MockSettingsEngine(),
            permissionProbe: MockPermissionProbe(.allGranted),
            agentStatus: MockAgentStatusEngine()
        )
        model.section = section
        return model
    }
}
