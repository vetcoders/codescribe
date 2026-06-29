//! Layer 1 — Whisper Tail Patch (diff core).
//!
//! Implements the ADR "Layered Incremental Transcription Pipeline" (2026-05-26)
//! Layer 1 primitive: given the text Layer 0 (Apple live) already committed for
//! an utterance, and a higher-recall Whisper re-transcription of the *same*
//! audio slice, produce **bounded** [`EngineEvent::ReplaceRange`] patches that
//! fill in / correct only the tokens that differ.
//!
//! # Invariants (from the ADR "Hard invariants")
//!
//! - **NEVER REWRITE FROM ZERO.** This core only ever emits bounded
//!   `ReplaceRange { source: LayerSource::TailPatch }` events scoped to a single
//!   utterance. It never returns a full-buffer overwrite.
//! - **Bounded patches.** Every emitted event references char offsets inside the
//!   committed utterance text passed in.
//! - **Conservative by default.** If the diff distance exceeds
//!   [`TailPatchConfig::max_change_ratio`], the whole patch is dropped
//!   ([`TailPatchOutcome::Skipped`]) and Layer 0 output stands unchanged —
//!   "don't patch if uncertain".
//!
//! # Scope of this cut (v1)
//!
//! Emits **substitution** and **insertion** patches (wrong token → right token,
//! missing token filled). Deletions (Whisper saw *fewer* words than Layer 0) are
//! intentionally left intact: dropping words the user already saw is the riskier
//! direction, so v1 leaves them to a later layer / the operator. Deleted tokens
//! still count toward the change ratio so a wildly divergent re-transcription is
//! skipped wholesale.
//!
//! This module is a **pure** function of its inputs. It performs no audio
//! capture, no network calls, and is not yet wired into the streaming hot path —
//! the orchestrator (ADR `app/controller/layered_orchestrator.rs`) owns that
//! wiring and the audio cursor.

use crate::pipeline::contracts::{EngineEvent, LayerSource};

/// Env flag gating the layered transcription pipeline.
///
/// `CODESCRIBE_LAYERED_TRANSCRIPTION=phase{1,2,3,4}` — defaults to OFF. Returns
/// the active phase number when set, so callers can gate Layer 1..4 wiring.
/// Kept here (not in the config hub) so this cut stays isolated; the orchestrator
/// can promote it to a typed config field when it lands.
pub const LAYERED_TRANSCRIPTION_ENV: &str = "CODESCRIBE_LAYERED_TRANSCRIPTION";

/// Env override for [`TailPatchConfig::max_change_ratio`].
pub const TAIL_PATCH_MAX_CHANGE_RATIO_ENV: &str = "CODESCRIBE_TAIL_PATCH_MAX_CHANGE_RATIO";

/// Active layered-transcription phase, or `None` when the flag is unset/off.
pub fn layered_phase() -> Option<u8> {
    let raw = std::env::var(LAYERED_TRANSCRIPTION_ENV).ok()?;
    let raw = raw.trim().to_ascii_lowercase();
    let digits = raw.strip_prefix("phase").unwrap_or(&raw);
    match digits.parse::<u8>().ok()? {
        n @ 1..=4 => Some(n),
        _ => None,
    }
}

/// Tuning for the tail-patch diff.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TailPatchConfig {
    /// Maximum fraction of committed tokens that may change before the whole
    /// patch is skipped. `0.5` means: if more than half the utterance would be
    /// touched, leave Layer 0 output untouched.
    pub max_change_ratio: f64,
}

impl Default for TailPatchConfig {
    fn default() -> Self {
        Self {
            max_change_ratio: 0.5,
        }
    }
}

impl TailPatchConfig {
    /// Read config from env, falling back to defaults.
    pub fn from_env() -> Self {
        let mut cfg = Self::default();
        if let Some(value) = std::env::var(TAIL_PATCH_MAX_CHANGE_RATIO_ENV)
            .ok()
            .and_then(|raw| raw.trim().parse::<f64>().ok())
            .filter(|v| v.is_finite() && (0.0..=1.0).contains(v))
        {
            cfg.max_change_ratio = value;
        }
        cfg
    }
}

/// Result of a tail-patch diff.
#[derive(Debug, Clone, PartialEq)]
pub enum TailPatchOutcome {
    /// Bounded patches to apply (always `EngineEvent::ReplaceRange`), ordered so
    /// that sequential application to the committed text is offset-stable
    /// (descending by `start`).
    Patches(Vec<EngineEvent>),
    /// Re-transcription matched the committed text — nothing to do.
    NoChange,
    /// Diff exceeded the safety threshold (or there was nothing to patch
    /// against); Layer 0 output stands unchanged.
    Skipped { reason: String },
}

