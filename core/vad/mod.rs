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
//! No global singletons. Silero VAD requires 16kHz audio —
//! `AccumulatingVad` handles resampling and chunk accumulation internally.
//!
//! Created by M&K (c)2026 VetCoders

pub mod config;
pub mod embedded;
pub mod install;
pub mod silero_ort;

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

/// Extract speech-only regions from audio using Silero VAD.
///
/// Runs AccumulatingVad over 500ms windows, keeps windows where
/// speech probability >= threshold. Returns concatenated speech
/// samples and stats.
///
/// Falls back to returning the original audio if the model is
/// unavailable (never silently drops audio).
pub fn extract_speech(samples: &[f32], sample_rate: u32) -> (Vec<f32>, VadExtractStats) {
    let window_size = (sample_rate * EXTRACT_WINDOW_MS / 1000) as usize;

    let mut vad = match AccumulatingVad::new(sample_rate) {
        Ok(v) => v,
        Err(e) => {
            warn!(
                "extract_speech: AccumulatingVad init failed at {} Hz, returning original audio: {}",
                sample_rate, e
            );
            return (
                samples.to_vec(),
                VadExtractStats {
                    speech_pct: 100.0,
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

    for window in samples.chunks(window_size) {
        if window.len() < window_size / 2 {
            // Trailing fragment — include it (don't lose tail audio)
            speech_samples.extend_from_slice(window);
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
        }
    }

    // Safety: never return empty — if VAD filtered everything, return original
    if speech_samples.is_empty() {
        return (
            samples.to_vec(),
            VadExtractStats {
                speech_pct: 100.0,
                speech_windows: total_windows,
                total_windows,
                sparkline,
            },
        );
    }

    let speech_pct = if total_windows > 0 {
        speech_windows as f32 / total_windows as f32 * 100.0
    } else {
        100.0
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
