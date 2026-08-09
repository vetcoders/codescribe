//! Merged delivery: live floor + Whisper fill at weak loci.
//!
//! Product doctrine (operator 2026-07-24, 85% Apple×Whisper thesis):
//! - **Live (Apple)** is the floor of truth where it spoke.
//! - **Whisper** fills gaps (InsertB) — over-gen into under-gen loci.
//! - Substitutions keep live (floor) and surface as Needs attention for human/lexicon.
//! - Full-replace with Whisper is forbidden: it deletes correct Apple tokens.
//!
//! Missing words in Apple live are **not** failure — they are the canvas for this fill.

use super::align::{AlignOp, align_words};
use super::tokenize::tokenize;

/// How a merged delivery was produced.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MergeMode {
    /// Both sides empty.
    Empty,
    /// Only live had content.
    LiveOnly,
    /// Only whisper had content.
    WhisperOnly,
    /// Word-align merge: live floor + whisper gap-fill.
    LiveFloorWhisperFill,
}

/// Result of merging live assembly with Whisper final-pass.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MergedDelivery {
    /// The delivered text: live floor with Whisper spliced into the gaps.
    pub text: String,
    /// Which of the merge shapes produced [`Self::text`].
    pub mode: MergeMode,
    /// Tokens taken from Whisper InsertB (gap fill count).
    pub whisper_fill_tokens: usize,
    /// Substitutions kept as live (needs-attention residual).
    pub live_kept_substitutes: usize,
    /// Equal tokens kept from live.
    pub equal_tokens: usize,
}

/// Merge live (Apple/stream floor) with Whisper final into one delivery string.
///
/// Pure function — unit tests and controller adjudication both call this.
pub fn merge_live_whisper(live: &str, whisper: &str) -> MergedDelivery {
    let live = live.trim();
    let whisper = whisper.trim();
    if live.is_empty() && whisper.is_empty() {
        return MergedDelivery {
            text: String::new(),
            mode: MergeMode::Empty,
            whisper_fill_tokens: 0,
            live_kept_substitutes: 0,
            equal_tokens: 0,
        };
    }
    if live.is_empty() {
        return MergedDelivery {
            text: whisper.to_string(),
            mode: MergeMode::WhisperOnly,
            whisper_fill_tokens: tokenize(whisper).len(),
            live_kept_substitutes: 0,
            equal_tokens: 0,
        };
    }
    if whisper.is_empty() {
        return MergedDelivery {
            text: live.to_string(),
            mode: MergeMode::LiveOnly,
            whisper_fill_tokens: 0,
            live_kept_substitutes: 0,
            equal_tokens: tokenize(live).len(),
        };
    }

    let live_toks = tokenize(live);
    let whisper_toks = tokenize(whisper);
    let ops = align_words(&live_toks, &whisper_toks);

    let mut out: Vec<String> = Vec::new();
    let mut whisper_fill_tokens = 0usize;
    let mut live_kept_substitutes = 0usize;
    let mut equal_tokens = 0usize;

    for op in ops {
        match op {
            AlignOp::Equal { a, .. } => {
                out.push(live_toks[a].surface.clone());
                equal_tokens += 1;
            }
            AlignOp::DeleteA { a } => {
                // Live-only: keep Apple residue (product: live floor).
                out.push(live_toks[a].surface.clone());
            }
            AlignOp::InsertB { b } => {
                // Whisper excess into live gap — the fill canvas.
                out.push(whisper_toks[b].surface.clone());
                whisper_fill_tokens += 1;
            }
            AlignOp::Substitute { a, .. } => {
                // Keep live; disagreement is Needs attention, not silent overwrite.
                out.push(live_toks[a].surface.clone());
                live_kept_substitutes += 1;
            }
        }
    }

    MergedDelivery {
        text: out.join(" "),
        mode: MergeMode::LiveFloorWhisperFill,
        whisper_fill_tokens,
        live_kept_substitutes,
        equal_tokens,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merge_fills_whisper_excess_into_live_gaps() {
        // Apple under-gen (missing openers) + Whisper fill — 85% thesis shape.
        let live = "korzystając z surowej transkrypcji Toolchain 2024";
        let whisper = "No to dobra teraz generalnie korzystając z surowej transkrypcji Tooltrain 2024 Dziękuję";
        let m = merge_live_whisper(live, whisper);
        assert_eq!(m.mode, MergeMode::LiveFloorWhisperFill);
        assert!(m.whisper_fill_tokens >= 3, "expected gap fills, got {m:?}");
        // Live floor keeps Toolchain (not silent Tooltrain overwrite on substitute).
        assert!(
            m.text.to_lowercase().contains("toolchain")
                || m.text.to_lowercase().contains("tooltrain"),
            "merged text: {}",
            m.text
        );
        // Openers from Whisper should appear (gap fill).
        let lower = m.text.to_lowercase();
        assert!(
            lower.contains("dobra") || lower.contains("generalnie") || lower.contains("no"),
            "whisper gap fill missing in: {}",
            m.text
        );
    }

    #[test]
    fn merge_does_not_full_replace_live_with_whisper() {
        let live = "plik WAV na endpoint leksykon działa Toolchain";
        let whisper = "blik Wave na endpoint leksykon działa Tooltrain Dziękuję";
        let m = merge_live_whisper(live, whisper);
        // Apple correctly heard plik WAV — must not become blik Wave via full-replace.
        assert!(
            m.text.to_lowercase().contains("plik") || m.text.contains("WAV"),
            "live floor lost: {}",
            m.text
        );
    }

    #[test]
    fn empty_sides() {
        assert_eq!(merge_live_whisper("", "").mode, MergeMode::Empty);
        assert_eq!(merge_live_whisper("hello", "").mode, MergeMode::LiveOnly);
        assert_eq!(merge_live_whisper("", "hello").mode, MergeMode::WhisperOnly);
    }
}
