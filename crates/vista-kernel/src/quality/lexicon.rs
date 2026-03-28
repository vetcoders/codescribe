//! Unified lexicon loader and transcript corrector for domain-specific STT quality.
//!
//! Parses three JSONL formats:
//! - `programming.jsonl` — `{term, mispronunciations[], category}`
//! - `veterinary.jsonl` — `{term, extras.mispronunciations[], synonyms[], definition}`
//! - `seed.jsonl` — `{canonical, normalization.input_variants[], knowledge.*}`
//!
//! Created by M&K (c)2026 VetCoders

use regex::Regex;
use serde::Deserialize;
use std::path::Path;
use tracing::{debug, warn};

// Embedded JSONL sources — always available as baseline.
const PROGRAMMING_JSONL: &str = include_str!("../../../../assets/programming.jsonl");
const VETERINARY_JSONL: &str = include_str!("../../../../assets/veterinary.jsonl");
const SEED_JSONL: &str = include_str!("../../../../assets/seed.jsonl");

/// A compiled correction rule: regex pattern -> canonical replacement.
#[derive(Debug)]
pub struct LexiconRule {
    pub pattern: Regex,
    pub replacement: String,
}

/// Holds all loaded lexicon rules from every source.
#[derive(Debug)]
pub struct Lexicon {
    rules: Vec<LexiconRule>,
    pub programming_count: usize,
    pub veterinary_count: usize,
    pub seed_count: usize,
}

// ---------- Deserialization structs for the three JSONL formats ----------

/// programming.jsonl / veterinary.jsonl top-level extras block
#[derive(Debug, Deserialize)]
struct LegacyExtras {
    #[serde(default)]
    mispronunciations: Vec<String>,
}

/// programming.jsonl and veterinary.jsonl share this shape
#[derive(Debug, Deserialize)]
struct LegacyEntry {
    term: String,
    #[serde(default)]
    mispronunciations: Vec<String>,
    #[serde(default)]
    extras: Option<LegacyExtras>,
}

/// seed.jsonl normalization block
#[derive(Debug, Deserialize)]
struct SeedNormalization {
    #[serde(default)]
    input_variants: Vec<String>,
    #[serde(default)]
    enabled: bool,
    #[serde(default)]
    case_sensitive: bool,
    #[serde(default)]
    whole_word_only: bool,
}

impl Default for SeedNormalization {
    fn default() -> Self {
        Self {
            input_variants: Vec::new(),
            enabled: true,
            case_sensitive: false,
            whole_word_only: true,
        }
    }
}

/// seed.jsonl entry
#[derive(Debug, Deserialize)]
struct SeedEntry {
    canonical: String,
    #[serde(default)]
    normalization: SeedNormalization,
}

impl Lexicon {
    /// Load all three embedded JSONL lexicons.
    pub fn from_embedded() -> Self {
        let mut rules = Vec::new();

        let programming_count = load_legacy_jsonl(PROGRAMMING_JSONL, "programming", &mut rules);
        let veterinary_count = load_legacy_jsonl(VETERINARY_JSONL, "veterinary", &mut rules);
        let seed_count = load_seed_jsonl(SEED_JSONL, "seed", &mut rules);

        debug!(
            "Lexicon loaded: {} rules (programming={}, veterinary={}, seed={})",
            rules.len(),
            programming_count,
            veterinary_count,
            seed_count
        );

        Self {
            rules,
            programming_count,
            veterinary_count,
            seed_count,
        }
    }

    /// Load from custom file paths instead of embedded data.
    /// Any path that is `None` falls back to embedded.
    pub fn from_paths(
        programming: Option<&Path>,
        veterinary: Option<&Path>,
        seed: Option<&Path>,
    ) -> Self {
        let mut rules = Vec::new();

        let prog_source = programming
            .and_then(|p| read_file_or_warn(p, "programming"))
            .unwrap_or_else(|| PROGRAMMING_JSONL.to_string());
        let vet_source = veterinary
            .and_then(|p| read_file_or_warn(p, "veterinary"))
            .unwrap_or_else(|| VETERINARY_JSONL.to_string());
        let seed_source = seed
            .and_then(|p| read_file_or_warn(p, "seed"))
            .unwrap_or_else(|| SEED_JSONL.to_string());

        let programming_count = load_legacy_jsonl(&prog_source, "programming", &mut rules);
        let veterinary_count = load_legacy_jsonl(&vet_source, "veterinary", &mut rules);
        let seed_count = load_seed_jsonl(&seed_source, "seed", &mut rules);

        debug!(
            "Lexicon loaded from paths: {} rules (programming={}, veterinary={}, seed={})",
            rules.len(),
            programming_count,
            veterinary_count,
            seed_count
        );

        Self {
            rules,
            programming_count,
            veterinary_count,
            seed_count,
        }
    }

