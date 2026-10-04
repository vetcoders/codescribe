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

/// Process-global stop-paste preemption (`STOP_PASTE_WAIT` in the controller)
/// is a single slot: any take start preempts whichever stop is waiting. Tests
/// that assert on a stop's unpremempted settlement (or on their own preempt
/// landing) cannot tolerate a foreign take start landing in their window —
/// observed 2026-10-01 under a 40-burner load loop, where
/// `next_take_preempts_pending_or_same_tick_final_once` preempted
/// `toggle_stop_publishes_the_live_session_engine`'s stop and the verdict was
/// never published. Slot-sensitive tests hold this lane for their whole body;
/// `preempt_stop_paste_for_next_take` acquires it in test builds, so a foreign
/// start waits instead of preempting. Reentrant per thread: a test that stops
/// one take and starts the next on its own thread must not deadlock on itself.
pub(crate) struct StopPastePreemptionTestLane;

impl StopPastePreemptionTestLane {
    pub(crate) fn acquire() -> impl Drop {
        struct Guard;
        impl Drop for Guard {
            fn drop(&mut self) {
                let (lock, released) = lane();
                let mut state = lock.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
                if let Some((owner, depth)) = *state {
                    if depth > 1 {
                        *state = Some((owner, depth - 1));
                    } else {
                        *state = None;
                        released.notify_all();
                    }
                }
            }
        }
        let (lock, released) = lane();
        let me = std::thread::current().id();
        let mut state = lock.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        loop {
            match *state {
                Some((owner, depth)) if owner == me => {
                    *state = Some((owner, depth + 1));
                    return Guard;
                }
                None => {
                    *state = Some((me, 1));
                    return Guard;
                }
                _ => {
                    state = released
                        .wait(state)
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                }
            }
        }
    }
}

type LaneState = (
    Mutex<Option<(std::thread::ThreadId, usize)>>,
    std::sync::Condvar,
);

fn lane() -> &'static LaneState {
    static LANE: OnceLock<LaneState> = OnceLock::new();
    LANE.get_or_init(|| (Mutex::new(None), std::sync::Condvar::new()))
}
