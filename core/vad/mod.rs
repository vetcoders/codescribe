//! Voice Activity Detection (VAD) module using Silero neural network.
//!
//! Custom wrapper that uses a shared ort runtime (no dependency conflicts).
//!
//! ## Quick Start
//!
//! ```ignore
//! use codescribe_core::vad;
//!
//! // Create a local VAD instance at your audio sample rate
//! let mut vad = vad::AccumulatingVad::new(44100)?;
//!
//! // Feed audio chunks — returns speech probability (0.0–1.0)
//! let prob = vad.feed(&audio_samples);
//! ```
//!
//! ## Architecture
//!
//! Each consumer owns its own `AccumulatingVad` (or raw `SileroVad`).
//! Silero VAD requires 16kHz audio —
//! `AccumulatingVad` handles resampling and chunk accumulation internally.
//!
//! `extract_speech()` uses a per-thread cache to avoid reloading the ONNX
//! model on every call. The cache is invalidated when sample rate changes.

pub mod config;
pub mod discriminator;
pub mod embedded;
pub mod install;
pub mod silero_ort;

use std::cell::RefCell;

use tracing::warn;

pub use config::VadConfig;
pub use discriminator::{DISCRIMINATOR_WINDOW_MS, VadTimeline, classify_windows};
pub use install::{
    SILERO_VAD_FILE, SILERO_VAD_URL, ensure_downloaded_to_user_dir, user_model_path,
    user_models_dir,
};
pub use silero_ort::{AccumulatingVad, Resampler, SileroVad, VAD_SAMPLE_RATE, default_model_path};

/// Expected sample rate for VAD (Silero requires 16kHz)
pub const SAMPLE_RATE: u32 = VAD_SAMPLE_RATE;

/// Recommended chunk size in samples (512 = 32ms at 16kHz)
pub const CHUNK_SIZE: usize = 512;

// ═══════════════════════════════════════════════════════════
// Speech extraction — Silero pre-filter for file transcription
// ═══════════════════════════════════════════════════════════

/// Stats from VAD speech extraction.
pub struct VadExtractStats {
    /// Percentage of audio that is speech (0–100).
    pub speech_pct: f32,
    /// Number of speech windows detected.
    pub speech_windows: usize,
    /// Total windows analysed.
    pub total_windows: usize,
    /// Reason preserved when extraction concludes with no usable speech.
    pub no_speech_reason: Option<String>,
    /// Sparkline visualisation (one char per 500ms window).
    pub sparkline: String,
    /// Raw per-window speech probabilities (one entry per processed
    /// 500ms window). Empty when extraction short-circuited.
    pub probabilities: Vec<f32>,
}

/// Window size for VAD analysis: 500ms of audio.
const EXTRACT_WINDOW_MS: u32 = 500;

thread_local! {
    /// Cached `AccumulatingVad` for `extract_speech()`.
    /// Stores `(sample_rate, vad)` — invalidated when sample_rate changes.
    static EXTRACT_VAD: RefCell<Option<(u32, AccumulatingVad)>> = const { RefCell::new(None) };
}

/// Take a cached VAD instance matching `sample_rate`, or create a new one.
fn take_extract_vad(sample_rate: u32) -> anyhow::Result<AccumulatingVad> {
    EXTRACT_VAD.with(|cell| {
        let mut slot = cell.borrow_mut();
        match slot.take() {
            Some((rate, vad)) if rate == sample_rate => Ok(vad),
            _ => AccumulatingVad::new(sample_rate),
        }
    })
}

/// Return a VAD instance to the thread-local cache for reuse.
fn return_extract_vad(sample_rate: u32, vad: AccumulatingVad) {
    EXTRACT_VAD.with(|cell| {
        *cell.borrow_mut() = Some((sample_rate, vad));
    });
}

