//! Env-tunable runtime knobs shared across the streaming modules.

/// Minimum audio to accumulate before running extract_speech + Whisper inference.
/// Interim chunks below this threshold are buffered; only speech-extracted audio
/// is submitted to Whisper, eliminating hallucinations on silence.
const DEFAULT_INTERIM_VAD_ACCUMULATE_SEC: f32 = 3.0;

// ── Env helpers ──────────────────────────────────────────────────────────────

/// Read `key` as a boolean flag, defaulting to `false` when unset or unparseable.
///
/// Accepts `1` or a case-insensitive `true`; anything else reads as off.
pub(crate) fn env_bool(key: &str) -> bool {
    std::env::var(key)
        .ok()
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false)
}

/// Like [`env_bool`], but for knobs whose off-state is not the safe default.
#[cfg(any(test, feature = "offline_eval"))]
pub(crate) fn env_bool_default(key: &str, default: bool) -> bool {
    std::env::var(key)
        .ok()
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(default)
}

/// Read `key` as an `f32`, falling back to `default` when unset or unparseable.
pub(crate) fn env_f32(key: &str, default: f32) -> f32 {
    std::env::var(key)
        .ok()
        .and_then(|v| v.parse::<f32>().ok())
        .unwrap_or(default)
}

/// Read `key` as a `u64`, falling back to `default` when unset or unparseable.
pub(crate) fn env_u64(key: &str, default: u64) -> u64 {
    std::env::var(key)
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(default)
}

/// Read `key` as a `usize`, falling back to `default` when unset or unparseable.
pub(crate) fn env_usize(key: &str, default: usize) -> usize {
    std::env::var(key)
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(default)
}

/// How many Whisper inferences may be in flight at once (`CODESCRIBE_MAX_INFERENCE_CONCURRENCY`).
///
/// Clamped to `1..=4`. See the inline note on why the default is 1.
pub(crate) fn inference_max_concurrency() -> usize {
    // Whisper singleton uses a single engine lock; defaulting to 1 avoids queue churn
    // that looks like "parallelism" but mostly adds latency/jitter in preview.
    const DEFAULT_MAX_INFERENCE_CONCURRENCY: usize = 1;
    const HARD_MAX_INFERENCE_CONCURRENCY: usize = 4;
    env_usize(
        "CODESCRIBE_MAX_INFERENCE_CONCURRENCY",
        DEFAULT_MAX_INFERENCE_CONCURRENCY,
    )
    .clamp(1, HARD_MAX_INFERENCE_CONCURRENCY)
}

/// `DEFAULT_INTERIM_VAD_ACCUMULATE_SEC` expressed in samples at `sample_rate`.
pub(crate) fn interim_vad_accumulate_samples(sample_rate: u32) -> usize {
    (DEFAULT_INTERIM_VAD_ACCUMULATE_SEC * sample_rate as f32) as usize
}

/// Fraction of a buffered segment treated as already-typed prefix during correction
/// (`CODESCRIBE_BUFFERED_CORRECTION_PREFIX`, clamped to `0.4..=0.9`).
pub(crate) fn buffered_correction_prefix_ratio() -> f64 {
    env_f32("CODESCRIBE_BUFFERED_CORRECTION_PREFIX", 0.50).clamp(0.4, 0.9) as f64
}
