import Foundation

// Canonical first-run wizard step flow. This mirrors the excised AppKit wizard's
// `STEP_FLOW` (git 37efe51^:app/ui/onboarding/steps.rs) AND the Rust setup
// sentinel in app/os/onboarding.rs (`WIZARD_STEPS_BEFORE_PERMISSIONS = 2`,
// `PERMISSION_STEP_ORDER` = mic → accessibility → input → screen → full-disk).
//
// The order and the 12 fixed indices are load-bearing: the resume marker
// persisted through `save_onboarding_progress` is a raw index into `flow`, and
// the Rust `setup_done_refresh_target` computes resume steps from the same
// offsets. Do NOT reorder or drop steps — B3a stubs `mode` / `language` /
// `hotkeyMode` / `agenticReadiness` as placeholders (see OnboardingSteps.swift),
// but they MUST keep their slots so indices stay stable across B3b.

/// One step of the first-run onboarding wizard.
enum OnboardingStep: Equatable {
    case welcome
    /// Basic vs Agentic operating lane. Stubbed in B3a — filled in B3b.
    case mode
    /// One of the five privacy scopes, in `PERMISSION_STEP_ORDER`.
    case permission(PermissionKind)
    /// Dictation language choice. Stubbed in B3a — filled in B3b.
    case language
    case apiKey
    /// Hold / toggle / hybrid hotkey lane. Stubbed in B3a — filled in B3b.
    case hotkeyMode
    /// Agentic-lane readiness verdict. Stubbed in B3a — filled in B3b.
    case agenticReadiness
    case done

    /// Fixed 12-step flow. Indices are the persisted resume contract — see the
    /// file header. Permission order matches `PERMISSION_STEP_ORDER`.
    static let flow: [OnboardingStep] = [
        .welcome,
        .mode,
        .permission(.microphone),
        .permission(.accessibility),
        .permission(.inputMonitoring),
        .permission(.screenRecording),
        .permission(.fullDiskAccess),
        .language,
        .apiKey,
        .hotkeyMode,
        .agenticReadiness,
        .done,
    ]

    /// Total number of steps (12). Kept in sync with the Rust
    /// `TOTAL_ONBOARDING_STEPS` clamp in app/os/onboarding.rs.
    static var count: Int { flow.count }

    /// Step at a persisted resume index, clamped to the valid range so a stale
    /// or out-of-range marker can never crash the wizard (falls back to Welcome).
    static func step(at index: Int) -> OnboardingStep {
        guard flow.indices.contains(index) else { return .welcome }
        return flow[index]
    }
}
