//! Process-wide coordination between configuration I/O and destructive reset.
//!
//! A reset moves the live data roots to Trash and the Swift host relaunches the
//! process immediately afterwards. Without a fence, a background config load
//! can finish a migration after the move and silently recreate `settings.json`
//! in the supposedly empty live root. The gate below gives reset exclusive
//! ownership of config/settings/prompt persistence, drains operations that
//! already started, and permanently rejects new configuration I/O once the
//! first destructive move is armed. The latch is
//! intentionally process-lifetime state: after a destructive reset, relaunch is
//! the only supported way back to an open data plane.

use std::cell::Cell;
use std::fmt;
use std::marker::PhantomData;
use std::path::Path;
use std::rc::Rc;
use std::sync::{Condvar, Mutex, MutexGuard, OnceLock};

/// State of the process-wide configuration persistence plane.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ResetPhase {
    Open,
    Resetting,
    Latched,
}

/// Mutable state protected by [`gate`].
#[derive(Debug)]
struct ResetState {
    phase: ResetPhase,
    active_operations: usize,
    waiting_operations: usize,
}

impl Default for ResetState {
    fn default() -> Self {
        Self {
            phase: ResetPhase::Open,
            active_operations: 0,
            waiting_operations: 0,
        }
    }
}

/// One mutex/condition-variable pair owns the whole process data plane.
fn gate() -> &'static (Mutex<ResetState>, Condvar) {
    static GATE: OnceLock<(Mutex<ResetState>, Condvar)> = OnceLock::new();
    GATE.get_or_init(|| (Mutex::new(ResetState::default()), Condvar::new()))
}

/// Recover from a poisoned lock: a panic in one caller must not disable reset
/// protection for the rest of the process.
fn lock_state() -> MutexGuard<'static, ResetState> {
    match gate().0.lock() {
        Ok(state) => state,
        Err(poisoned) => {
            let mut state = poisoned.into_inner();
            state.phase = ResetPhase::Latched;
            gate().1.notify_all();
            state
        }
    }
}

/// Wait while preserving the same poison-recovery policy as [`lock_state`].
fn wait_state(guard: MutexGuard<'static, ResetState>) -> MutexGuard<'static, ResetState> {
    match gate().1.wait(guard) {
        Ok(state) => state,
        Err(poisoned) => {
            let mut state = poisoned.into_inner();
            state.phase = ResetPhase::Latched;
            gate().1.notify_all();
            state
        }
    }
}

thread_local! {
    /// Nested config operations are one logical active operation. This matters
    /// because `Config::load()` calls `UserSettings::load()`, and a reset may
    /// begin between those two calls. Counting the nested call separately would
    /// deadlock the loader against the reset waiting for its outer guard.
    static APP_DATA_IO_DEPTH: Cell<usize> = const { Cell::new(0) };
}

/// Configuration I/O is unavailable because a destructive reset owns the process.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AppDataUnavailable {
    reason: UnavailableReason,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum UnavailableReason {
    Phase(ResetPhase),
    ReentrantReset,
}

impl fmt::Display for AppDataUnavailable {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.reason {
            UnavailableReason::ReentrantReset => {
                formatter.write_str("cannot start app-data reset inside an active data operation")
            }
            UnavailableReason::Phase(phase) => match phase {
                ResetPhase::Open => formatter.write_str("app-data I/O unavailable"),
                ResetPhase::Resetting => formatter.write_str("app-data reset is in progress"),
                ResetPhase::Latched => {
                    formatter.write_str("app-data reset completed; process relaunch required")
                }
            },
        }
    }
}

impl std::error::Error for AppDataUnavailable {}

/// RAII admission for one configuration persistence operation.
pub(crate) struct AppDataIoGuard {
    counted_globally: bool,
    /// The nesting counter is thread-local, so moving this guard across threads
    /// would decrement a different thread's depth and corrupt the reset fence.
    _not_send: PhantomData<Rc<()>>,
}

