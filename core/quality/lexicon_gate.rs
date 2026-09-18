//! Admission gate between extracted correction pairs and the live lexicon.
//!
//! WHY THIS EXISTS. `extract_lexicon_candidates` answers "what changed between
//! the delivered and the edited text". That is an alignment question, and a
//! correct answer to it is still frequently a terrible lexicon rule. Replaying
//! the operator's own `corrections.jsonl` (2026-09-18, 242 pairs / 163 unique)
//! produced, alongside real mishearings like `grypa -> grepa`, three classes
//! that must never reach a substitution table:
//!
//! - grammar inflections (`nasz -> nasza`, `chciałby -> chciałbym`): a rule
//!   keyed on a common word rewrites every future occurrence of it;
//! - reversed product names (`Codex -> Kodeks`, `Loctree -> LogTree`): the
//!   variant side is the correct term and the rule would destroy it;
//! - whole-phrase rewrites (`April -> jako pierwszy etap`): alignment bleed
//!   from a sentence edit, never a pronunciation fact.
//!
//! The gate is deliberately three-tier rather than binary. A pair that is
//! merely *suspicious* (keyed on a common word, or carrying a dangling
//! function word from the alignment) is quarantined for review instead of
//! being thrown away: the operator promotes it by hand. Only the accepted tier
//! is ever written to the live lexicon.
//!
//! Contradiction and ambiguity are properties of the *set*, not of a pair, so
//! the entry point takes the whole batch.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};

use serde::Serialize;

/// Minimum shared prefix, in chars, before a single-token pair can be read as
/// an inflection of one stem rather than two different words.
const INFLECTION_MIN_STEM_CHARS: usize = 4;
/// The shared prefix must also cover this fraction of the shorter token, so
/// `kromu -> Chrome` (prefix 0) never reads as an inflection.
const INFLECTION_MIN_STEM_RATIO: f64 = 0.7;
/// Longest differing tail on either side of an inflection pair. Polish endings
/// are short; a longer tail means the words diverge, not decline.
const INFLECTION_MAX_TAIL_CHARS: usize = 3;
/// Tokens per side. A rule keyed on more words than this never matches twice.
const MAX_TOKENS_PER_SIDE: usize = 3;
/// Allowed difference in token count between the two sides. Beyond this the
/// "correction" added or dropped content instead of respelling it.
const MAX_TOKEN_COUNT_DELTA: usize = 1;
/// Normalized char edit distance above which the two sides cannot plausibly be
/// the same utterance heard twice (`film -> chwilą` scores 0.83).
const MAX_PHONETIC_DISTANCE: f64 = 0.55;

/// Terms this project must never be taught to spell away from. The variant
/// side of a pair matching one of these is the *correct* form, so the rule
/// would corrupt it. Operators extend the list through
/// `<config_dir>/protected_terms.txt`.
const DEFAULT_PROTECTED_TERMS: &[&str] = &[
    "anthropic",
    "apple",
    "bash",
    "claude",
    "codescribe",
    "codex",
    "docker",
    "fork",
    "github",
    "grok",
    "junie",
    "launch",
    "loctree",
    "merge",
    "opencode",
    "oauth",
    "pansieve",
    "python",
    "rust",
    "silero",
    "vetcoders",
    "vibecrafted",
    "whisper",
    "windows",
    "workflux",
    "worktree",
];