/// Extract speech-only regions from audio using Silero VAD.
///
/// Runs AccumulatingVad over 500ms windows, keeps windows where
/// speech probability >= threshold. Returns concatenated speech
/// samples and stats.
///
/// The Silero ONNX model is cached per-thread and reused across calls
/// with the same `sample_rate`. A sample-rate change invalidates the cache.
///
/// Returns an empty vector when no speech is detected or VAD is unavailable.
pub fn extract_speech(samples: &[f32], sample_rate: u32) -> (Vec<f32>, VadExtractStats) {
    if samples.is_empty() {
        return (
            Vec::new(),
            VadExtractStats {
                speech_pct: 0.0,
                speech_windows: 0,
                total_windows: 0,
                no_speech_reason: Some("vad_input_empty".to_string()),
                sparkline: String::new(),
                probabilities: Vec::new(),
            },
        );
    }
    if sample_rate == 0 {
        return (
            Vec::new(),
            VadExtractStats {
                speech_pct: 0.0,
                speech_windows: 0,
                total_windows: 0,
                no_speech_reason: Some("vad_invalid_sample_rate".to_string()),
                sparkline: String::new(),
                probabilities: Vec::new(),
            },
        );
    }

    let window_size = (sample_rate * EXTRACT_WINDOW_MS / 1000) as usize;
    if window_size == 0 {
        return (
            Vec::new(),
            VadExtractStats {
                speech_pct: 0.0,
                speech_windows: 0,
                total_windows: 0,
                no_speech_reason: Some("vad_invalid_window_size".to_string()),
                sparkline: String::new(),
                probabilities: Vec::new(),
            },
        );
    }

    let mut vad = match take_extract_vad(sample_rate) {
        Ok(v) => v,
        Err(e) => {
            warn!(
                "extract_speech: AccumulatingVad init failed at {} Hz, returning no speech: {}",
                sample_rate, e
            );
            return (
                Vec::new(),
                VadExtractStats {
                    speech_pct: 0.0,
                    speech_windows: 0,
                    total_windows: 0,
                    no_speech_reason: Some("vad_unavailable".to_string()),
                    sparkline: String::new(),
                    probabilities: Vec::new(),
                },
            );
        }
    };

    // Reset once at the start of this extraction: the VAD is reused from a
    // thread-local cache and may carry Silero state/accumulator from a previous,
    // unrelated utterance. Within this call we keep the state CONTINUOUS across
    // windows (Silero v6 is stateful) instead of cold-starting each 500ms window,
    // which previously truncated speech onsets.
    vad.reset();

    let threshold = vad.threshold();
    let mut speech_samples = Vec::with_capacity(samples.len() / 2);
    let mut speech_windows = 0usize;
    let mut total_windows = 0usize;
    let mut sparkline = String::new();
    let mut probabilities = Vec::new();
    let mut last_window_was_speech = false;

    for window in samples.chunks(window_size) {
        if window.len() < window_size / 2 {
            // Keep very short trailing tails only when they clearly continue speech.
            if should_include_trailing_fragment(
                window.len(),
                window_size,
                speech_windows > 0,
                last_window_was_speech,
            ) {
                speech_samples.extend_from_slice(window);
            }
            break;
        }

        // No per-window reset: Silero state is carried across windows so speech
        // onsets spanning a window boundary are not lost (see reset() above).
        //
        // Use the per-window MAX probability (feed_max) rather than the last
        // 32ms chunk: a ~500ms window that contains a fully-spoken word but ends
        // in the brief pause after it must NOT be dropped on the strength of its
        // trailing chunk alone.
        let prob = vad.feed_max(window);
        total_windows += 1;
        probabilities.push(prob);

        sparkline.push(if prob >= 0.9 {
            '\u{2588}' // █
        } else if prob >= threshold {
            '\u{2593}' // ▓
        } else if prob >= 0.1 {
            '\u{2591}' // ░
        } else {
            ' '
        });

        if prob >= threshold {
            speech_samples.extend_from_slice(window);
            speech_windows += 1;
            last_window_was_speech = true;
        } else {
            last_window_was_speech = false;
        }
    }

    // Return VAD to thread-local cache for reuse
    return_extract_vad(sample_rate, vad);

    let speech_pct = if total_windows > 0 {
        speech_windows as f32 / total_windows as f32 * 100.0
    } else {
        0.0
    };
    let no_speech_reason = if !speech_samples.is_empty() {
        None
    } else if total_windows == 0 {
        Some("vad_audio_too_short".to_string())
    } else {
        Some("vad_no_speech_detected".to_string())
    };

    (
        speech_samples,
        VadExtractStats {
            speech_pct,
            speech_windows,
            total_windows,
            no_speech_reason,
            sparkline,
            probabilities,
        },
    )
}

