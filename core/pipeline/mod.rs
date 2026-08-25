//! Transcription pipeline: the event contracts every stage speaks, the sinks
//! that fan those events out to consumers, overlap dedup, streaming session
//! management, and the post-processing passes applied to emitted text.

/// Acoustic occurrence identity, observation identity, and mutation receipts.
pub mod acoustic_ledger;
/// Event contracts: EngineEvent, sinks trait, and shared pipeline types.
pub mod contracts;
/// W13-6B overlay highlight layer (lexicon corrections + speech-gap pustki).
pub mod highlight;
/// Event sink helpers: collectors and fan-out to consumers.
pub mod sinks;
/// Live streaming session state for partial/final engine events.
pub mod streaming;

// Re-export core event types for ergonomic access
pub use contracts::{DropKind, EngineEvent, EventSink};
pub use sinks::{CollectorEventSink, FanoutEventSink};