/// High-frequency PL/EN words. A single-token rule keyed on one of these fires
/// on nearly every utterance, so such a pair is quarantined rather than
/// applied. Multi-token variants containing one of these stay eligible.
///
/// This is the wider sibling of `overlay_quality::is_function_word`: that list
/// rejects outright at extraction time, this one only diverts to review, so a
/// genuine domain term that happens to be a common word (`mecz` vs `merge`)
/// stays recoverable by hand.
const COMMON_WORDS: &[&str] = &[
    "a",
    "aby",
    "ale",
    "an",
    "and",
    "are",
    "asset",
    "być",
    "bo",
    "by",
    "chciał",
    "chciałby",
    "chciałbym",
    "chwila",
    "chwilę",
    "co",
    "czego",
    "czemu",
    "czy",
    "czym",
    "dać",
    "daje",
    "dlaczego",
    "do",
    "dobra",
    "dobre",
    "dobry",
    "dodaje",
    "dotyczy",
    "film",
    "gdzie",
    "gitara",
    "go",
    "i",
    "ich",
    "im",
    "in",
    "is",
    "it",
    "itd",
    "itp",
    "ja",
    "jak",
    "jakby",
    "jako",
    "jedna",
    "jedno",
    "jeden",
    "jego",
    "jej",
    "jest",
    "jestem",
    "jesteś",
    "już",
    "kiedy",
    "kim",
    "kogo",
    "komu",
    "ma",
    "mam",
    "może",
    "można",
    "mi",
    "mnie",
    "mów",
    "mówi",
    "mówimy",
    "mówić",
    "my",
    "na",
    "nam",
    "nas",
    "nasz",
    "nasza",
    "nasze",
    "nie",
    "no",
    "np",
    "o",
    "od",
    "of",
    "on",
    "ona",
    "one",
    "oni",
    "ono",
    "or",
    "oraz",
    "pisać",
    "pisze",
    "piwo",
    "play",
    "po",
    "prawidłowo",
    "prawidłowy",
    "raz",
    "sam",
    "sama",
    "same",
    "sami",
    "samo",
    "się",
    "ski",
    "skąd",
    "sprawdź",
    "sprawdzić",
    "stać",
    "stanie",
    "stało",
    "są",
    "swoja",
    "swoje",
    "swój",
    "ta",
    "tak",
    "taka",
    "taki",
    "takich",
    "takie",
    "tam",
    "te",
    "tego",
    "tej",
    "ten",
    "the",
    "to",
    "trzeba",
    "trzy",
    "tu",
    "ty",
    "tych",
    "tym",
    "też",
    "w",
    "wam",
    "was",
    "wasz",
    "wiem",
    "wiemy",
    "wróci",
    "wrócić",
    "wróć",
    "wy",
    "z",
    "za",
    "zerknij",
    "że",
    "żeby",
];

/// What the gate decided about one candidate pair.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "tier", rename_all = "snake_case")]
pub enum LexiconVerdict {
    /// Safe to write to the live lexicon.
    Accept,
    /// Plausible but unsafe to apply unattended; quarantined for the operator.
    Review { reason: ReviewReason },
    /// Never applied.
    Reject { reason: RejectReason },
}

impl LexiconVerdict {
    /// True only for the tier `--apply` is allowed to write.
    pub fn is_accepted(&self) -> bool {
        matches!(self, LexiconVerdict::Accept)
    }

    /// Stable lowercase label for reports and JSONL rows.
    pub fn label(&self) -> &'static str {
        match self {
            LexiconVerdict::Accept => "accept",
            LexiconVerdict::Review { reason } => reason.label(),
            LexiconVerdict::Reject { reason } => reason.label(),
        }
    }
}

/// Why a pair was quarantined instead of applied.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ReviewReason {
    /// The variant is a bare high-frequency word; the rule would fire constantly.
    CommonWordVariant,
    /// The canonical carries a trailing function word the variant does not —
    /// alignment bleed from the neighbouring sentence.
    DanglingFunctionWord,
}

impl ReviewReason {
    /// Stable label used in reports and JSONL rows.
    pub fn label(&self) -> &'static str {
        match self {
            ReviewReason::CommonWordVariant => "review:common-word",
            ReviewReason::DanglingFunctionWord => "review:dangling-function-word",
        }
    }
}

/// Why a pair may never reach the lexicon.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum RejectReason {
    /// The reverse pair is also present; applying both would oscillate.
    Contradiction,
    /// The same variant is taught two different canonicals.
    AmbiguousVariant,
    /// The variant side is a protected term — the rule would destroy it.
    ProtectedVariant,
    /// One stem, two grammatical endings. Grammar is the formatter's job.
    Inflection,
    /// Token counts diverge, or a side is longer than a keyable phrase.
    PhraseRewrite,
    /// Too far apart in characters to be the same utterance heard twice.
    NotPhonetic,
}

