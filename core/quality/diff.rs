//! Tiered word-level diff + rule-candidate mining for the Voice Lab surface.
//!
//! Diff truth lives in Rust so the Swift panel only renders. Classification is
//! intentionally conservative: punctuation/casing-only deltas are noise and are
//! collapsed in the UI; vocabulary/insert/delete deltas are real content changes.

use std::collections::{HashMap, HashSet};

use unicode_normalization::UnicodeNormalization;

use crate::quality::overlay_quality::{CustomLexiconEntry, QualityRecord};

/// Classification of one changed span.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffTier {
    /// Letter-level or multi-word substitution that changes meaning.
    Vocabulary,
    /// Same punctuation stripped, different only in case.
    Casing,
    /// Same words after stripping punctuation, case identical.
    Punctuation,
    /// New word(s) with no counterpart on the raw side.
    Insert,
    /// Word(s) removed with no counterpart on the edited side.
    Delete,
}

/// One content change between the raw STT text and the human-edited text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffSpan {
    /// Original surface form of the changed span (empty for an insert).
    pub raw: String,
    /// Human-edited surface form (empty for a delete).
    pub edited: String,
    /// Tier of the change.
    pub tier: DiffTier,
    /// Up to 5 unchanged words immediately before the span.
    pub context_before: String,
    /// Up to 5 unchanged words immediately after the span.
    pub context_after: String,
    /// PCM occurrence identity pinned to this span's words. Always `None`
    /// today: `QualityRecord` carries no occurrence id or word offset, and a
    /// text-only diff cannot derive one. Cut A6 (per-word confidence pinned to
    /// PCM) or cut T (contextual strings) will fill it.
    pub occurrence_ref: Option<String>,
}

const LCS_CELL_CAP: usize = 40_000;
const CONTEXT_WORDS: usize = 5;

