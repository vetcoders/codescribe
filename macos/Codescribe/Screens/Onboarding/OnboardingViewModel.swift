import SwiftUI

// State machine for the first-run wizard: current step index (persisted on every
// transition so a relaunch resumes exactly where the user left off), live
// permission snapshot, and the API-key step's provider/draft state.
//
// Persistence contract (see OnboardingStep.swift header):
//   - init reads the resume index from `engine.onboardingProgress()`.
//   - every advance()/back() writes the new index via `saveOnboardingProgress`.
//   - finishing the Done step calls `markOnboardingDone()`, which clears the
//     resume marker and writes `setup_done` so `shouldShowOnboarding()` is false.

// MARK: - Step choices (mirror the excised AppKit wizard's state.rs enums)

/// First-run operating lane. `basic` keeps codescribe a plain dictation tool;
/// `agentic` opts into the agent chat + MCP substrate (and un-hides the Agentic
/// Readiness step). `basic` is the safe default — a corrupt/forward token can
/// never force the agentic lane. Mirrors `OnboardingModeChoice` in state.rs.
enum OnboardingModeChoice: String, CaseIterable {
    case basic
    case agentic

    /// Stable token persisted to settings.json (`ONBOARDING_MODE`).
    var value: String { rawValue }

    var label: String {
        switch self {
        case .basic: return "Basic"
        case .agentic: return "Agentic"
        }
    }

    /// Decode a persisted token, defaulting to `basic` for unknown values.
    static func from(_ value: String?) -> OnboardingModeChoice {
        value == "agentic" ? .agentic : .basic
    }
}

/// Recording-trigger preset. Maps onto the three core mode bindings (Dictation /
/// Formatting / Assistive) — the exact same triples the AppKit wizard wrote via
/// `save_hotkey_mode`. Full per-mode editing stays in Settings › Shortcuts.
enum HotkeyModeChoice: String, CaseIterable {
    case hold
    case toggle
    case both

    var label: String {
        switch self {
        case .hold: return "Hold to talk"
        case .toggle: return "Hands-off (toggle)"
        case .both: return "Hybrid (both)"
        }
    }

    var summary: String {
        switch self {
        case .hold: return "Press and hold Fn/Globe while you speak; release to stop."
        case .toggle: return "Double-tap Left/Right Option to start, tap again to stop."
        case .both: return "Hold Fn/Globe to dictate, or double-tap Option to toggle."
        }
    }

    /// Dictation / Formatting / Assistive bindings for this preset, mirroring
    /// `actions::save_hotkey_mode` in the excised wizard.
    var bindings: (CsShortcutBinding, CsShortcutBinding, CsShortcutBinding) {
        switch self {
        case .hold: return (.holdFn, .disabled, .disabled)
        case .toggle: return (.disabled, .doubleLeftOption, .doubleRightOption)
        case .both: return (.holdFn, .doubleLeftOption, .doubleRightOption)
        }
    }

    /// Derive the closest preset from live bindings, mirroring
    /// `initial_hotkey_choice`. Defaults to `both` when nothing hold-like or
    /// toggle-like is bound.
    static func derive(from bindings: [CsModeBinding]) -> HotkeyModeChoice {
        func binding(_ mode: CsWorkMode) -> CsShortcutBinding? {
            bindings.first { $0.mode == mode }?.binding
        }
        let dictation = binding(.dictation)
        let holdEnabled: Bool
        switch dictation {
        case .holdFn, .holdCtrl, .holdCtrlAlt, .holdCtrlShift, .holdCtrlCmd:
            holdEnabled = true
        default:
            holdEnabled = false
        }
        let toggleEnabled = dictation == .doubleCtrl
            || binding(.formatting) == .doubleLeftOption
            || binding(.assistive) == .doubleRightOption
        switch (holdEnabled, toggleEnabled) {
        case (true, false): return .hold
        case (false, true): return .toggle
        default: return .both
        }
    }
}

@MainActor
final class OnboardingViewModel: ObservableObject {
    @Published private(set) var stepIndex: Int
    @Published private(set) var permissions: PermissionSnapshot
    @Published private(set) var keyStatus: CsKeyStatus

    // Mode step state.
    @Published private(set) var onboardingMode: OnboardingModeChoice

    // Language step state.
    @Published private(set) var selectedLanguage: CsLanguage

    // Hotkey-mode step state.
    @Published private(set) var hotkeyMode: HotkeyModeChoice

