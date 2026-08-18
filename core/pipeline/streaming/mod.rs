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
/// Phase-2 correction pass: trigger policy, Refine-lane scheduling, reconciliation.
pub(crate) mod correction;
/// Buffered "typing" emission of transcript deltas.
pub(crate) mod emitter;
/// Coalesce ~5 Apple segments into one Layer 1 Whisper window.
pub(crate) mod layer1_window;
/// Assembly of the live transcript from engine events.
pub mod live_assembly;
/// Bounded per-session PCM retention, so a sealed utterance can be re-read for tail-patch.
pub(crate) mod live_audio_buffer;
/// Offline replay of recorded samples, for tests and evaluation.
#[cfg(any(test, feature = "offline_eval"))]
pub(crate) mod offline;
/// Per-session text postprocess: hallucination drops, overlap dedup, emitted-suffix tracking.
pub(crate) mod pipeline;
/// Progressive seal state machine: double-closed spans → lexicon → Light+ → committed.
pub mod progressive_seal;
/// Pre/post-inference gates: hallucination, silence, short-utterance, word-rate anomaly.
pub(crate) mod quality_gate;
/// Event-based transcription session: VAD ingestion, the Whisper inference loop, final emission.
pub(crate) mod session;
/// W13-3B Silero identity + conservative per-word fusion (lane flag default OFF).
pub(crate) mod silero_fusion;
/// W13-4 sealed-span replay refusal + in-span loop fence (lane flag default OFF).
pub(crate) mod span_idempotence;
/// Session stream-log sink (`CODESCRIBE_STREAM_LOG*` env contract).
pub(crate) mod stream_log;
/// Env-tunable runtime knobs shared across these modules.
pub(crate) mod tuning;

/// Streaming pipeline unit tests (quality gates, assembly, session helpers).
#[cfg(test)]
mod tests;

pub use apple_live_session::APPLE_FINAL_OVERLAP_WARNING_CODE;
pub use emitter::{BufferedEmitter, emitter_tick_loop};
pub use live_assembly::{LiveAssembly, assemble_live_from_events};
#[cfg(any(test, feature = "offline_eval"))]
pub use offline::transcribe_streaming_samples;
pub use session::{
    SessionConfig, collect_buffered_engine_events, collect_buffered_engine_events_with_config,
    transcribe_buffered_samples,
};

#[cfg(test)]
pub(crate) use quality_gate::should_drop_silence_chunk;
pub(crate) use session::transcription_session;
pub(crate) use stream_log::stream_log_path;
