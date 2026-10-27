//! Measured, cancellable access to the process-wide Apple bridge lock.
//!
//! The lock itself stays: SFSpeech / AVAudioEngine misbehave under concurrent
//! bridge processes. What changes is how a live session *waits* for it. A
//! worker that blocks inside `Mutex::lock` stops draining PCM and stops
//! observing speech edges for as long as another take holds the bridge (78 s
//! on channel 2, 2026-10-27). Here the wait is a poll: the caller keeps its
//! loop turning, can cancel at any moment, and every wait / acquire / cancel /
//! release leaves a structured receipt.
//!
//! Receipt events (`tracing` target `bridge_lock`, field `event`):
//! `bridge_lock_wait_started`, `bridge_lock_acquired { wait_ms, holder_session,
//! previous_holder }`, `bridge_lock_cancelled { wait_ms, reason }`,
//! `bridge_lock_released { held_ms }`.

use std::sync::{Mutex, MutexGuard, TryLockError};
use std::time::Instant;

use tracing::info;

/// Label of whoever currently holds the bridge, for the next waiter's receipt.
/// `None` while free; holders that bypass this module (`run_bridge_stream`,
/// request/response commands) are not labelled.
static HOLDER: Mutex<Option<String>> = Mutex::new(None);

fn set_holder(holder: Option<String>) {
    *HOLDER.lock().unwrap_or_else(|p| p.into_inner()) = holder;
}

fn current_holder() -> Option<String> {
    HOLDER.lock().unwrap_or_else(|p| p.into_inner()).clone()
}

/// Ownership of the bridge. Dropping it releases the lock and writes the
/// `bridge_lock_released` receipt.
pub struct BridgeLockGuard {
    _guard: MutexGuard<'static, ()>,
    holder: String,
    acquired_at: Instant,
    wait_ms: u64,
    labelled: bool,
}

impl BridgeLockGuard {
    /// A held guard on a private lock, for tests that build a session by hand.
    #[cfg(test)]
    pub(crate) fn for_test(lock: &'static Mutex<()>) -> Self {
        BridgeLockWait::start_on(lock, "test").block()
    }

    /// Milliseconds this holder spent waiting before it got the bridge.
    pub fn wait_ms(&self) -> u64 {
        self.wait_ms
    }
}

impl Drop for BridgeLockGuard {
    fn drop(&mut self) {
        if self.labelled {
            set_holder(None);
        }
        info!(
            target: "bridge_lock",
            event = "bridge_lock_released",
            holder_session = %self.holder,
            held_ms = self.acquired_at.elapsed().as_millis() as u64,
        );
    }
}

/// A started wait. Poll it from the worker loop; it never blocks.
pub struct BridgeLockWait {
    lock: &'static Mutex<()>,
    holder: String,
    started: Instant,
}

impl BridgeLockWait {
    /// Begin waiting on the process bridge lock on behalf of `holder`.
    pub fn start(holder: &str) -> Self {
        Self::start_on(super::bridge_global_lock(), holder)
    }

    pub(crate) fn start_on(lock: &'static Mutex<()>, holder: &str) -> Self {
        info!(
            target: "bridge_lock",
            event = "bridge_lock_wait_started",
            holder_session = holder,
            current_holder = current_holder().as_deref().unwrap_or("unlabelled"),
        );
        Self {
            lock,
            holder: holder.to_owned(),
            started: Instant::now(),
        }
    }

    /// Milliseconds since the wait started.
    pub fn waited_ms(&self) -> u64 {
        self.started.elapsed().as_millis() as u64
    }

