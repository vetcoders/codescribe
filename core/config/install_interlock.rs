//! Cross-process lease preventing app runtime and bundle installation overlap.

use std::fs::{File, OpenOptions};
use std::io;
use std::os::fd::AsRawFd;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use directories::BaseDirs;

/// Shared filename used by the app runtime and `install-if-idle`.
pub const INSTALL_INTERLOCK_FILE_NAME: &str = "install-runtime.lock";

/// A process-lifetime shared lease. Closing the file releases the kernel lock.
pub struct AppRuntimeInstallLease {
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
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("create install interlock dir {}", parent.display()))?;
    }
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .open(path)
        .with_context(|| format!("open install interlock {}", path.display()))?;
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
}