impl RejectReason {
    /// Stable label used in reports and JSONL rows.
    pub fn label(&self) -> &'static str {
        match self {
            RejectReason::Contradiction => "reject:contradiction",
            RejectReason::AmbiguousVariant => "reject:ambiguous-variant",
            RejectReason::ProtectedVariant => "reject:protected-variant",
            RejectReason::Inflection => "reject:inflection",
            RejectReason::PhraseRewrite => "reject:phrase-rewrite",
            RejectReason::NotPhonetic => "reject:not-phonetic",
        }
    }
}

/// The protected vocabulary in force for one adjudication pass.
#[derive(Debug, Clone, Default)]
pub struct ProtectedTerms {
    folded: BTreeSet<String>,
}

impl ProtectedTerms {
    /// Built-in project vocabulary only — the shape used by tests and by any
    /// caller that must not depend on the operator's config directory.
    pub fn builtin() -> Self {
        Self {
            folded: DEFAULT_PROTECTED_TERMS.iter().map(|t| fold(t)).collect(),
        }
    }

    /// Built-in vocabulary plus one term per line from `path`, if it exists.
    /// Blank lines and `#` comments are skipped. A missing or unreadable file
    /// is not an error: the built-in list is the floor, never the ceiling.
    pub fn load_from(path: &Path) -> Self {
        let mut terms = Self::builtin();
        let Ok(text) = std::fs::read_to_string(path) else {
            return terms;
        };
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            terms.folded.insert(fold(line));
        }
        terms
    }

    /// Conventional location of the operator-editable list.
    pub fn default_path(config_dir: &Path) -> PathBuf {
        config_dir.join("protected_terms.txt")
    }

    /// True when the whole phrase, casefolded, is a protected term.
    pub fn contains(&self, term: &str) -> bool {
        self.folded.contains(&fold(term))
    }

    /// How many terms are in force, built-ins included.
    pub fn len(&self) -> usize {
        self.folded.len()
    }

    /// True when not even the built-ins are present (only via `Default`).
    pub fn is_empty(&self) -> bool {
        self.folded.is_empty()
    }
}

/// Adjudicate a whole candidate batch.
///
/// Returns one verdict per input pair, in input order. Contradiction and
/// ambiguity are decided across the batch, so the same pair can be accepted in
/// isolation and rejected here — that is the point of taking the set.
pub fn adjudicate_lexicon_candidates(
    pairs: &[(String, String)],
    protected: &ProtectedTerms,
) -> Vec<LexiconVerdict> {
    let directed: HashSet<(String, String)> = pairs
        .iter()
        .map(|(v, c)| (fold(v), fold(c)))
        .filter(|(v, c)| v != c)
        .collect();

    let mut canonicals_per_variant: HashMap<String, HashSet<String>> = HashMap::new();
    for (variant, canonical) in pairs {
        canonicals_per_variant
            .entry(fold(variant))
            .or_default()
            .insert(fold(canonical));
    }

    pairs
        .iter()
        .map(|(variant, canonical)| {
            adjudicate_one(
                variant,
                canonical,
                protected,
                &directed,
                &canonicals_per_variant,
            )
        })
        .collect()
}

