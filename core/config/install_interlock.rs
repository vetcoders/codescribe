//! Cross-process leases the installer consults before replacing the bundle.
//!
//! Two files under `~/.codescribe`:
//! - `install-runtime.lock` — held shared for the app process lifetime. Since
//!   2026-09-08 (Founder) a merely running app does **not** refuse
//!   `make install-if-idle`; the lease only lets an exclusive installer step
//!   refuse a *new* runtime start while it is copying.
//! - `agent-turn.lock` — held shared only while an agent turn is in flight.
//!   The installer probes it exclusively and refuses while a turn runs, so a
//!   bundle swap never lands under a streaming reply or a running tool.
//! A live recording is judged from the Transcript Bus, not from a lock.

use std::fs::{File, OpenOptions};
use std::io;
use std::os::fd::AsRawFd;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use directories::BaseDirs;

/// Shared filename used by the app runtime and `install-if-idle`.
pub const INSTALL_INTERLOCK_FILE_NAME: &str = "install-runtime.lock";

/// Lease file held shared for the duration of one agent turn.
pub const AGENT_TURN_LEASE_FILE_NAME: &str = "agent-turn.lock";

/// A process-lifetime shared lease. Closing the file releases the kernel lock.
pub struct AppRuntimeInstallLease {
    _file: File,
}

/// A turn-lifetime shared lease. Dropping it (normal end, error, Stop, or
/// process death) releases the kernel lock, so the installer can never be
/// wedged by a turn that is no longer running.
pub struct AgentTurnLease {
    _file: File,
}

/// Resolve the per-user interlock independently of runtime data-path overrides.
///
/// The app acquires this before dotenv bootstrap, so allowing
/// `CODESCRIBE_DATA_DIR` to relocate it would let the installer and runtime
/// lock different files.
pub fn install_interlock_path() -> PathBuf {
    BaseDirs::new()
        .map(|dirs| {
            dirs.home_dir()
                .join(".codescribe")
                .join(INSTALL_INTERLOCK_FILE_NAME)
        })
        .unwrap_or_else(|| PathBuf::from(".codescribe").join(INSTALL_INTERLOCK_FILE_NAME))
}

/// Resolve the agent-turn lease next to the runtime interlock, with the same
/// data-dir-override independence.
pub fn agent_turn_lease_path() -> PathBuf {
    install_interlock_path().with_file_name(AGENT_TURN_LEASE_FILE_NAME)
}

fn lock(file: &File, operation: libc::c_int) -> io::Result<()> {
    let result = unsafe { libc::flock(file.as_raw_fd(), operation) };
    if result == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

/// Acquire the shared runtime lease at an explicitly selected path.
///
/// Application hosts should normally use [`acquire_app_runtime_install_lease`].
/// The explicit form exists so hermetic host tests can keep lock artifacts out
/// of the operator's real Codescribe directory.
#[doc(hidden)]
pub fn acquire_app_runtime_install_lease_at(path: &Path) -> Result<AppRuntimeInstallLease> {
    let file = open_shared_lease(path, "install interlock")?;
    lock(&file, libc::LOCK_SH | libc::LOCK_NB).with_context(|| {
        format!(
            "Codescribe installation owns {}; application runtime start refused",
            path.display()
        )
    })?;
    Ok(AppRuntimeInstallLease { _file: file })
}

/// Acquire the shared lease before any application runtime worker can start.
pub fn acquire_app_runtime_install_lease() -> Result<AppRuntimeInstallLease> {
    acquire_app_runtime_install_lease_at(&install_interlock_path())
}

/// Acquire the shared agent-turn lease at an explicit path (hermetic tests).
#[doc(hidden)]
pub fn acquire_agent_turn_lease_at(path: &Path) -> Result<AgentTurnLease> {
    let file = open_shared_lease(path, "agent-turn lease")?;
    lock(&file, libc::LOCK_SH | libc::LOCK_NB).with_context(|| {
        format!(
            "Codescribe installation owns {}; agent turn lease refused",
            path.display()
        )
    })?;
    Ok(AgentTurnLease { _file: file })
}

/// Acquire the shared lease for one agent turn. Hold the value for the whole
/// turn; the caller decides whether a failure blocks the turn (it should not:
/// the lease protects the installer, not the conversation).
pub fn acquire_agent_turn_lease() -> Result<AgentTurnLease> {
    acquire_agent_turn_lease_at(&agent_turn_lease_path())
}

fn open_shared_lease(path: &Path, what: &str) -> Result<File> {
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("create {what} dir {}", parent.display()))?;
    }
    OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .open(path)
        .with_context(|| format!("open {what} {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn app_shared_lease_blocks_install_exclusive_until_drop() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join(INSTALL_INTERLOCK_FILE_NAME);
        let lease = acquire_app_runtime_install_lease_at(&path).unwrap();
        let installer = OpenOptions::new()
            .read(true)
            .write(true)
            .open(&path)
            .unwrap();

        let blocked = lock(&installer, libc::LOCK_EX | libc::LOCK_NB).unwrap_err();
        assert!(matches!(
            blocked.raw_os_error(),
            Some(code) if code == libc::EWOULDBLOCK || code == libc::EAGAIN
        ));

        drop(lease);
        lock(&installer, libc::LOCK_EX | libc::LOCK_NB).unwrap();
        lock(&installer, libc::LOCK_UN).unwrap();
    }

    #[test]
    fn agent_turn_lease_blocks_install_exclusive_until_drop() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join(AGENT_TURN_LEASE_FILE_NAME);
        let lease = acquire_agent_turn_lease_at(&path).unwrap();
        let installer = OpenOptions::new()
            .read(true)
            .write(true)
            .open(&path)
            .unwrap();

        let blocked = lock(&installer, libc::LOCK_EX | libc::LOCK_NB).unwrap_err();
        assert!(matches!(
            blocked.raw_os_error(),
            Some(code) if code == libc::EWOULDBLOCK || code == libc::EAGAIN
        ));

        drop(lease);
        lock(&installer, libc::LOCK_EX | libc::LOCK_NB).unwrap();
        lock(&installer, libc::LOCK_UN).unwrap();
    }

    #[test]
    fn agent_turn_lease_lives_next_to_the_runtime_interlock() {
        let turn = agent_turn_lease_path();
        assert_eq!(turn.parent(), install_interlock_path().parent());
        assert_eq!(
            turn.file_name().and_then(|name| name.to_str()),
            Some(AGENT_TURN_LEASE_FILE_NAME)
        );
    }
}