fn should_include_trailing_fragment(
    fragment_len: usize,
    window_size: usize,
    saw_any_speech: bool,
    last_window_was_speech: bool,
) -> bool {
    if fragment_len == 0 || window_size == 0 {
        return false;
    }
    if fragment_len >= window_size / 2 {
        return true;
    }
    saw_any_speech && last_window_was_speech
}

/// Inclusive `[first, last]` window indices spanning detected speech.
///
/// The caller slices a *contiguous* range, so any interior window — including
/// non-speech pauses between two spoken words — is implicitly kept. Returns
/// `None` when no window is speech.
fn speech_slab_bounds(window_is_speech: &[bool]) -> Option<(usize, usize)> {
    let first = window_is_speech.iter().position(|&s| s)?;
    let last = window_is_speech.iter().rposition(|&s| s)?;
    Some((first, last))
}

/// Commit-lane prefilter: trim ONLY leading and trailing silence, never excise
/// interior windows.
///
/// The authoritative final transcript must never lose mid-utterance speech, so
/// unlike [`extract_speech`] (which concatenates only the speech windows and
/// would drop interior windows that dip below threshold) this returns the
/// contiguous slab spanning the first..=last speech window. A pause between two
/// spoken digits stays inside the slab and is sent to Whisper intact.
///
/// Returns an empty vector only when at least one window was measured and NONE
/// contained speech (genuine silence) — preserving the Commit lane's
/// "empty => no speech" contract. When the audio is too short to measure even a
/// single window, or the VAD is unavailable, the full audio is returned rather
/// than dropped: the final lane fails *open*, never silently swallowing words.
pub fn extract_speech_trim_edges(samples: &[f32], sample_rate: u32) -> Vec<f32> {
    if samples.is_empty() || sample_rate == 0 {
        return Vec::new();
    }
    let window_size = (sample_rate * EXTRACT_WINDOW_MS / 1000) as usize;
    if window_size == 0 {
        return samples.to_vec();
    }

    let mut vad = match take_extract_vad(sample_rate) {
        Ok(v) => v,
        Err(e) => {
            warn!(
                "extract_speech_trim_edges: VAD init failed at {} Hz, returning full audio: {}",
                sample_rate, e
            );
            // Fail open: never drop committed audio just because VAD is down.
            return samples.to_vec();
        }
    };
    vad.reset();
    let threshold = vad.threshold();

    let mut window_is_speech: Vec<bool> = Vec::new();
    for window in samples.chunks(window_size) {
        if window.len() < window_size / 2 {
            break;
        }
        // Per-window MAX (see extract_speech): a window with a spoken word that
        // ends in a pause must register as speech so it is not trimmed as an edge.
        let prob = vad.feed_max(window);
        window_is_speech.push(prob >= threshold);
    }

    return_extract_vad(sample_rate, vad);

    match speech_slab_bounds(&window_is_speech) {
        Some((first, last)) => {
            let start = first * window_size;
            // If speech ran to the final measured window, extend to the end so
            // the trailing partial fragment (which likely continues speech) is
            // kept; otherwise stop at the end of the last speech window.
            let end = if last + 1 >= window_is_speech.len() {
                samples.len()
            } else {
                ((last + 1) * window_size).min(samples.len())
            };
            samples[start..end].to_vec()
        }
        None => {
            if window_is_speech.is_empty() {
                // Too short to measure a window — don't drop from the commit lane.
                samples.to_vec()
            } else {
                // At least one full window measured, all silence: genuine no-speech.
                Vec::new()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trailing_fragment_requires_speech_context() {
        assert!(!should_include_trailing_fragment(1000, 8000, false, false));
        assert!(!should_include_trailing_fragment(1000, 8000, true, false));
        assert!(should_include_trailing_fragment(1000, 8000, true, true));
    }

    #[test]
    fn very_small_or_empty_trailing_fragment_is_not_kept() {
        assert!(!should_include_trailing_fragment(0, 8000, true, true));
        assert!(!should_include_trailing_fragment(10, 0, true, true));
    }

    #[test]
    fn empty_input_returns_no_speech_output() {
        let (samples, stats) = extract_speech(&[], SAMPLE_RATE);
        assert!(samples.is_empty());
        assert_eq!(stats.speech_pct, 0.0);
        assert_eq!(stats.speech_windows, 0);
        assert_eq!(stats.total_windows, 0);
        assert_eq!(stats.no_speech_reason.as_deref(), Some("vad_input_empty"));
        assert!(stats.probabilities.is_empty());
    }

    #[test]
    fn invalid_sample_rate_reports_specific_no_speech_reason() {
        let (samples, stats) = extract_speech(&[0.0; 1024], 0);
        assert!(samples.is_empty());
        assert_eq!(
            stats.no_speech_reason.as_deref(),
            Some("vad_invalid_sample_rate")
        );
        assert!(stats.probabilities.is_empty());
    }

    #[test]
    fn multi_window_extraction_runs_with_continuous_state() {
        // 1.5s at 16kHz => 3 windows of EXTRACT_WINDOW_MS (500ms). With the
        // per-window reset removed, Silero state must flow across all windows
        // without panic and every window must still be measured.
        let window_size = (SAMPLE_RATE * EXTRACT_WINDOW_MS / 1000) as usize;
        let samples = vec![0.0f32; window_size * 3];
        let (_speech, stats) = extract_speech(&samples, SAMPLE_RATE);
        assert_eq!(stats.total_windows, 3, "all full windows measured");
        assert_eq!(stats.probabilities.len(), 3);
        // Silence input must not be misclassified as speech.
        assert_eq!(stats.speech_windows, 0);
    }

    #[test]
    fn slab_bounds_keeps_interior_pause() {
        // speech, pause, speech => the interior pause window (index 1) must stay
        // inside the slab; the commit lane must not split the utterance there.
        assert_eq!(speech_slab_bounds(&[true, false, true]), Some((0, 2)));
        // Leading + trailing silence trimmed, two interior pauses kept.
        assert_eq!(
            speech_slab_bounds(&[false, true, false, true, false]),
            Some((1, 3))
        );
    }

    #[test]
    fn slab_bounds_none_on_pure_silence() {
        assert_eq!(speech_slab_bounds(&[false, false, false]), None);
        assert_eq!(speech_slab_bounds(&[]), None);
    }

    #[test]
    fn slab_bounds_single_and_edge_windows() {
        assert_eq!(speech_slab_bounds(&[true]), Some((0, 0)));
        // Leading silence trimmed down to the single trailing speech window.
        assert_eq!(speech_slab_bounds(&[false, true]), Some((1, 1)));
    }

    #[test]
    fn trim_edges_returns_full_audio_on_short_input() {
        // Shorter than one window: the commit lane must NOT drop it.
        let window_size = (SAMPLE_RATE * EXTRACT_WINDOW_MS / 1000) as usize;
        let samples = vec![0.1f32; window_size / 4];
        let out = extract_speech_trim_edges(&samples, SAMPLE_RATE);
        assert_eq!(out.len(), samples.len());
    }

    #[test]
    fn trim_edges_pure_silence_returns_empty() {
        // Several full windows of silence => genuine no-speech => empty (commit
        // lane "empty => no speech" contract).
        let window_size = (SAMPLE_RATE * EXTRACT_WINDOW_MS / 1000) as usize;
        let samples = vec![0.0f32; window_size * 3];
        let out = extract_speech_trim_edges(&samples, SAMPLE_RATE);
        assert!(
            out.is_empty(),
            "pure silence trims to empty for commit lane"
        );
    }

    #[test]
    fn short_audio_reports_vad_audio_too_short() {
        let samples = vec![0.0; (SAMPLE_RATE as usize / 10).max(1)];
        let (speech, stats) = extract_speech(&samples, SAMPLE_RATE);
        assert!(speech.is_empty());
        assert_eq!(stats.total_windows, 0);
        assert_eq!(
            stats.no_speech_reason.as_deref(),
            Some("vad_audio_too_short")
        );
        assert!(stats.probabilities.is_empty());
    }
}
