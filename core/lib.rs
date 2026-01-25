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
// Core modules (namespaced)
// ═══════════════════════════════════════════════════════════

pub mod audio;
pub mod config;
pub mod ipc;
pub mod llm;
pub mod pipeline;
pub mod quality;
pub mod state;
pub mod stt;
pub mod util;
pub use stt::whisper;

// ═══════════════════════════════════════════════════════════
// Public API - Whisper (main interface)
// ═══════════════════════════════════════════════════════════

/// Initialize and transcribe with embedded model
pub mod stt_api {
    pub use crate::stt::whisper::embedded::{
        EmbeddedModel, get_embedded_data, is_embedded_available,
    };
    pub use crate::stt::whisper::{
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

// ═══════════════════════════════════════════════════════════
// Public re-exports
// ═══════════════════════════════════════════════════════════

pub use llm::{ai_formatting, client};
pub use pipeline::stream_postprocess;
pub use quality::{quality_loop, quality_report};
pub use util::{safe_path, status};