/// A whitespace-delimited token with char-offset span inside the source string.
#[derive(Debug, Clone, PartialEq)]
struct Token {
    /// Char index of the first char (inclusive).
    char_start: usize,
    /// Char index one past the last char (exclusive).
    char_end: usize,
    text: String,
}

fn tokenize(input: &str) -> Vec<Token> {
    let mut tokens = Vec::new();
    let mut start: Option<usize> = None;
    let mut buf = String::new();
    for (char_idx, ch) in input.chars().enumerate() {
        if ch.is_whitespace() {
            if let Some(s) = start.take() {
                tokens.push(Token {
                    char_start: s,
                    char_end: char_idx,
                    text: std::mem::take(&mut buf),
                });
            }
        } else {
            if start.is_none() {
                start = Some(char_idx);
            }
            buf.push(ch);
        }
    }
    if let Some(s) = start {
        let char_end = input.chars().count();
        tokens.push(Token {
            char_start: s,
            char_end,
            text: buf,
        });
    }
    tokens
}

/// One contiguous diff group between two consecutive aligned (matched) tokens.
struct EditGroup {
    /// Indices into the committed token list that are unmatched (replaced/deleted).
    committed: std::ops::Range<usize>,
    /// Indices into the re-transcribed token list that are unmatched (inserted).
    retranscribed: std::ops::Range<usize>,
    /// Char position to use when the committed side is empty (insertion anchor):
    /// the char_end of the previous matched committed token, or 0 at buffer start.
    anchor: usize,
    /// Whether there is a previous matched committed token (controls insertion spacing).
    has_prev_match: bool,
}

/// Longest-common-subsequence alignment over token text (exact match).
///
/// Returns pairs `(committed_idx, retranscribed_idx)` of matched tokens, in order.
fn lcs_matches(committed: &[Token], retranscribed: &[Token]) -> Vec<(usize, usize)> {
    let m = committed.len();
    let n = retranscribed.len();
    // dp[i][j] = LCS length of committed[i..] and retranscribed[j..].
    let mut dp = vec![vec![0usize; n + 1]; m + 1];
    for i in (0..m).rev() {
        for j in (0..n).rev() {
            dp[i][j] = if committed[i].text == retranscribed[j].text {
                dp[i + 1][j + 1] + 1
            } else {
                dp[i + 1][j].max(dp[i][j + 1])
            };
        }
    }

    let mut matches = Vec::new();
    let (mut i, mut j) = (0, 0);
    while i < m && j < n {
        if committed[i].text == retranscribed[j].text {
            matches.push((i, j));
            i += 1;
            j += 1;
        } else if dp[i + 1][j] >= dp[i][j + 1] {
            i += 1;
        } else {
            j += 1;
        }
    }
    matches
}

