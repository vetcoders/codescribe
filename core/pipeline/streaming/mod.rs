//! Streaming transcription pipeline — orchestration, buffered emission, and policy.
//!
//! Extracted from `audio::streaming_recorder` to decouple pipeline logic
//! (hallucination filtering, overlap dedup, re-transcription, buffered "typing"
//! emission) from the audio capture layer.
//!
//! Decomposed into responsibility modules; this facade preserves the original
//! `pipeline::streaming::*` import surface for all external consumers.

pub(crate) mod correction;
pub(crate) mod emitter;
#[cfg(any(test, feature = "offline_eval"))]
pub(crate) mod offline;
pub(crate) mod pipeline;
pub(crate) mod quality_gate;
pub(crate) mod session;
pub(crate) mod stream_log;
pub(crate) mod tuning;

#[cfg(test)]
mod tests;

pub use emitter::{BufferedEmitter, emitter_tick_loop};
#[cfg(any(test, feature = "offline_eval"))]
pub use offline::transcribe_streaming_samples;
pub use session::{SessionConfig, collect_buffered_engine_events, transcribe_buffered_samples};

#[cfg(test)]
pub(crate) use quality_gate::should_drop_silence_chunk;
pub(crate) use session::transcription_session;
pub(crate) use stream_log::stream_log_path;
