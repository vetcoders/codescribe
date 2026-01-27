//! Voice Activity Detection (VAD) module using Silero neural network.
//!
//! Custom wrapper that shares ort runtime with fastembed (no dependency conflicts).
//! Uses worker thread to avoid blocking audio callbacks.
//!
//! ## Quick Start
//!
//! ```ignore
//! use codescribe_core::vad;
//!
//! // Initialize VAD with model path
//! vad::init(&vad::default_model_path())?;
//!
//! // Check if audio contains speech (with sample rate)
//! let is_speech = vad::is_speech(&samples, 48000);
//!
//! // Get raw probability (0.0 - 1.0)
//! let prob = vad::speech_probability(&samples, 48000);
//! ```
//!
//! ## Resampling
//!
//! Silero VAD requires 16kHz audio. The module automatically resamples
//! from common rates (44.1kHz, 48kHz) when you pass the sample_rate parameter.
//!
//! Created by M&K (c)2026 VetCoders

pub mod config;
pub mod silero_ort;

pub use config::VadConfig;
pub use silero_ort::{
    Resampler, SileroVad, VAD_SAMPLE_RATE, default_model_path, init, init_with_config,
    is_initialized, is_speech, reset, speech_probability,
};

/// Expected sample rate for VAD (Silero requires 16kHz)
pub const SAMPLE_RATE: u32 = VAD_SAMPLE_RATE;

/// Recommended chunk size in samples (512 = 32ms at 16kHz)
pub const CHUNK_SIZE: usize = 512;
