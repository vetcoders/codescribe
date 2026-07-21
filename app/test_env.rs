//! Process-global serialization for tests that read or mutate the data-dir /
//! workspace-root environment. `std::env` is process state: one test's guard
//! rewires `AgentAssetStore` / settings paths under a parallel sibling's feet
//! (observed 2026-07-22: image-asset tests panicking on a foreign tempdir in
//! a parallel `cargo test --workspace` run). Tests that resolve those paths
//! take this lock for their whole body; serial runs are unaffected.

use std::sync::{Mutex, MutexGuard, OnceLock};

pub(crate) fn data_dir_env_serial() -> MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}
