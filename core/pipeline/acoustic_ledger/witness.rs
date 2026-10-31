//! Ledger hold witness: out-of-band visibility for an unavailable ledger lock.
//!
//! The one-throne ledger is shared by the live session, the streaming recorder
//! and the presentation emitter. If any holder wedges while others wait, the
//! take ends silently: no terminal event fires and nothing in the logs names
//! the lock. This witness probes each watched ledger from its own thread with
//! `try_lock` — it never queues on the mutex — and reports sustained
//! unavailability as a structured receipt.
//!
//! Honesty bound: a failed probe proves the lock was unavailable at that
//! instant. A streak of failed probes proves sustained unavailability across
//! the streak, not that one holder held the lock continuously. The receipt
//! wording keeps that distinction; stack-level attribution belongs to an
//! external sampler.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, PoisonError, TryLockError, Weak};
use std::time::{Duration, Instant};

use tracing::warn;

use super::AcousticLedger;

/// How often the witness probes each watched ledger.
const PROBE_TICK: Duration = Duration::from_secs(5);

/// A ledger unavailable across consecutive probes for at least this long
/// produces a receipt. Must stay above legitimate hold bursts (slice scans and
/// seal passes hold the ledger for milliseconds, not tens of seconds).
const HOLD_WARN_AFTER: Duration = Duration::from_secs(30);

/// Cadence of repeated receipts while the same streak continues.
const HOLD_REWARN_EVERY: Duration = Duration::from_secs(120);

/// One watched ledger. The weak handle keeps the witness from extending the
/// take's lifetime; a dead handle retires the entry.
struct Watched {
    ledger: Weak<Mutex<AcousticLedger>>,
    /// Session identity for the receipt. An identifier, never content.
    label: String,
    blocked_since: Option<Instant>,
    last_warned: Option<Instant>,
}

static REGISTRY: OnceLock<Mutex<Vec<Watched>>> = OnceLock::new();
static WITNESS_STARTED: OnceLock<()> = OnceLock::new();

/// Count of receipts emitted over process life; a cheap signal for tests and
/// diagnostics that the witness actually fired.
static HOLD_RECEIPTS: AtomicU64 = AtomicU64::new(0);

/// Register a live take's ledger with the witness. Idempotent registration is
/// not required: each take registers once at session start and the entry
/// retires itself when the take's `Arc` drops.
pub fn watch(ledger: &Arc<Mutex<AcousticLedger>>, label: &str) {
    let registry = REGISTRY.get_or_init(|| Mutex::new(Vec::new()));
    {
        let mut entries = registry.lock().unwrap_or_else(PoisonError::into_inner);
        entries.retain(|watched| watched.ledger.strong_count() > 0);
        entries.push(Watched {
            ledger: Arc::downgrade(ledger),
            label: label.to_string(),
            blocked_since: None,
            last_warned: None,
        });
    }
    WITNESS_STARTED.get_or_init(|| {
        let spawned = std::thread::Builder::new()
            .name("ledger-hold-witness".into())
            .spawn(witness_loop);
        if let Err(e) = spawned {
            warn!(
                event = "acoustic_ledger_witness",
                "Failed to spawn ledger hold witness: {e}"
            );
        }
    });
}

/// Receipts emitted so far (process lifetime).
pub fn hold_receipt_count() -> u64 {
    HOLD_RECEIPTS.load(Ordering::Acquire)
}

fn witness_loop() {
    loop {
        std::thread::sleep(PROBE_TICK);
        let Some(registry) = REGISTRY.get() else {
            continue;
        };
        let mut entries = registry.lock().unwrap_or_else(PoisonError::into_inner);
        entries.retain_mut(|watched| probe(watched, HOLD_WARN_AFTER, HOLD_REWARN_EVERY));
    }
}

/// Probe one ledger; `false` retires the entry (take ended or lock poisoned).
/// Thresholds are parameters so tests exercise the decision without waiting
/// out production windows.
fn probe(watched: &mut Watched, warn_after: Duration, rewarn_every: Duration) -> bool {
    let Some(ledger) = watched.ledger.upgrade() else {
        return false;
    };
    match ledger.try_lock() {
        Ok(guard) => {
            drop(guard);
            watched.blocked_since = None;
            watched.last_warned = None;
            true
        }
        Err(TryLockError::WouldBlock) => {
            let since = *watched.blocked_since.get_or_insert_with(Instant::now);
            let blocked = since.elapsed();
            if blocked >= warn_after {
                let due = watched
                    .last_warned
                    .is_none_or(|warned_at| warned_at.elapsed() >= rewarn_every);
                if due {
                    watched.last_warned = Some(Instant::now());
                    HOLD_RECEIPTS.fetch_add(1, Ordering::AcqRel);
                    warn!(
                        event = "acoustic_ledger_hold_overdue",
                        take = %watched.label,
                        blocked_ms = blocked.as_millis() as u64,
                        "Acoustic ledger lock stayed unavailable across consecutive \
                         witness probes; a holder may be wedged"
                    );
                }
            }
            true
        }
        Err(TryLockError::Poisoned(_)) => {
            warn!(
                event = "acoustic_ledger_hold_overdue",
                take = %watched.label,
                "Acoustic ledger lock is poisoned; retiring the witness entry"
            );
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn watched_for(ledger: &Arc<Mutex<AcousticLedger>>) -> Watched {
        Watched {
            ledger: Arc::downgrade(ledger),
            label: "test-take".into(),
            blocked_since: None,
            last_warned: None,
        }
    }

    #[test]
    fn free_ledger_probe_resets_streak() {
        let ledger = Arc::new(Mutex::new(AcousticLedger::new()));
        let mut watched = watched_for(&ledger);
        watched.blocked_since = Some(Instant::now());
        watched.last_warned = Some(Instant::now());
        assert!(probe(&mut watched, Duration::ZERO, Duration::ZERO));
        assert!(watched.blocked_since.is_none());
        assert!(watched.last_warned.is_none());
    }

    #[test]
    fn held_ledger_past_threshold_emits_and_respects_rewarn_window() {
        let ledger = Arc::new(Mutex::new(AcousticLedger::new()));
        let mut watched = watched_for(&ledger);
        let guard = ledger.lock().unwrap();
        let before = hold_receipt_count();
        // First blocked probe starts the streak without warning (streak age 0
        // stays under any positive threshold).
        assert!(probe(
            &mut watched,
            Duration::from_millis(50),
            Duration::from_secs(600)
        ));
        assert_eq!(hold_receipt_count(), before);
        std::thread::sleep(Duration::from_millis(60));
        // Streak crossed the threshold: exactly one receipt...
        assert!(probe(
            &mut watched,
            Duration::from_millis(50),
            Duration::from_secs(600)
        ));
        assert_eq!(hold_receipt_count(), before + 1);
        // ...and the re-warn window suppresses an immediate second one.
        assert!(probe(
            &mut watched,
            Duration::from_millis(50),
            Duration::from_secs(600)
        ));
        assert_eq!(hold_receipt_count(), before + 1);
        drop(guard);
        // Lock is free again: the streak resets instead of warning forever.
        assert!(probe(
            &mut watched,
            Duration::from_millis(50),
            Duration::from_secs(600)
        ));
        assert!(watched.blocked_since.is_none());
    }

    #[test]
    fn dropped_take_retires_the_entry() {
        let ledger = Arc::new(Mutex::new(AcousticLedger::new()));
        let mut watched = watched_for(&ledger);
        drop(ledger);
        assert!(!probe(&mut watched, Duration::ZERO, Duration::ZERO));
    }
}
