//! Onboarding setup-sentinel checks that survive the legacy AppKit UI excision.
//!
//! This is the non-UI half of the old `ui/onboarding/session` module: the
//! filesystem/permission sentinel (`should_show_onboarding`) plus the marker
//! migration and permission-invalidation helpers it transitively needs. The
//! AppKit wizard window (`show_onboarding_wizard`) was removed with the rest of
//! the legacy UI; this logic lives here in `os` because its real dependency is
//! the permission probe surface (`crate::os::permissions`).

use std::fs;
use std::path::PathBuf;

use tracing::warn;

use crate::config::Config;
use crate::os::permissions::{PermissionKind, PermissionStatus, permission_status};

/// The canonical "first run is complete" sentinel. Its presence is the single
/// thing [`should_show_onboarding`] ultimately reads.
fn setup_done_path() -> PathBuf {
    Config::config_dir().join("setup_done")
}

/// Legacy half-marker: onboarding finished, in builds that tracked onboarding
/// and settings completion separately. Only read by the migration.
fn onboarding_done_path() -> PathBuf {
    Config::config_dir().join("onboarding_done")
}

/// Legacy half-marker: settings bootstrap finished. Migration requires it
/// *together with* [`onboarding_done_path`] before writing the canonical
/// sentinel.
fn legacy_bootstrap_done_path() -> PathBuf {
    Config::config_dir().join("bootstrap_done")
}

/// Resume marker holding the wizard step to reopen on. Distinct from the
/// completion sentinel: this one exists only while onboarding is unfinished.
fn onboarding_progress_path() -> PathBuf {
    Config::config_dir().join("onboarding_progress")
}

/// Canonical first-run wizard step count: `Welcome`, `Mode`, 6× `Permission`
/// (mic → accessibility → input → screen → speech → full-disk), `Language`,
/// `ApiKey`, `HotkeyMode`, `AgenticReadiness`, `Done`. Mirrors the SwiftUI
/// `OnboardingStep` flow; kept here so the persisted resume index can be
/// clamped without depending on the (excised) AppKit step table.
const TOTAL_ONBOARDING_STEPS: usize = 13;

/// Version tag for the persisted resume marker. A bare integer (no tag) is the
/// legacy 12-step layout from before the Speech Recognition step existed; it is
/// remapped on read so mid-onboarding users resume on the same screen after an
/// update instead of a shifted one.
const ONBOARDING_PROGRESS_VERSION_PREFIX: &str = "v2:";

/// Index where the Speech Recognition permission step was inserted (v1 → v2).
const SPEECH_STEP_INSERT_INDEX: usize = 6;

/// Persist the wizard's current step so a relaunch resumes where the user left
/// off. Writer half of the `onboarding_progress` marker; the SwiftUI wizard is
/// now the reader via [`load_onboarding_progress`].
pub fn save_onboarding_progress(step_index: usize) {
    let path = onboarding_progress_path();
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let _ = fs::write(
        path,
        format!("{ONBOARDING_PROGRESS_VERSION_PREFIX}{step_index}"),
    );
}

/// Resume step persisted by [`save_onboarding_progress`], clamped to the last
/// valid step. Returns `0` (Welcome) when no marker exists or it is unparsable.
pub fn load_onboarding_progress() -> usize {
    let raw = fs::read_to_string(onboarding_progress_path()).ok();
    let step = raw
        .as_deref()
        .map(str::trim)
        .and_then(parse_onboarding_progress)
        .unwrap_or(0);
    step.min(TOTAL_ONBOARDING_STEPS.saturating_sub(1))
}

/// Parse a persisted marker in either format. `v2:<n>` is the current 13-step
/// layout verbatim; a bare integer is the pre-Speech 12-step layout, so indices
/// at or past the insertion point shift up by one.
fn parse_onboarding_progress(raw: &str) -> Option<usize> {
    if let Some(current) = raw.strip_prefix(ONBOARDING_PROGRESS_VERSION_PREFIX) {
        return current.trim().parse::<usize>().ok();
    }
    let legacy = raw.parse::<usize>().ok()?;
    Some(if legacy >= SPEECH_STEP_INSERT_INDEX {
        legacy + 1
    } else {
        legacy
    })
}

/// Mark first-run onboarding complete: clear the resume marker and write the
/// canonical `setup_done` sentinel so [`should_show_onboarding`] returns `false`
/// on the next launch.
pub fn mark_onboarding_done() {
    let _ = fs::remove_file(onboarding_progress_path());
    let setup_done = setup_done_path();
    if let Some(parent) = setup_done.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let _ = fs::write(setup_done, "done");
}

