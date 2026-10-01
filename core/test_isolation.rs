//! Test-process fence for Codescribe-owned filesystem paths.
//!
//! Writes under the account's real home panic before mutation, and reads of the
//! account's real Codescribe stores (`~/.codescribe`, Application Support
//! `Codescribe`) panic before their content is observed. Resolvers whose
//! default root would be one of those stores resolve to a per-process temporary
//! root in test builds instead, so a test process can neither read nor write
//! the host's real Codescribe configuration.

#[cfg(any(test, feature = "test-isolation"))]
use std::os::unix::ffi::OsStrExt;
use std::path::Path;
#[cfg(any(test, feature = "test-isolation"))]
use std::path::{Component, PathBuf};
#[cfg(any(test, feature = "test-isolation"))]
use std::sync::OnceLock;

#[cfg(any(test, feature = "test-isolation"))]
mod environment;
#[cfg(any(test, feature = "test-isolation"))]
pub use environment::{EnvGuard, ScopedEnv};

/// Per-process temporary root replacing `$HOME/.codescribe` as the default
/// config root in test builds. Created once; every test in the process shares
/// it, and no test ever observes the account's real configuration through a
/// resolver default.
#[cfg(any(test, feature = "test-isolation"))]
pub fn test_process_config_root() -> PathBuf {
    static ROOT: OnceLock<PathBuf> = OnceLock::new();
    ROOT.get_or_init(|| {
        let temp = std::env::temp_dir();
        sweep_dead_process_roots(&temp);
        let root = temp.join(format!("{TEST_CONFIG_ROOT_PREFIX}{}", std::process::id()));
        std::fs::create_dir_all(&root).expect("create per-process test config root");
        root.canonicalize().unwrap_or(root)
    })
    .clone()
}

#[cfg(any(test, feature = "test-isolation"))]
const TEST_CONFIG_ROOT_PREFIX: &str = "codescribe-test-config-";

/// Every test binary creates one root and nothing removes it at exit (a static
/// is never dropped), so `cargo test --workspace` leaves one directory per
/// test process — 344 after one evening of gate loops. The next process
/// removes the roots of processes that no longer exist; a live process,
/// including one owned by another user (`EPERM`), keeps its root.
#[cfg(any(test, feature = "test-isolation"))]
fn sweep_dead_process_roots(temp: &Path) {
    let Ok(entries) = std::fs::read_dir(temp) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(pid) = name
            .to_str()
            .and_then(|name| name.strip_prefix(TEST_CONFIG_ROOT_PREFIX))
            .and_then(|pid| pid.parse::<libc::pid_t>().ok())
        else {
            continue;
        };
        if pid <= 0 || pid as u32 == std::process::id() {
            continue;
        }
        // SAFETY: signal 0 performs only the existence and permission check.
        let dead = unsafe { libc::kill(pid, 0) } == -1
            && std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH);
        if dead {
            let _ = std::fs::remove_dir_all(entry.path());
        }
    }
}

/// Per-process temporary root replacing `~/Library/Application Support/Codescribe`
/// as the default data root in test builds. Kept distinct from
/// [`test_process_config_root`], mirroring the production layout.
#[cfg(any(test, feature = "test-isolation"))]
pub fn test_process_data_root() -> PathBuf {
    static ROOT: OnceLock<PathBuf> = OnceLock::new();
    ROOT.get_or_init(|| {
        let root = test_process_config_root().join("application-support");
        std::fs::create_dir_all(&root).expect("create per-process test data root");
        root
    })
    .clone()
}

/// Panic before a test process reads the account's real Codescribe stores
/// (`~/.codescribe` or `~/Library/Application Support/Codescribe`). Reads of
/// model caches and everywhere else remain legal. `HOME` is intentionally
/// ignored when locating the real account home.
#[track_caller]
#[cfg(any(test, feature = "test-isolation"))]
pub fn assert_test_read_allowed(path: &Path) {
    let Some(home) = account_home() else {
        panic!(
            "test isolation cannot determine the account home before reading {}",
            path.display()
        );
    };
    let home = home.canonicalize().unwrap_or(home);
    let resolved = resolve_physical(path);
    for store in [
        home.join(".codescribe"),
        home.join("Library/Application Support/Codescribe"),
    ] {
        if resolved.starts_with(&store) {
            let test = std::thread::current()
                .name()
                .unwrap_or("<unnamed>")
                .to_owned();
            panic!(
                "test process refused read under the account's real Codescribe store: {} (test: {test})",
                path.display()
            );
        }
    }
}

/// Shipped builds have no test fence or refusal text.
#[inline]
#[cfg(not(any(test, feature = "test-isolation")))]
pub fn assert_test_read_allowed(_path: &Path) {}

/// Panic before a test process mutates the account's real home directory.
/// `HOME` is intentionally ignored when locating the real account home.
#[track_caller]
#[cfg(any(test, feature = "test-isolation"))]
pub fn assert_test_write_allowed(path: &Path) {
    let Some(home) = account_home() else {
        panic!(
            "test isolation cannot determine the account home before writing {}",
            path.display()
        );
    };
    // The candidate below compares in physical form, so the fence's own
    // anchor must be physical too: a symlinked account home would otherwise
    // never prefix-match the canonicalized candidate and the fence would
    // wave the write through.
    let home = home.canonicalize().unwrap_or(home);
    if resolve_physical(path).starts_with(&home) {
        panic!(
            "test process refused write under real home: {}",
            path.display()
        );
    }
}