    /// Total number of compiled rules.
    pub fn rule_count(&self) -> usize {
        self.rules.len()
    }

    /// Apply all lexicon rules to a transcript, returning the corrected text.
    pub fn apply(&self, text: &str) -> String {
        let mut out = text.to_string();
        for rule in &self.rules {
            if rule.pattern.is_match(&out) {
                out = rule
                    .pattern
                    .replace_all(&out, rule.replacement.as_str())
                    .to_string();
            }
        }
        out
    }
}

use std::sync::LazyLock;

static GLOBAL_LEXICON: LazyLock<Lexicon> = LazyLock::new(|| Lexicon::from_embedded());

/// Convenience function: load embedded lexicons + apply to transcript in one call.
pub fn apply_lexicons(transcript: String) -> String {
    GLOBAL_LEXICON.apply(&transcript)
}

// ---------- Internal loading ----------

fn load_legacy_jsonl(source: &str, label: &str, rules: &mut Vec<LexiconRule>) -> usize {
    let mut added = 0usize;
    for (idx, line) in source.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        let entry: LegacyEntry = match serde_json::from_str(line) {
            Ok(e) => e,
            Err(e) => {
                warn!("Lexicon {}: line {} parse error: {}", label, idx + 1, e);
                continue;
            }
        };

        let mut all_mis = entry.mispronunciations;
        if let Some(extras) = entry.extras {
            all_mis.extend(extras.mispronunciations);
        }

        for mis in &all_mis {
            if mis.eq_ignore_ascii_case(&entry.term) {
                continue;
            }
            if let Some(pattern) = build_word_regex(mis) {
                rules.push(LexiconRule {
                    pattern,
                    replacement: entry.term.clone(),
                });
                added += 1;
            }
        }
    }
    added
}

fn load_seed_jsonl(source: &str, label: &str, rules: &mut Vec<LexiconRule>) -> usize {
    let mut added = 0usize;
    for (idx, line) in source.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        let entry: SeedEntry = match serde_json::from_str(line) {
            Ok(e) => e,
            Err(e) => {
                warn!("Lexicon {}: line {} parse error: {}", label, idx + 1, e);
                continue;
            }
        };

        if !entry.normalization.enabled {
            continue;
        }

        for variant in &entry.normalization.input_variants {
            if variant.eq_ignore_ascii_case(&entry.canonical) {
                continue;
            }
            let pattern = if entry.normalization.whole_word_only {
                build_word_regex(variant)
            } else {
                build_plain_regex(variant, entry.normalization.case_sensitive)
            };
            if let Some(pattern) = pattern {
                rules.push(LexiconRule {
                    pattern,
                    replacement: entry.canonical.clone(),
                });
                added += 1;
            }
        }
    }
    added
}

/// Build a case-insensitive word-boundary regex from a mispronunciation string.
/// Spaces in the input become `\s+` to match flexible whitespace.
fn build_word_regex(input: &str) -> Option<Regex> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return None;
    }
    let escaped = regex::escape(trimmed);
    // regex::escape preserves literal spaces — replace them with \s+
    let flexible = escaped.replace(' ', r"\s+");
    let pattern = format!(r"(?i)\b{flexible}\b");
    Regex::new(&pattern).ok()
}

/// Build a regex without word boundaries (for non-whole-word matching).
fn build_plain_regex(input: &str, case_sensitive: bool) -> Option<Regex> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return None;
    }
    let escaped = regex::escape(trimmed);
    let flexible = escaped.replace(' ', r"\s+");
    let pattern = if case_sensitive {
        flexible
    } else {
        format!("(?i){flexible}")
    };
    Regex::new(&pattern).ok()
}