/// TCC scopes that must be Granted before `setup_done` may remain valid.
///
/// Full Disk Access is intentionally absent — optional step, never a gate.
const REQUIRED_SETUP_PERMISSIONS: [PermissionKind; 5] = [
    PermissionKind::Microphone,
    PermissionKind::Accessibility,
    PermissionKind::InputMonitoring,
    PermissionKind::ScreenRecording,
    PermissionKind::SpeechRecognition,
];

/// Leading non-permission wizard steps (`Welcome`, `Mode`) that precede the
/// permission block. This offset defines the resume-step layout persisted to
/// the `onboarding_progress` marker; it must match whatever onboarding surface
/// consumes that marker (none does today — the legacy wizard was excised).
const WIZARD_STEPS_BEFORE_PERMISSIONS: usize = 2;

/// Permission steps in resume-flow order, immediately following the leading
/// steps. Must match Swift `OnboardingStep.flow` permission slots.
const PERMISSION_STEP_ORDER: [PermissionKind; 6] = [
    PermissionKind::Microphone,
    PermissionKind::Accessibility,
    PermissionKind::InputMonitoring,
    PermissionKind::ScreenRecording,
    PermissionKind::SpeechRecognition,
    PermissionKind::FullDiskAccess,
];

/// Resolve a permission's index within the resume flow (leading steps +
/// permission offset). Self-contained so this module does not depend on the
/// removed `app/ui` wizard.
fn permission_step_index(kind: PermissionKind) -> Option<usize> {
    PERMISSION_STEP_ORDER
        .iter()
        .position(|candidate| *candidate == kind)
        .map(|offset| WIZARD_STEPS_BEFORE_PERMISSIONS + offset)
}

/// Whether this process is running from inside an `.app` bundle.
///
/// The permission model only applies to bundled runs, so this gates the whole
/// invalidation path. Unresolvable executable path is treated as "not
/// bundled" — the conservative answer, since it cannot revoke a sentinel.
fn current_runtime_is_app_bundle() -> bool {
    std::env::current_exe()
        .map(|path| executable_is_app_bundle(&path))
        .unwrap_or(false)
}

/// Pure half of [`current_runtime_is_app_bundle`], testable without a real
/// executable path.
fn executable_is_app_bundle(path: &std::path::Path) -> bool {
    path.to_string_lossy().contains(".app/Contents/MacOS/")
}

/// Pick one permission's status out of an already-probed snapshot.
///
/// Taking the statuses as parameters keeps the decision logic pure and
/// testable. `FullDiskAccess` is answered `Granted` unconditionally because it
/// is optional — it appears in the step order but never in
/// `REQUIRED_SETUP_PERMISSIONS`, so it must never invalidate the sentinel.
fn permission_status_from_snapshot(
    kind: PermissionKind,
    microphone: PermissionStatus,
    accessibility: PermissionStatus,
    input_monitoring: PermissionStatus,
    screen_recording: PermissionStatus,
    speech_recognition: PermissionStatus,
) -> PermissionStatus {
    match kind {
        PermissionKind::Microphone => microphone,
        PermissionKind::Accessibility => accessibility,
        PermissionKind::InputMonitoring => input_monitoring,
        PermissionKind::ScreenRecording => screen_recording,
        PermissionKind::SpeechRecognition => speech_recognition,
        PermissionKind::FullDiskAccess => PermissionStatus::Granted,
    }
}

/// The step onboarding should reopen on, or `None` to leave `setup_done`
/// standing.
///
/// Pure decision core of [`invalidate_setup_done_if_permissions_missing`].
/// Returns the *earliest* required permission that is not granted, in
/// `REQUIRED_SETUP_PERMISSIONS` order, so the user resumes at the first gap
/// rather than the last one probed.
fn setup_done_refresh_target(
    setup_done_exists: bool,
    app_bundle_runtime: bool,
    microphone: PermissionStatus,
    accessibility: PermissionStatus,
    input_monitoring: PermissionStatus,
    screen_recording: PermissionStatus,
    speech_recognition: PermissionStatus,
) -> Option<usize> {
    if !setup_done_exists || !app_bundle_runtime {
        return None;
    }

    REQUIRED_SETUP_PERMISSIONS
        .into_iter()
        .find(|kind| {
            permission_status_from_snapshot(
                *kind,
                microphone,
                accessibility,
                input_monitoring,
                screen_recording,
                speech_recognition,
            ) != PermissionStatus::Granted
        })
        .and_then(permission_step_index)
}

