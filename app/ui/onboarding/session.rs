//! Onboarding session lifecycle on disk: completion markers, resume
//! progress, legacy marker migration, and the flock(2)-based single-session
//! lock.

use std::fs;
use std::fs::{File, OpenOptions};
use std::io::{Seek, SeekFrom, Write};
use std::os::fd::AsRawFd;
use std::path::PathBuf;
use std::sync::Mutex;

use tracing::warn;

use crate::config::Config;
use crate::os::permissions::PermissionStatus;

use super::permission_flow::permission_status;
use super::steps::{PermissionKind, STEP_FLOW, TOTAL_STEPS, WizardStep};

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

fn onboarding_lock_path() -> PathBuf {
    Config::config_dir().join("onboarding_session.lock")
}

pub(super) fn load_onboarding_progress() -> usize {
    let raw = fs::read_to_string(onboarding_progress_path()).ok();
    let step = raw
        .as_deref()
        .and_then(|s| s.trim().parse::<usize>().ok())
        .unwrap_or(0);
    step.min(TOTAL_STEPS.saturating_sub(1))
}

pub(super) fn save_onboarding_progress(step_index: usize) {
    let path = onboarding_progress_path();
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let _ = fs::write(path, step_index.to_string());
}

fn clear_onboarding_progress() {
    let _ = fs::remove_file(onboarding_progress_path());
}

const REQUIRED_SETUP_PERMISSIONS: [PermissionKind; 4] = [
    PermissionKind::Microphone,
    PermissionKind::Accessibility,
    PermissionKind::InputMonitoring,
    PermissionKind::ScreenRecording,
];

fn permission_step_index(kind: PermissionKind) -> Option<usize> {
    STEP_FLOW
        .iter()
        .position(|step| *step == WizardStep::Permission(kind))
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

pub(super) fn setup_done_refresh_target(
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

/// Best-effort liveness probe for a PID via `kill(pid, 0)`.
///
/// Retained for diagnostics and possible future tooling: the current lock
/// path uses `flock(2)` and no longer relies on PID liveness to gate access.
// FORGOTTEN-GEM(vc-prune 2026-06-10): kill(pid,0) liveness probe with no
// callers — likely intended for onboarding daemon checks that never landed.
// Wire it or delete it; operator decision tracked in forgotten-gems report.
#[allow(dead_code)]
fn process_is_alive(pid: u32) -> bool {
    let result = unsafe { libc::kill(pid as i32, 0) };
    if result == 0 {
        return true;
    }

    std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

/// Holds the open `File` for the onboarding lock for the lifetime of the
/// process. Dropping the `File` closes the fd, which atomically releases the
/// `flock(2)` advisory lock — so we MUST keep it parked here.
static ONBOARDING_LOCK_FILE: Mutex<Option<File>> = Mutex::new(None);

/// Acquire an exclusive, non-blocking advisory lock on the onboarding session
/// file using `flock(2)`. Returns `true` iff this process now holds the lock.
///
/// Contract:
/// - Two simultaneous launches: exactly one wins, the other gets `false`.
/// - The lock is released automatically when the process exits (kernel closes
///   the fd) OR when [`release_onboarding_lock`] is called explicitly.
/// - The PID written to the file is informational only (for `ps`/log triage);
///   correctness comes from `flock`, not from the PID contents.
/// - Replaces an earlier check-then-create scheme that had a TOCTOU window
///   between liveness check and re-create — two launches could both pass and
///   both create the file. `flock` closes that window at the kernel level.
pub(super) fn acquire_onboarding_lock() -> bool {
    let path = onboarding_lock_path();
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }

    let mut file = match OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(&path)
    {
        Ok(file) => file,
        Err(e) => {
            warn!("Onboarding: failed to open lock file: {e}");
            return false;
        }
    };

    // Non-blocking exclusive advisory lock. If another process holds it,
    // `flock` returns -1 with errno EWOULDBLOCK and we bail out cleanly.
    // SAFETY: `file.as_raw_fd()` is a valid borrowed fd for the lifetime of
    // `file`, which outlives the `flock(2)` syscall. The flag bitmask is
    // composed of libc-provided constants. No memory is read or written.
    let rc = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    if rc != 0 {
        let err = std::io::Error::last_os_error();
        if err.raw_os_error() == Some(libc::EWOULDBLOCK) {
            // Try to read the holder PID for a useful diagnostic. Best-effort.
            let holder_pid = fs::read_to_string(&path)
                .ok()
                .and_then(|raw| raw.trim().parse::<u32>().ok());
            match holder_pid {
                Some(pid) => warn!(
                    "Onboarding: lock is held by live process pid={pid}, skipping duplicate wizard"
                ),
                None => {
                    warn!("Onboarding: lock is held by another process, skipping duplicate wizard")
                }
            }
        } else {
            warn!("Onboarding: failed to acquire lock via flock: {err}");
        }
        return false;
    }

    // We own the lock. Refresh the PID record for human diagnostics. Failures
    // here do not affect correctness — the lock is what gates concurrency.
    let pid = std::process::id();
    let _ = file.set_len(0);
    let _ = file.seek(SeekFrom::Start(0));
    let _ = write!(file, "{pid}");
    let _ = file.flush();

    // Park the file so the fd stays open and the lock persists for the
    // process lifetime. Dropping the file would close the fd and release
    // the kernel-level lock immediately.
    let mut guard = match ONBOARDING_LOCK_FILE.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };
    *guard = Some(file);
    true
}

pub(super) fn release_onboarding_lock() {
    let mut guard = match ONBOARDING_LOCK_FILE.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };
    if let Some(file) = guard.take() {
        // Explicit unlock first; dropping the File closes the fd which would
        // release the lock anyway, but explicit `LOCK_UN` is cheap insurance.
        // SAFETY: `file.as_raw_fd()` is a valid borrowed fd for the lifetime
        // of `file`, which is held until the explicit `drop(file)` below.
        // `LOCK_UN` is a single libc constant. No memory is read or written.
        let _ = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_UN) };
        drop(file);
    }
    // Best-effort cleanup so a stale lock file does not linger between runs.
    let _ = fs::remove_file(onboarding_lock_path());
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

pub fn should_show_onboarding() -> bool {
    migrate_legacy_setup_done_marker();
    invalidate_setup_done_if_permissions_missing();
    !setup_done_path().exists()
}

pub(super) fn mark_onboarding_done() {
    clear_onboarding_progress();
    let setup_done = setup_done_path();
    if let Some(parent) = setup_done.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let _ = fs::write(setup_done, "done");
}