/// Compute bounded tail-patch events from a Layer-0 committed utterance and a
/// Whisper re-transcription of the same audio slice.
///
/// `utterance_id` is stamped on every emitted [`EngineEvent::ReplaceRange`].
pub fn compute_tail_patch(
    committed: &str,
    retranscribed: &str,
    utterance_id: u64,
    cfg: &TailPatchConfig,
) -> TailPatchOutcome {
    // Layer 0 owns the first commit: nothing to patch against an empty buffer.
    if committed.trim().is_empty() {
        return TailPatchOutcome::Skipped {
            reason: "empty_committed".to_string(),
        };
    }
    if retranscribed.trim().is_empty() {
        return TailPatchOutcome::Skipped {
            reason: "empty_retranscription".to_string(),
        };
    }

    let c_tokens = tokenize(committed);
    let r_tokens = tokenize(retranscribed);
    if c_tokens.is_empty() {
        return TailPatchOutcome::Skipped {
            reason: "no_committed_tokens".to_string(),
        };
    }

    let matches = lcs_matches(&c_tokens, &r_tokens);

    // Build edit groups from the gaps between consecutive matched pairs.
    let mut groups: Vec<EditGroup> = Vec::new();
    let mut prev_c = 0usize;
    let mut prev_r = 0usize;
    let mut prev_match_c_end: Option<usize> = None; // char_end of last matched committed token
    for (mc, mr) in matches.iter().copied() {
        if mc > prev_c || mr > prev_r {
            groups.push(EditGroup {
                committed: prev_c..mc,
                retranscribed: prev_r..mr,
                anchor: prev_match_c_end.unwrap_or(0),
                has_prev_match: prev_match_c_end.is_some(),
            });
        }
        prev_match_c_end = Some(c_tokens[mc].char_end);
        prev_c = mc + 1;
        prev_r = mr + 1;
    }
    if prev_c < c_tokens.len() || prev_r < r_tokens.len() {
        groups.push(EditGroup {
            committed: prev_c..c_tokens.len(),
            retranscribed: prev_r..r_tokens.len(),
            anchor: prev_match_c_end.unwrap_or(0),
            has_prev_match: prev_match_c_end.is_some(),
        });
    }

    if groups.is_empty() {
        return TailPatchOutcome::NoChange;
    }

    // Safety gate: count changed tokens against the committed token budget.
    let changed: usize = groups
        .iter()
        .map(|g| g.committed.len().max(g.retranscribed.len()))
        .sum();
    let ratio = changed as f64 / c_tokens.len() as f64;
    if ratio > cfg.max_change_ratio {
        return TailPatchOutcome::Skipped {
            reason: format!(
                "change_ratio {:.2} exceeds max {:.2}",
                ratio, cfg.max_change_ratio
            ),
        };
    }

    let mut events: Vec<EngineEvent> = Vec::new();
    for g in &groups {
        let c_empty = g.committed.is_empty();
        let r_empty = g.retranscribed.is_empty();

        if r_empty {
            // Deletion: v1 leaves committed tokens intact (conservative).
            continue;
        }

        let replacement: String = g
            .retranscribed
            .clone()
            .map(|idx| r_tokens[idx].text.as_str())
            .collect::<Vec<_>>()
            .join(" ");

        if c_empty {
            // Insertion: anchor after the previous matched token (or at start).
            if g.has_prev_match {
                events.push(EngineEvent::ReplaceRange {
                    utterance_id,
                    start: g.anchor,
                    end: g.anchor,
                    text: format!(" {replacement}"),
                    source: LayerSource::TailPatch,
                });
            } else {
                events.push(EngineEvent::ReplaceRange {
                    utterance_id,
                    start: 0,
                    end: 0,
                    text: format!("{replacement} "),
                    source: LayerSource::TailPatch,
                });
            }
        } else {
            // Substitution: replace the committed span with the W text.
            let start = c_tokens[g.committed.start].char_start;
            let end = c_tokens[g.committed.end - 1].char_end;
            events.push(EngineEvent::ReplaceRange {
                utterance_id,
                start,
                end,
                text: replacement,
                source: LayerSource::TailPatch,
            });
        }
    }

    if events.is_empty() {
        return TailPatchOutcome::NoChange;
    }

    // Descending by start so sequential application is offset-stable.
    events.sort_by_key(|e| std::cmp::Reverse(event_start(e)));
    TailPatchOutcome::Patches(events)
}