/// Revoke a `setup_done` that no longer reflects reality, and leave a resume
/// marker pointing at the first missing scope.
///
/// A user can grant permissions during onboarding and revoke them later in
/// System Settings; without this the app would keep believing setup is done
/// while the features behind those scopes are dead. Effectful wrapper around
/// [`setup_done_refresh_target`] — it probes the system and writes files.
fn invalidate_setup_done_if_permissions_missing() {
    let setup_done = setup_done_path();
    if !setup_done.exists() {
        return;
    }

    // Outside an app bundle the permission model does not apply, so setup_done is
    // never invalidated here. Return before the system-wide permission probes
    // below (evaluated as call arguments) so dev/CLI runs pay nothing.
    if !current_runtime_is_app_bundle() {
        return;
    }

    let Some(resume_step) = setup_done_refresh_target(
        true,
        current_runtime_is_app_bundle(),
        permission_status(PermissionKind::Microphone),
        permission_status(PermissionKind::Accessibility),
        permission_status(PermissionKind::InputMonitoring),
        permission_status(PermissionKind::ScreenRecording),
        permission_status(PermissionKind::SpeechRecognition),
    ) else {
        return;
    };

    match fs::remove_file(&setup_done) {
        Ok(()) => {
            save_onboarding_progress(resume_step);
            warn!(
                "Onboarding: removed stale setup_done because required permissions are missing; resuming at step {resume_step}"
            );
        }
        Err(error) => warn!(
            "Onboarding: failed to remove stale setup_done despite missing required permissions: {error}"
        ),
    }
}

/// Fold the two legacy completion markers into the canonical sentinel.
///
/// Both halves are required: a user who finished onboarding but never
/// completed settings bootstrap has not finished first run, and must not be
/// migrated into a completed state.
fn migrate_legacy_setup_done_marker() {
    let setup_done = setup_done_path();
    if setup_done.exists() {
        return;
    }

    // Older builds tracked onboarding and settings completion separately.
    // The current runtime only needs one canonical setup marker.
    if onboarding_done_path().exists() && legacy_bootstrap_done_path().exists() {
        if let Some(parent) = setup_done.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let _ = fs::write(setup_done, "done");
    }
}

/// Returns `true` iff first-run onboarding should be shown: migrates any legacy
/// completion markers, invalidates a stale `setup_done` when required
/// permissions are missing, then reports whether the canonical `setup_done`
/// marker is absent.
pub fn should_show_onboarding() -> bool {
    migrate_legacy_setup_done_marker();
    invalidate_setup_done_if_permissions_missing();
    !setup_done_path().exists()
}

/// Resume-flow layout, required-permission gates, and legacy marker remaps.
#[cfg(test)]
mod tests {
    use super::*;

    /// Wizard steps after the permission block in Swift `OnboardingStep.flow`:
    /// `Language`, `ApiKey`, `HotkeyMode`, `AgenticReadiness`, `Done`.
    const WIZARD_STEPS_AFTER_PERMISSIONS: usize = 5;

    /// The persisted `onboarding_progress` marker is a raw index into the Swift
    /// `OnboardingStep.flow` table, so the Rust step count must stay arithmetically
    /// tied to the same layout. Drifting either side silently resumes users on the
    /// wrong screen.
    #[test]
    fn total_steps_match_swift_flow_layout() {
        assert_eq!(
            TOTAL_ONBOARDING_STEPS,
            WIZARD_STEPS_BEFORE_PERMISSIONS
                + PERMISSION_STEP_ORDER.len()
                + WIZARD_STEPS_AFTER_PERMISSIONS
        );
        assert_eq!(TOTAL_ONBOARDING_STEPS, 13);
    }

    /// Literal indices mirror `OnboardingStep.flow`. Speech Recognition sits after
    /// Screen Recording and before the optional Full Disk Access step.
    #[test]
    fn permission_step_indices_mirror_swift_flow() {
        assert_eq!(permission_step_index(PermissionKind::Microphone), Some(2));
        assert_eq!(
            permission_step_index(PermissionKind::Accessibility),
            Some(3)
        );
        assert_eq!(
            permission_step_index(PermissionKind::InputMonitoring),
            Some(4)
        );
        assert_eq!(
            permission_step_index(PermissionKind::ScreenRecording),
            Some(5)
        );
        assert_eq!(
            permission_step_index(PermissionKind::SpeechRecognition),
            Some(6)
        );
        assert_eq!(
            permission_step_index(PermissionKind::FullDiskAccess),
            Some(7)
        );
    }