/// Enter the data plane, waiting for a non-destructive reset preparation to
/// finish. Once reset is latched, callers fail instead of touching live roots.
pub(crate) fn begin_app_data_io() -> Result<AppDataIoGuard, AppDataUnavailable> {
    let nested = APP_DATA_IO_DEPTH.with(|depth| {
        let current = depth.get();
        if current > 0 {
            depth.set(current + 1);
            true
        } else {
            false
        }
    });
    if nested {
        return Ok(AppDataIoGuard {
            counted_globally: false,
            _not_send: PhantomData,
        });
    }

    let mut state = lock_state();
    while state.phase == ResetPhase::Resetting {
        state.waiting_operations += 1;
        gate().1.notify_all();
        state = wait_state(state);
        if state.waiting_operations == 0 {
            state.phase = ResetPhase::Latched;
        } else {
            state.waiting_operations -= 1;
        }
    }
    if state.phase == ResetPhase::Latched {
        return Err(AppDataUnavailable {
            reason: UnavailableReason::Phase(state.phase),
        });
    }

    state.active_operations += 1;
    APP_DATA_IO_DEPTH.with(|depth| depth.set(1));
    Ok(AppDataIoGuard {
        counted_globally: true,
        _not_send: PhantomData,
    })
}

impl Drop for AppDataIoGuard {
    fn drop(&mut self) {
        let (remaining_depth, depth_underflow) = APP_DATA_IO_DEPTH.with(|depth| {
            let current = depth.get();
            let remaining = if current == 0 { 0 } else { current - 1 };
            depth.set(remaining);
            (remaining, current == 0)
        });

        if depth_underflow {
            let mut state = lock_state();
            state.phase = ResetPhase::Latched;
            gate().1.notify_all();
            return;
        }

        if !self.counted_globally {
            return;
        }
        let mut state = lock_state();
        if remaining_depth != 0 || state.active_operations == 0 {
            // A broken RAII/nesting invariant must fail closed. Decrementing
            // anyway could let reset move a root beneath a still-live writer.
            state.phase = ResetPhase::Latched;
            gate().1.notify_all();
            return;
        }
        state.active_operations -= 1;
        if state.active_operations == 0 {
            gate().1.notify_all();
        }
    }
}

/// Exclusive ownership of the app-data plane during a reset.
///
/// Dropping this guard before [`Self::mark_destructive_started`] reopens the
/// plane. Dropping it afterwards deliberately leaves the process latched.
pub struct AppDataResetGuard {
    destructive_started: bool,
}

/// Stop new app-data operations and wait until every already-admitted operation
/// has finished. A second reset is rejected instead of sharing ownership.
pub fn begin_app_data_reset() -> Result<AppDataResetGuard, AppDataUnavailable> {
    if APP_DATA_IO_DEPTH.with(|depth| depth.get() > 0) {
        return Err(AppDataUnavailable {
            reason: UnavailableReason::ReentrantReset,
        });
    }
    let mut state = lock_state();
    if state.phase != ResetPhase::Open {
        return Err(AppDataUnavailable {
            reason: UnavailableReason::Phase(state.phase),
        });
    }
    state.phase = ResetPhase::Resetting;
    gate().1.notify_all();
    while state.active_operations > 0 {
        state = wait_state(state);
    }
    Ok(AppDataResetGuard {
        destructive_started: false,
    })
}

impl AppDataResetGuard {
    /// Whether any irreversible move/remove has happened. Callers must relaunch
    /// even when later cleanup reports an error, because this process may no
    /// longer resume normal app-data I/O.
    pub fn relaunch_required(&self) -> bool {
        self.destructive_started
    }