    // Agentic-readiness step state (lazy — probed when the step appears).
    @Published private(set) var readiness: CsAgenticReadiness?
    @Published private(set) var mcpStatus: CsMcpStatusReport?

    // API-key step state.
    @Published private(set) var providers: [CsProviderOption] = []
    @Published var selectedProviderId: String
    @Published var apiKeyDraft: String = ""

    @Published var lastError: String?

    private let engine: OnboardingEngine
    private let hotkeys: HotkeysEngine
    private let agentStatus: AgentStatusEngine
    private let probe: PermissionProbing

    /// Invoked when the wizard is finished (Done confirmed) so the host can close
    /// and release the window.
    var onFinished: (() -> Void)?

    init(
        engine: OnboardingEngine,
        hotkeys: HotkeysEngine = RealHotkeysEngine(),
        agentStatus: AgentStatusEngine = RealAgentStatusEngine(),
        probe: PermissionProbing = NativePermissionProbe()
    ) {
        self.engine = engine
        self.hotkeys = hotkeys
        self.agentStatus = agentStatus
        self.probe = probe
        // Resume from the persisted step; `onboardingProgress` is already clamped
        // to a valid index by the Rust side.
        self.stepIndex = Int(engine.onboardingProgress())
        self.permissions = probe.snapshot()
        self.keyStatus = engine.keyStatus()
        self.onboardingMode = OnboardingModeChoice.from(engine.onboardingMode())
        self.selectedLanguage = engine.currentLanguage()
        self.hotkeyMode = HotkeyModeChoice.derive(from: hotkeys.modeBindings())
        self.selectedProviderId =
            engine.availableProviders().first?.id ?? "openai-responses"
    }

    // MARK: - Derived

    var step: OnboardingStep { OnboardingStep.step(at: stepIndex) }
    var totalSteps: Int { OnboardingStep.count }
    var canGoBack: Bool { stepIndex > 0 }

    /// Human-readable "Step N of M" — permission steps are still counted by their
    /// absolute flow index so the bar never jumps.
    var progressLabel: String { "Step \(stepIndex + 1) of \(totalSteps)" }

    var isDone: Bool { step == .done }

    /// Primary-button label: "Finish" on Done, "Continue" everywhere else.
    var primaryLabel: String { isDone ? "Finish" : "Continue" }

    var selectedProvider: CsProviderOption? {
        providers.first { $0.id == selectedProviderId } ?? providers.first
    }

    // MARK: - Lifecycle refresh

    /// Refresh live state when a step (re)appears. Called on view `onAppear` and
    /// after each transition so permission rows and key presence stay current
    /// without a manual poll.
    func refreshForCurrentStep() {
        loadProvidersIfNeeded()
        switch step {
        case .permission:
            permissions = probe.snapshot()
        case .apiKey, .done:
            keyStatus = engine.keyStatus()
            permissions = probe.snapshot()
        case .agenticReadiness:
            refreshReadiness()
        default:
            break
        }
    }

    /// Re-probe the agentic-lane readiness verdict + MCP server status. Called on
    /// the readiness step's appear and by its "Refresh" button. Read-only.
    func refreshReadiness() {
        readiness = agentStatus.agenticReadiness()
        mcpStatus = agentStatus.mcpStatus()
    }

    private func loadProvidersIfNeeded() {
        guard providers.isEmpty else { return }
        providers = engine.availableProviders()
        if selectedProvider == nil, let first = providers.first {
            selectedProviderId = first.id
        }
    }

    // MARK: - Navigation

    /// Advance to the next VISIBLE step (also the Skip action for the API-key
    /// step). Commits the leaving step's choice first so a default (untouched
    /// radio) still lands in config, mirroring the AppKit wizard's save-on-confirm.
    /// The Agentic Readiness step is skipped in the Basic lane. Advancing off the
    /// end is treated as finishing so we never index past the flow.
    func advance() {
        commitCurrentChoice()
        guard let next = nextVisibleIndex(after: stepIndex) else {
            finish()
            return
        }
        stepIndex = next
        persistProgress()
        refreshForCurrentStep()
    }

    func back() {
        guard let prev = prevVisibleIndex(before: stepIndex) else { return }
        stepIndex = prev
        persistProgress()
        refreshForCurrentStep()
    }