    /// One non-blocking attempt. `Some` ends the wait with an acquire receipt.
    pub fn poll(&self) -> Option<BridgeLockGuard> {
        let previous_holder = current_holder();
        let guard = match self.lock.try_lock() {
            Ok(guard) => guard,
            Err(TryLockError::Poisoned(poisoned)) => poisoned.into_inner(),
            Err(TryLockError::WouldBlock) => return None,
        };
        let wait_ms = self.waited_ms();
        // Only the process-wide lock is labelled; a private lock (tests) must
        // not overwrite who holds the real bridge.
        let labelled = std::ptr::eq(self.lock, super::bridge_global_lock());
        if labelled {
            set_holder(Some(self.holder.clone()));
        }
        info!(
            target: "bridge_lock",
            event = "bridge_lock_acquired",
            holder_session = %self.holder,
            wait_ms,
            previous_holder = previous_holder.as_deref().unwrap_or("none"),
        );
        Some(BridgeLockGuard {
            _guard: guard,
            holder: self.holder.clone(),
            acquired_at: Instant::now(),
            wait_ms,
            labelled,
        })
    }

    /// Abandon the wait. Returns the wait duration written to the receipt.
    pub fn cancel(self, reason: &str) -> u64 {
        let wait_ms = self.waited_ms();
        info!(
            target: "bridge_lock",
            event = "bridge_lock_cancelled",
            holder_session = %self.holder,
            wait_ms,
            reason,
            current_holder = current_holder().as_deref().unwrap_or("unlabelled"),
        );
        wait_ms
    }

    /// Wait without a cancellation point. Only for callers that have no loop
    /// to keep turning (the unarmed single-stream open before the PCM pump).
    pub fn block(self) -> BridgeLockGuard {
        loop {
            if let Some(guard) = self.poll() {
                return guard;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn private_lock() -> &'static Mutex<()> {
        Box::leak(Box::new(Mutex::new(())))
    }

    #[test]
    fn poll_while_held_returns_immediately_and_cancel_reports_wait() {
        let lock = private_lock();
        let _first = lock.lock().unwrap();
        let wait = BridgeLockWait::start_on(lock, "second");

        let polled = Instant::now();
        assert!(wait.poll().is_none(), "held lock must not be granted");
        assert!(
            polled.elapsed() < Duration::from_millis(100),
            "poll must not block on the holder"
        );

        std::thread::sleep(Duration::from_millis(30));
        let wait_ms = wait.cancel("hangup");
        assert!(wait_ms >= 30, "cancel reports the real wait, got {wait_ms}");
    }

    #[test]
    fn poll_acquires_after_release_and_records_wait() {
        let lock = private_lock();
        let first = lock.lock().unwrap();
        let wait = BridgeLockWait::start_on(lock, "second");
        assert!(wait.poll().is_none());
        std::thread::sleep(Duration::from_millis(20));
        drop(first);

        let guard = wait.poll().expect("free lock is acquired");
        assert!(guard.wait_ms() >= 20, "wait_ms = {}", guard.wait_ms());
        assert!(lock.try_lock().is_err(), "guard keeps the lock held");
        drop(guard);
        assert!(lock.try_lock().is_ok(), "drop releases the lock");
    }

    #[test]
    fn block_returns_once_the_holder_lets_go() {
        let lock = private_lock();
        let (held_tx, held_rx) = std::sync::mpsc::channel();
        let holder = std::thread::spawn(move || {
            let _held = lock.lock().unwrap();
            held_tx.send(()).unwrap();
            std::thread::sleep(Duration::from_millis(40));
        });
        held_rx.recv().unwrap();
        let guard = BridgeLockWait::start_on(lock, "late").block();
        assert!(guard.wait_ms() >= 20, "wait_ms = {}", guard.wait_ms());
        holder.join().unwrap();
    }

    #[test]
    fn poisoned_lock_is_still_acquirable() {
        let lock = private_lock();
        let _ = std::thread::spawn(move || {
            let _held = lock.lock().unwrap();
            panic!("poison the bridge lock");
        })
        .join();
        assert!(
            BridgeLockWait::start_on(lock, "after-panic")
                .poll()
                .is_some()
        );
    }
}
