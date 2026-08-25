//! Whisper speech-to-text module.
//!
//! This module provides local Whisper transcription capabilities using
//! Candle for inference on Metal (macOS) or CPU.
//!
//! ## Usage
//!
//! The recommended way is to use the global singleton:
//! ```ignore
//! use codescribe::whisper;
//!
//! // Initialize once at startup
//! whisper::init()?;
//!
//! // Transcribe anywhere
//! let text = whisper::transcribe(&samples, sample_rate, Some("pl"))?;
//! ```
//!
//! ## Module Structure
//!
//! - `singleton` - Global engine instance (recommended)
//! - `engine` - The LocalWhisperEngine implementation
//! - `params` - Decoding parameters

/// Optional build-time embedded Whisper weight bytes (fat/offline SKUs).
pub mod embedded;
/// Local Candle Whisper engine implementation.
mod engine;
/// Encoder/decoder layer graph ported for local inference.
mod model;
/// Decoding hyperparameters (temperature, beam, language, …).
mod params;
/// Process-wide engine singleton and public transcribe API.
pub mod singleton;
/// Timestamp token helpers for segmented decoder output.
pub mod timestamps;
/// Thread-local final-pass stage timing (latency truth).
pub mod timing;

// Public API exports
pub use engine::LocalWhisperEngine; // Kept for advanced usage if needed
pub use engine::append_with_overlap_dedup;
pub use params::DecodingParams; // Kept for params config if needed
pub use timing::{FinalPassTiming, take_final_pass_timing};

// Re-export singleton functions at module level (main API).
//
// File-level transcription stays structured on purpose: callers should use
// `transcribe_file_verdict` so VAD, confidence, final-pass, and engine
// provisioning provenance do not collapse back into plain text.
pub use singleton::{
    detect_language, get_model_path, init, is_initialized, transcribe, transcribe_file_verdict,
    transcribe_with_segments,
};