    /// Apple live dictation is unusable without the Speech TCC grant, so a missing
    /// one must invalidate `setup_done` like the other required scopes.
    #[test]
    fn speech_recognition_is_required_for_setup_done() {
        assert!(REQUIRED_SETUP_PERMISSIONS.contains(&PermissionKind::SpeechRecognition));
        assert_eq!(
            setup_done_refresh_target(
                true,
                true,
                PermissionStatus::Granted,
                PermissionStatus::Granted,
                PermissionStatus::Granted,
                PermissionStatus::Granted,
                PermissionStatus::NotDetermined,
            ),
            Some(6)
        );
    }

    /// All five required grants leave `setup_done` intact (no resume step).
    #[test]
    fn all_required_permissions_granted_keeps_setup_done() {
        assert_eq!(
            setup_done_refresh_target(
                true,
                true,
                PermissionStatus::Granted,
                PermissionStatus::Granted,
                PermissionStatus::Granted,
                PermissionStatus::Granted,
                PermissionStatus::Granted,
            ),
            None
        );
    }

    /// Full Disk Access is optional — it is in the step order but never in
    /// `REQUIRED_SETUP_PERMISSIONS`, so it can never invalidate `setup_done`.
    #[test]
    fn full_disk_access_never_invalidates_setup_done() {
        assert!(!REQUIRED_SETUP_PERMISSIONS.contains(&PermissionKind::FullDiskAccess));
        assert_eq!(
            permission_status_from_snapshot(
                PermissionKind::FullDiskAccess,
                PermissionStatus::Denied,
                PermissionStatus::Denied,
                PermissionStatus::Denied,
                PermissionStatus::Denied,
                PermissionStatus::Denied,
            ),
            PermissionStatus::Granted
        );
    }

    /// Resume lands on the *earliest* missing scope, not the last one probed.
    #[test]
    fn earliest_missing_permission_wins_the_resume_step() {
        assert_eq!(
            setup_done_refresh_target(
                true,
                true,
                PermissionStatus::Denied,
                PermissionStatus::Granted,
                PermissionStatus::Granted,
                PermissionStatus::Granted,
                PermissionStatus::Denied,
            ),
            Some(2)
        );
    }

    /// Outside an app bundle (dev/CLI runs) the TCC model does not apply, and with
    /// no `setup_done` there is nothing to invalidate.
    #[test]
    fn non_bundle_or_missing_sentinel_never_invalidates() {
        let all_missing = |bundle: bool, sentinel: bool| {
            setup_done_refresh_target(
                sentinel,
                bundle,
                PermissionStatus::Denied,
                PermissionStatus::Denied,
                PermissionStatus::Denied,
                PermissionStatus::Denied,
                PermissionStatus::Denied,
            )
        };
        assert_eq!(all_missing(false, true), None);
        assert_eq!(all_missing(true, false), None);
    }

    /// Legacy bare-integer markers come from the 12-step layout without the
    /// Speech Recognition step: indices at or past the insertion point must
    /// shift by one so the user resumes on the same *screen*, not the same raw
    /// number (e.g. legacy 8 = ApiKey → 9 = ApiKey in the 13-step flow).
    #[test]
    fn legacy_progress_markers_remap_across_the_speech_step_insertion() {
        // Before the insertion point: unchanged.
        assert_eq!(parse_onboarding_progress("0"), Some(0));
        assert_eq!(parse_onboarding_progress("5"), Some(5));
        // At/after the insertion point: shifted by one.
        assert_eq!(parse_onboarding_progress("6"), Some(7));
        assert_eq!(parse_onboarding_progress("8"), Some(9));
        assert_eq!(parse_onboarding_progress("11"), Some(12));
        // Current format passes through verbatim.
        assert_eq!(parse_onboarding_progress("v2:6"), Some(6));
        assert_eq!(parse_onboarding_progress("v2:12"), Some(12));
        // Garbage is unparsable in both formats.
        assert_eq!(parse_onboarding_progress("v2:x"), None);
        assert_eq!(parse_onboarding_progress("not-a-number"), None);
    }

    /// Bundle path under `*.app/Contents/MacOS/` vs bare cargo/bin install.
    #[test]
    fn app_bundle_detection_matches_bundle_layout() {
        assert!(executable_is_app_bundle(std::path::Path::new(
            "/Applications/Codescribe.app/Contents/MacOS/codescribe"
        )));
        assert!(!executable_is_app_bundle(std::path::Path::new(
            "/Users/someone/.cargo/bin/codescribe"
        )));
    }
}
