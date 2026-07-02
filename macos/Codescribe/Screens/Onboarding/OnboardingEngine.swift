import Foundation

// Seam between the onboarding wizard and the REAL codescribe core through the
// UniFFI bridge (CodescribeConfig). Same Real/Mock pattern as SettingsEngine /
// AgentStatusEngine / HotkeysEngine: the view-model talks to this protocol so
// #Preview can inject a deterministic in-memory stand-in while the live app
// injects `RealOnboardingEngine`.
//
// Onboarding-specific FFI (progress reader/writer + completion marker) was added
// alongside the pre-existing `shouldShowOnboarding` gate in bridge/src/config.rs.
// The API-key step reuses the exact same config surface as the Settings Keys
// panel (availableProviders / setApiKey / updateConfig / keyStatus) — no logic
// is duplicated, only the wizard chrome differs.

/// Subset of the codescribe config surface the onboarding wizard consumes.
protocol OnboardingEngine {
    // First-run gate + resume/completion markers.
    func shouldShowOnboarding() -> Bool
    func onboardingProgress() -> UInt32
    func saveOnboardingProgress(step: UInt32)
    func markOnboardingDone()

    // Mode step: first-run operating lane (Basic / Agentic). Reader + setter
    // route to the promoted `ONBOARDING_MODE` key in settings.json.
    func onboardingMode() -> String?
    func setOnboardingMode(_ mode: String) throws

    // Language step: current dictation language (seeds the picker). Writes reuse
    // the shared `updateConfig("WHISPER_LANGUAGE", …)` path below — no new mechanism.
    func currentLanguage() -> CsLanguage

    // API-key step (shared with the Settings Keys panel).
    func keyStatus() -> CsKeyStatus
    func availableProviders() -> [CsProviderOption]
    func setApiKey(account: String, secret: String) throws
    func updateConfig(key: String, value: String) throws
}

// MARK: - Real engine (UniFFI bridge adapter)

/// Concrete adapter over the `CodescribeConfig` bridge object. Stateless: every
/// call reads/writes live on-disk truth (config dir markers / settings.json /
/// Keychain). Injected by App.swift for the live app.
final class RealOnboardingEngine: OnboardingEngine {
    private let config = CodescribeConfig()

    func shouldShowOnboarding() -> Bool { config.shouldShowOnboarding() }
    func onboardingProgress() -> UInt32 { config.onboardingProgress() }
    func saveOnboardingProgress(step: UInt32) { config.saveOnboardingProgress(step: step) }
    func markOnboardingDone() { config.markOnboardingDone() }

    func onboardingMode() -> String? { config.onboardingMode() }
    func setOnboardingMode(_ mode: String) throws { try config.setOnboardingMode(mode: mode) }
    func currentLanguage() -> CsLanguage { config.loadSettings().whisperLanguage }

    func keyStatus() -> CsKeyStatus { config.keyStatus() }
    func availableProviders() -> [CsProviderOption] { config.availableProviders() }
    func setApiKey(account: String, secret: String) throws {
        try config.setApiKey(account: account, secret: secret)
    }
    func updateConfig(key: String, value: String) throws {
        try config.updateConfig(key: key, value: value)
    }
}

// MARK: - Mock engine (previews)

/// In-memory stand-in for #Preview and standalone rendering. Progress writes are
/// captured so a preview can be seeded at a specific step; key writes are no-ops.
final class MockOnboardingEngine: OnboardingEngine {
    var showOnboarding: Bool = true
    var progress: UInt32 = 0
    var status: CsKeyStatus = .sampleAllSet
    var mode: String?
    var language: CsLanguage = .auto

    init(progress: UInt32 = 0) { self.progress = progress }

    func shouldShowOnboarding() -> Bool { showOnboarding }
    func onboardingProgress() -> UInt32 { progress }
    func saveOnboardingProgress(step: UInt32) { progress = step }
    func markOnboardingDone() { showOnboarding = false }

    func onboardingMode() -> String? { mode }
    func setOnboardingMode(_ mode: String) throws { self.mode = mode }
    func currentLanguage() -> CsLanguage { language }

    func keyStatus() -> CsKeyStatus { status }
    func availableProviders() -> [CsProviderOption] { CsProviderOption.sampleProviders }
    func setApiKey(account: String, secret: String) throws {}
    func updateConfig(key: String, value: String) throws {}
}
