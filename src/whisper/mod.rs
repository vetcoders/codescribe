//! Whisper speech-to-text module.
//!
//! This module provides local Whisper transcription capabilities using
//! Candle for inference on Metal (macOS) or CPU.
//!
//! ## Module Structure
//!
//! - `params` - Decoding parameters for transcription tuning
//! - `engine` - The main LocalWhisperEngine implementation

mod engine;
mod params;

pub use engine::LocalWhisperEngine;
pub use params::DecodingParams;
