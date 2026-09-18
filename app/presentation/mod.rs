//! Presentation layer — converts engine events to user-facing output.
//!
//! This module owns all presentation decisions: typing animation, buffer delays,
//! delta encoding for overlays, etc. The engine emits `EngineEvent`s (what happened),
//! and this module decides how to show them.

pub mod cli_transcript_lane;
pub mod emitter;
pub mod status_projection;
pub mod transcript_bus;
/// Retention for the clean transcript bus: status and schema-aware compaction.
pub mod transcript_bus_maintenance;
pub mod transcript_projection;

pub use cli_transcript_lane::CliTranscriptLane;
pub use emitter::{
    PresentationEmitter, TerminalFormatterRequest, UserRevisionCommit, UserRevisionIntent,
};
pub use transcript_bus::{TranscriptBus, TranscriptMode, TranscriptSession};
pub use transcript_bus_maintenance::{BusStatus, CompactionReport, bus_status, compact_bus};