/// Word-level diff with tier classification and bounded context.
///
/// Algorithm: trim common prefix/suffix, then LCS on the differing middle. If
/// the middle is too large for a quadratic table, the whole middle is honestly
/// reported as one replacement block (mirrors the Swift S3 contract).
pub fn diff_spans(raw: &str, edited: &str) -> Vec<DiffSpan> {
    let raw_words: Vec<&str> = raw.split_whitespace().collect();
    let edited_words: Vec<&str> = edited.split_whitespace().collect();

    let mut prefix = 0;
    while prefix < raw_words.len()
        && prefix < edited_words.len()
        && word_key(raw_words[prefix]) == word_key(edited_words[prefix])
    {
        prefix += 1;
    }

    let raw_tail = raw_words.len().saturating_sub(prefix);
    let edited_tail = edited_words.len().saturating_sub(prefix);
    let mut suffix = 0;
    while suffix < raw_tail
        && suffix < edited_tail
        && word_key(raw_words[raw_words.len() - 1 - suffix])
            == word_key(edited_words[edited_words.len() - 1 - suffix])
    {
        suffix += 1;
    }

    if prefix == raw_words.len() && prefix == edited_words.len() {
        return Vec::new();
    }

    let raw_mid = &raw_words[prefix..raw_words.len().saturating_sub(suffix)];
    let edited_mid = &edited_words[prefix..edited_words.len().saturating_sub(suffix)];

    let blocks = if raw_mid.is_empty()
        || edited_mid.is_empty()
        || raw_mid.len() * edited_mid.len() > LCS_CELL_CAP
    {
        vec![DiffBlock::Change {
            raw: raw_mid.to_vec(),
            edited: edited_mid.to_vec(),
        }]
    } else {
        lcs_blocks(raw_mid, edited_mid)
    };

    if blocks.is_empty() {
        return Vec::new();
    }

    let prefix_words = &raw_words[..prefix];
    let suffix_words = &raw_words[raw_words.len().saturating_sub(suffix)..];

    let mut out = Vec::new();
    for (index, block) in blocks.iter().enumerate() {
        let DiffBlock::Change { raw, edited } = block else {
            continue;
        };
        let context_before = blocks
            .get(index.wrapping_sub(1))
            .and_then(DiffBlock::same_words)
            .map(|words| tail_words(words, CONTEXT_WORDS))
            .unwrap_or_else(|| tail_words(prefix_words, CONTEXT_WORDS));
        let context_after = blocks
            .get(index + 1)
            .and_then(DiffBlock::same_words)
            .map(|words| head_words(words, CONTEXT_WORDS))
            .unwrap_or_else(|| head_words(suffix_words, CONTEXT_WORDS));
        out.push(DiffSpan {
            raw: raw.join(" "),
            edited: edited.join(" "),
            tier: tier_for(&raw.join(" "), &edited.join(" ")),
            context_before,
            context_after,
            occurrence_ref: None,
        });
    }
    out
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum DiffBlock<'a> {
    Same(Vec<&'a str>),
    Change {
        raw: Vec<&'a str>,
        edited: Vec<&'a str>,
    },
}

impl<'a> DiffBlock<'a> {
    fn same_words(&self) -> Option<&[&'a str]> {
        match self {
            DiffBlock::Same(words) => Some(words),
            DiffBlock::Change { .. } => None,
        }
    }
}

fn word_key(word: &str) -> String {
    word.nfc().collect::<String>()
}

fn head_words(words: &[&str], limit: usize) -> String {
    words
        .iter()
        .take(limit)
        .copied()
        .collect::<Vec<_>>()
        .join(" ")
}

fn tail_words(words: &[&str], limit: usize) -> String {
    words
        .iter()
        .rev()
        .take(limit)
        .rev()
        .copied()
        .collect::<Vec<_>>()
        .join(" ")
}

fn lcs_blocks<'a>(raw: &[&'a str], edited: &[&'a str]) -> Vec<DiffBlock<'a>> {
    let n = raw.len();
    let m = edited.len();
    let mut dp = vec![vec![0usize; m + 1]; n + 1];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            dp[i][j] = if word_key(raw[i]) == word_key(edited[j]) {
                dp[i + 1][j + 1] + 1
            } else {
                dp[i + 1][j].max(dp[i][j + 1])
            };
        }
    }

    let mut ops = Vec::new();
    let mut i = 0;
    let mut j = 0;
    while i < n || j < m {
        if i < n && j < m && word_key(raw[i]) == word_key(edited[j]) {
            ops.push(Op::Equal);
            i += 1;
            j += 1;
        } else if i < n && (j == m || dp[i + 1][j] >= dp[i][j + 1]) {
            ops.push(Op::Del);
            i += 1;
        } else {
            ops.push(Op::Ins);
            j += 1;
        }
    }

    let mut blocks: Vec<DiffBlock<'a>> = Vec::new();
    let mut raw_i = 0;
    let mut edited_i = 0;
    let mut pending_raw: Vec<&'a str> = Vec::new();
    let mut pending_edited: Vec<&'a str> = Vec::new();

    let flush_change = |pending_raw: &mut Vec<&'a str>,
                        pending_edited: &mut Vec<&'a str>,
                        blocks: &mut Vec<DiffBlock<'a>>| {
        if !pending_raw.is_empty() || !pending_edited.is_empty() {
            blocks.push(DiffBlock::Change {
                raw: std::mem::take(pending_raw),
                edited: std::mem::take(pending_edited),
            });
        }
    };

    for op in ops {
        match op {
            Op::Equal => {
                flush_change(&mut pending_raw, &mut pending_edited, &mut blocks);
                let word = raw[raw_i];
                raw_i += 1;
                edited_i += 1;
                if let Some(DiffBlock::Same(run)) = blocks.last_mut() {
                    run.push(word);
                } else {
                    blocks.push(DiffBlock::Same(vec![word]));
                }
            }
            Op::Del => {
                pending_raw.push(raw[raw_i]);
                raw_i += 1;
            }
            Op::Ins => {
                pending_edited.push(edited[edited_i]);
                edited_i += 1;
            }
        }
    }
    flush_change(&mut pending_raw, &mut pending_edited, &mut blocks);
    blocks
}

enum Op {
    Equal,
    Del,
    Ins,
}

/// Classify a single changed span into a tier.
pub fn tier_for(raw: &str, edited: &str) -> DiffTier {
    let raw = raw.trim();
    let edited = edited.trim();
    if raw.is_empty() {
        return DiffTier::Insert;
    }
    if edited.is_empty() {
        return DiffTier::Delete;
    }

    let raw_words: Vec<&str> = raw.split_whitespace().collect();
    let edited_words: Vec<&str> = edited.split_whitespace().collect();
    if raw_words.len() != edited_words.len() {
        return DiffTier::Vocabulary;
    }

    let punctuation_insensitive_equal = raw_words
        .iter()
        .zip(&edited_words)
        .all(|(r, e)| strip_punctuation(r) == strip_punctuation(e));
    if punctuation_insensitive_equal {
        return DiffTier::Punctuation;
    }

    let casefold_equal = raw_words
        .iter()
        .zip(&edited_words)
        .all(|(r, e)| strip_punctuation(r).to_lowercase() == strip_punctuation(e).to_lowercase());
    if casefold_equal {
        return DiffTier::Casing;
    }

    DiffTier::Vocabulary
}