    /// Try one atomic destructive filesystem operation. When the operation is
    /// the first one in this reset, an error proves no atomic move occurred, so
    /// the guard returns to `Resetting` and may safely perform a copy fallback.
    /// Once any earlier destructive operation succeeded, the latch is permanent.
    #[doc(hidden)]
    pub fn rename_destructively(
        &mut self,
        source: &Path,
        destination: &Path,
    ) -> std::io::Result<()> {
        if self.destructive_started {
            return std::fs::rename(source, destination);
        }

        // Keep the state lock across this one atomic syscall. New admissions
        // therefore observe either Resetting after a failed rename or Latched
        // after a successful one, never the speculative state in between.
        let mut state = lock_state();
        debug_assert_eq!(state.phase, ResetPhase::Resetting);
        state.phase = ResetPhase::Latched;
        match std::fs::rename(source, destination) {
            Ok(()) => {
                self.destructive_started = true;
                gate().1.notify_all();
                Ok(())
            }
            Err(error) => {
                state.phase = ResetPhase::Resetting;
                Err(error)
            }
        }
    }

    /// Arm the process-lifetime latch immediately before the first move/remove.
    /// Waiting operations wake and fail without ever reaching the live roots.
    pub fn mark_destructive_started(&mut self) {
        if self.destructive_started {
            return;
        }
        let mut state = lock_state();
        debug_assert_eq!(state.phase, ResetPhase::Resetting);
        debug_assert_eq!(state.active_operations, 0);
        state.phase = ResetPhase::Latched;
        self.destructive_started = true;
        gate().1.notify_all();
    }

    /// Only the reset owner may restore explicitly preserved bytes after the
    /// live root has moved. Normal writers remain fenced out.
    pub(crate) fn permits_preserved_restore(&self) -> bool {
        self.destructive_started
    }
}

impl Drop for AppDataResetGuard {
    fn drop(&mut self) {
        if self.destructive_started {
            return;
        }
        let mut state = lock_state();
        if state.phase == ResetPhase::Resetting {
            state.phase = ResetPhase::Open;
            gate().1.notify_all();
        }
    }
}

#[cfg(test)]
fn wait_for_blocked_io_for_tests() {
    let mut state = lock_state();
    while state.waiting_operations == 0 {
        state = wait_state(state);
    }
}

#[cfg(test)]
fn wait_for_resetting_for_tests() {
    let mut state = lock_state();
    while state.phase != ResetPhase::Resetting {
        state = wait_state(state);
    }
}

