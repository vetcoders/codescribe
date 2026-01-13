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

pub mod embedded;
mod engine;
mod params;
pub mod singleton;

// Public API exports
pub use engine::LocalWhisperEngine;
pub use params::DecodingParams;

// Re-export singleton functions at module level
pub use singleton::{detect_language, get_model_path, init, transcribe_file};
