//! CodeScribe Core - Speech-to-text and text-to-speech with embedded models
//!
//! ## Quick Start
//!
//! ```ignore
//! // Transcribe with embedded Whisper model (zero config!)
//! codescribe_core::whisper::init()?;
//! let text = codescribe_core::whisper::transcribe(&samples, 16000, Some("pl"))?;
//!
//! // Synthesize speech with embedded CSM model
//! codescribe_core::tts::init()?;
//! codescribe_core::tts::play("Hello, world!")?;
//! ```
//!
//! ## Architecture
//!
//! - **whisper** - Embedded Whisper model (~900MB in binary), zero I/O
//! - **tts** - Embedded CSM-1B model (~1GB in binary), text-to-speech
//! - **vad** - Voice activity detection using WebRTC VAD
//! - **embedder** - Text embeddings using E5 model via fastembed
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
pub mod conversation;
pub mod embedder;
pub mod ipc;
pub mod llm;
pub mod pipeline;
pub mod quality;
pub mod state;
pub mod stt;
pub mod tts;
pub mod util;
pub mod vad;
pub use stt::whisper;

// ═══════════════════════════════════════════════════════════
// Public API - Whisper (STT main interface)
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
// Public API - TTS (text-to-speech interface)
// ═══════════════════════════════════════════════════════════

/// Initialize and synthesize speech with embedded CSM model
pub mod tts_api {
    pub use crate::tts::embedded::{EmbeddedTts, get_embedded_data, is_embedded_available};
    pub use crate::tts::{
        AudioPlayer, SAMPLE_RATE, get_model_path, init, is_initialized, play, synthesize,
        synthesize_to_file,
    };
}

// ═══════════════════════════════════════════════════════════
// Public API - VAD (voice activity detection)
// ═══════════════════════════════════════════════════════════

/// Voice activity detection using Silero VAD (neural network)
pub mod vad_api {
    pub use crate::vad::{
        CHUNK_SIZE, Resampler, SAMPLE_RATE, SileroVad, VadConfig, default_model_path, init,
        init_with_config, is_initialized, is_speech, reset, speech_probability,
    };
}

// ═══════════════════════════════════════════════════════════
// Public API - Embedder (text embeddings)
// ═══════════════════════════════════════════════════════════

/// Text embeddings using E5 model via fastembed
pub mod embedder_api {
    pub use crate::embedder::{
        DEFAULT_MODEL, EMBEDDING_DIM, EmbedderConfig, EmbedderEngine, embed, embed_batch, init,
        is_initialized, similarity,
    };
}

// ═══════════════════════════════════════════════════════════
// Public API - Conversation (Moshi voice AI)
// ═══════════════════════════════════════════════════════════

/// Full-duplex conversational AI using Moshi
pub mod conversation_api {
    pub use crate::conversation::{
        ConversationContext, ConversationEngine, FRAME_SAMPLES, MoshiConfig, NUM_CODEBOOKS,
        SAMPLE_RATE, TurnManager,
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
// Public re-exports for legacy paths
// ═══════════════════════════════════════════════════════════

pub use llm::{ai_formatting, client, voice_chat};
pub use pipeline::stream_postprocess;
pub use quality::{quality_loop, quality_report};
pub use util::{safe_path, status};
