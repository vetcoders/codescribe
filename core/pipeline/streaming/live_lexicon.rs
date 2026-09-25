//! Lexical labels for one PCM occurrence. This module changes text only; the
//! acoustic ledger remains the authority for identity, admission, and seals.

use std::collections::HashSet;
use std::fs;
use std::path::Path;
use std::sync::LazyLock;

use regex::RegexBuilder;
use serde::Deserialize;

const SEED: &[u8] = include_bytes!("../../../assets/seed.jsonl");
const PROGRAMMING: &[u8] = include_bytes!("../../../assets/programming.jsonl");
const PROTECTED: &[u8] = include_bytes!("../../../assets/protected_terms.jsonl");

#[derive(Deserialize)]
struct Normalization {
    #[serde(default = "enabled_by_default")]
    enabled: bool,
    #[serde(default)]
    input_variants: Vec<String>,
    #[serde(default)]
    case_sensitive: bool,
}

fn enabled_by_default() -> bool {
    true
}

#[derive(Deserialize)]
struct SeedRow {
    canonical: String,
    normalization: Normalization,
}

#[derive(Default, Deserialize)]
struct Extras {
    #[serde(default)]
    mispronunciations: Vec<String>,
}

#[derive(Deserialize)]
struct TermRow {
    term: String,
    #[serde(default)]
    mispronunciations: Vec<String>,
    #[serde(default)]
    extras: Option<Extras>,
}

struct Rule {
    variant: String,
    lower_variant: String,
    canonical: String,
    case_sensitive: bool,
    protected: bool,
}

impl Rule {
    fn new(
        variant: String,
        canonical: String,
        case_sensitive: bool,
        protected: bool,
    ) -> Option<Self> {
        let variant = variant.trim().to_string();
        let canonical = canonical.trim().to_string();
        if variant.is_empty() || canonical.is_empty() || variant == canonical {
            return None;
        }
        Some(Self {
            lower_variant: variant.to_lowercase(),
            variant,
            canonical,
            case_sensitive,
            protected,
        })
    }

    fn pattern(&self) -> Option<regex::Regex> {
        let words = self
            .variant
            .split_whitespace()
            .map(regex::escape)
            .collect::<Vec<_>>();
        let pattern = words.join(r"\s+");
        RegexBuilder::new(&pattern)
            .case_insensitive(!self.case_sensitive)
            .build()
            .ok()
    }
}

struct Bundled {
    rules: Vec<Rule>,
    protected_canonicals: Vec<String>,
}

static BUNDLED: LazyLock<Bundled> =
    LazyLock::new(|| {
        let mut rules = Vec::new();
        let mut protected_canonicals = Vec::new();
        let seed = std::str::from_utf8(SEED).expect("embedded seed UTF-8");
        for line in seed.lines() {
            if let Ok(row) = serde_json::from_str::<SeedRow>(line)
                && row.normalization.enabled
            {
                rules.extend(
                    row.normalization
                        .input_variants
                        .into_iter()
                        .filter_map(|variant| {
                            Rule::new(
                                variant,
                                row.canonical.clone(),
                                row.normalization.case_sensitive,
                                false,
                            )
                        }),
                );
            }
        }
        for (source, protected) in [(PROGRAMMING, false), (PROTECTED, true)] {
            let content = std::str::from_utf8(source).expect("embedded lexicon UTF-8");
            for line in content.lines() {
                if let Ok(mut row) = serde_json::from_str::<TermRow>(line) {
                    if protected && !protected_canonicals.contains(&row.term) {
                        protected_canonicals.push(row.term.clone());
                    }
                    row.mispronunciations
                        .extend(row.extras.unwrap_or_default().mispronunciations);
                    rules.extend(row.mispronunciations.into_iter().filter_map(|variant| {
                        Rule::new(variant, row.term.clone(), false, protected)
                    }));
                }
            }
        }
        Bundled {
            rules,
            protected_canonicals,
        }
    });

