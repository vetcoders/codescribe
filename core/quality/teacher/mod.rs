//! Teacher — standalone learning triangle for Codescribe.
//!
//! Product thesis (operator):
//! - **Apple live** under-generates at uncertainty (gaps in time, not a black void).
//! - **Whisper** over-generates / hallucinates, often *into those gaps*.
//! - **Diff** = Needs attention (human label surface).
//! - **Human correction** → lexicon candidates (the petarda on the next run).
//!
//! This module is deliberately pure: no mic, no Metal, no UniFFI. CLI / HTML /
//! future overlay all call the same `teach()` entry.
//!
//! Proof mode: feed live (Apple-proxy or real), whisper (final-pass), optional
//! human reference — emit attention spans + lexicon hints + a hit-rate score
//! for the "gaps ≡ hallucination sites" bet.

/// Word-level alignment operators for live × whisper (and optional human).
mod align;
/// Live-floor + Whisper gap-fill merge used by stop-path delivery.
mod merge;
/// Attention spans, lexicon hints, and HTML report rendering.
mod report;
/// Lightweight tokenizer shared by align / merge / teach.
mod tokenize;

pub use align::{AlignOp, align_words};
pub use merge::{
    Layer1MergeMode, Layer1MergedDelivery, MergeMode, MergedDelivery, merge_live_layer1,
    merge_live_whisper, merge_live_whisper_with_terms,
};
pub use report::{
    AttentionKind, AttentionSpan, LexiconHint, TeacherInput, TeacherReport, report_to_html, teach,
};
pub use tokenize::{Token, normalize_token, tokenize};

/// Integration-style teacher tests (sibling `tests.rs` file).
#[cfg(test)]
mod tests;
