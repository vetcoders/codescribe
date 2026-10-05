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
    label(locale: Locale(identifier: Bundle.main.preferredLocalizations.first ?? "en"))
  }

  func label(locale: Locale) -> String {
    switch self {
    case .basic:
      return String(
        localized: LocalizedStringResource(
          "Basic", locale: locale, comment: "Operating lane: dictation only"))
    case .agentic:
      return String(
        localized: LocalizedStringResource(
          "Agentic", locale: locale, comment: "Operating lane: dictation plus an AI agent"))
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
    label(locale: Locale(identifier: Bundle.main.preferredLocalizations.first ?? "en"))
  }

  func label(locale: Locale) -> String {
    switch self {
    case .hold:
      return String(
        localized: LocalizedStringResource(
          "Hold to talk", locale: locale, comment: "Hotkey preset name"))
    case .toggle:
      return String(
        localized: LocalizedStringResource(
          "Hands-off (toggle)", locale: locale, comment: "Hotkey preset name"))
    case .both:
      return String(
        localized: LocalizedStringResource(
          "Hybrid (both)", locale: locale, comment: "Hotkey preset name: hold and toggle"))
    }
  }

  var summary: String {
    summary(locale: Locale(identifier: Bundle.main.preferredLocalizations.first ?? "en"))
  }

  func summary(locale: Locale) -> String {
    switch self {
    case .hold:
      return String(
        localized: LocalizedStringResource(
          "Hold Fn/Globe while you speak. Release to stop.", locale: locale,
          comment: "Hotkey preset detail; Fn and Globe are the key caps on a Mac keyboard"))
    case .toggle:
      return String(
        localized: LocalizedStringResource(
          "Double-tap left Option: dictation with formatting. Double-tap right Option: talk to the Agent. Tap again to stop.",
          locale: locale,
          comment: "Hotkey preset detail; left and right Option activate different modes"))
    case .both:
      return String(
        localized: LocalizedStringResource(
          "Use both methods.", locale: locale,
          comment: "Hotkey preset detail: hold and toggle methods are both enabled"))
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
    let toggleEnabled =
      dictation == .doubleCtrl
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

  @Published private(set) var interfaceLanguage: InterfaceLanguage
  var interfaceLocale: Locale { interfaceLanguage.locale }
  private let languagePreferences: UserDefaults

  // Mode step state.
  @Published private(set) var onboardingMode: OnboardingModeChoice

  // Language step state.
  @Published private(set) var selectedLanguage: CsLanguage

  // Hotkey-mode step state.
  @Published private(set) var hotkeyMode: HotkeyModeChoice

  // Agentic-readiness step state (lazy — probed when the step appears).
  @Published private(set) var readiness: CsAgenticReadiness?
  @Published private(set) var agentBridgeStatus: AgentBridgeInstallationStatus
  @Published private(set) var selectedAgentClients: Set<AgentBridgeClient>
  @Published private(set) var agentBridgeError: String?

  // API-key step state.
  @Published private(set) var providers: [CsProviderOption] = []
  @Published private(set) var providerAccessPending = false
  @Published private(set) var providerMutationPending = false
  @Published private(set) var providerAccessResolved = false
  @Published private(set) var providerAccessError: String?
  @Published private(set) var providerAccountErrors: [String: String] = [:]
  @Published private(set) var apiKeyEditorExpanded = false
  private var providerAccessGeneration: UInt64 = 0
  private var providerRefreshRequested = false
  private var apiKeyDraftsByProviderId: [String: String] = [:]
  @Published var selectedProviderId: String
  @Published var apiKeyDraft: String = ""

  @Published var lastError: String?

  private let engine: OnboardingEngine
  private let hotkeys: HotkeysEngine
  private let agentStatus: AgentStatusEngine
  private let agentBridge: AgentBridgeInstalling
  private let probe: PermissionProbing

  /// Invoked when the wizard is finished (Done confirmed) so the host can close
  /// and release the window.
  var onFinished: (() -> Void)?
  var onApplyInterfaceLanguage: ((@escaping @MainActor () -> Void) async throws -> Void)?
  @Published private(set) var applyingInterfaceLanguage = false
  private let processInterfaceLanguage: InterfaceLanguage

  init(
    engine: OnboardingEngine,
    hotkeys: HotkeysEngine = RealHotkeysEngine(),
    agentStatus: AgentStatusEngine = RealAgentStatusEngine(),
    agentBridge: AgentBridgeInstalling = RealAgentBridgeInstaller(),
    probe: PermissionProbing = NativePermissionProbe(),
    languagePreferences: UserDefaults = .standard,
    preferredLanguages: [String] = Locale.preferredLanguages,
    processInterfaceLanguage: InterfaceLanguage = .preferred(
      from: Bundle.main.preferredLocalizations)
  ) {
    let bridgeStatus = agentBridge.status()
    self.engine = engine
    self.hotkeys = hotkeys
    self.agentStatus = agentStatus
    self.agentBridge = agentBridge
    self.probe = probe
    self.languagePreferences = languagePreferences
    self.processInterfaceLanguage = processInterfaceLanguage
    self.interfaceLanguage = InterfaceLanguage.preferred(
      from: languagePreferences.stringArray(forKey: "AppleLanguages") ?? preferredLanguages
    )
    // Resume from the persisted step; `onboardingProgress` is already clamped
    // to a valid index by the Rust side.
    self.stepIndex = Int(engine.onboardingProgress())
    self.permissions = probe.snapshot()
    self.keyStatus = engine.keyStatus()
    self.onboardingMode = OnboardingModeChoice.from(engine.onboardingMode())
    self.selectedLanguage = engine.currentLanguage()
    self.hotkeyMode = HotkeyModeChoice.derive(from: hotkeys.modeBindings())
    self.agentBridgeStatus = bridgeStatus
    self.selectedAgentClients = Set(bridgeStatus.installedClients)
    self.agentBridgeError = nil
    self.selectedProviderId =
      engine.assistiveProvider()
      ?? "openai-responses"
  }

  // MARK: - Derived

  func selectInterfaceLanguage(_ language: InterfaceLanguage) {
    guard !applyingInterfaceLanguage else { return }
    interfaceLanguage = language
    languagePreferences.set([language.rawValue], forKey: "AppleLanguages")
    lastError = nil
  }

  var interfaceLanguageNeedsRestart: Bool { interfaceLanguage != processInterfaceLanguage }

  var step: OnboardingStep { OnboardingStep.step(at: stepIndex) }
  var windowTitle: String {
    String(
      localized: LocalizedStringResource(
        "Getting started", locale: interfaceLocale,
        comment: "Setup wizard window title"
      ))
  }
  var totalSteps: Int { OnboardingStep.count }
  var canGoBack: Bool { stepIndex > 0 }

  /// Human-readable "Step N of M" — permission steps are still counted by their
  /// absolute flow index so the bar never jumps.
  var progressLabel: String {
    String(
      localized: LocalizedStringResource(
        "Step \(stepIndex + 1) of \(totalSteps)", locale: interfaceLocale,
        comment: "Setup wizard progress; first %lld is the current step, second the total"
      ))
  }

  var isDone: Bool { step == .done }

  /// Primary-button label: "Finish" on Done, "Continue" everywhere else.
  var primaryLabel: String {
    if step == .interfaceLanguage, interfaceLanguageNeedsRestart {
      return String(
        localized: LocalizedStringResource(
          "Restart and continue", locale: interfaceLocale,
          comment: "Apply the interface language to the entire app and resume setup"))
    }
    return isDone
      ? String(
        localized: LocalizedStringResource(
          "Finish", locale: interfaceLocale, comment: "Setup wizard button: close the wizard"))
      : String(
        localized: LocalizedStringResource(
          "Continue", locale: interfaceLocale, comment: "Setup wizard button: go to the next step"))
  }

  var selectedProvider: CsProviderOption? {
    providers.first { $0.id == selectedProviderId }
  }

  var agentBridgeReadyToGo: Bool {
    readiness?.ready == true && agentBridgeError == nil
  }

  func agentClientIsInstalled(_ client: AgentBridgeClient) -> Bool {
    agentBridgeStatus.installedClients.contains(client)
  }

  func agentClientNeedsSetup(_: AgentBridgeClient) -> Bool {
    !agentBridgeReadyToGo
  }

  func agentClientShowsError(_ client: AgentBridgeClient) -> Bool {
    guard agentBridgeError != nil else { return false }
    guard
      let errorClient = selectedAgentClients.sorted(by: { $0.rawValue < $1.rawValue }).first
        ?? AgentBridgeClient.allCases.first
    else { return false }
    return client == errorClient
  }

  // MARK: - Lifecycle refresh

  /// Refresh live state when a step (re)appears. Called on view `onAppear` and
  /// after each transition so permission rows and key presence stay current
  /// without a manual poll.
  func refreshForCurrentStep() {
    if step != .agenticReadiness { refreshProviders() }
    switch step {
    case .permission:
      reprobePermissions()
    case .apiKey, .done:
      keyStatus = engine.keyStatus()
      reprobePermissions()
    case .agenticReadiness:
      refreshReadiness()
    default:
      break
    }
  }

  /// Re-probe live permission state AND re-arm the global hotkey tap if a
  /// first-run grant just landed. The CGEventTap reads Accessibility / Input
  /// Monitoring only when it is created, so a grant made mid-wizard leaves
  /// hotkeys dead until this re-arm (or an app restart). The bridge call is
  /// idempotent — a no-op once the tap is already live.
  private func reprobePermissions() {
    permissions = probe.snapshot()
    hotkeys.rearmAfterPermissionGrant()
  }

  /// Re-probe the agentic-lane readiness verdict + MCP server status. Called on
  /// the readiness step's appear and by its "Refresh" button. Read-only.
  func refreshReadiness() {
    refreshProviders()
    keyStatus = engine.keyStatus()
    refreshReadinessState()
  }

  private func refreshReadinessState() {
    guard providerAccessResolved else { return }
    if selectedProviderAccountError != nil, !selectedProviderKeySet {
      readiness = nil
      return
    }
    readiness = agentStatus.agenticReadiness()
    agentBridgeStatus = agentBridge.status()
  }

  func toggleAgentClient(_ client: AgentBridgeClient) {
    if selectedAgentClients.contains(client) {
      selectedAgentClients.remove(client)
    } else {
      selectedAgentClients.insert(client)
    }
    // An installation error belongs to the selection that produced it. Once
    // the user changes that decision, do not present the stale failure as the
    // status of the new selection.
    agentBridgeError = nil
  }

  /// The only home-directory write on the readiness step. It runs from an
  /// explicit Set up action or from Continue when the selected clients differ
  /// from the managed receipt. Visiting, refreshing, Back, and Skip stay
  /// read-only.
  func installAgentBridge() {
    do {
      agentBridgeStatus = try agentBridge.install(selectedClients: selectedAgentClients)
      agentBridgeError = nil
    } catch {
      agentBridgeError = error.userFacingMessage
      agentBridgeStatus = agentBridge.status()
    }
  }

  func setUpAgentClient(_ client: AgentBridgeClient) {
    selectedAgentClients.insert(client)
    installAgentBridge()
  }

  /// Select Agent diagnostics before the view opens the shared Settings window.
  /// The wizard stays open so the user can resolve readiness and then return.
  func prepareAgentDiagnosticsDeepLink() {
    SettingsDeepLink.shared.present(tab: .agentStatus)
  }

  func prepareProviderSettingsDeepLink() {
    SettingsDeepLink.shared.present(.keys)
  }

  private func refreshProviders() {
    if providerMutationPending {
      providerRefreshRequested = true
      return
    }
    guard !providerAccessPending else { return }
    // This read consumes the queued request; later mutations may queue another.
    providerRefreshRequested = false
    providerAccessPending = true
    let generation = providerAccessGeneration
    Task { @MainActor [self] in
      defer {
        providerAccessPending = false
        if providerRefreshRequested {
          providerRefreshRequested = false
          refreshProviders()
        }
      }
      do {
        let snapshot = try await engine.providerAccessSnapshot()
        guard generation == providerAccessGeneration,
          snapshot.revision == engine.providerAccessRevision()
        else {
          providerRefreshRequested = true
          return
        }
        providers = snapshot.providers
        providerAccountErrors = snapshot.accountErrors
        keyStatus = snapshot.keyStatus
        providerAccessResolved = true
        providerAccessError = nil
        if step == .agenticReadiness { refreshReadinessState() }
        if selectedProvider == nil, let first = providers.first {
          selectedProviderId = first.id
        }
      } catch {
        guard generation == providerAccessGeneration else {
          providerRefreshRequested = true
          return
        }
        providerAccessError = error.userFacingMessage
      }
    }
  }

  // MARK: - Navigation

  /// Advance to the next VISIBLE step (also the Skip action for the API-key
  /// step). Commits the leaving step's choice first so a default (untouched
  /// radio) still lands in config, mirroring the AppKit wizard's save-on-confirm.
  /// The Agentic Readiness step is skipped in the Basic lane. Advancing off the
  /// end is treated as finishing so we never index past the flow.
  func advance() {
    guard !providerMutationPending, !applyingInterfaceLanguage else { return }
    if step == .interfaceLanguage, interfaceLanguageNeedsRestart {
      guard let onApplyInterfaceLanguage else {
        lastError = InterfaceLanguageRestartError.unavailable.message(locale: interfaceLocale)
        return
      }
      applyingInterfaceLanguage = true
      lastError = nil
      Task { @MainActor [weak self] in
        guard let self else { return }
        defer { applyingInterfaceLanguage = false }
        do {
          // Flush the per-app preference before the new process resolves its bundle.
          guard languagePreferences.synchronize() else {
            throw InterfaceLanguageRestartError.unavailable
          }
          try await onApplyInterfaceLanguage { [self] in advanceAfterCommit() }
        } catch {
          lastError = (error as? InterfaceLanguageRestartError ?? .unavailable)
            .message(locale: interfaceLocale)
        }
      }
      return
    }
    if step == .apiKey, apiKeySaveAvailable,
      !apiKeyDraft.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
    {
      saveApiKey(advanceOnSuccess: true)
      return
    }
    if step == .agenticReadiness,
      selectedAgentClients != Set(agentBridgeStatus.installedClients)
    {
      installAgentBridge()
      guard agentBridgeError == nil,
        selectedAgentClients == Set(agentBridgeStatus.installedClients)
      else { return }
    }
    commitCurrentChoice()
    advanceAfterCommit()
  }

  private func advanceAfterCommit() {
    guard let next = nextVisibleIndex(after: stepIndex) else {
      finish()
      return
    }
    stepIndex = next
    persistProgress()
    refreshForCurrentStep()
  }

  func back() {
    guard !providerMutationPending, !applyingInterfaceLanguage else { return }
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
    case .interfaceLanguage: selectInterfaceLanguage(interfaceLanguage)
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
    guard !providerMutationPending, !applyingInterfaceLanguage else { return }
    engine.markOnboardingDone()
    onFinished?()
  }

  // MARK: - Permission step actions

  func openSystemSettings(for kind: PermissionKind) {
    kind.openSystemSettings()
  }

  /// Same grant path as Settings › Dictation / Creator checklist:
  /// - undetermined + in-app-requestable → system dialog from this process
  /// - otherwise → Privacy deep-link (macOS never re-prompts once determined)
  /// Always re-probes (and re-arms hotkeys) after the attempt settles.
  func grantPermission(for kind: PermissionKind) {
    let state = permissions.state(kind)
    if state == .notDetermined, kind.supportsInAppPermissionRequest {
      Task { @MainActor [weak self] in
        _ = await kind.requestInApp()
        self?.reprobePermissions()
      }
      return
    }
    kind.openSystemSettings()
    // User may flip the toggle and return without leaving the step — refresh
    // once after the deep-link so a fast grant is visible immediately.
    DispatchQueue.main.asyncAfter(deadline: .now() + 0.6) { [weak self] in
      self?.reprobePermissions()
    }
  }

  func refreshPermissions() {
    reprobePermissions()
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
      lastError = error.userFacingMessage
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
      lastError = error.userFacingMessage
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
      lastError = error.userFacingMessage
    }
  }

  // MARK: - API-key step actions

  func selectProvider(_ id: String) {
    guard !providerMutationPending else { return }
    selectedProviderId = id
    do {
      try engine.updateConfig(key: "LLM_ASSISTIVE_PROVIDER", value: id)
    } catch {
      lastError = error.userFacingMessage
    }
  }

  /// True when the currently selected provider's key is present in the Keychain.
  var selectedProviderKeySet: Bool {
    selectedProvider?.apiKeySet == true
    guard id != selectedProviderId else { return }
    if apiKeyDraft.isEmpty {
      apiKeyDraftsByProviderId[selectedProviderId] = nil
    } else {
      apiKeyDraftsByProviderId[selectedProviderId] = apiKeyDraft
    }
  }
    apiKeyDraft = apiKeyDraftsByProviderId[id] ?? ""
    apiKeyEditorExpanded = false
    lastError = nil

  var apiKeySaveAvailable: Bool {
    providerAccessResolved && providerAccessError == nil
      && selectedProvider?.apiKeyAccount.isEmpty == false
  }

  var selectedProviderAccountError: String? { providerAccountErrors[selectedProviderId] }

  var selectedProviderAccountConnected: Bool {
    selectedProvider?.accountSignedIn == true
  }

  var selectedProviderRequiresApiKey: Bool {
    selectedProvider?.keyRequired == true
  }

  var selectedProviderHasAccountAccess: Bool {
    selectedProvider?.accountLoginEnabled == true || selectedProviderAccountConnected
      && selectedProviderRequiresApiKey
  }

  var selectedProviderAccountStatus: String {
    if providerAccessPending {
      return String(
        localized: LocalizedStringResource(
          "Checking provider access…", locale: interfaceLocale,
          comment: "Setup provider row status while credentials are loading"))
    }
    if providerAccessError != nil {
      return String(
        localized: LocalizedStringResource(
          "Provider access unavailable", locale: interfaceLocale,
          comment: "Setup provider row status when the credential snapshot failed"))
    }
    if selectedProviderAccountError != nil {
      return String(
        localized: LocalizedStringResource(
          "Account access unavailable", locale: interfaceLocale,
          comment: "Setup Agent account row status when account credentials cannot be read"))
    }
    return selectedProviderAccountConnected
      ? String(
        localized: LocalizedStringResource(
          "Connected", locale: interfaceLocale,
          comment: "Setup Agent account row status"))
      : String(
        localized: LocalizedStringResource(
          "Not connected", locale: interfaceLocale,
          comment: "Setup Agent account row status"))
  }

  var selectedProviderKeyStatus: String {
    if providerAccessPending {
      return String(
        localized: LocalizedStringResource(
          "Checking provider access…", locale: interfaceLocale,
          comment: "Setup provider row status while credentials are loading"))
    }
    if providerAccessError != nil {
      return String(
        localized: LocalizedStringResource(
          "Provider access unavailable", locale: interfaceLocale,
          comment: "Setup provider row status when the credential snapshot failed"))
    }
    return selectedProviderKeySet
      ? String(
        localized: LocalizedStringResource(
          "Set", locale: interfaceLocale,
          comment: "Setup API key row status"))
      : String(
        localized: LocalizedStringResource(
          "Not set", locale: interfaceLocale,
          comment: "Setup API key row status"))
  }

  func beginApiKeyEditing() {
    guard selectedProviderRequiresApiKey else { return }
    apiKeyEditorExpanded = true
  }

  func refreshProviderAccess() {
    refreshProviders()
    keyStatus = engine.keyStatus()
  }

  func saveApiKey(advanceOnSuccess: Bool = false) {
    let submitted = apiKeyDraft
    let providerId = selectedProviderId
    let trimmed = submitted.trimmingCharacters(in: .whitespacesAndNewlines)
    guard apiKeySaveAvailable, !trimmed.isEmpty, let account = selectedProvider?.apiKeyAccount,
      !providerMutationPending
    else { return }
    providerMutationPending = true
    providerAccessGeneration &+= 1
    Task { @MainActor [self] in
      defer {
        providerMutationPending = false
        if providerAccessPending { providerRefreshRequested = true } else { refreshProviders() }
      }
      do {
        try await engine.setApiKeyAsync(account: account, secret: trimmed)
        let stillCurrent =
          apiKeyDraft == submitted
    lastError = nil
          && selectedProviderId == providerId
          && selectedProvider?.apiKeyAccount == account
        if stillCurrent {
          apiKeyDraft = ""
          apiKeyDraftsByProviderId[providerId] = nil
          apiKeyEditorExpanded = false
        }
        lastError = nil
        if advanceOnSuccess, stillCurrent, step == .apiKey { advanceAfterCommit() }
      } catch { lastError = error.userFacingMessage }
    }
  }

}