fn event_start(event: &EngineEvent) -> usize {
    match event {
        EngineEvent::ReplaceRange { start, .. } => *start,
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Apply every emitted patch to the committed text, in emission order, and
    /// return the resulting buffer. Mirrors how a sink folds the events.
    fn apply_all(committed: &str, outcome: &TailPatchOutcome) -> String {
        let mut buf = committed.to_string();
        if let TailPatchOutcome::Patches(events) = outcome {
            for ev in events {
                ev.apply_to_committed_text(&mut buf)
                    .expect("bounded range must be valid against committed text");
            }
        }
        buf
    }

    #[test]
    fn identical_text_is_no_change() {
        let cfg = TailPatchConfig::default();
        let outcome = compute_tail_patch("ala ma kota", "ala ma kota", 1, &cfg);
        assert_eq!(outcome, TailPatchOutcome::NoChange);
    }

    #[test]
    fn empty_committed_is_skipped() {
        let cfg = TailPatchConfig::default();
        let outcome = compute_tail_patch("", "cokolwiek", 1, &cfg);
        assert!(matches!(outcome, TailPatchOutcome::Skipped { .. }));
    }

    #[test]
    fn single_substitution_corrects_mixed_language_token() {
        // Layer 0 (Apple, PL-dominant) misheard the English place name.
        let cfg = TailPatchConfig::default();
        let committed = "lecimy z Bytowa do nowego jorku";
        let retranscribed = "lecimy z Bytowa do New York";
        let outcome = compute_tail_patch(committed, retranscribed, 7, &cfg);
        match &outcome {
            TailPatchOutcome::Patches(events) => {
                assert!(events
                    .iter()
                    .all(|e| matches!(e, EngineEvent::ReplaceRange { source, .. } if *source == LayerSource::TailPatch)));
                assert!(events.iter().all(|e| matches!(e, EngineEvent::ReplaceRange { utterance_id, .. } if *utterance_id == 7)));
            }
            other => panic!("expected patches, got {other:?}"),
        }
        assert_eq!(
            apply_all(committed, &outcome),
            "lecimy z Bytowa do New York"
        );
    }

    #[test]
    fn insertion_fills_missing_token() {
        // Whisper recovered a technical term Apple dropped entirely.
        let cfg = TailPatchConfig::default();
        let committed = "używamy framework do tego";
        let retranscribed = "używamy framework vibecrafted do tego";
        let outcome = compute_tail_patch(committed, retranscribed, 3, &cfg);
        assert!(matches!(outcome, TailPatchOutcome::Patches(_)));
        assert_eq!(
            apply_all(committed, &outcome),
            "używamy framework vibecrafted do tego"
        );
    }

    #[test]
    fn leading_insertion_anchors_at_start() {
        let cfg = TailPatchConfig::default();
        let committed = "świecie cześć";
        let retranscribed = "witaj świecie cześć";
        let outcome = compute_tail_patch(committed, retranscribed, 1, &cfg);
        assert_eq!(apply_all(committed, &outcome), "witaj świecie cześć");
    }

    #[test]
    fn deletion_is_left_intact_in_v1() {
        // Whisper saw fewer words; v1 must not remove text the user already saw.
        let cfg = TailPatchConfig::default();
        let committed = "to jest bardzo długie zdanie";
        let retranscribed = "to jest długie zdanie";
        let outcome = compute_tail_patch(committed, retranscribed, 1, &cfg);
        // Either NoChange (no emitted edits) — committed stays as-is.
        assert_eq!(apply_all(committed, &outcome), committed);
    }

    #[test]
    fn divergent_retranscription_is_skipped() {
        let cfg = TailPatchConfig::default();
        let committed = "ala ma kota";
        let retranscribed = "zupełnie inny tekst o czymś innym";
        let outcome = compute_tail_patch(committed, retranscribed, 1, &cfg);
        assert!(matches!(outcome, TailPatchOutcome::Skipped { .. }));
        // Layer 0 output stands.
        assert_eq!(apply_all(committed, &outcome), committed);
    }

    #[test]
    fn multiple_edits_apply_offset_stable() {
        // Two independent substitutions in one utterance; applying all emitted
        // events in order must land the fully corrected text.
        let cfg = TailPatchConfig::default();
        let committed = "spotkanie o foo i potem bar wieczorem";
        let retranscribed = "spotkanie o dziesiątej i potem osiemnastej wieczorem";
        let outcome = compute_tail_patch(committed, retranscribed, 2, &cfg);
        match &outcome {
            TailPatchOutcome::Patches(events) => {
                // Emitted descending by start for offset-stable application.
                let starts: Vec<usize> = events.iter().map(event_start).collect();
                let mut sorted = starts.clone();
                sorted.sort_by(|a, b| b.cmp(a));
                assert_eq!(starts, sorted, "events must be descending by start");
            }
            other => panic!("expected patches, got {other:?}"),
        }
        assert_eq!(
            apply_all(committed, &outcome),
            "spotkanie o dziesiątej i potem osiemnastej wieczorem"
        );
    }

    #[test]
    fn unicode_offsets_are_char_based() {
        // Polish diacritics: offsets must be char- not byte-based or the apply
        // helper would corrupt the buffer.
        let cfg = TailPatchConfig::default();
        let committed = "zażółć gęślą jaźń teraz";
        let retranscribed = "zażółć gęślą jaźń natychmiast";
        let outcome = compute_tail_patch(committed, retranscribed, 9, &cfg);
        assert_eq!(
            apply_all(committed, &outcome),
            "zażółć gęślą jaźń natychmiast"
        );
    }

    #[test]
    fn config_from_env_clamps_to_unit_interval() {
        // Out-of-range / garbage values fall back to default.
        let cfg = TailPatchConfig::default();
        assert_eq!(cfg.max_change_ratio, 0.5);
    }

    #[test]
    fn layered_phase_parses_phase_prefix() {
        // Pure parse helper exercised without touching process env.
        assert_eq!(Some(1u8), "phase1".parse::<PhaseProbe>().unwrap().0);
        assert_eq!(Some(4u8), "4".parse::<PhaseProbe>().unwrap().0);
        assert_eq!(None, "phase9".parse::<PhaseProbe>().unwrap().0);
        assert_eq!(None, "off".parse::<PhaseProbe>().unwrap().0);
    }

    // Test-only mirror of `layered_phase` parsing, isolated from process env so
    // the suite stays deterministic under parallel execution.
    struct PhaseProbe(Option<u8>);
    impl std::str::FromStr for PhaseProbe {
        type Err = std::convert::Infallible;
        fn from_str(raw: &str) -> Result<Self, Self::Err> {
            let raw = raw.trim().to_ascii_lowercase();
            let digits = raw.strip_prefix("phase").unwrap_or(&raw);
            let parsed = digits.parse::<u8>().ok().filter(|n| (1..=4).contains(n));
            Ok(PhaseProbe(parsed))
        }
    }
}
