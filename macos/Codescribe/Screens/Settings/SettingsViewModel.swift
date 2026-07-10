import AppKit
import SwiftUI

// Rail sections. Creator · Keys · Prompts · Engine are interactive; Audio · Voice
// Lab · User render but are inert (present-but-disabled), matching the mock.
enum SettingsSection: String, CaseIterable, Identifiable {
    case creator = "Creator"
    case shortcuts = "Shortcuts"
    case keys = "Keys"
    case prompts = "Prompts"
    case engine = "Engine"
    case audio = "Audio"
    case voiceLab = "Voice Lab"
    case user = "User"

    var id: String { rawValue }
    var isInteractive: Bool {
        switch self {
        case .creator, .shortcuts, .keys, .prompts, .engine: return true
        case .audio, .voiceLab, .user: return false
        }
    }
}

/// One-shot deep-link target for the Settings window. A surface outside Settings
/// (e.g. the onboarding wizard routing the user to MCP setup) sets this before
/// opening or focusing the window; `SettingsView` consumes it once on appear and
/// whenever an already-open window receives a new target. Nil means "open on the
/// last/default section".
@MainActor
enum SettingsDeepLink {
    static let pendingSectionDidChange = Notification.Name("codescribe.settingsDeepLink.pendingSectionDidChange")

    static var pendingSection: SettingsSection? {
        didSet {
            guard pendingSection != nil else { return }
            NotificationCenter.default.post(name: pendingSectionDidChange, object: nil)
        }
    }

    /// Take the pending target (if any), clearing it so a later open is unaffected.
    static func consume() -> SettingsSection? {
        guard let target = pendingSection else { return nil }
        pendingSection = nil
        return target
    }
}

/// Clears the app's preferences domain and relaunches a fresh instance. Used by
/// the destructive "Reset app data" flow so restored window frames / SwiftUI scene
/// state do not survive the wipe. The relaunch is deferred via a detached `open`
/// so this instance fully exits first — otherwise AppDelegate's duplicate-instance
/// guard would terminate the freshly-launched copy.
@MainActor
enum AppRelaunch {
    static func clearDefaultsAndRelaunch() {
        if let bundleId = Bundle.main.bundleIdentifier {
            UserDefaults.standard.removePersistentDomain(forName: bundleId)
            UserDefaults.standard.synchronize()
        }
        let bundlePath = Bundle.main.bundlePath
        let task = Process()
        task.launchPath = "/bin/sh"
        // `$0` carries the bundle path as a positional arg so it is safely quoted,
        // never interpolated into the script string.
        task.arguments = ["-c", "sleep 1; open \"$0\"", bundlePath]
        try? task.run()
        NSApp.terminate(nil)
    }
}

/// View-model owning the Settings screen state. Seeded with mock data so the
/// #Preview renders standalone; the live app injects `RealSettingsEngine`
/// (over the `CodescribeConfig` bridge) + the native permission probe.
private struct BackgroundSettingsEngine: @unchecked Sendable {
    let engine: SettingsEngine
}

@MainActor
final class SettingsViewModel: ObservableObject {
    @Published var section: SettingsSection = .creator

    @Published private(set) var permissions: PermissionSnapshot
    @Published private(set) var settings: CsSettings
    @Published private(set) var keyStatus: CsKeyStatus
    @Published private(set) var providers: [CsProviderOption]
    @Published private(set) var modelDiscovery: CsModelDiscovery
    @Published private(set) var configDir: String
    @Published private(set) var needsOnboarding: Bool
    @Published private(set) var agentReadiness: CsAgenticReadiness
    @Published private(set) var mcpStatus: CsMcpStatusReport
    @Published private(set) var mcpServers: [CsMcpServer] = []
    @Published private(set) var mcpTestResults: [String: CsMcpTestResult] = [:]
    @Published private(set) var mcpTestPending: Set<String> = []
    @Published private(set) var keyProbeResults: [String: CsApiKeyProbeResult] = [:]
    @Published private(set) var keyProbePending: Set<String> = []
    @Published var lastError: String?

    // MARK: - Hotkeys (mode bindings)

    /// Persisted per-mode bindings as last read from disk.
    @Published private(set) var modeBindings: [CsModeBinding] = []
    /// The closed set of selectable gestures for the pickers.
    @Published private(set) var bindingOptions: [CsBindingOption] = []
    /// Editable copy the Shortcuts panel mutates before a save.
    @Published private(set) var draftBindings: [CsModeBinding] = []
    /// Conflicts for the CURRENT draft (recomputed on every edit).
    @Published private(set) var bindingConflicts: [CsHotkeyConflict] = []

