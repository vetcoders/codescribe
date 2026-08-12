//! Quality surfaces — where transcription truth is measured, corrected, and learned from.
//!
//! Four independent loops share this facade:
//!
//! - `overlay_quality` — captures human edits of the overlay FINAL transcript and
//!   distils them into custom lexicon rules (the live, per-user loop).
//! - `qube_report` — batch WAV evaluation: transcribe, format, score, emit artifacts.
//! - `qube_daemon` — wraps the report in a self-improving cycle (report → regression
//!   analysis → tuning updates → re-run).
//! - `teacher` — the offline learning triangle (Apple live × Whisper × human reference)
//!   that produces merged deliveries and attention spans.
//!
//! Only `teacher` is re-exported here; the other three are reached through their
//! own module paths.

pub mod overlay_quality;
/// Background Qube donor daemon: opt-in stop-path WAV/transcript persistence.
pub mod qube_daemon;
/// Qube report types and serialization for quality/donor telemetry surfaces.
pub mod qube_report;
/// Teacher loop: attention flags, lexicon feedback, polygon token helpers.
pub mod teacher;

pub use teacher::{
    Layer1MergeMode, Layer1MergedDelivery, MergeMode, MergedDelivery, TeacherInput, TeacherReport,
    merge_live_layer1, merge_live_whisper, merge_live_whisper_with_terms, report_to_html, teach,
};
