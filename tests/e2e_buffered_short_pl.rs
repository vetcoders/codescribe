//! Regression coverage: short Polish utterances must not be dropped in utterance-mode postprocessing.
//!
//! In buffered mode, VAD yields full utterances. Those should be postprocessed with
//! `StreamPostProcessor::process_utterance` (lexicon + cleanup, no semantic gate),
//! so short acknowledgements like "tak"/"nie" never disappear.

use codescribe_core::pipeline::stream_postprocess::StreamPostProcessor;
use serial_test::serial;

struct EnvGuard {
    key: &'static str,
    prev: Option<String>,
}

impl EnvGuard {
    fn set(key: &'static str, value: &str) -> Self {
        let prev = std::env::var(key).ok();
        // SAFETY: tests run single-threaded with controlled env usage.
        unsafe { std::env::set_var(key, value) };
        Self { key, prev }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        if let Some(prev) = &self.prev {
            // SAFETY: tests run single-threaded with controlled env usage.
            unsafe { std::env::set_var(self.key, prev) };
        } else {
            // SAFETY: tests run single-threaded with controlled env usage.
            unsafe { std::env::remove_var(self.key) };
        }
    }
}

#[test]
#[serial]
fn test_postprocess_utterance_keeps_short_polish() {
    // Even if embeddings are enabled (like in production), utterance-mode processing must not
    // consult the semantic gate (and must not require the embedder model).
    let _g = EnvGuard::set("CODESCRIBE_STREAM_FORCE_EMBEDDINGS", "1");

    let mut processor = StreamPostProcessor::new();

    let cases = [
        "tak", "nie", "co?", "co", "dobra", "dobrze", "ok", "okej", "no", "mhm", "aha", "jasne",
        "pewnie", "super", "hej", "halo", "cześć", "siema", "dzięki", "proszę",
    ];

    for input in cases {
        let out = processor
            .process_utterance(input)
            .unwrap_or_else(|| panic!("process_utterance() dropped short utterance: {input:?}"));
        assert!(
            !out.trim().is_empty(),
            "process_utterance() produced empty output for: {input:?}"
        );
    }
}

#[test]
#[serial]
fn test_postprocess_utterance_allows_repetition() {
    let _g = EnvGuard::set("CODESCRIBE_STREAM_FORCE_EMBEDDINGS", "1");

    let mut processor = StreamPostProcessor::new();

    for i in 0..5 {
        let out = processor
            .process_utterance("tak")
            .unwrap_or_else(|| panic!("process_utterance() dropped repetition #{i}"));
        assert_eq!(out, "tak");
    }
}