/// Single-pair decision with the batch-level facts already computed.
fn adjudicate_one(
    variant: &str,
    canonical: &str,
    protected: &ProtectedTerms,
    directed: &HashSet<(String, String)>,
    canonicals_per_variant: &HashMap<String, HashSet<String>>,
) -> LexiconVerdict {
    let v_folded = fold(variant);
    let c_folded = fold(canonical);

    if v_folded != c_folded && directed.contains(&(c_folded.clone(), v_folded.clone())) {
        return LexiconVerdict::Reject {
            reason: RejectReason::Contradiction,
        };
    }
    if canonicals_per_variant
        .get(&v_folded)
        .is_some_and(|set| set.len() > 1)
    {
        return LexiconVerdict::Reject {
            reason: RejectReason::AmbiguousVariant,
        };
    }
    if protected.contains(variant) {
        return LexiconVerdict::Reject {
            reason: RejectReason::ProtectedVariant,
        };
    }

    let v_tokens: Vec<&str> = variant.split_whitespace().collect();
    let c_tokens: Vec<&str> = canonical.split_whitespace().collect();

    // A protected canonical means the pair *restores* a real term, so the
    // inflection shape is not evidence of grammar. `loctry -> Loctree` keeps
    // its stem on purpose.
    let restores_protected_term = protected.contains(canonical);
    if v_tokens.len() == 1
        && c_tokens.len() == 1
        && !restores_protected_term
        && is_inflection_of_one_stem(&v_folded, &c_folded)
    {
        return LexiconVerdict::Reject {
            reason: RejectReason::Inflection,
        };
    }

    if v_tokens.len() > MAX_TOKENS_PER_SIDE
        || c_tokens.len() > MAX_TOKENS_PER_SIDE
        || v_tokens.len().abs_diff(c_tokens.len()) > MAX_TOKEN_COUNT_DELTA
    {
        return LexiconVerdict::Reject {
            reason: RejectReason::PhraseRewrite,
        };
    }

    if normalized_distance(&v_folded, &c_folded) > MAX_PHONETIC_DISTANCE {
        return LexiconVerdict::Reject {
            reason: RejectReason::NotPhonetic,
        };
    }

    if v_tokens.len() == 1 && is_common_word(&v_folded) {
        return LexiconVerdict::Review {
            reason: ReviewReason::CommonWordVariant,
        };
    }

    // `Pensif -> Pensieve na`: the trailing "na" belongs to the next sentence,
    // not to the term. Keying the rule on it would paste the stray word in.
    let variant_ends_common = v_tokens.last().is_some_and(|t| is_common_word(&fold(t)));
    let canonical_ends_common = c_tokens.last().is_some_and(|t| is_common_word(&fold(t)));
    if c_tokens.len() > 1 && canonical_ends_common && !variant_ends_common {
        return LexiconVerdict::Review {
            reason: ReviewReason::DanglingFunctionWord,
        };
    }

    LexiconVerdict::Accept
}

/// True when both tokens share one stem and differ only in a short ending.
fn is_inflection_of_one_stem(a: &str, b: &str) -> bool {
    let shared = a.chars().zip(b.chars()).take_while(|(x, y)| x == y).count();
    let a_len = a.chars().count();
    let b_len = b.chars().count();
    let shorter = a_len.min(b_len);
    shared >= INFLECTION_MIN_STEM_CHARS
        && shared as f64 >= INFLECTION_MIN_STEM_RATIO * shorter as f64
        && a_len - shared <= INFLECTION_MAX_TAIL_CHARS
        && b_len - shared <= INFLECTION_MAX_TAIL_CHARS
}

/// Char-level edit distance scaled by the longer side, so the threshold means
/// the same thing for `si -> się` as for `eksplorowaliśmy -> eksploatowaliśmy`.
fn normalized_distance(a: &str, b: &str) -> f64 {
    let longest = a.chars().count().max(b.chars().count());
    if longest == 0 {
        return 0.0;
    }
    edit_distance_chars(a, b) as f64 / longest as f64
}

/// Levenshtein over Unicode chars, two rolling rows.
fn edit_distance_chars(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    if a.is_empty() {
        return b.len();
    }
    if b.is_empty() {
        return a.len();
    }
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut curr = vec![0usize; b.len() + 1];
    for (i, ach) in a.iter().enumerate() {
        curr[0] = i + 1;
        for (j, bch) in b.iter().enumerate() {
            let substitution = prev[j] + usize::from(ach != bch);
            curr[j + 1] = substitution.min(prev[j + 1] + 1).min(curr[j] + 1);
        }
        std::mem::swap(&mut prev, &mut curr);
    }
    prev[b.len()]
}

/// True for a bare high-frequency word.
fn is_common_word(folded: &str) -> bool {
    COMMON_WORDS.contains(&folded)
}