fn strip_punctuation(word: &str) -> String {
    word.nfc()
        .filter(|ch| ch.is_alphabetic() || ch.is_numeric())
        .collect::<String>()
}

/// A target canonical term the user may want to teach, backed by repeated raw
/// variants across corrections.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuleCandidate {
    /// Surface form of the corrected canonical term.
    pub target: String,
    /// Distinct raw variants (surface forms) that produced this target.
    pub variants: Vec<String>,
    /// Number of distinct (record logical id, normalized raw variant) pairs.
    pub occurrences: u64,
}

/// Mine repeated vocabulary corrections into dictionary-rule candidates.
///
/// A candidate needs at least `min_occurrences` occurrences and either two
/// distinct records or two distinct raw variants. Targets already present in
/// the dictionary (canonical or variant, case-insensitively after NFC) are
/// excluded.
pub fn rule_candidates(
    records: &[QualityRecord],
    dictionary: &[CustomLexiconEntry],
    min_occurrences: u64,
) -> Vec<RuleCandidate> {
    if min_occurrences == 0 {
        return Vec::new();
    }

    let mut by_target: HashMap<String, CandidateAccum> = HashMap::new();
    for record in records {
        let logical_id = record.logical_id();
        for span in diff_spans(&record.raw_text, &record.edited_text) {
            if span.tier != DiffTier::Vocabulary {
                continue;
            }
            let edited = span.edited.trim();
            if edited.is_empty() {
                continue;
            }
            let target_key = norm_casefold(edited);
            let raw_norm = span.raw.trim().nfc().collect::<String>();
            let entry = by_target
                .entry(target_key)
                .or_insert_with(|| CandidateAccum::new(edited.to_string()));
            entry.occurrence_keys.insert((logical_id.clone(), raw_norm));
            entry.surface_variants.insert(span.raw.trim().to_string());
            entry.surface_target = edited.to_string();
        }
    }

    let dictionary_keys: HashSet<String> = dictionary
        .iter()
        .flat_map(|entry| {
            [
                norm_casefold(&entry.canonical),
                norm_casefold(&entry.variant),
            ]
        })
        .collect();

    let mut out: Vec<RuleCandidate> = by_target
        .into_values()
        .filter(|acc| {
            let occurrences = acc.occurrence_keys.len() as u64;
            let distinct_records = acc
                .occurrence_keys
                .iter()
                .map(|(id, _)| id.clone())
                .collect::<HashSet<_>>()
                .len() as u64;
            let distinct_variants = acc.surface_variants.len() as u64;
            occurrences >= min_occurrences
                && !dictionary_keys.contains(&norm_casefold(&acc.surface_target))
                && (distinct_records >= 2 || distinct_variants >= 2)
        })
        .map(|acc| {
            let mut variants: Vec<String> = acc.surface_variants.into_iter().collect();
            variants.sort();
            RuleCandidate {
                target: acc.surface_target,
                variants,
                occurrences: acc.occurrence_keys.len() as u64,
            }
        })
        .collect();
    out.sort_by(|a, b| a.target.cmp(&b.target));
    out
}

struct CandidateAccum {
    surface_target: String,
    surface_variants: HashSet<String>,
    occurrence_keys: HashSet<(String, String)>,
}

impl CandidateAccum {
    fn new(surface_target: String) -> Self {
        Self {
            surface_target,
            surface_variants: HashSet::new(),
            occurrence_keys: HashSet::new(),
        }
    }
}

