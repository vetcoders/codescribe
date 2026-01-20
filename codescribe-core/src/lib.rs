//! CodeScribe Core - Speech-to-text with embedded Whisper model
//!
//! ## Quick Start
//!
//! ```ignore
//! // Transcribe with embedded model (zero config!)
//! codescribe_core::whisper::init()?;
//! let text = codescribe_core::whisper::transcribe(&samples, 16000, Some("pl"))?;
//! ```
//!
//! ## Architecture
//!
//! - **whisper** - Embedded Whisper model (~900MB in binary), zero I/O
//! - **audio** - Recording and audio loading
//! - **config** - User configuration
//! - **ai_formatting** - Post-processing with LLMs
//!
//! Created by M&K (c)2026 VetCoders

// ═══════════════════════════════════════════════════════════
// Core modules
// ═══════════════════════════════════════════════════════════

pub mod ai_formatting;
pub mod audio;
pub mod client;
pub mod config;
pub mod ipc;
pub mod quality_loop;
pub mod quality_report;
pub mod safe_path;
pub mod state;
pub mod status;
pub mod stream_postprocess;
pub mod voice_chat;
pub mod whisper;

// ═══════════════════════════════════════════════════════════
// Public API - Whisper (main interface)
// ═══════════════════════════════════════════════════════════

/// Initialize and transcribe with embedded model
pub mod stt {
    pub use crate::whisper::embedded::{EmbeddedModel, get_embedded_data, is_embedded_available};
    pub use crate::whisper::{
        detect_language, get_model_path, init, transcribe, transcribe_file, transcribe_streaming,
    };
}

// ═══════════════════════════════════════════════════════════
// Public API - Audio
// ═══════════════════════════════════════════════════════════

pub use audio::recorder::{Recorder, RecorderConfig};

// ═══════════════════════════════════════════════════════════
// Public API - AI & Context
// ═══════════════════════════════════════════════════════════

pub use config::{get_assistive_prompt_path, get_formatting_prompt_path, reset_to_defaults};
