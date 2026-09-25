//! Test-process fence for filesystem mutations at Codescribe-owned paths.
//! Path resolution and reads remain legal; call this immediately before a write.

use std::os::unix::ffi::OsStrExt;
use std::path::{Component, Path, PathBuf};

/// Panic before a test process mutates the account's real home directory.
/// `HOME` is intentionally ignored when locating the real account home.
#[track_caller]
pub fn assert_test_write_allowed(path: &Path) {
    if !cfg!(test) && std::env::var("CODESCRIBE_TEST_ISOLATION").as_deref() != Ok("1") {
        return;
    }
    let Some(home) = account_home() else {
        panic!(
            "test isolation cannot determine the account home before writing {}",
            path.display()
        );
    };
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .expect("test isolation current directory")
            .join(path)
    };
    // Canonicalize the closest existing ancestor so a symlink outside HOME
    // cannot hide a write into it. Normalize remaining `..` components.
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
    if normalized.starts_with(&home) {
        panic!(
            "test process refused write under real home: {}",
            path.display()
        );
    }
}

fn account_home() -> Option<PathBuf> {
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
}
