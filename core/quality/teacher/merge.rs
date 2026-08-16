//! Merged delivery: live floor + Layer 1 fill at weak loci.
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
    /// Substitutions where Whisper won on known-term evidence (code-switch).
    pub whisper_won_substitutes: usize,
}

/// Provider-neutral decision shape used by Layer 1 adjudication.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Layer1MergeMode {
    /// Both live and provider results were empty.
    Empty,
    /// Only committed live content was available.
    LiveOnly,
    /// Only the Layer 1 provider had content.
    ProviderOnly,
    /// Live floor preserved with provider gap/tail additions.
    LiveFloorGapFill,
}

/// Provider-neutral result exposed to controller adjudication.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Layer1MergedDelivery {
    pub text: String,
    pub mode: Layer1MergeMode,
    pub provider_fill_tokens: usize,
    pub live_kept_substitutes: usize,
    pub equal_tokens: usize,
    pub provider_won_substitutes: usize,
}

impl From<MergedDelivery> for Layer1MergedDelivery {
    fn from(delivery: MergedDelivery) -> Self {
        let mode = match delivery.mode {
            MergeMode::Empty => Layer1MergeMode::Empty,
            MergeMode::LiveOnly => Layer1MergeMode::LiveOnly,
            MergeMode::WhisperOnly => Layer1MergeMode::ProviderOnly,
            MergeMode::LiveFloorWhisperFill => Layer1MergeMode::LiveFloorGapFill,
        };
        Self {
            text: delivery.text,
            mode,
            provider_fill_tokens: delivery.whisper_fill_tokens,
            live_kept_substitutes: delivery.live_kept_substitutes,
            equal_tokens: delivery.equal_tokens,
            provider_won_substitutes: delivery.whisper_won_substitutes,
        }
    }
}

/// Merge a live (Apple/stream) floor with a provider-neutral Layer 1 result.
///
/// Layer 1 may add aligned gaps and tail tokens. Ordinary substitutions keep
/// the committed live token; an obvious short-prefix truncation may be
/// completed by the provider. Provider-specific lexicon evidence belongs in
/// an explicit extension such as [`merge_live_whisper_with_terms`].
pub fn merge_live_layer1(live: &str, layer1: &str) -> Layer1MergedDelivery {
    merge_live_whisper_with_terms(live, layer1, &[]).into()
}

/// Backward-compatible local Whisper entrypoint.
///
/// This preserves the existing no-catalog behavior while the controller uses
/// [`merge_live_layer1`] for provider-neutral cloud adjudication.
pub fn merge_live_whisper(live: &str, whisper: &str) -> MergedDelivery {
    merge_live_whisper_with_terms(live, whisper, &[])
}

/// Normalized word sequence of one side of a disagreement region.
fn region_norms(tokens: &[super::tokenize::Token], idx: &[usize]) -> String {
    idx.iter()
        .map(|&i| tokens[i].norm.as_str())
        .collect::<Vec<_>>()
        .join(" ")
}

/// True when any known term appears as a word-boundary phrase in `haystack`
/// (both already normalized to lowercase space-joined words).
fn contains_known_term(haystack: &str, term_norms: &[String]) -> bool {
    if haystack.is_empty() {
        return false;
    }
    let padded = format!(" {haystack} ");
    term_norms
        .iter()
        .any(|t| !t.is_empty() && padded.contains(&format!(" {t} ")))
}

/// A very short live prefix followed by a materially longer provider token is
/// an interrupted word, not an ordinary substitution (`pow.` →
/// `powkurwiać`). Replacing that fragment cannot delete a complete Apple word.
fn provider_expands_truncated_live(
    live: &super::tokenize::Token,
    provider: &super::tokenize::Token,
) -> bool {
    let live_len = live.norm.chars().count();
    let provider_len = provider.norm.chars().count();
    (2..=4).contains(&live_len)
        && provider_len >= live_len.saturating_add(3)
        && provider.norm.starts_with(&live.norm)
}