    /// Version label. No FFI surface exposes the running version, so this stays a
    /// build-time constant (tracked gap).
    let appVersion: String = "0.8.0"

    private let engine: SettingsEngine?
    private let permissionProbe: PermissionProbing
    private let agentStatus: AgentStatusEngine?
    private let mcpAdmin: MCPAdminEngine?
    private let hotkeys: HotkeysEngine?

    init(
        engine: SettingsEngine? = nil,
        permissionProbe: PermissionProbing = NativePermissionProbe(),
        agentStatus: AgentStatusEngine? = nil,
        mcpAdmin: MCPAdminEngine? = nil,
        hotkeys: HotkeysEngine? = nil
    ) {
        self.engine = engine
        self.permissionProbe = permissionProbe
        self.agentStatus = agentStatus
        self.mcpAdmin = mcpAdmin
        self.hotkeys = hotkeys

        // Keep construction side-effect free. SwiftUI may instantiate the
        // Settings scene at app launch; live config/keychain reads happen in
        // `refresh()` when the Settings window actually appears.
        self.permissions = permissionProbe.snapshot()
        self.settings = .sample
        self.keyStatus = .sampleAllSet
        self.providers = CsProviderOption.sampleProviders
        self.modelDiscovery = CsModelDiscovery.sample(for: CsSettings.sample.llmAssistiveProvider ?? "openai-responses")
        self.configDir = ""
        self.needsOnboarding = false
        self.agentReadiness = .sample
        self.mcpStatus = .sample
    }

    /// Re-read live state (permissions can change while the window is open).
    func refresh() {
        permissions = permissionProbe.snapshot()
        // A permission granted while Settings is open (e.g. via the checklist's
        // "Open System Settings") should bring hotkeys live without an app
        // restart. Idempotent bridge call — a no-op once the tap is already armed.
        hotkeys?.rearmAfterPermissionGrant()
        if let engine {
            settings = engine.loadSettings()
            keyStatus = engine.keyStatus()
            providers = engine.availableProviders()
            configDir = engine.configDir()
            needsOnboarding = engine.shouldShowOnboarding()
            refreshAssistiveModels()
        }
        refreshAgentStatus()
        reloadMcpServers()
        loadHotkeys()
    }

    /// Re-probe just the agent substrate (readiness + MCP status). Cheap on-disk
    /// reads; used by the Engine panel's "Refresh" action so re-checking MCP does
    /// not disturb the rest of the panel.
    func refreshAgentStatus() {
        guard let agentStatus else { return }
        agentReadiness = agentStatus.agenticReadiness()
        mcpStatus = agentStatus.mcpStatus()
    }

    // MARK: - Hotkeys (mode-binding editor)

    /// Re-read persisted bindings + the option catalog, then reset the editable
    /// draft to match disk and revalidate. A missing engine leaves the seeds.
    func loadHotkeys() {
        guard let hotkeys else { return }
        modeBindings = hotkeys.modeBindings()
        bindingOptions = hotkeys.availableBindings()
        draftBindings = modeBindings
        revalidateBindings()
    }

    /// Any blocking (reachability / system) conflict in the current draft.
    var hasBlockingBindingConflicts: Bool { bindingConflicts.contains { $0.blocking } }

    /// The draft differs from persisted state (something to save).
    var hasPendingBindingChanges: Bool {
        draftBindings.map(\.binding) != modeBindings.map(\.binding)
    }

    /// Save is allowed only for a changed, conflict-clean draft.
    var canSaveBindings: Bool { hasPendingBindingChanges && !hasBlockingBindingConflicts }

    /// Current draft binding for a mode (falls back to the persisted value).
    func draftBinding(for mode: CsWorkMode) -> CsShortcutBinding {
        draftBindings.first { $0.mode == mode }?.binding
            ?? modeBindings.first { $0.mode == mode }?.binding
            ?? .disabled
    }

    /// Stage a binding change for one mode WITHOUT persisting, then re-validate so
    /// conflicts surface inline before the user commits.
    func editDraftBinding(mode: CsWorkMode, binding: CsShortcutBinding) {
        guard let index = draftBindings.firstIndex(where: { $0.mode == mode }) else { return }
        let label = bindingOptions.first { $0.binding == binding }?.label
            ?? draftBindings[index].bindingLabel
        draftBindings[index] = CsModeBinding(
            mode: mode,
            modeLabel: draftBindings[index].modeLabel,
            modeDescription: draftBindings[index].modeDescription,
            binding: binding,
            bindingLabel: label
        )
        revalidateBindings()
    }