#[cfg(test)]
fn reopen_after_test() {
    let mut state = lock_state();
    state.phase = ResetPhase::Open;
    gate().1.notify_all();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Config, UserSettings};
    use serial_test::serial;
    use std::ffi::OsString;
    use std::fs;
    use std::sync::mpsc;
    use std::thread;
    use tempfile::TempDir;

    /// Restore the process env only after the reset latch has held every other
    /// config caller. Then reopen the gate so the rest of the test process sees
    /// the restored root, never this test's disappearing temp directory.
    struct TestResetCleanup {
        previous_data_dir: Option<OsString>,
    }

    impl TestResetCleanup {
        fn install(data_dir: &std::path::Path) -> Self {
            let previous_data_dir = std::env::var_os("CODESCRIBE_DATA_DIR");
            // SAFETY: this regression owns the serial config/reset lane.
            unsafe { std::env::set_var("CODESCRIBE_DATA_DIR", data_dir) };
            Self { previous_data_dir }
        }
    }

    impl Drop for TestResetCleanup {
        fn drop(&mut self) {
            // SAFETY: restore the exact process environment before waking any
            // config caller that was held behind this test's reset latch.
            unsafe {
                match &self.previous_data_dir {
                    Some(value) => std::env::set_var("CODESCRIBE_DATA_DIR", value),
                    None => std::env::remove_var("CODESCRIBE_DATA_DIR"),
                }
            }
            reopen_after_test();
        }
    }

    /// Deterministic reproduction of the I4E-F audit RED: a config load reaches
    /// the reset fence while the legacy settings root is being moved. It must
    /// return without migrating or recreating anything in the live root.
    #[test]
    #[serial]
    fn reset_fence_prevents_concurrent_config_migration_from_recreating_live_root() {
        const CHILD_FLAG: &str = "CODESCRIBE_TEST_RESET_FENCE_CHILD";
        const CHILD_WITNESS: &str = "CODESCRIBE_TEST_RESET_FENCE_WITNESS";
        const WITNESS_BYTES: &[u8] = b"reset-fence-pass";
        if std::env::var_os(CHILD_FLAG).is_none() {
            let witness_dir = TempDir::new().expect("child witness dir");
            let witness = witness_dir.path().join("passed");
            let current_test = concat!(
                "config::storage_reset::tests::",
                "reset_fence_prevents_concurrent_config_migration_from_recreating_live_root"
            );
            let status = std::process::Command::new(
                std::env::current_exe().expect("current core test executable"),
            )
            .args(["--exact", current_test, "--nocapture"])
            .env(CHILD_FLAG, "1")
            .env(CHILD_WITNESS, &witness)
            .status()
            .expect("spawn isolated reset-fence regression");
            assert!(status.success(), "isolated reset-fence regression failed");
            assert_eq!(
                fs::read(witness).expect("child completed exact reset-fence test"),
                WITNESS_BYTES,
                "child command exited successfully without executing the exact regression"
            );
            return;
        }

        let sandbox = TempDir::new().expect("reset race sandbox");
        let live_root = sandbox.path().join("live");
        let trashed_root = sandbox.path().join("trashed");
        fs::create_dir_all(&live_root).expect("create live root");
        fs::write(live_root.join("settings.json"), b"{}").expect("seed legacy settings");
        let _cleanup = TestResetCleanup::install(&live_root);

        // Admit one real settings transaction first. Reset must wait for it,
        // while a later Config load must queue behind Resetting.
        let admitted = begin_app_data_io().expect("admit pre-existing config transaction");
        let moved_live_root = live_root.clone();
        let moved_trashed_root = trashed_root.clone();
        let (reset_acquired_tx, reset_acquired_rx) = mpsc::channel();
        let resetter = thread::spawn(move || {
            let mut reset = begin_app_data_reset().expect("reset drains admitted writer");
            reset_acquired_tx
                .send(())
                .expect("report exclusive reset ownership");
            reset
                .rename_destructively(&moved_live_root, &moved_trashed_root)
                .expect("move live root after admitted writer drains");
        });
        wait_for_resetting_for_tests();
        assert!(
            matches!(reset_acquired_rx.try_recv(), Err(mpsc::TryRecvError::Empty)),
            "reset crossed an already-admitted app-data transaction"
        );

        let late_writer = thread::spawn(Config::load_without_keychain);
        wait_for_blocked_io_for_tests();

        // This is the exact operation seen in the audit residue: a V1 load
        // writes its backup and V3 replacement. It is allowed to finish because
        // it entered before reset, and reset may move only after this guard ends.
        let _ = UserSettings::load();
        assert!(live_root.join("settings.v1.bak.json").is_file());
        drop(admitted);

        reset_acquired_rx
            .recv()
            .expect("reset acquires after admitted writer finishes");
        resetter.join().expect("reset thread joins");
        let _ = late_writer.join().expect("late config load joins");

        let blocked_save = UserSettings::default().save();
        assert!(
            blocked_save.is_err(),
            "settings save must fail after reset latch"
        );
        assert!(!live_root.exists(), "writer recreated the reset live root");
        let migrated: serde_json::Value = serde_json::from_slice(
            &fs::read(trashed_root.join("settings.json")).expect("read trashed settings"),
        )
        .expect("parse migrated settings");
        assert_eq!(
            migrated
                .get("schema_version")
                .and_then(|value| value.as_u64()),
            Some(3)
        );
        assert_eq!(
            fs::read(trashed_root.join("settings.v1.bak.json")).expect("read trashed V1 backup"),
            b"{}"
        );
        let mut names: Vec<_> = fs::read_dir(&trashed_root)
            .expect("read trashed root")
            .map(|entry| entry.expect("trashed entry").file_name())
            .collect();
        names.sort();
        assert_eq!(
            names,
            [
                OsString::from("settings.json"),
                OsString::from("settings.v1.bak.json")
            ]
        );
        fs::write(
            std::env::var_os(CHILD_WITNESS).expect("child witness path"),
            WITNESS_BYTES,
        )
        .expect("write reset-fence child witness");
    }
}
