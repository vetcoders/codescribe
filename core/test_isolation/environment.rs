//! Scope-owned process environment for serialized tests, absent from shipped builds.

use std::ffi::{OsStr, OsString};

/// Restores one exact OS value, including absence and values outside UTF-8.
/// Callers must retain their existing process-environment lock for the whole scope.
#[must_use]
pub struct EnvGuard {
    key: &'static str,
    previous: Option<OsString>,
}

impl EnvGuard {
    /// Snapshot a key without changing it.
    pub fn capture(key: &'static str) -> Self {
        Self {
            key,
            previous: std::env::var_os(key),
        }
    }

    /// Set a key until the guard drops.
    pub fn set(key: &'static str, value: impl AsRef<OsStr>) -> Self {
        let previous = std::env::var_os(key);
        // SAFETY: callers serialize environment access until this guard drops.
        unsafe { std::env::set_var(key, value) };
        Self { key, previous }
    }

    /// Remove a key until the guard drops.
    pub fn remove(key: &'static str) -> Self {
        let previous = std::env::var_os(key);
        // SAFETY: callers serialize environment access until this guard drops.
        unsafe { std::env::remove_var(key) };
        Self { key, previous }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        // SAFETY: the caller's environment lock still spans this guard's lifetime.
        unsafe {
            match &self.previous {
                Some(value) => std::env::set_var(self.key, value),
                None => std::env::remove_var(self.key),
            }
        }
    }
}

/// A stack of mutations; repeated changes to the same key unwind in reverse order.
#[derive(Default)]
pub struct ScopedEnv {
    previous: Vec<EnvGuard>,
}

impl ScopedEnv {
    /// Start an empty mutation stack.
    pub fn new() -> Self {
        Self::default()
    }

    /// Stack a new value; later mutations unwind first.
    pub fn set(&mut self, key: &'static str, value: impl AsRef<OsStr>) {
        self.previous.push(EnvGuard::set(key, value));
    }

    /// Stack a removal; later mutations unwind first.
    pub fn remove(&mut self, key: &'static str) {
        self.previous.push(EnvGuard::remove(key));
    }
}

impl Drop for ScopedEnv {
    fn drop(&mut self) {
        for guard in self.previous.drain(..).rev() {
            drop(guard);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;

    #[test]
    #[serial]
    fn restores_absence_and_empty_values_without_conflating_them() {
        const KEY: &str = "CODESCRIBE_TEST_ENV_GUARD_VALUE";
        let _original = EnvGuard::remove(KEY);
        {
            let _empty = EnvGuard::set(KEY, "");
            assert_eq!(std::env::var_os(KEY), Some(OsString::new()));
            {
                let _removed = EnvGuard::remove(KEY);
                assert_eq!(std::env::var_os(KEY), None);
            }
            assert_eq!(std::env::var_os(KEY), Some(OsString::new()));
        }
        assert_eq!(std::env::var_os(KEY), None);
    }

    #[test]
    #[serial]
    fn stack_restores_repeated_keys_in_reverse_order_during_unwind() {
        const KEY: &str = "CODESCRIBE_TEST_ENV_GUARD_STACK";
        let _original = EnvGuard::set(KEY, "original");
        let failure = std::panic::catch_unwind(|| {
            let mut scope = ScopedEnv::new();
            scope.set(KEY, "first");
            scope.remove(KEY);
            scope.set(KEY, "last");
            assert_eq!(std::env::var(KEY).unwrap(), "last");
            panic!("exercise scope cleanup");
        });
        assert!(failure.is_err());
        assert_eq!(std::env::var(KEY).unwrap(), "original");
    }

    #[test]
    #[serial]
    fn invalid_key_panics_without_attempting_restore_during_unwind() {
        assert!(std::panic::catch_unwind(|| EnvGuard::set("", "value")).is_err());
        assert!(std::panic::catch_unwind(|| EnvGuard::remove("")).is_err());
    }

    #[cfg(unix)]
    #[test]
    #[serial]
    fn restores_non_utf8_os_value_after_set_and_remove() {
        use std::os::unix::ffi::OsStringExt;
        const KEY: &str = "CODESCRIBE_TEST_ENV_GUARD_NON_UTF8";
        let bytes = OsString::from_vec(vec![b'a', 0xff, b'z']);
        let _original = EnvGuard::set(KEY, &bytes);
        assert!(std::env::var(KEY).is_err());
        {
            let _set = EnvGuard::set(KEY, "temporary");
            assert_eq!(std::env::var(KEY).unwrap(), "temporary");
        }
        assert_eq!(std::env::var_os(KEY), Some(bytes.clone()));
        {
            let _removed = EnvGuard::remove(KEY);
            assert_eq!(std::env::var_os(KEY), None);
        }
        assert_eq!(std::env::var_os(KEY), Some(bytes));
    }
}
