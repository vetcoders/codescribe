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

@MainActor
final class OnboardingViewModel: ObservableObject {
    @Published private(set) var stepIndex: Int
    @Published private(set) var permissions: PermissionSnapshot
    @Published private(set) var keyStatus: CsKeyStatus

    // API-key step state.
    @Published private(set) var providers: [CsProviderOption] = []
    @Published var selectedProviderId: String
    @Published var apiKeyDraft: String = ""

    @Published var lastError: String?

    private let engine: OnboardingEngine
    private let probe: PermissionProbing

    /// Invoked when the wizard is finished (Done confirmed) so the host can close
    /// and release the window.
    var onFinished: (() -> Void)?

    init(engine: OnboardingEngine, probe: PermissionProbing = NativePermissionProbe()) {
        self.engine = engine
        self.probe = probe
        // Resume from the persisted step; `onboardingProgress` is already clamped
        // to a valid index by the Rust side.
        self.stepIndex = Int(engine.onboardingProgress())
        self.permissions = probe.snapshot()
        self.keyStatus = engine.keyStatus()
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
        default:
            break
        }
    }

    private func loadProvidersIfNeeded() {
        guard providers.isEmpty else { return }
        providers = engine.availableProviders()
        if selectedProvider == nil, let first = providers.first {
            selectedProviderId = first.id
        }
    }

    // MARK: - Navigation

    /// Advance to the next step (also the Skip action for the API-key step). The
    /// Done step is terminal and uses `finish()` instead — advancing off the end
    /// is treated as finishing so we never index past the flow.
    func advance() {
        let next = stepIndex + 1
        guard next < totalSteps else {
            finish()
            return
        }
        stepIndex = next
        persistProgress()
        refreshForCurrentStep()
    }

    func back() {
        guard stepIndex > 0 else { return }
        stepIndex -= 1
        persistProgress()
        refreshForCurrentStep()
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
