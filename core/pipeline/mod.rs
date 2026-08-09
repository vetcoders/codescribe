//! Transcription pipeline: the event contracts every stage speaks, the sinks
//! that fan those events out to consumers, overlap dedup, streaming session
//! management, and the post-processing passes applied to emitted text.

pub mod contracts;
pub mod dedup;
pub mod light_plus;
pub mod sinks;
pub mod stream_postprocess;
pub mod streaming;

// Re-export core event types for ergonomic access
pub use contracts::{DropKind, EngineEvent, EventSink};
pub use sinks::{CollectorEventSink, DeltaSinkAdapter, FanoutEventSink};

#[cfg(test)]
mod tests;
