//! Codescribe Core - speech, transcription, and assistive runtime primitives.
//!
//! ## Quick Start
//!
//! ```ignore
//! // Resolve local Whisper, then transcribe.
//! codescribe_core::whisper::init()?;
//! let text = codescribe_core::whisper::transcribe(&samples, 16000, Some("pl"))?;
//!
//! // File-level transcription is verdict-first on purpose.
//! let verdict = codescribe_core::whisper::transcribe_file_verdict(
//!     std::path::Path::new("clip.wav"),
//!     Some("pl"),
//!     codescribe_core::contracts::FileTranscriptionOptions {
//!         final_pass: codescribe_core::contracts::FinalPassMode::EmbeddedLexiconCleanup,
//!     },
//! )?;
//!
//! // Synthesize speech (optional embedded/runtime TTS depending on build).
//! codescribe_core::tts::init()?;
//! codescribe_core::tts::play("Hello, world!")?;
//! ```
//!
//! ## Architecture
//!
//! - **whisper** - Local Whisper (runtime/cache by default; optional fat embed)
//! - **tts** - Text-to-speech with optional embedded assets depending on build policy
//! - **vad** - Voice activity detection using Silero VAD neural network
//! - **embedder** - Text embeddings using MiniLM model (offline)
//! - **audio** - Recording and audio loading
//! - **config** - User configuration
//! - **ai_formatting** - Post-processing with LLMs

// ═══════════════════════════════════════════════════════════
// Core modules (namespaced)
// ═══════════════════════════════════════════════════════════

/// Agent runtime: threads, sessions, capabilities, permissions, tool grants,
/// and durable run monitoring.
pub mod agent;
pub mod attachment;
pub mod audio;
pub mod config;
pub mod connectors;
pub mod conversation;
/// Streaming tag demultiplexer: splits a model stream into plain text and
/// tagged events as the bytes arrive.
pub mod demux;
pub mod embedded;
pub mod embedder;
mod hf_cache;
/// Message types shared across the app's process boundaries.
pub mod ipc;
pub mod licensing;
/// LLM surface: provider clients, Responses streaming, account auth, key
/// liveness, model discovery, and AI formatting.
pub mod llm;
/// Model Context Protocol: client, server config store, and secret migration.
pub mod mcp;
pub mod memory;
/// Transcription pipeline: engine contracts, streaming, dedup, post-processing,
/// and event sinks.
pub mod pipeline;
/// Transcript quality: overlay scoring, the qube daemon and its report, and the
/// teacher merge between live and Whisper text.
pub mod quality;
pub mod state;
pub mod stt;
pub mod transcript_tagging;
pub mod tts;
/// Shared utilities: capability-checked path access and status reporting.
pub mod util;
pub mod vad;
pub use stt::whisper;

// ═══════════════════════════════════════════════════════════
// Public API - Whisper (STT main interface)
// ═══════════════════════════════════════════════════════════

/// Initialize and transcribe with the embedded-first Whisper path.
pub mod stt_api {
    pub use crate::pipeline::contracts::{FileTranscriptionOptions, FinalPassMode};
    pub use crate::stt::whisper::embedded::{
        EmbeddedModel, get_embedded_data, is_embedded_available,
    };
    pub use crate::stt::whisper::{
        detect_language, get_model_path, init, transcribe, transcribe_file_verdict,
        transcribe_streaming, transcribe_with_segments,
    };
}

// ═══════════════════════════════════════════════════════════
// Public API - TTS (text-to-speech interface)
// ═══════════════════════════════════════════════════════════

/// Initialize and synthesize speech with the configured TTS engine.
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
        AccumulatingVad, CHUNK_SIZE, Resampler, SAMPLE_RATE, SileroVad, VadConfig, VadExtractStats,
        default_model_path, extract_speech,
    };
}

// ═══════════════════════════════════════════════════════════
// Public API - Embedder (text embeddings)
// ═══════════════════════════════════════════════════════════

/// Text embeddings using MiniLM model (offline)
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
// Public re-exports
// ═══════════════════════════════════════════════════════════

pub use llm::{ai_formatting, client};
pub use pipeline::contracts;
pub use pipeline::stream_postprocess;
pub use quality::{overlay_quality, qube_daemon, qube_report};
pub use util::{safe_path, status};