fn read_file_or_warn(path: &Path, label: &str) -> Option<String> {
    match std::fs::read_to_string(path) {
        Ok(content) => Some(content),
        Err(e) => {
            warn!(
                "Lexicon {}: failed to read {}: {} — falling back to embedded",
                label,
                path.display(),
                e
            );
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_programming_jsonl_parses() {
        let mut rules = Vec::new();
        let count = load_legacy_jsonl(PROGRAMMING_JSONL, "programming", &mut rules);
        assert!(count > 0, "programming.jsonl should produce rules");
        assert!(count > 100, "expected 100+ programming rules, got {count}");
    }

    #[test]
    fn test_veterinary_jsonl_parses() {
        let mut rules = Vec::new();
        let count = load_legacy_jsonl(VETERINARY_JSONL, "veterinary", &mut rules);
        assert!(count > 0, "veterinary.jsonl should produce rules");
        assert!(count > 1000, "expected 1000+ veterinary rules, got {count}");
    }

    #[test]
    fn test_seed_jsonl_parses() {
        let mut rules = Vec::new();
        let count = load_seed_jsonl(SEED_JSONL, "seed", &mut rules);
        assert!(count > 0, "seed.jsonl should produce rules");
        assert!(count > 1000, "expected 1000+ seed rules, got {count}");
    }

    #[test]
    fn test_from_embedded_loads_all_sources() {
        let lex = Lexicon::from_embedded();
        assert!(lex.programming_count > 0);
        assert!(lex.veterinary_count > 0);
        assert!(lex.seed_count > 0);
        assert_eq!(
            lex.rule_count(),
            lex.programming_count + lex.veterinary_count + lex.seed_count
        );
    }

    #[test]
    fn test_apply_programming_correction() {
        let lex = Lexicon::from_embedded();
        // "doker" -> "Docker" (from programming.jsonl)
        let result = lex.apply("uruchom doker compose");
        assert!(
            result.contains("Docker"),
            "expected 'doker' -> 'Docker', got: {result}"
        );
    }

    #[test]
    fn test_apply_veterinary_correction() {
        let lex = Lexicon::from_embedded();
        // veterinary.jsonl has extras.mispronunciations for many terms
        // Acepromazyna has mispronunciation "aacepromazyna"
        let result = lex.apply("podaj aacepromazyna pacjentowi");
        assert!(
            result.contains("Acepromazyna"),
            "expected 'aacepromazyna' -> 'Acepromazyna', got: {result}"
        );
    }

    #[test]
    fn test_apply_seed_correction() {
        let lex = Lexicon::from_embedded();
        // seed.jsonl: Acepromazyna has input_variant "accepromazyna"
        let result = lex.apply("dawka accepromazyna jest za wysoka");
        assert!(
            result.contains("Acepromazyna"),
            "expected 'accepromazyna' -> 'Acepromazyna', got: {result}"
        );
    }

    #[test]
    fn test_apply_lexicons_convenience() {
        let result = apply_lexicons("uruchom doker build".to_string());
        assert!(
            result.contains("Docker"),
            "apply_lexicons should correct 'doker' -> 'Docker': {result}"
        );
    }

    #[test]
    fn test_apply_preserves_clean_text() {
        let lex = Lexicon::from_embedded();
        let clean = "To jest normalne zdanie bez literówek";
        assert_eq!(lex.apply(clean), clean);
    }

    #[test]
    fn test_case_insensitive_matching() {
        let lex = Lexicon::from_embedded();
        let result = lex.apply("użyj DOKER do deploymentu");
        assert!(
            result.contains("Docker"),
            "case-insensitive match failed: {result}"
        );
    }

    #[test]
    fn test_build_word_regex_empty() {
        assert!(build_word_regex("").is_none());
        assert!(build_word_regex("   ").is_none());
    }

    #[test]
    fn test_build_word_regex_with_spaces() {
        let re = build_word_regex("api ki").expect("should build regex");
        assert!(re.is_match("wpisz api ki tutaj"));
        assert!(re.is_match("wpisz api  ki tutaj")); // flexible whitespace
    }

    #[test]
    fn test_from_paths_falls_back_to_embedded() {
        // Non-existent paths should fall back to embedded
        let lex = Lexicon::from_paths(
            Some(Path::new("/nonexistent/programming.jsonl")),
            None,
            None,
        );
        assert!(lex.programming_count > 0, "should fall back to embedded");
        assert!(lex.veterinary_count > 0);
        assert!(lex.seed_count > 0);
    }

    #[test]
    fn test_multiple_corrections_in_single_text() {
        let lex = Lexicon::from_embedded();
        let result = lex.apply("uruchom doker i sprawdź githab");
        assert!(
            result.contains("Docker"),
            "should correct 'doker': {result}"
        );
        assert!(
            result.contains("GitHub"),
            "should correct 'githab': {result}"
        );
    }
}