/// Merge with a known-terms catalog (Voice Lab / custom lexicon canonical
/// terms). Doctrine extension (operator 2026-08-10, "delivered ≠ what I
/// spoke"): inside a disagreement region, Whisper wins ONLY when its side
/// carries a known term and the live side carries none — deterministic,
/// fail-closed evidence that the code-switched entity was heard by Whisper
/// and garbled by Apple. Everything else keeps the live floor unchanged.
pub fn merge_live_whisper_with_terms(
    live: &str,
    whisper: &str,
    known_terms: &[String],
) -> MergedDelivery {
    let live = live.trim();
    let whisper = whisper.trim();
    if live.is_empty() && whisper.is_empty() {
        return MergedDelivery {
            text: String::new(),
            mode: MergeMode::Empty,
            whisper_fill_tokens: 0,
            live_kept_substitutes: 0,
            equal_tokens: 0,
            whisper_won_substitutes: 0,
        };
    }
    if live.is_empty() {
        return MergedDelivery {
            text: whisper.to_string(),
            mode: MergeMode::WhisperOnly,
            whisper_fill_tokens: tokenize(whisper).len(),
            live_kept_substitutes: 0,
            equal_tokens: 0,
            whisper_won_substitutes: 0,
        };
    }
    if whisper.is_empty() {
        return MergedDelivery {
            text: live.to_string(),
            mode: MergeMode::LiveOnly,
            whisper_fill_tokens: 0,
            live_kept_substitutes: 0,
            equal_tokens: tokenize(live).len(),
            whisper_won_substitutes: 0,
        };
    }

    let live_toks = tokenize(live);
    let whisper_toks = tokenize(whisper);
    let ops = align_words(&live_toks, &whisper_toks);
    let term_norms: Vec<String> = known_terms
        .iter()
        .map(|t| {
            tokenize(t)
                .iter()
                .map(|tok| tok.norm.as_str())
                .collect::<Vec<_>>()
                .join(" ")
        })
        .collect();

    let mut out: Vec<String> = Vec::new();
    let mut whisper_fill_tokens = 0usize;
    let mut live_kept_substitutes = 0usize;
    let mut equal_tokens = 0usize;
    let mut whisper_won_substitutes = 0usize;

    // Buffered disagreement region: contiguous non-Equal ops, resolved as one
    // unit so multi-word terms can match across Substitute+InsertB boundaries.
    let mut region: Vec<AlignOp> = Vec::new();

    let flush_region = |region: &mut Vec<AlignOp>,
                        out: &mut Vec<String>,
                        whisper_fill_tokens: &mut usize,
                        live_kept_substitutes: &mut usize,
                        whisper_won_substitutes: &mut usize| {
        if region.is_empty() {
            return;
        }
        let live_idx: Vec<usize> = region
            .iter()
            .filter_map(|op| match op {
                AlignOp::DeleteA { a } | AlignOp::Substitute { a, .. } => Some(*a),
                _ => None,
            })
            .collect();
        let whisper_idx: Vec<usize> = region
            .iter()
            .filter_map(|op| match op {
                AlignOp::InsertB { b } | AlignOp::Substitute { b, .. } => Some(*b),
                _ => None,
            })
            .collect();
        let live_norm = region_norms(&live_toks, &live_idx);
        let whisper_norm = region_norms(&whisper_toks, &whisper_idx);
        let whisper_evidence = contains_known_term(&whisper_norm, &term_norms);
        let live_evidence = contains_known_term(&live_norm, &term_norms);

        if whisper_evidence && !live_evidence {
            // Term-backed Whisper region: take the whisper side wholesale.
            for &b in &whisper_idx {
                out.push(whisper_toks[b].surface.clone());
            }
            let subs = region
                .iter()
                .filter(|op| matches!(op, AlignOp::Substitute { .. }))
                .count();
            *whisper_won_substitutes += subs;
            *whisper_fill_tokens += whisper_idx.len() - subs;
        } else {
            // Status quo: live floor + whisper gap-fill, per op.
            for op in region.iter() {
                match op {
                    AlignOp::DeleteA { a } => out.push(live_toks[*a].surface.clone()),
                    AlignOp::InsertB { b } => {
                        out.push(whisper_toks[*b].surface.clone());
                        *whisper_fill_tokens += 1;
                    }
                    AlignOp::Substitute { a, b }
                        if provider_expands_truncated_live(&live_toks[*a], &whisper_toks[*b]) =>
                    {
                        out.push(whisper_toks[*b].surface.clone());
                        *whisper_won_substitutes += 1;
                    }
                    AlignOp::Substitute { a, .. } => {
                        out.push(live_toks[*a].surface.clone());
                        *live_kept_substitutes += 1;
                    }
                    AlignOp::Equal { .. } => unreachable!("regions hold non-Equal ops only"),
                }
            }
        }
        region.clear();
    };

    for op in ops {
        match op {
            AlignOp::Equal { a, .. } => {
                flush_region(
                    &mut region,
                    &mut out,
                    &mut whisper_fill_tokens,
                    &mut live_kept_substitutes,
                    &mut whisper_won_substitutes,
                );
                out.push(live_toks[a].surface.clone());
                equal_tokens += 1;
            }
            other => region.push(other),
        }
    }
    flush_region(
        &mut region,
        &mut out,
        &mut whisper_fill_tokens,
        &mut live_kept_substitutes,
        &mut whisper_won_substitutes,
    );

    MergedDelivery {
        text: out.join(" "),
        mode: MergeMode::LiveFloorWhisperFill,
        whisper_fill_tokens,
        live_kept_substitutes,
        equal_tokens,
        whisper_won_substitutes,
    }
}

/// Doctrine guards: gap-fill, live floor, and empty-side modes.
#[cfg(test)]
mod layer1_contract_tests {
    use super::{Layer1MergeMode, Layer1MergedDelivery, merge_live_layer1, merge_live_whisper};

