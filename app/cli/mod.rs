//! Command surfaces shared by `codescribe <subcommand>` and the legacy bins.
//!
//! WHY THIS EXISTS. The crate shipped five binaries — `codescribe`,
//! `qube-report`, `qube-daemon`, `codescribe-teacher`, `codescribe-corpus` —
//! each with its own `--help`, its own flag spellings and its own copy of the
//! engine boot. Nothing pointed from one to another, so a surface was
//! discoverable only if you already knew its binary existed.
//!
//! Each module here owns one job's arguments and dispatch. `bin/codescribe.rs`
//! mounts them as subcommands; the legacy bins stay as one-line shims over the
//! same functions, so both spellings run identical code rather than drifting.
//!
//! A process-level consequence matters for the engine: a unified entry point
//! loads Whisper once per invocation instead of once per binary in a pipeline.
//! Sharing a model ACROSS processes is a different problem and already has its
//! own answer in `codescribe-stt-sidecar`.

/// Private corpus census and production-overlay replay.
pub mod corpus;
/// Self-improving quality loop: report, regression analysis, tuning.
pub mod daemon;
/// Batch quality report and the corrections replay.
pub mod report;
/// Learning triangle: Apple-live × Whisper × human reference.
pub mod teacher;
