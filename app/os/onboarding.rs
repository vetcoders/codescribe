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

/// Nine semantic chapters, matching Swift `OnboardingStep.flow`.
const TOTAL_ONBOARDING_STEPS: usize = 9;
const ONBOARDING_PROGRESS_VERSION_PREFIX: &str = "v3:";
const PERMISSIONS_CHAPTER_INDEX: usize = 2;

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
        format!(
            "{ONBOARDING_PROGRESS_VERSION_PREFIX}{}",
            step_index.min(TOTAL_ONBOARDING_STEPS - 1)
        ),
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

/// v3 contains chapter indices. v2 contains the former 13-screen layout.
/// Bare markers predate the Speech Recognition insertion at index six: apply
/// that insertion first, then collapse permission screens into their chapter.
fn parse_onboarding_progress(raw: &str) -> Option<usize> {
    if let Some(current) = raw.strip_prefix(ONBOARDING_PROGRESS_VERSION_PREFIX) {
        return current.trim().parse::<usize>().ok();
    }
    let old_step = if let Some(v2) = raw.strip_prefix("v2:") {
        v2.trim().parse::<usize>().ok()?
    } else {
        let bare = raw.parse::<usize>().ok()?;
        if bare >= 6 {
            bare.saturating_add(1)
        } else {
            bare
        }
    };
    Some(match old_step {
        0..=1 => old_step,
        2..=7 => PERMISSIONS_CHAPTER_INDEX,
        8 => 3,
        9 => 5,
        10 => 6,
        11 => 7,
        _ => 8,
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

/// Required grants can reopen setup. Screen Recording and Full Disk Access
/// are optional feature scopes and never invalidate the completion sentinel.
const REQUIRED_SETUP_PERMISSIONS: [PermissionKind; 4] = [
    PermissionKind::Microphone,
    PermissionKind::Accessibility,
    PermissionKind::InputMonitoring,
    PermissionKind::SpeechRecognition,
];

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
/// testable. `FullDiskAccess` and `ScreenRecording` are answered `Granted`
/// because they are optional. Neither appears in `REQUIRED_SETUP_PERMISSIONS`,
/// so neither may invalidate the sentinel.
fn permission_status_from_snapshot(
    kind: PermissionKind,
    microphone: PermissionStatus,
    accessibility: PermissionStatus,
    input_monitoring: PermissionStatus,
    _screen_recording: PermissionStatus,
    speech_recognition: PermissionStatus,
) -> PermissionStatus {
    match kind {
        PermissionKind::Microphone => microphone,
        PermissionKind::Accessibility => accessibility,
        PermissionKind::InputMonitoring => input_monitoring,
        PermissionKind::ScreenRecording => PermissionStatus::Granted,
        PermissionKind::SpeechRecognition => speech_recognition,
        PermissionKind::FullDiskAccess => PermissionStatus::Granted,
    }
}

/// The step onboarding should reopen on, or `None` to leave `setup_done`
/// standing.
///
/// Pure decision core of [`invalidate_setup_done_if_permissions_missing`].
/// Returns the shared permissions chapter for any missing required grant.
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
        .map(|_| PERMISSIONS_CHAPTER_INDEX)
}

/// Revoke a `setup_done` that no longer reflects reality, and leave a resume
/// marker pointing at the shared permissions chapter.
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
        PermissionStatus::Granted,
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

/// Persisted setup chapters and permission requirements across wizard versions.
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grouped_setup_has_nine_chapters() {
        assert_eq!(TOTAL_ONBOARDING_STEPS, 9);
        assert_eq!(ONBOARDING_PROGRESS_VERSION_PREFIX, "v3:");
    }

    #[test]
    fn every_missing_required_grant_resumes_the_permissions_chapter() {
        for missing in [0, 1, 2, 4] {
            let mut statuses = [PermissionStatus::Granted; 5];
            statuses[missing] = PermissionStatus::Denied;
            assert_eq!(
                setup_done_refresh_target(
                    true,
                    true,
                    statuses[0],
                    statuses[1],
                    statuses[2],
                    statuses[3],
                    statuses[4],
                ),
                Some(2),
                "missing permission position {missing}"
            );
        }
    }

    #[test]
    fn speech_recognition_remains_a_required_grant() {
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
            Some(2)
        );
    }

    #[test]
    fn optional_screen_and_disk_access_never_invalidate_setup_done() {
        assert!(!REQUIRED_SETUP_PERMISSIONS.contains(&PermissionKind::ScreenRecording));
        assert!(!REQUIRED_SETUP_PERMISSIONS.contains(&PermissionKind::FullDiskAccess));
        for screen in [PermissionStatus::Denied, PermissionStatus::NotDetermined] {
            assert_eq!(
                setup_done_refresh_target(
                    true,
                    true,
                    PermissionStatus::Granted,
                    PermissionStatus::Granted,
                    PermissionStatus::Granted,
                    screen,
                    PermissionStatus::Granted,
                ),
                None
            );
        }
    }

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

    #[test]
    fn version_two_markers_keep_their_semantic_chapter() {
        let expected = [0, 1, 2, 2, 2, 2, 2, 2, 3, 5, 6, 7, 8];
        for (index, chapter) in expected.into_iter().enumerate() {
            assert_eq!(
                parse_onboarding_progress(&format!("v2:{index}")),
                Some(chapter)
            );
        }
    }

    #[test]
    fn bare_markers_include_the_speech_insertion_before_grouping() {
        let expected = [0, 1, 2, 2, 2, 2, 2, 3, 5, 6, 7, 8];
        for (index, chapter) in expected.into_iter().enumerate() {
            assert_eq!(parse_onboarding_progress(&index.to_string()), Some(chapter));
        }
    }

    #[test]
    fn version_three_markers_keep_current_chapters_and_reject_malformed_values() {
        for index in 0..9 {
            assert_eq!(
                parse_onboarding_progress(&format!("v3:{index}")),
                Some(index)
            );
        }
        for malformed in ["v3:x", "v2:x", "not-a-number", "-1", "v4:5", ""] {
            assert_eq!(parse_onboarding_progress(malformed), None, "{malformed}");
        }
    }

    #[test]
    fn app_bundle_detection_matches_bundle_layout() {
        assert!(executable_is_app_bundle(std::path::Path::new(
            "/Applications/Codescribe.app/Contents/MacOS/codescribe"
        )));
        assert!(!executable_is_app_bundle(std::path::Path::new(
            "/Users/tester/.cargo/bin/codescribe"
        )));
    }
}