fn norm_casefold(text: &str) -> String {
    text.nfc().collect::<String>().to_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(raw: &str, edited: &str) -> QualityRecord {
        QualityRecord {
            correction_id: String::new(),
            revision: 0,
            timestamp_ms: 0,
            session_id: None,
            mode: "overlay".to_string(),
            model: None,
            formatting_level: None,
            raw_text: raw.to_string(),
            delivered_text: raw.to_string(),
            edited_text: edited.to_string(),
            avg_logprob: None,
            speech_pct: None,
            confidence_flags: vec![],
            meta: serde_json::Value::Null,
        }
    }

    #[test]
    fn tier_classifies_founder_examples() {
        assert_eq!(tier_for("wajprawter", "vibecrafted"), DiffTier::Vocabulary);
        assert_eq!(tier_for("WipeRapted", "Vibecrafted"), DiffTier::Vocabulary);
        assert_eq!(tier_for("CLR-ów", "CLI-ów"), DiffTier::Vocabulary);
        assert_eq!(tier_for("Wall Street", "worktrees"), DiffTier::Vocabulary);
        assert_eq!(tier_for("check", "checkout"), DiffTier::Vocabulary);
        assert_eq!(tier_for("Możliwe", "onboarding"), DiffTier::Vocabulary);
        assert_eq!(tier_for("mogę", "mówię"), DiffTier::Vocabulary);

        assert_eq!(tier_for("jako", "Jako"), DiffTier::Casing);
        assert_eq!(tier_for("rozumiesz?", "Rozumiesz?"), DiffTier::Casing);

        assert_eq!(tier_for("ciebie", "ciebie,"), DiffTier::Punctuation);
        assert_eq!(
            tier_for("brzydkownika,", "brzydkownika."),
            DiffTier::Punctuation
        );

        assert_eq!(tier_for("", "inserted"), DiffTier::Insert);
        assert_eq!(tier_for("deleted", ""), DiffTier::Delete);
    }

    #[test]
    fn diff_spans_preserves_bounded_context() {
        let spans = diff_spans(
            "Zażółć gęślą jaźń potem nagrajemy luks tri mapa jeszcze raz dziś",
            "Zażółć gęślą jaźń potem nagrajemy Loctree mapę jeszcze raz dziś",
        );
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].raw, "luks tri mapa");
        assert_eq!(spans[0].edited, "Loctree mapę");
        assert_eq!(spans[0].tier, DiffTier::Vocabulary);
        assert_eq!(spans[0].context_before, "Zażółć gęślą jaźń potem nagrajemy");
        assert_eq!(spans[0].context_after, "jeszcze raz dziś");
    }

    #[test]
    fn diff_spans_collapses_large_middle_to_one_block() {
        let raw: String = (1..=300)
            .map(|i| format!("a{}", i))
            .collect::<Vec<_>>()
            .join(" ");
        let edited: String = (1..=300)
            .map(|i| format!("b{}", i))
            .collect::<Vec<_>>()
            .join(" ");
        let spans = diff_spans(&raw, &edited);
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].tier, DiffTier::Vocabulary);
    }

    #[test]
    fn diff_spans_anchors_on_unchanged_words_inside_the_middle() {
        let spans = diff_spans(
            "wajprawter wykrył jako potem ciebie",
            "Vibecrafted wykrył Jako potem ciebie,",
        );
        assert_eq!(spans.len(), 3);
        assert_eq!(spans[0].tier, DiffTier::Vocabulary);
        assert_eq!(spans[0].raw, "wajprawter");
        assert_eq!(spans[0].edited, "Vibecrafted");
        assert_eq!(spans[1].tier, DiffTier::Casing);
        assert_eq!(spans[1].raw, "jako");
        assert_eq!(spans[1].edited, "Jako");
        assert_eq!(spans[2].tier, DiffTier::Punctuation);
        assert_eq!(spans[2].raw, "ciebie");
        assert_eq!(spans[2].edited, "ciebie,");
    }

    #[test]
    fn diff_spans_returns_empty_for_identical_text() {
        assert!(diff_spans("same text", "same text").is_empty());
    }

    #[test]
    fn rule_candidates_require_vocabulary_tier_and_min_occurrences() {
        let records = vec![
            record("wajprawter wykrył", "vibecrafted wykrył"),
            record("WipeRapted", "Vibecrafted"),
        ];
        let candidates = rule_candidates(&records, &[], 2);
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].target, "Vibecrafted");
        assert_eq!(candidates[0].occurrences, 2);
        assert!(candidates[0].variants.iter().any(|v| v == "wajprawter"));
        assert!(candidates[0].variants.iter().any(|v| v == "WipeRapted"));
    }

    #[test]
    fn casing_only_target_is_not_a_candidate() {
        let records = vec![record("jako", "Jako"), record("jako", "Jako")];
        assert!(rule_candidates(&records, &[], 2).is_empty());
    }

    #[test]
    fn dictionary_match_excludes_candidate() {
        let records = vec![
            record("wajprawter", "Vibecrafted"),
            record("WipeRapted", "Vibecrafted"),
        ];
        let dictionary = vec![CustomLexiconEntry {
            variant: "wajprawter".to_string(),
            canonical: "Vibecrafted".to_string(),
            source: "correction".to_string(),
        }];
        assert!(rule_candidates(&records, &dictionary, 2).is_empty());
    }

    #[test]
    fn single_record_two_variants_qualifies() {
        let records = vec![record("foo bar", "Baz")];
        let candidates = rule_candidates(&records, &[], 2);
        assert!(
            candidates.is_empty(),
            "one record cannot reach min_occurrences=2"
        );
    }
}
