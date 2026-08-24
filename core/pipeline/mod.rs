//! Transcription pipeline: the event contracts every stage speaks, the sinks
//! that fan those events out to consumers, overlap dedup, streaming session
//! management, and the post-processing passes applied to emitted text.

/// Occurrence / observation / mutation-receipt ledger for acoustic spans.
pub mod acoustic_identity;
/// Acoustic occurrence identity, observation identity, and mutation receipts.
pub mod acoustic_ledger;
/// Event contracts: EngineEvent, sinks trait, and shared pipeline types.
pub mod contracts;
/// Overlap/duplicate utterance suppression for streamed transcript events.
pub mod dedup;
/// W13-6B overlay highlight layer (lexicon corrections + speech-gap pustki).
pub mod highlight;
/// Light-plus post-pass for low-latency transcript cleanup.
pub mod light_plus;
/// MiniLM meaning check for AI-formatted deliveries (calibrated floor).
pub mod semantic_guard;
/// Event sink adapters: collector, delta, and fan-out to consumers.
pub mod sinks;
/// Streaming text post-process (punctuation, casing) before sinks.
pub mod stream_postprocess;
/// Live streaming session state for partial/final engine events.
pub mod streaming;

// Re-export core event types for ergonomic access
pub use acoustic_identity::{
    AcousticObservation, MutationReceipt, ObservationIdentity, ObservationLedger,
    ObservationProducer, OccurrenceIdentity, admit_acoustic_spans, admit_observations,
};
pub use contracts::{DropKind, EngineEvent, EventSink};
pub use sinks::{CollectorEventSink, DeltaSinkAdapter, FanoutEventSink};

/// Pipeline module integration tests (cfg-gated companion).
#[cfg(test)]
mod tests;
