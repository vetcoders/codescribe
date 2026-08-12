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
/// Neutral Layer 1 ASR session contract: typed events, monotonic ingest, the
/// provider seam, a live normalized gateway transport, and deterministic fakes.
/// No recorder wiring or local model.
pub mod asr_session;
/// Attachment model and on-disk store used by agent chat and LLM context.
pub mod attachment;
/// Audio capture, loading, resampling, and playback primitives.
pub mod audio;
/// Persistent user settings with the tiered config truth model.
pub mod config;
/// External content connectors that produce attachment payloads.
pub mod connectors;
/// Full-duplex Moshi conversational AI (voice turn management).
pub mod conversation;
/// Streaming tag demultiplexer: splits a model stream into plain text and
/// tagged events as the bytes arrive.
pub mod demux;
/// Cross-asset metadata for optionally compiled-in model blobs.
pub mod embedded;
/// Offline MiniLM text embeddings for RAG and similarity.
pub mod embedder;
/// HuggingFace cache lookup for locally downloaded model snapshots.
mod hf_cache;
/// Message types shared across the app's process boundaries.
pub mod ipc;
/// Local-first CSK1 license verification and entitlement state.
pub mod licensing;
/// LLM surface: provider clients, Responses streaming, account auth, key
/// liveness, model discovery, and AI formatting.
pub mod llm;
/// Model Context Protocol: client, server config store, and secret migration.
pub mod mcp;
/// Process memory hygiene after heavy STT/TTS work returns buffers.
pub mod memory;
/// Transcription pipeline: engine contracts, streaming, dedup, post-processing,
/// and event sinks.
pub mod pipeline;
/// Transcript quality: overlay scoring, the qube daemon and its report, and the
/// teacher merge between live and Whisper text.
pub mod quality;
/// Conversation state tracking and voice-chat history helpers.
pub mod state;
/// Speech-to-text engine router (Candle, ONNX, Apple live backends).
pub mod stt;
/// Transcript tagging helpers for paste-delivery wrappers.
pub mod transcript_tagging;
/// Local CSM-1B text-to-speech synthesis surface.
pub mod tts;
/// Shared utilities: capability-checked path access and status reporting.
pub mod util;
/// Silero neural voice-activity detection.
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
