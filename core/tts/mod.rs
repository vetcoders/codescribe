//! Text-to-speech module - Sesame CSM-1B via Candle.
//!
//! This module provides local TTS synthesis using the Conversational Speech Model
//! (CSM-1B) from Sesame, running on Metal (macOS) or CPU via Candle.
//!
//! ## Usage
//!
//! The recommended way is to use the global singleton:
//! ```ignore
//! use codescribe_core::tts;
//!
//! // Initialize once at startup
//! tts::init()?;
//!
//! // Synthesize text to audio
//! let samples = tts::synthesize("Hello, world!")?;          // Vec<f32> @ 24kHz
//! tts::synthesize_to_file("Hello", Path::new("out.wav"))?;  // Save to WAV
//! tts::play("Hello, world!")?;                              // Immediate playback
//! ```
//!
//! ## Module Structure
//!
//! - `singleton` - Global engine instance (recommended)
//! - `engine` - The TtsEngine implementation with CSM + Mimi
//! - `audio_output` - Audio playback and WAV export
//! - `embedded` - Compile-time model embedding (release builds)
//!
//! Created by M&K (c)2026 VetCoders

pub mod audio_output;
pub mod embedded;
mod engine;
pub mod singleton;

// Public API exports
pub use audio_output::AudioPlayer;
pub use engine::TtsEngine;

// Re-export singleton functions at module level (main API)
pub use singleton::{get_model_path, init, is_initialized, play, synthesize, synthesize_to_file};

/// Default sample rate for CSM output (24kHz)
pub const SAMPLE_RATE: u32 = 24000;

/// Default speaker index (0 = first voice in model)
pub const DEFAULT_SPEAKER: usize = 0;