fn custom_rules(path: &Path) -> Vec<Rule> {
    let Ok(content) = fs::read_to_string(path) else {
        return Vec::new();
    };
    let mut rules = Vec::new();
    for line in content.lines() {
        if let Ok(mut row) = serde_json::from_str::<TermRow>(line) {
            row.mispronunciations
                .extend(row.extras.unwrap_or_default().mispronunciations);
            rules.extend(
                row.mispronunciations
                    .into_iter()
                    .filter_map(|variant| Rule::new(variant, row.term.clone(), false, false)),
            );
        }
    }
    rules
}

#[derive(Clone, Copy, Debug, Default)]
pub(super) struct LexiconCounts {
    pub custom: usize,
}

struct Candidate<'a> {
    start: usize,
    end: usize,
    rule: &'a Rule,
    rank: u8,
}

fn whole_word_match(text: &str, start: usize, end: usize) -> bool {
    let word_char = |ch: char| ch.is_alphanumeric() || ch == '_';
    !text[..start].chars().next_back().is_some_and(word_char)
        && !text[end..].chars().next().is_some_and(word_char)
}

fn rewrite_once(text: &str, custom: &[Rule], bundled: &Bundled) -> String {
    let lower = text.to_lowercase();
    let mut protected_spans = Vec::new();
    for canonical in &bundled.protected_canonicals {
        if !lower.contains(&canonical.to_lowercase()) {
            continue;
        }
        let pattern = regex::escape(canonical);
        if let Ok(pattern) = RegexBuilder::new(&pattern).case_insensitive(true).build() {
            protected_spans.extend(
                pattern
                    .find_iter(text)
                    .filter(|found| whole_word_match(text, found.start(), found.end()))
                    .map(|found| (found.start(), found.end(), canonical)),
            );
        }
    }

    let mut candidates = Vec::new();
    let mut seen = HashSet::new();
    for (rank, rule) in custom.iter().map(|rule| (0, rule)).chain(
        bundled
            .rules
            .iter()
            .map(|rule| (if rule.protected { 1 } else { 2 }, rule)),
    ) {
        // A cheap filter avoids compiling and searching thousands of patterns
        // that cannot occur in this one short label. Internal spaces are matched
        // by the regex below, including runs of whitespace.
        let first = rule.lower_variant.split_whitespace().next().unwrap_or("");
        if first.is_empty() || !lower.contains(first) {
            continue;
        }
        let Some(pattern) = rule.pattern() else {
            continue;
        };
        for found in pattern.find_iter(text) {
            if !whole_word_match(text, found.start(), found.end()) {
                continue;
            }
            if text[found.range()] == rule.canonical {
                continue;
            }
            if protected_spans.iter().any(|(start, end, canonical)| {
                found.start() < *end && *start < found.end() && canonical.as_str() != rule.canonical
            }) {
                continue;
            }
            if seen.insert((found.start(), found.end(), rule.canonical.as_str())) {
                candidates.push(Candidate {
                    start: found.start(),
                    end: found.end(),
                    rule,
                    rank,
                });
            }
        }
    }
    candidates.sort_by(|left, right| {
        left.start
            .cmp(&right.start)
            .then_with(|| (right.end - right.start).cmp(&(left.end - left.start)))
            .then_with(|| left.rank.cmp(&right.rank))
    });
    let mut result = String::with_capacity(text.len());
    let mut cursor = 0;
    for candidate in candidates {
        if candidate.start < cursor {
            continue;
        }
        result.push_str(&text[cursor..candidate.start]);
        result.push_str(&candidate.rule.canonical);
        cursor = candidate.end;
    }
    result.push_str(&text[cursor..]);
    result
}

/// Rewrite only registered whole-word variants. A conflicting rule chain is
/// refused so a second pass cannot mutate the resulting canonical label.
pub(super) fn rewrite(text: &str, custom_path: &Path) -> (String, LexiconCounts) {
    let bundled = &*BUNDLED;
    let custom = custom_rules(custom_path);
    let counts = LexiconCounts {
        custom: custom.len(),
    };
    let first = rewrite_once(text, &custom, bundled);
    if first == text || rewrite_once(&first, &custom, bundled) == first {
        (first, counts)
    } else {
        (text.to_string(), counts)
    }
}

