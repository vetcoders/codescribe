//! Presentation layer — converts engine events to user-facing output.
//!
//! This module owns all presentation decisions: typing animation, buffer delays,
//! delta encoding for overlays, etc. The engine emits `EngineEvent`s (what happened),
//! and this module decides how to show them.
//!
//! Created by M&K (c)2026 VetCoders

pub mod emitter;

pub use emitter::PresentationEmitter;