/// Full Unicode lowercase, so Polish `Ż` folds like `ż`.
fn fold(text: &str) -> String {
    text.trim().chars().flat_map(char::to_lowercase).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn adjudicate(pairs: &[(&str, &str)]) -> Vec<LexiconVerdict> {
        let owned: Vec<(String, String)> = pairs
            .iter()
            .map(|(v, c)| ((*v).to_string(), (*c).to_string()))
            .collect();
        adjudicate_lexicon_candidates(&owned, &ProtectedTerms::builtin())
    }

    fn label(pair: (&str, &str)) -> &'static str {
        adjudicate(&[pair])[0].label()
    }

    #[test]
    fn real_mishearings_of_terms_are_accepted() {
        for pair in [
            ("grypa", "grepa"),
            ("kromu", "Chrome"),
            ("Cowscribe", "Codescribe"),
            ("lubaku", "loopbacku"),
            ("tradycie", "threadzie"),
            ("komuterze", "komputerze"),
            ("franculskiego", "francuskiego"),
            ("si", "się"),
        ] {
            assert_eq!(label(pair), "accept", "{pair:?} should reach the lexicon");
        }
    }

    #[test]
    fn product_names_are_never_spelled_away() {
        // The variant side is the correct term; the rule would destroy it.
        assert_eq!(label(("Codex", "Kodeks")), "reject:protected-variant");
        assert_eq!(label(("Loctree", "LogTree")), "reject:protected-variant");
    }

    #[test]
    fn a_protected_canonical_survives_the_inflection_shape() {
        // Same stem, short tail — but this pair restores the real term.
        assert_eq!(label(("loctry", "Loctree")), "accept");
        // Without protection the same shape reads as grammar.
        assert_eq!(label(("zasadne", "zasadny")), "reject:inflection");
    }

    #[test]
    fn grammar_inflections_never_become_substitution_rules() {
        for pair in [
            ("agent", "agenta"),
            ("korekta", "korektę"),
            ("dobra", "dobry"),
        ] {
            assert_eq!(label(pair), "reject:inflection", "{pair:?}");
        }
    }

    #[test]
    fn sentence_bleed_is_rejected_as_a_phrase_rewrite() {
        assert_eq!(
            label(("April", "jako pierwszy etap")),
            "reject:phrase-rewrite"
        );
        assert_eq!(
            label(("kody Skype nie A", "Codescribe")),
            "reject:phrase-rewrite"
        );
    }

    #[test]
    fn semantically_unrelated_pairs_fail_the_phonetic_bar() {
        assert_eq!(label(("film", "chwilą")), "reject:not-phonetic");
        assert_eq!(label(("Tego", "release")), "reject:not-phonetic");
    }

    #[test]
    fn opposing_pairs_cancel_each_other() {
        let verdicts = adjudicate(&[("nasz", "nasza"), ("nasza", "nasz")]);
        assert_eq!(verdicts[0].label(), "reject:contradiction");
        assert_eq!(verdicts[1].label(), "reject:contradiction");
    }

    #[test]
    fn one_variant_taught_two_canonicals_teaches_neither() {
        let verdicts = adjudicate(&[("prompt", "promptu"), ("prompt", "promptem")]);
        assert!(
            verdicts
                .iter()
                .all(|v| v.label() == "reject:ambiguous-variant"),
            "{verdicts:?}"
        );
    }

    #[test]
    fn common_word_variants_are_quarantined_not_discarded() {
        // Quarantine, so the operator can still promote a domain term that
        // happens to collide with everyday vocabulary.
        assert_eq!(label(("pisać", "pisze")), "review:common-word");
        assert!(!adjudicate(&[("pisać", "pisze")])[0].is_accepted());
    }

    #[test]
    fn a_stray_trailing_word_sends_the_pair_to_review() {
        assert_eq!(
            label(("Pensif", "Pensieve na")),
            "review:dangling-function-word"
        );
    }

    #[test]
    fn operator_terms_extend_the_builtin_list() {
        let dir = std::env::temp_dir().join(format!("codescribe-protected-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let path = ProtectedTerms::default_path(&dir);
        std::fs::write(&path, "# project vocabulary\nVistascribe\n\nQube\n").expect("write");

        let terms = ProtectedTerms::load_from(&path);
        assert!(
            !ProtectedTerms::builtin().contains("Vistascribe"),
            "the sample term must not already be a builtin, or the count proves nothing"
        );
        assert!(terms.contains("vistascribe"), "operator term is in force");
        assert!(terms.contains("Codex"), "builtins survive the merge");
        assert_eq!(terms.len(), ProtectedTerms::builtin().len() + 2);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_missing_operator_file_leaves_the_builtins_standing() {
        let terms = ProtectedTerms::load_from(Path::new("/nonexistent/protected_terms.txt"));
        assert_eq!(terms.len(), ProtectedTerms::builtin().len());
        assert!(terms.contains("Loctree"));
    }
}
