//! Neutral Layer 1 ASR session contract.
//!
//! Layer 0 (the Apple live canvas) draws instantly and owns committed text.
//! Layer 1 is a *refiner*: it may fill gaps and patch tails, and it may never
//! rewrite what the canvas already committed. This module is the seam that
//! Layer 1 providers plug into — typed, transport-free, and vendor-neutral.
//!
//! ## What lives here
//!
//! - [`events`] — the typed event vocabulary: every event carries session,
//!   utterance and sequence identity, partial-vs-final is a variant (not a
//!   boolean), the audio span is optional and bounded, and errors/usage are
//!   typed with no free-form payload.
//! - [`ingest`] — the ordering state machine: monotonic sequencing, idempotent
//!   duplicate finals, and a sealed utterance that no later partial can reopen.
//! - [`provider`] — [`AsrSessionProvider`] plus the selection types that keep
//!   Layer 0 canvas choice and Layer 1 refiner mode on two separate axes.
//! - [`fake`] — a deterministic in-memory provider for tests and later cuts.
//!
//! ## What deliberately does NOT live here
//!
//! No sockets, no HTTP, no recorder wiring, no settings surface, no gateway
//! session mint, no local model. A provider implementation is a follow-on cut;
//! this module only fixes the shape it must satisfy.
//!
//! The existing whole-file `client::transcribe_cloud` / `transcribe_websocket`
//! API is **outside** this contract. It uploads one completed recording and is
//! a stop/recovery path, not a live session — routing it through this interface
//! and calling the result "live" is precisely the confusion this seam prevents.
//!
//! ## Doctrine encoded in the types
//!
//! - A refiner failure degrades to canvas + lexicon
//!   ([`LayerSelection::degraded`]); it can never swap the canvas engine, and
//!   nothing here can trigger a local model load.
//! - A final seals its utterance. Re-delivery of that same final is idempotent;
//!   anything else aimed at a sealed utterance is refused rather than applied.
//! - Errors carry a typed kind and nothing else, so no transcript fragment,
//!   audio, or credential can ride an error into a log line.

/// Typed Layer 1 session events, identity, bounded ranges, errors, and usage.
pub mod events;
/// Deterministic in-memory provider used by tests and follow-on transport cuts.
pub mod fake;
/// Ordering state machine: monotonic sequencing and idempotent duplicate finals.
pub mod ingest;
/// Provider trait plus the canvas/refiner selection split.
pub mod provider;

#[cfg(test)]
mod tests;

pub use events::{
    AsrErrorKind, AsrSessionEvent, AudioRange, ErrorEvent, EventIdentity, SessionId,
    TranscriptEvent, UsageEvent,
};
pub use fake::FakeAsrSessionProvider;
pub use ingest::{IngestVerdict, SessionIngest};
pub use provider::{AsrSessionProvider, CanvasEngine, LayerSelection, RefinerMode, SessionInput};
