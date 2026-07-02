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
    @Published private(set) var mcpServers: [CsMcpServer] = []
    @Published private(set) var mcpTestResults: [String: CsMcpTestResult] = [:]
    @Published private(set) var mcpTestPending: Set<String> = []
    @Published var lastError: String?

    /// Version label. No FFI surface exposes the running version, so this stays a
    /// build-time constant (tracked gap).
    let appVersion: String = "0.8.0"

    private let engine: SettingsEngine?
    private let permissionProbe: PermissionProbing
    private let agentStatus: AgentStatusEngine?
    private let mcpAdmin: MCPAdminEngine?

    init(
        engine: SettingsEngine? = nil,
        permissionProbe: PermissionProbing = NativePermissionProbe(),
        agentStatus: AgentStatusEngine? = nil,
        mcpAdmin: MCPAdminEngine? = nil
    ) {
        self.engine = engine
        self.permissionProbe = permissionProbe
        self.agentStatus = agentStatus
        self.mcpAdmin = mcpAdmin

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
        reloadMcpServers()
    }

    /// Re-probe just the agent substrate (readiness + MCP status). Cheap on-disk
    /// reads; used by the Engine panel's "Refresh" action so re-checking MCP does
    /// not disturb the rest of the panel.
    func refreshAgentStatus() {
        guard let agentStatus else { return }
        agentReadiness = agentStatus.agenticReadiness()
        mcpStatus = agentStatus.mcpStatus()
    }

    // MARK: - MCP server management (writes through the atomic config store)

    /// Re-read the configured MCP servers from `mcp.json`. A missing config is an
    /// empty list, not an error.
    func reloadMcpServers() {
        guard let mcpAdmin else { return }
        do {
            mcpServers = try mcpAdmin.listServers()
        } catch {
            lastError = String(describing: error)
            mcpServers = []
        }
    }

    /// Add a server from the form. `args` is already split into tokens. On success
    /// the list + readiness re-probe so the panel reflects the new state.
    func addMcpServer(name: String, command: String, args: [String]) {
        guard let mcpAdmin else { return }
        do {
            try mcpAdmin.addServer(
                CsMcpServerInput(name: name, command: command, args: args, enabled: true)
            )
            reloadMcpServers()
            refreshAgentStatus()
        } catch {
            lastError = String(describing: error)
        }
    }

    /// Flip a server's `enabled` flag, preserving its command / args / env.
    func toggleMcpServer(_ server: CsMcpServer) {
        guard let mcpAdmin else { return }
        do {
            try mcpAdmin.updateServer(
                name: server.name,
                input: CsMcpServerInput(
                    name: server.name, command: server.command,
                    args: server.args, enabled: !server.enabled
                )
            )
            reloadMcpServers()
            refreshAgentStatus()
        } catch {
            lastError = String(describing: error)
        }
    }

    /// Remove a server and drop any cached test result for it.
    func removeMcpServer(_ name: String) {
        guard let mcpAdmin else { return }
        do {
            try mcpAdmin.removeServer(name: name)
            mcpTestResults[name] = nil
            reloadMcpServers()
            refreshAgentStatus()
        } catch {
            lastError = String(describing: error)
        }
    }

    /// Spawn + handshake the named server and record the result inline. Runs off
    /// the main actor (the engine detaches) so the up-to-10s test never freezes
    /// the window; `mcpTestPending` drives a spinner in the row.
    func testMcpServer(_ name: String) {
        guard let mcpAdmin else { return }
        guard !mcpTestPending.contains(name) else { return }
        mcpTestPending.insert(name)
        Task {
            let result = await mcpAdmin.testServer(name)
            mcpTestPending.remove(name)
            mcpTestResults[name] = result
        }
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
        case "LLM_ASSISTIVE_API_KEY": return "Assistive API key (OpenAI)"
        case "LLM_ANTHROPIC_API_KEY": return "Anthropic API key"
        case "GITHUB_TOKEN": return "GitHub token"
        default: return account
        }
    }

    var keyAccounts: [String] { engine?.keyAccounts() ?? [] }

    // MARK: - Agent provider / model selection (assistive lane)

    /// Provider + model catalog with per-provider key presence.
    var availableProviders: [CsProviderOption] { engine?.availableProviders() ?? [] }

    /// Currently selected assistive-lane provider id (falls back to OpenAI).
    var assistiveProviderId: String { settings.llmAssistiveProvider ?? "openai-responses" }

    /// Currently configured assistive-lane model id (may be empty until set).
    var assistiveModel: String { settings.llmAssistiveModel ?? "" }

    /// The selected provider's catalog entry, if present.
    var selectedProvider: CsProviderOption? {
        let id = assistiveProviderId
        return availableProviders.first { $0.id == id } ?? availableProviders.first
    }

    func setAssistiveProvider(_ id: String) {
        settings.llmAssistiveProvider = id
        persist("LLM_ASSISTIVE_PROVIDER", id)
    }

    func setAssistiveModel(_ id: String) {
        settings.llmAssistiveModel = id
        persist("LLM_ASSISTIVE_MODEL", id)
    }

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
            agentStatus: MockAgentStatusEngine(),
            mcpAdmin: MockMCPAdminEngine()
        )
        model.section = section
        model.reloadMcpServers()
        return model
    }
}
