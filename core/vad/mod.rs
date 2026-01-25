//! Voice Activity Detection (VAD) module using Silero neural network.
//!
//! Replaces RMS-based silence detection with neural speech detection.
//! Silero VAD is trained to distinguish speech from background noise,
//! breathing, keyboard clicks, and other non-speech sounds.
//!
//! ## Quick Start
//!
//! ```ignore
//! use codescribe_core::vad;
//!
//! // Initialize VAD (downloads ~2MB ONNX model on first use)
//! vad::init()?;
//!
//! // Check if audio contains speech (probability > threshold)
//! let is_speech = vad::is_speech(&samples);
//!
//! // Get raw probability (0.0 - 1.0)
//! let prob = vad::speech_probability(&samples);
//! ```
//!
//! ## Requirements
//!
//! - Audio must be 16kHz mono f32 samples
//! - Chunk size should be 512 samples (~32ms) for best accuracy
//!
//! Created by M&K (c)2026 VetCoders

pub mod config;
pub mod silero;

pub use config::VadConfig;
pub use silero::{SileroVad, init, is_initialized, is_speech, reset, speech_probability};

/// Expected sample rate for VAD (must match Silero's training)
pub const SAMPLE_RATE: u32 = 16000;

/// Recommended chunk size in samples (512 = 32ms at 16kHz)
pub const CHUNK_SIZE: usize = 512;
