//! Thread-local final-pass stage timing (A2 latency truth).
//!
//! The stop-path final pass runs `transcribe_file_verdict` on one
//! `spawn_blocking` thread. The singleton records how long it waited for the
//! engine mutex and whether it paid a cold model load; the engine records the
//! pure decode span. The caller takes the whole struct on the same thread
//! right after the verdict returns, so no cross-thread synchronization is
//! needed and concurrent streaming partials on other threads cannot clobber
//! the reading.

use std::cell::Cell;

/// Stage durations for the last engine call made on the current thread.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct FinalPassTiming {
    /// Wall time spent waiting on the engine mutex before the call could run.
    pub lock_wait_ms: u64,
    /// Cold model load duration; 0 on a warm hit.
    pub model_load_ms: u64,
    /// Whether this call paid a cold model load.
    pub cold_load: bool,
    /// Pure decode span (`transcribe_long_with_language_segments`), excluding
    /// audio file load, VAD, silero tail filter, and lexicon final pass.
    pub inference_ms: u64,
}

thread_local! {
    static LAST_TIMING: Cell<FinalPassTiming> = const { Cell::new(FinalPassTiming {
        lock_wait_ms: 0,
        model_load_ms: 0,
        cold_load: false,
        inference_ms: 0,
    }) };
}

/// Record engine acquisition costs. Called by the singleton at the start of
/// every engine call, so it also resets any stale `inference_ms` left by a
/// previous task on this (pooled) thread.
pub(crate) fn record_engine_acquire(lock_wait_ms: u64, model_load_ms: u64, cold_load: bool) {
    LAST_TIMING.with(|cell| {
        cell.set(FinalPassTiming {
            lock_wait_ms,
            model_load_ms,
            cold_load,
            inference_ms: 0,
        })
    });
}

/// Record the pure decode span of the current call.
pub(crate) fn record_inference_ms(inference_ms: u64) {
    LAST_TIMING.with(|cell| {
        let mut timing = cell.get();
        timing.inference_ms = inference_ms;
        cell.set(timing);
    });
}

/// Take (and reset) the calling thread's last final-pass timing.
pub fn take_final_pass_timing() -> FinalPassTiming {
    LAST_TIMING.with(|cell| cell.replace(FinalPassTiming::default()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn acquire_resets_stale_inference_and_take_resets_all() {
        record_engine_acquire(3, 1200, true);
        record_inference_ms(4500);
        record_engine_acquire(1, 0, false);
        let timing = take_final_pass_timing();
        assert_eq!(
            timing,
            FinalPassTiming {
                lock_wait_ms: 1,
                model_load_ms: 0,
                cold_load: false,
                inference_ms: 0,
            }
        );
        assert_eq!(take_final_pass_timing(), FinalPassTiming::default());
    }

    #[test]
    fn inference_layers_on_top_of_acquire() {
        record_engine_acquire(2, 800, true);
        record_inference_ms(950);
        let timing = take_final_pass_timing();
        assert_eq!(timing.lock_wait_ms, 2);
        assert_eq!(timing.model_load_ms, 800);
        assert!(timing.cold_load);
        assert_eq!(timing.inference_ms, 950);
    }
}
