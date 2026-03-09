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
//!
//! Created by M&K (c)2026 VetCoders

pub mod config;
pub mod embedded;
pub mod install;
pub mod silero_ort;

use std::cell::RefCell;

use tracing::warn;

pub use config::VadConfig;
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
    /// Sparkline visualisation (one char per 500ms window).
    pub sparkline: String,
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
    if samples.is_empty() || sample_rate == 0 {
        return (
            Vec::new(),
            VadExtractStats {
                speech_pct: 0.0,
                speech_windows: 0,
                total_windows: 0,
                sparkline: String::new(),
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
                sparkline: String::new(),
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
                    sparkline: String::new(),
                },
            );
        }
    };

    let threshold = vad.threshold();
    let mut speech_samples = Vec::with_capacity(samples.len() / 2);
    let mut speech_windows = 0usize;
    let mut total_windows = 0usize;
    let mut sparkline = String::new();
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

        // Reset between windows for independent measurement
        vad.reset();
        let prob = vad.feed(window);
        total_windows += 1;

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

    (
        speech_samples,
        VadExtractStats {
            speech_pct,
            speech_windows,
            total_windows,
            sparkline,
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
    }
}
