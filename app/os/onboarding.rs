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

fn setup_done_path() -> PathBuf {
    Config::config_dir().join("setup_done")
}

fn onboarding_done_path() -> PathBuf {
    Config::config_dir().join("onboarding_done")
}

fn legacy_bootstrap_done_path() -> PathBuf {
    Config::config_dir().join("bootstrap_done")
}

fn onboarding_progress_path() -> PathBuf {
    Config::config_dir().join("onboarding_progress")
}

fn save_onboarding_progress(step_index: usize) {
    let path = onboarding_progress_path();
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let _ = fs::write(path, step_index.to_string());
}

const REQUIRED_SETUP_PERMISSIONS: [PermissionKind; 4] = [
    PermissionKind::Microphone,
    PermissionKind::Accessibility,
    PermissionKind::InputMonitoring,
    PermissionKind::ScreenRecording,
];

/// Leading non-permission wizard steps (`Welcome`, `Mode`) that precede the
/// permission block. This offset defines the resume-step layout persisted to
/// the `onboarding_progress` marker; it must match whatever onboarding surface
/// consumes that marker (none does today — the legacy wizard was excised).
const WIZARD_STEPS_BEFORE_PERMISSIONS: usize = 2;

/// Permission steps in resume-flow order, immediately following the leading
/// steps.
const PERMISSION_STEP_ORDER: [PermissionKind; 5] = [
    PermissionKind::Microphone,
    PermissionKind::Accessibility,
    PermissionKind::InputMonitoring,
    PermissionKind::ScreenRecording,
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

fn current_runtime_is_app_bundle() -> bool {
    std::env::current_exe()
        .map(|path| executable_is_app_bundle(&path))
        .unwrap_or(false)
}

fn executable_is_app_bundle(path: &std::path::Path) -> bool {
    path.to_string_lossy().contains(".app/Contents/MacOS/")
}

fn permission_status_from_snapshot(
    kind: PermissionKind,
    microphone: PermissionStatus,
    accessibility: PermissionStatus,
    input_monitoring: PermissionStatus,
    screen_recording: PermissionStatus,
) -> PermissionStatus {
    match kind {
        PermissionKind::Microphone => microphone,
        PermissionKind::Accessibility => accessibility,
        PermissionKind::InputMonitoring => input_monitoring,
        PermissionKind::ScreenRecording => screen_recording,
        PermissionKind::FullDiskAccess => PermissionStatus::Granted,
    }
}

fn setup_done_refresh_target(
    setup_done_exists: bool,
    app_bundle_runtime: bool,
    microphone: PermissionStatus,
    accessibility: PermissionStatus,
    input_monitoring: PermissionStatus,
    screen_recording: PermissionStatus,
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
            ) != PermissionStatus::Granted
        })
        .and_then(permission_step_index)
}

fn invalidate_setup_done_if_permissions_missing() {
    let setup_done = setup_done_path();
    if !setup_done.exists() {
        return;
    }

    let Some(resume_step) = setup_done_refresh_target(
        true,
        current_runtime_is_app_bundle(),
        permission_status(PermissionKind::Microphone),
        permission_status(PermissionKind::Accessibility),
        permission_status(PermissionKind::InputMonitoring),
        permission_status(PermissionKind::ScreenRecording),
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