    #[test]
    fn provider_neutral_layer1_preserves_live_substitutions_and_adds_tail() {
        let merged = merge_live_layer1(
            "live_token shared_token",
            "provider_token shared_token tail_token",
        );

        assert_eq!(merged.mode, Layer1MergeMode::LiveFloorGapFill);
        assert!(merged.text.starts_with("live_token shared_token"));
        assert!(merged.text.ends_with("tail_token"));
        assert_eq!(merged.provider_won_substitutes, 0);
    }

    #[test]
    fn local_legacy_entrypoint_retains_provider_neutral_default_behavior() {
        let live = "live_token shared_token";
        let layer1 = "provider_token shared_token tail_token";

        assert_eq!(
            Layer1MergedDelivery::from(merge_live_whisper(live, layer1)),
            merge_live_layer1(live, layer1)
        );
    }

    #[test]
    fn provider_completes_a_truncated_live_word_without_wholesale_rewrite() {
        let merged = merge_live_layer1(
            "Możesz odczytać i też pow.",
            "Możesz odczytać i też powkurwiać się razem.",
        );

        assert_eq!(merged.text, "Możesz odczytać i też powkurwiać się razem.");
        assert_eq!(merged.provider_won_substitutes, 1);
        assert_eq!(merged.live_kept_substitutes, 0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Whisper InsertB fills Apple under-gen gaps; substitutes keep live floor.
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

    /// Live tokens (plik/WAV) must survive; full Whisper replace is forbidden.
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

    /// Empty live/whisper combinations select Empty / LiveOnly / WhisperOnly.
    #[test]
    fn empty_sides() {
        assert_eq!(merge_live_whisper("", "").mode, MergeMode::Empty);
        assert_eq!(merge_live_whisper("hello", "").mode, MergeMode::LiveOnly);
        assert_eq!(merge_live_whisper("", "hello").mode, MergeMode::WhisperOnly);
    }

    /// Code-switch loss (operator 2026-08-10, "delivered ≠ what I spoke"):
    /// Apple garbled the English term, Whisper heard it. With the term in the
    /// known-terms catalog, the disagreement region flips to Whisper.
    #[test]
    fn known_whisper_term_wins_substitute_region() {
        let live = "ten pas są dla mnie wystarczająco dobre";
        let whisper = "ten partial passes są dla mnie wystarczająco dobre";
        let terms = vec!["partial passes".to_string()];
        let m = merge_live_whisper_with_terms(live, whisper, &terms);
        assert!(
            m.text.to_lowercase().contains("partial passes"),
            "whisper term should win the region: {}",
            m.text
        );
        assert!(
            !m.text.to_lowercase().split_whitespace().any(|w| w == "pas"),
            "garbled live token should be replaced, not kept alongside: {}",
            m.text
        );
        assert!(
            m.whisper_won_substitutes >= 1,
            "expected whisper win: {m:?}"
        );
    }

    /// No term evidence → status quo: live floor keeps the substitute.
    #[test]
    fn no_term_evidence_keeps_live_floor() {
        let live = "ten pas są dla mnie wystarczająco dobre";
        let whisper = "ten partial passes są dla mnie wystarczająco dobre";
        let m = merge_live_whisper_with_terms(live, whisper, &[]);
        assert!(
            m.text.contains("pas"),
            "live floor lost without terms: {}",
            m.text
        );
        assert_eq!(m.whisper_won_substitutes, 0);
    }

    /// A known term on the LIVE side blocks the whisper override (Apple heard
    /// the entity correctly — Whisper's variant is the phonetic drift).
    #[test]
    fn live_term_blocks_whisper_override() {
        let live = "plik WAV na endpoint działa";
        let whisper = "blik Wave na endpoint działa";
        let terms = vec!["WAV".to_string(), "Wave".to_string()];
        let m = merge_live_whisper_with_terms(live, whisper, &terms);
        assert!(m.text.contains("WAV"), "live entity lost: {}", m.text);
        assert_eq!(m.whisper_won_substitutes, 0);
    }

    /// Today's real sample: the English tail garbled by Apple flips to Whisper
    /// only where term evidence exists; regions without evidence keep live.
    #[test]
    fn real_sample_code_switch_tail() {
        let live = "delivered nie równa się Power in site D";
        let whisper = "delivered nie równa się what I spoke";
        let terms = vec!["what I spoke".to_string()];
        let m = merge_live_whisper_with_terms(live, whisper, &terms);
        assert!(
            m.text.to_lowercase().contains("what i spoke"),
            "term-backed whisper region should win: {}",
            m.text
        );
        assert!(
            !m.text.to_lowercase().contains("power in site"),
            "apple garble should be replaced in the won region: {}",
            m.text
        );
    }

    /// The zero-terms delegate is byte-for-byte the legacy behavior.
    #[test]
    fn legacy_wrapper_delegates_with_no_terms() {
        let live = "korzystając z surowej transkrypcji Toolchain 2024";
        let whisper = "No to dobra korzystając z surowej transkrypcji Tooltrain 2024";
        assert_eq!(
            merge_live_whisper(live, whisper),
            merge_live_whisper_with_terms(live, whisper, &[])
        );
    }
}
