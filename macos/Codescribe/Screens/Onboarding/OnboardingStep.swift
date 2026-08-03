import Foundation

// Canonical first-run wizard step flow. This mirrors the excised AppKit wizard's
// `STEP_FLOW` AND the Rust setup sentinel in app/os/onboarding.rs
// (`WIZARD_STEPS_BEFORE_PERMISSIONS = 2`, `PERMISSION_STEP_ORDER` = mic →
// accessibility → input → screen → speech → full-disk).
//
// The order and the step indices are load-bearing: the resume marker persisted
// through `save_onboarding_progress` is a raw index into `flow`, and the Rust
// `setup_done_refresh_target` computes resume steps from the same offsets.
// Do NOT reorder or drop steps without updating `TOTAL_ONBOARDING_STEPS` and
// `PERMISSION_STEP_ORDER` in app/os/onboarding.rs in lockstep.
//
// Speech Recognition is a first-class permission step (Apple live dictation
// TCC). It sits after Screen Recording and before optional Full Disk Access.

/// One step of the first-run onboarding wizard.
enum OnboardingStep: Equatable {
    case welcome
    /// Basic vs Agentic operating lane.
    case mode
    /// Privacy scopes in `PERMISSION_STEP_ORDER` (mic → … → speech → full-disk).
    case permission(PermissionKind)
    /// Dictation language choice.
    case language
    case apiKey
    /// Hold / toggle / hybrid hotkey lane.
    case hotkeyMode
    /// Agentic-lane readiness verdict.
    case agenticReadiness
    case done

    /// Fixed 13-step flow. Indices are the persisted resume contract — see the
    /// file header. Permission order matches `PERMISSION_STEP_ORDER`.
    static let flow: [OnboardingStep] = [
        .welcome,
        .mode,
        .permission(.microphone),
        .permission(.accessibility),
        .permission(.inputMonitoring),
        .permission(.screenRecording),
        .permission(.speechRecognition),
        .permission(.fullDiskAccess),
        .language,
        .apiKey,
        .hotkeyMode,
        .agenticReadiness,
        .done,
    ]

    /// Total number of steps (13). Kept in sync with the Rust
    /// `TOTAL_ONBOARDING_STEPS` clamp in app/os/onboarding.rs.
    static var count: Int { flow.count }

    /// Step at a persisted resume index, clamped to the valid range so a stale
    /// or out-of-range marker can never crash the wizard (falls back to Welcome).
    static func step(at index: Int) -> OnboardingStep {
        guard flow.indices.contains(index) else { return .welcome }
        return flow[index]
    }
}
