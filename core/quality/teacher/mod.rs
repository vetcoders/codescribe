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

mod align;
mod report;
mod tokenize;

pub use align::{AlignOp, align_words};
pub use report::{
    AttentionKind, AttentionSpan, LexiconHint, TeacherInput, TeacherReport, report_to_html, teach,
};
pub use tokenize::{Token, normalize_token, tokenize};

#[cfg(test)]
mod tests;