/// Shipped builds have no test fence or refusal text.
#[inline]
#[cfg(not(any(test, feature = "test-isolation")))]
pub fn assert_test_write_allowed(_path: &Path) {}

/// Resolve `path` to a physical, normalized absolute form: canonicalize the
/// closest existing ancestor so a symlink cannot hide the real destination,
/// then normalize remaining `.`/`..` components.
#[cfg(any(test, feature = "test-isolation"))]
fn resolve_physical(path: &Path) -> PathBuf {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .expect("test isolation current directory")
            .join(path)
    };
    let mut ancestor = absolute.as_path();
    let mut suffix = Vec::new();
    while !ancestor.exists() {
        let Some(name) = ancestor.file_name() else {
            break;
        };
        suffix.push(name.to_os_string());
        ancestor = ancestor.parent().expect("path has parent");
    }
    let mut resolved = ancestor
        .canonicalize()
        .unwrap_or_else(|_| ancestor.to_path_buf());
    for name in suffix.iter().rev() {
        resolved.push(name);
    }
    let mut normalized = PathBuf::new();
    for component in resolved.components() {
        match component {
            Component::ParentDir => {
                normalized.pop();
            }
            Component::CurDir => {}
            other => normalized.push(other.as_os_str()),
        }
    }
    normalized
}

#[cfg(any(test, feature = "test-isolation"))]
pub fn account_home() -> Option<PathBuf> {
    let uid = unsafe { libc::geteuid() };
    let mut pwd = std::mem::MaybeUninit::<libc::passwd>::uninit();
    let mut result = std::ptr::null_mut();
    let size = unsafe { libc::sysconf(libc::_SC_GETPW_R_SIZE_MAX) };
    let mut buf = vec![0_u8; if size > 0 { size as usize } else { 16_384 }];
    let rc = unsafe {
        libc::getpwuid_r(
            uid,
            pwd.as_mut_ptr(),
            buf.as_mut_ptr().cast(),
            buf.len(),
            &mut result,
        )
    };
    if rc != 0 || result.is_null() {
        return None;
    }
    let pwd = unsafe { pwd.assume_init() };
    let home = unsafe { std::ffi::CStr::from_ptr(pwd.pw_dir) };
    Some(PathBuf::from(std::ffi::OsStr::from_bytes(home.to_bytes())))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn real_home_mutations_panic_but_reads_and_temp_writes_do_not() {
        let home = account_home().expect("account home");
        let target = home.join(".codescribe/test-isolation-probe");
        for operation in ["write", "create_dir_all"] {
            let outcome = std::panic::catch_unwind(|| {
                assert_test_write_allowed(&target);
                match operation {
                    "write" => std::fs::write(&target, "x").unwrap(),
                    "create_dir_all" => std::fs::create_dir_all(&target).unwrap(),
                    _ => unreachable!(),
                }
            });
            assert!(
                outcome.is_err(),
                "{operation} must be refused before mutation"
            );
        }
        let tmp = tempfile::tempdir().unwrap();
        let safe = tmp.path().join("safe");
        assert_test_write_allowed(&safe);
        std::fs::write(&safe, "ok").unwrap();
        let _ = std::fs::read_dir(&home).unwrap();
    }

    #[test]
    fn real_codescribe_store_reads_panic_but_temp_reads_do_not() {
        let home = account_home().expect("account home");
        for target in [
            home.join(".codescribe/.env"),
            home.join("Library/Application Support/Codescribe/settings.json"),
        ] {
            let outcome = std::panic::catch_unwind(|| assert_test_read_allowed(&target));
            assert!(
                outcome.is_err(),
                "read of {} must be refused before content is observed",
                target.display()
            );
        }
        let tmp = tempfile::tempdir().unwrap();
        let safe = tmp.path().join("settings.json");
        std::fs::write(&safe, "{}").unwrap();
        assert_test_read_allowed(&safe);
        assert_test_read_allowed(&test_process_config_root().join(".env"));
        assert_test_read_allowed(&test_process_data_root().join("settings.json"));
    }

    #[test]
    fn sweep_removes_only_roots_of_processes_that_no_longer_exist() {
        let base = tempfile::tempdir().unwrap();
        let mut child = std::process::Command::new("/usr/bin/true").spawn().unwrap();
        let dead_pid = child.id();
        child.wait().unwrap();
        let live_pid = std::os::unix::process::parent_id();
        let root = |pid: u32| base.path().join(format!("{TEST_CONFIG_ROOT_PREFIX}{pid}"));
        let unrelated = base.path().join("codescribe-test-config-notapid");
        for dir in [
            root(dead_pid),
            root(live_pid),
            root(std::process::id()),
            unrelated.clone(),
        ] {
            std::fs::create_dir_all(dir.join("application-support")).unwrap();
        }

        sweep_dead_process_roots(base.path());

        assert!(!root(dead_pid).exists(), "a dead process's root is removed");
        assert!(root(live_pid).is_dir(), "a live process keeps its root");
        assert!(
            root(std::process::id()).is_dir(),
            "the sweeper keeps its own root"
        );
        assert!(unrelated.is_dir(), "names without a pid are not touched");
    }

    #[test]
    fn test_process_roots_exist_outside_the_account_home() {
        let home = account_home().expect("account home");
        let home = home.canonicalize().unwrap_or(home);
        for root in [test_process_config_root(), test_process_data_root()] {
            assert!(root.is_dir(), "{} must exist", root.display());
            assert!(
                !root.starts_with(&home),
                "{} must not live under the account home",
                root.display()
            );
        }
        assert_ne!(test_process_config_root(), test_process_data_root());
    }
}
