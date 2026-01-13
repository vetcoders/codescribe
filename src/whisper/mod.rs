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

mod engine;
mod params;
pub mod singleton;

// Public API exports - used by library consumers
#[allow(unused_imports)]
pub use engine::{ChunkCallback, LocalWhisperEngine};
pub use params::DecodingParams;

// Re-export singleton functions at module level for convenience
// These are part of the public API for library consumers
#[allow(unused_imports)]
pub use singleton::{
    detect_language, engine, get_model_path, init, is_initialized, transcribe, transcribe_file,
    transcribe_streaming, DEFAULT_MODEL,
};
