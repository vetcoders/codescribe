//! Presentation layer — converts engine events to user-facing output.
//!
//! This module owns all presentation decisions: typing animation, buffer delays,
//! delta encoding for overlays, etc. The engine emits `EngineEvent`s (what happened),
//! and this module decides how to show them.

pub mod cli_transcript_lane;
pub mod emitter;
pub mod status_projection;
pub mod transcript_bus;
pub mod transcript_projection;

pub use cli_transcript_lane::CliTranscriptLane;
pub use emitter::PresentationEmitter;
pub use transcript_bus::{TranscriptBus, TranscriptMode, TranscriptSession};