    /// Whether a step participates in the active lane. Only the Agentic Readiness
    /// verdict is lane-dependent — hidden in Basic so a plain-dictation install
    /// never blocks on the agent substrate. Mirrors `actions::step_is_visible`.
    private func isVisible(_ step: OnboardingStep) -> Bool {
        switch step {
        case .agenticReadiness: return onboardingMode == .agentic
        default: return true
        }
    }

    /// Next flow index visible in the current lane, or nil past the end.
    private func nextVisibleIndex(after index: Int) -> Int? {
        var candidate = index + 1
        while candidate < totalSteps {
            if isVisible(OnboardingStep.step(at: candidate)) { return candidate }
            candidate += 1
        }
        return nil
    }

    /// Previous flow index visible in the current lane, or nil before the start.
    private func prevVisibleIndex(before index: Int) -> Int? {
        var candidate = index
        while candidate > 0 {
            candidate -= 1
            if isVisible(OnboardingStep.step(at: candidate)) { return candidate }
        }
        return nil
    }

    /// Persist the leaving step's choice. Idempotent so re-committing an unchanged
    /// value is a safe no-op; guarantees the Basic default lands even if the user
    /// pressed Continue without touching a radio.
    private func commitCurrentChoice() {
        switch step {
        case .mode: persistMode()
        case .language: persistLanguage()
        case .hotkeyMode: persistHotkeyMode()
        default: break
        }
    }

    /// Primary-button action: finish on Done, otherwise advance.
    func primaryAction() {
        if isDone { finish() } else { advance() }
    }

    private func persistProgress() {
        engine.saveOnboardingProgress(step: UInt32(stepIndex))
    }

    func finish() {
        engine.markOnboardingDone()
        onFinished?()
    }

    // MARK: - Permission step actions

    func openSystemSettings(for kind: PermissionKind) {
        kind.openSystemSettings()
    }

    func refreshPermissions() {
        permissions = probe.snapshot()
    }

    // MARK: - Mode step actions

    /// Select the operating lane and persist it immediately (also flips whether
    /// the Agentic Readiness step is visible for the rest of the flow).
    func selectMode(_ mode: OnboardingModeChoice) {
        onboardingMode = mode
        persistMode()
    }

    private func persistMode() {
        do {
            try engine.setOnboardingMode(onboardingMode.value)
        } catch {
            lastError = error.localizedDescription
        }
    }

    // MARK: - Language step actions

    /// Select the dictation language and persist it through the shared config
    /// router key (`WHISPER_LANGUAGE`) — the same path Settings › Creator uses.
    func selectLanguage(_ language: CsLanguage) {
        selectedLanguage = language
        persistLanguage()
    }

    private func persistLanguage() {
        do {
            try engine.updateConfig(key: "WHISPER_LANGUAGE", value: selectedLanguage.shortCode)
        } catch {
            lastError = error.localizedDescription
        }
    }

    // MARK: - Hotkey-mode step actions

    /// Select a recording-trigger preset and persist it by writing the three core
    /// mode bindings (Dictation / Formatting / Assistive) through the shared
    /// HotkeysEngine — the same seam and live-reload the Shortcuts panel uses.
    func selectHotkeyMode(_ mode: HotkeyModeChoice) {
        hotkeyMode = mode
        persistHotkeyMode()
    }

    private func persistHotkeyMode() {
        let (dictation, formatting, assistive) = hotkeyMode.bindings
        do {
            try hotkeys.setModeBinding(mode: .dictation, binding: dictation)
            try hotkeys.setModeBinding(mode: .formatting, binding: formatting)
            try hotkeys.setModeBinding(mode: .assistive, binding: assistive)
        } catch {
            lastError = error.localizedDescription
        }
    }

    // MARK: - API-key step actions

    func selectProvider(_ id: String) {
        selectedProviderId = id
        do {
            try engine.updateConfig(key: "LLM_ASSISTIVE_PROVIDER", value: id)
        } catch {
            lastError = error.localizedDescription
        }
    }

    /// True when the currently selected provider's key is present in the Keychain.
    var selectedProviderKeySet: Bool {
        guard let account = selectedProvider?.apiKeyAccount else { return false }
        return keyStatus.isSet(account: account)
    }

    func saveApiKey() {
        let trimmed = apiKeyDraft.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty, let account = selectedProvider?.apiKeyAccount else { return }
        do {
            try engine.setApiKey(account: account, secret: trimmed)
            apiKeyDraft = ""
            keyStatus = engine.keyStatus()
        } catch {
            lastError = error.localizedDescription
        }
    }
}
