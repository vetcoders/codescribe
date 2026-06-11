//! Env-tunable runtime knobs shared across the streaming modules.

/// Minimum audio to accumulate before running extract_speech + Whisper inference.
/// Interim chunks below this threshold are buffered; only speech-extracted audio
/// is submitted to Whisper, eliminating hallucinations on silence.
const DEFAULT_INTERIM_VAD_ACCUMULATE_SEC: f32 = 3.0;

// ── Env helpers ──────────────────────────────────────────────────────────────

pub(crate) fn env_bool(key: &str) -> bool {
    std::env::var(key)
        .ok()
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false)
}

#[cfg(any(test, feature = "offline_eval"))]
pub(crate) fn env_bool_default(key: &str, default: bool) -> bool {
    std::env::var(key)
        .ok()
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(default)
}

pub(crate) fn env_f32(key: &str, default: f32) -> f32 {
    std::env::var(key)
        .ok()
        .and_then(|v| v.parse::<f32>().ok())
        .unwrap_or(default)
}

pub(crate) fn env_u64(key: &str, default: u64) -> u64 {
    std::env::var(key)
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(default)
}

pub(crate) fn env_usize(key: &str, default: usize) -> usize {
    std::env::var(key)
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(default)
}

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

pub(crate) fn interim_vad_accumulate_samples(sample_rate: u32) -> usize {
    (DEFAULT_INTERIM_VAD_ACCUMULATE_SEC * sample_rate as f32) as usize
}

pub(crate) fn buffered_correction_prefix_ratio() -> f64 {
    env_f32("CODESCRIBE_BUFFERED_CORRECTION_PREFIX", 0.50).clamp(0.4, 0.9) as f64
}