pub(super) fn bundled_count() -> usize {
    BUNDLED.rules.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_rules_preserve_ordinary_polish_words() {
        let absent = Path::new("/nonexistent/codescribe-lexicon-test.jsonl");
        for text in [
            "następny krok",
            "grupa",
            "wraz z człowiekiem",
            "ten PR",
            "huk",
            "kontrola",
            "pull request",
        ] {
            assert_eq!(rewrite(text, absent).0, text, "input: {text}");
        }
        for (input, expected) in [("rast", "Rust"), ("postgres", "PostgreSQL")] {
            assert_eq!(rewrite(input, absent).0, expected, "input: {input}");
        }
    }

    #[test]
    fn every_bundled_row_parses_without_a_canonical_noop() {
        let seed = std::str::from_utf8(SEED).unwrap();
        for (index, line) in seed.lines().enumerate() {
            let row: SeedRow = serde_json::from_str(line)
                .unwrap_or_else(|error| panic!("seed row {}: {error}", index + 1));
            assert!(
                row.normalization
                    .input_variants
                    .iter()
                    .all(|variant| variant != &row.canonical),
                "seed row {}: {}",
                index + 1,
                row.canonical
            );
        }
        for (name, source) in [("programming", PROGRAMMING), ("protected", PROTECTED)] {
            let content = std::str::from_utf8(source).unwrap();
            for (index, line) in content.lines().enumerate() {
                let row: TermRow = serde_json::from_str(line)
                    .unwrap_or_else(|error| panic!("{name} row {}: {error}", index + 1));
                let variants = row
                    .mispronunciations
                    .iter()
                    .chain(row.extras.iter().flat_map(|extras| &extras.mispronunciations));
                assert!(
                    variants.into_iter().all(|variant| variant != &row.term),
                    "{name} row {}: {}",
                    index + 1,
                    row.term
                );
            }
        }
    }

    #[test]
    fn rewrite_is_idempotent_and_conserves_surrounding_words() {
        let absent = Path::new("/nonexistent/codescribe-lexicon-test.jsonl");
        let input = "Przed accepromazyna, po schowek i zaznaczenie.";
        let (once, _) = rewrite(input, absent);
        assert_eq!(once, "Przed Acepromazyna, po schowek i zaznaczenie.");
        assert_eq!(rewrite(&once, absent).0, once);
        assert_eq!(rewrite("xacc epromazyna", absent).0, "xacc epromazyna");
    }

    #[test]
    fn protected_canonical_survives_a_conflicting_custom_rule() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("lexicon.custom.jsonl");
        fs::write(
            &path,
            "{\"term\":\"WrongName\",\"mispronunciations\":[\"Loctree\"]}\n",
        )
        .unwrap();
        let (label, counts) = rewrite("Loctree", &path);
        assert_eq!(label, "Loctree");
        assert_eq!(counts.custom, 1);
        assert!(bundled_count() > 0);
    }

    #[test]
    fn longest_custom_variant_wins_and_is_stable() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("lexicon.custom.jsonl");
        fs::write(
            &path,
            "{\"term\":\"Short\",\"mispronunciations\":[\"lux\"]}\n{\"term\":\"Long\",\"mispronunciations\":[\"lux tree\"]}\n",
        )
        .unwrap();
        let (label, _) = rewrite("The lux tree works", &path);
        assert_eq!(label, "The Long works");
        assert_eq!(rewrite(&label, &path).0, label);
    }

    #[test]
    fn punctuation_variant_respects_both_word_edges() {
        let rules = [Rule::new("C++".into(), "Cpp".into(), false, false).unwrap()];
        let bundled = Bundled {
            rules: Vec::new(),
            protected_canonicals: Vec::new(),
        };
        assert_eq!(
            rewrite_once("C++ C++code xC++", &rules, &bundled),
            "Cpp C++code xC++"
        );
    }
}
