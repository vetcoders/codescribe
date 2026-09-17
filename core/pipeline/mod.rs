//! Transcription pipeline: the event contracts every stage speaks, the sinks
//! that fan those events out to consumers, the acoustic ledger that records
//! occurrence identity, highlight spans, and streaming session management.

/// Acoustic occurrence identity, observation identity, and mutation receipts.
pub mod acoustic_ledger;
/// Event contracts: EngineEvent, sinks trait, and shared pipeline types.
pub mod contracts;
/// W13-6B overlay highlight layer (lexicon corrections + speech-gap pustki).
pub mod highlight;
/// Light+ — deterministic, idempotent sentence shaping (L2 floor, no LLM).
pub mod light_plus;
/// Event sink helpers: collectors and fan-out to consumers.
pub mod sinks;
/// Live streaming session state for partial/final engine events.
pub mod streaming;
/// Take truth sidecar (`.truth.json`) — the schema-v2 observer contract.
pub mod take_truth;

// Re-export core event types for ergonomic access
pub use contracts::{DropKind, EngineEvent, EventSink};
pub use sinks::{CollectorEventSink, FanoutEventSink};