    /// Recompute conflicts for the current draft via the revived shortcut registry.
    func revalidateBindings() {
        guard let hotkeys else {
            bindingConflicts = []
            return
        }
        bindingConflicts = hotkeys.validate(candidate: draftBindings)
    }

    /// Persist every changed mode through the core `set_mode_binding` contract
    /// (each write live-reloads the detector), then re-read disk truth. Guarded by
    /// `canSaveBindings`, so a conflicted or unchanged draft never writes.
    func saveBindings() {
        guard let hotkeys, canSaveBindings else { return }
        do {
            for draft in draftBindings {
                let current = modeBindings.first { $0.mode == draft.mode }
                if current?.binding != draft.binding {
                    try hotkeys.setModeBinding(mode: draft.mode, binding: draft.binding)
                }
            }
            loadHotkeys()
        } catch {
            lastError = String(describing: error)
        }
    }

    /// Reset all bindings to the built-in defaults and re-read.
    func resetBindingsToDefaults() {
        guard let hotkeys else { return }
        do {
            try hotkeys.resetToDefaults()
            loadHotkeys()
        } catch {
            lastError = String(describing: error)
        }
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
        if target == .keys {
            refreshAssistiveModels()
        }
    }

    // MARK: - Reset app data (destructive privacy action)

    /// Wipe all local app data through the Rust bridge, clear the app's
    /// UserDefaults domain, then relaunch so codescribe comes up fresh (first-run
    /// wizard from the top). `includeKeys` also removes the Keychain API keys.
    /// On failure the error surfaces in `lastError` and nothing is relaunched.
    func resetAppData(includeKeys: Bool) {
        guard let engine else { return }
        do {
            try engine.resetAppData(includeKeys: includeKeys)
        } catch {
            lastError = String(describing: error)
            return
        }
        AppRelaunch.clearDefaultsAndRelaunch()
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
            || keyStatus.llmAnthropicApiKeySet || keyStatus.llmFormattingApiKeySet || keyStatus.sttApiKeySet
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

    // MARK: - STT engine / layered transcription (Engine panel controls)

    /// Selected STT engine id ("auto" | "apple" | "whisper"); absent → auto policy.
    var sttEngineId: String { settings.sttEngine ?? "auto" }

    /// Display label for the current STT engine selection.
    var sttEngineLabel: String {
        switch sttEngineId {
        case "apple": return "Apple (live)"
        case "whisper", "candle": return "Whisper (Candle)"
        default: return "Auto"
        }
    }

    func setSttEngine(_ id: String) {
        settings.sttEngine = id
        persist("CODESCRIBE_STT_ENGINE", id)
    }

    /// ON for any phase value ("phase1".."phase4" or bare "1".."4"); anything
    /// else (including "off"/absent) is OFF — mirrors the core `layered_phase`.
    var layeredTranscriptionEnabled: Bool {
        let value = settings.layeredTranscription ?? "off"
        return value.hasPrefix("phase") || Int(value) != nil
    }

    /// The GUI only exposes Phase 1 (Apple live layer + Whisper tail patch);
    /// phases 2-4 do not exist as features yet.
    func setLayeredTranscription(_ on: Bool) {
        let value = on ? "phase1" : "off"
        settings.layeredTranscription = value
        persist("CODESCRIBE_LAYERED_TRANSCRIPTION", value)
    }

    // MARK: - Agent workspace roots (list_projects tool)

    /// Effective workspace roots the `list_projects` tool scans. Never empty —
    /// the bridge fills the built-in default (`~/Git`) when unset.
    var agentWorkspaceRoots: [String] { settings.agentWorkspaceRoots }

    /// Persist the workspace roots as the colon-joined `AGENT_WORKSPACE_ROOTS`
    /// value. Blank/whitespace rows are dropped; an all-empty list clears the
    /// override so the core falls back to `~/Git`.
    func setAgentWorkspaceRoots(_ roots: [String]) {
        let cleaned = roots
            .map { $0.trimmingCharacters(in: .whitespaces) }
            .filter { !$0.isEmpty }
        settings.agentWorkspaceRoots = cleaned.isEmpty ? ["~/Git"] : cleaned
        persist("AGENT_WORKSPACE_ROOTS", cleaned.joined(separator: ":"))
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

    /// Provider catalog with per-provider key presence.
    var availableProviders: [CsProviderOption] { providers }

    /// Currently selected assistive-lane provider id (falls back to OpenAI).
    var assistiveProviderId: String { settings.llmAssistiveProvider ?? "openai-responses" }

    /// Currently configured assistive-lane model id (may be empty until set).
    var assistiveModel: String { settings.llmAssistiveModel ?? "" }

    /// The selected provider's catalog entry, if present.
    var selectedProvider: CsProviderOption? {
        let id = assistiveProviderId
        return availableProviders.first { $0.id == id } ?? availableProviders.first
    }

    /// Models discovered from the selected provider's live `/models` API.
    var discoveredModels: [CsModelOption] {
        modelDiscovery.providerId == assistiveProviderId ? modelDiscovery.models : []
    }

    var modelDiscoveryStatus: String { modelDiscovery.status }

    var modelDiscoveryDescription: String {
        switch modelDiscovery.status {
        case "fresh":
            let count = modelDiscovery.models.count
            let noun = count == 1 ? "model" : "models"
            return count == 0 ? "no models returned by provider" : "\(count) \(noun) discovered from provider"
        case "cached":
            if let message = modelDiscovery.message, !message.isEmpty {
                return "using cached models — \(message)"
            }
            return "using cached models"
        case "no_key":
            return "Add API key to discover models"
        default:
            if let message = modelDiscovery.message, !message.isEmpty {
                return "model discovery failed — \(message)"
            }
            return "model discovery failed"
        }
    }

    func setAssistiveProvider(_ id: String) {
        settings.llmAssistiveProvider = id
        persist("LLM_ASSISTIVE_PROVIDER", id)
        refreshAssistiveModels()
        // The stored model belonged to the previous provider; keeping it would make
        // the first send hit a model the new provider doesn't serve (e.g. gpt-5.5 on
        // Anthropic). Re-anchor to the new provider's first discovered model, or
        // clear it so the provider default applies.
        setAssistiveModel(discoveredModels.first?.id ?? "")
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
            keyProbeResults[account] = nil
            keyStatus = engine.keyStatus()
            providers = engine.availableProviders()
            if account == selectedProvider?.apiKeyAccount {
                refreshAssistiveModels()
            }
        } catch {
            lastError = String(describing: error)
        }
    }

    func clearKey(account: String) {
        guard let engine else { return }
        do {
            try engine.clearApiKey(account: account)
            keyProbeResults[account] = nil
            keyStatus = engine.keyStatus()
            providers = engine.availableProviders()
            if account == selectedProvider?.apiKeyAccount {
                refreshAssistiveModels()
            }
        } catch {
            lastError = String(describing: error)
        }
    }

    func testKey(account: String) {
        guard let engine else { return }
        guard !keyProbePending.contains(account) else { return }
        let backgroundEngine = BackgroundSettingsEngine(engine: engine)
        keyProbePending.insert(account)

        DispatchQueue.global(qos: .userInitiated).async { [backgroundEngine, account] in
            let result: Result<CsApiKeyProbeResult, Error>
            do {
                result = .success(try backgroundEngine.engine.testApiKey(account: account))
            } catch {
                result = .failure(error)
            }

            DispatchQueue.main.async { [weak self] in
                guard let self else { return }
                self.keyProbePending.remove(account)
                switch result {
                case .success(let probe):
                    self.keyProbeResults[account] = probe
                case .failure(let error):
                    self.keyProbeResults[account] = CsApiKeyProbeResult(
                        account: account,
                        status: .network,
                        message: String(describing: error)
                    )
                    self.lastError = String(describing: error)
                }
            }
        }
    }

    func providerForKeyAccount(_ account: String) -> CsProviderOption? {
        availableProviders.first { $0.apiKeyAccount == account && $0.id == "openai-responses" }
    }

    func startAccountLogin(providerId: String) {
        guard let engine else { return }
        do {
            let result = try engine.startAccountLogin(providerId: providerId)
            if let authUrl = result.authUrl, let url = URL(string: authUrl) {
                NSWorkspace.shared.open(url)
            }
            providers = engine.availableProviders()
        } catch {
            lastError = String(describing: error)
        }
    }

    func refreshAssistiveModels() {
        guard let engine else {
            modelDiscovery = CsModelDiscovery.sample(for: assistiveProviderId)
            return
        }
        modelDiscovery = engine.discoverModels(providerId: assistiveProviderId)
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
            mcpAdmin: MockMCPAdminEngine(),
            hotkeys: MockHotkeysEngine()
        )
        model.section = section
        model.reloadMcpServers()
        model.loadHotkeys()
        return model
    }
}
