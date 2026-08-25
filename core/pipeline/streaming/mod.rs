//! Streaming transcription pipeline — orchestration, buffered emission, and policy.
//!
//! Extracted from `audio::streaming_recorder` to decouple pipeline logic
//! (hallucination filtering, overlap dedup, re-transcription, buffered "typing"
//! emission) from the audio capture layer.
//!
//! Decomposed into responsibility modules; this facade preserves the original
//! `pipeline::streaming::*` import surface for all external consumers.

/// Apple on-device recognition session driving the live layer.
pub(crate) mod apple_live_session;
/// Coalesce ~5 Apple segments into one Layer 1 Whisper window.
pub(crate) mod layer1_window;
/// Bounded per-session PCM retention, so a sealed utterance can be re-read for tail-patch.
pub(crate) mod live_audio_buffer;
/// Per-session text postprocess: hallucination drops, overlap dedup, emitted-suffix tracking.
pub(crate) mod pipeline;
/// Event-based transcription session: VAD ingestion, the Whisper inference loop, final emission.
pub(crate) mod session;
/// W13-3B Silero identity + conservative per-word fusion (lane flag default OFF).
pub(crate) mod silero_fusion;
/// Session stream-log sink (`CODESCRIBE_STREAM_LOG*` env contract).
pub(crate) mod stream_log;
/// Env-tunable runtime knobs shared across these modules.
pub(crate) mod tuning;

pub use apple_live_session::APPLE_FINAL_OVERLAP_WARNING_CODE;
pub use session::{
    SessionConfig, TAIL_PATCH_SESSION_RECEIPT_WARNING_CODE, TailPatchDrainDisposition,
    TailPatchSessionReceipt, collect_buffered_engine_events,
    collect_buffered_engine_events_with_config, transcribe_buffered_samples,
};

pub(crate) use session::transcription_session;
pub(crate) use stream_log::stream_log_path;
