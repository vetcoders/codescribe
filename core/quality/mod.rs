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
pub mod qube_daemon;
pub mod qube_report;
pub mod teacher;

pub use teacher::{
    MergeMode, MergedDelivery, TeacherInput, TeacherReport, merge_live_whisper, report_to_html,
    teach,
};
