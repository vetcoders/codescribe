//! Deterministic post-processing for streamed and finalized STT text.
//!
//! Three concerns live here, in the order a transcript meets them:
//! 1. **Lexicon** — a hot-reloadable rewrite table (builtin + seed + operator
//!    vocabulary + curated protected terms + the user's custom file) that maps
//!    Whisper mis-hears onto canonical spellings. [`apply_lexicon`] is the single
//!    deterministic pass and is safe to re-run after any non-deterministic stage.
//! 2. **Cleanup + semantic gate** — [`StreamPostProcessor`] strips ASR artifacts
//!    and, for interim chunks only, drops near-duplicate repeats that the decoder
//!    emits while a phrase is still settling.
//! 3. **Guardrails** — [`protected_terms_lost`] and [`final_pass_guardrail_reason`]
//!    detect when a downstream LLM pass silently corrupted operator vocabulary or
//!    drifted into hallucinated filler.
//!
//! The lexicon lives in one process-global singleton so every lane (streaming,
//! final pass, quality report) rewrites text identically.

use std::collections::HashSet;
use std::fs;
use std::path::PathBuf;
use std::sync::{LazyLock, RwLock};
use std::time::{Instant, SystemTime};

use lazy_static::lazy_static;
use regex::Regex;
use serde::{Deserialize, Serialize};
use tracing::{debug, info, warn};

use crate::config::Config;

/// Embedded legacy-format lexicon sources shipped with the binary (programming domain).
/// Each `(label, jsonl)` pair is compiled at startup into the global rewrite table.
const BUILTIN_LEXICONS: &[(&str, &str)] = &[(
    "programming",
    include_str!("../../assets/programming.jsonl"),
)];
/// Seed-format domain vocabulary baked into the binary and loaded before operator vocab.
/// Seed rows carry whole-word / case policy so rewrites cannot hit substrings.
const SEED_JSONL: &str = include_str!("../../assets/seed.jsonl");
/// Curated operator/command vocabulary. Spoken Polish UI-command phrases and
/// their Whisper mis-hears normalize to the canonical *code token* the codebase
/// actually uses (e.g. "schowek"/"schowku"/"schowka"/"schopku" -> "clipboard").
/// Loaded rules-only via `load_seed_jsonl` (seed format gives whole-word +
/// case control), so these common words never enter `protected_canonicals` and
/// never trip the downstream loss-detection gate. Canonicals were confirmed
/// real and high-frequency via `loct occurrences` before being chosen.
const OPERATOR_VOCAB_JSONL: &str = include_str!("../../assets/operator_vocabulary.jsonl");
/// Curated proper-noun / operator-vocabulary lexicon. Unlike the generic
/// programming/seed sources, entries here are case-normalizing: a variant that
/// differs from the canonical only by casing (e.g. "aicx" -> "AICX") still
/// produces a rewrite rule. The list is hand-vetted so capitalization is always
/// correct for these terms — generic English words (rust, rest, diesel) are NOT
/// in this file, so they never get capitalized.
const PROTECTED_TERMS_JSONL: &str = include_str!("../../assets/protected_terms.jsonl");

/// Default cosine similarity above which an interim chunk is a near-duplicate of the last.
/// Overridable via `CODESCRIBE_STREAM_SIMILARITY`.
const DEFAULT_SIMILARITY_THRESHOLD: f32 = 0.93;
/// Default Jaccard novelty floor: chunks at or below this and high similarity may drop.
/// Overridable via `CODESCRIBE_STREAM_NOVELTY`.
const DEFAULT_NOVELTY_THRESHOLD: f32 = 0.12;
/// Max characters the semantic gate will embed; longer text skips embedding (fail-open).
const MAX_EMBED_CHARS: usize = 512;
/// Cap on consecutive gate drops so a genuinely repetitive speaker is never silenced.
const MAX_DROPS_IN_ROW: u8 = 2;
/// Filler tokens that fingerprint final-pass drift when newly introduced into a candidate.
const FINAL_PASS_ARTIFACT_TOKENS: &[&str] = &["going", "use"];
/// Whisper `initial_prompt` token budget; over-approximated so the decoder never truncates.
pub const WHISPER_INITIAL_PROMPT_TOKEN_BUDGET: usize = 224;
/// Fixed prefix for the Whisper vocabulary hint string built by `build_whisper_initial_prompt`.
const WHISPER_INITIAL_PROMPT_PREFIX: &str = "Vocabulary:";
/// Env override for Whisper initial-prompt opt-in; wins over persisted config when set.
pub const STT_INITIAL_PROMPT_ENABLED_ENV: &str = "CODESCRIBE_STT_INITIAL_PROMPT_ENABLED";

lazy_static! {
    /// Matches trailing Whisper `:D` / `:-D` emoticon bursts stripped by `cleanup_artifacts`.
    /// Applied only at utterance end so mid-text punctuation is never removed.
    static ref TRAILING_SMILEY_D_RE: Regex =
        Regex::new(r"(?i)(?:\s*:+-?d)+(?:\s*:+\s*)*$").expect("trailing :D regex");
}

/// Nested `extras` block of a legacy lexicon row. Older seed files stored the
/// mis-hear list here instead of at the top level; both shapes are merged on load.
#[derive(Debug, Deserialize)]
struct LexiconExtras {
    #[serde(default)]
    mispronunciations: Vec<String>,
}

/// One row of a legacy-format lexicon file: a canonical `term` plus the spoken
/// variants that should rewrite to it.
#[derive(Debug, Deserialize)]
struct LegacyEntry {
    term: String,
    #[serde(default)]
    mispronunciations: Vec<String>,
    #[serde(default)]
    extras: Option<LexiconExtras>,
}

/// Per-entry matching policy of a seed-format row. Defaults are deliberately
/// conservative: enabled, case-insensitive, and whole-word only, so a seed entry
/// cannot rewrite text inside a longer word.
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
    /// Conservative defaults: enabled, case-insensitive, whole-word only.
    fn default() -> Self {
        Self {
            input_variants: Vec::new(),
            enabled: true,
            case_sensitive: false,
            whole_word_only: true,
        }
    }
}

/// One row of a seed-format lexicon file: the canonical spelling plus the
/// matching policy that governs its input variants.
#[derive(Debug, Deserialize)]
struct SeedEntry {
    canonical: String,
    #[serde(default)]
    normalization: SeedNormalization,
}

/// A compiled rewrite: every match of `pattern` becomes `replacement`.
#[derive(Debug)]
struct LexiconRule {
    pattern: Regex,
    replacement: String,
}

/// The loaded rewrite table. Builtin rules are compiled once at startup; custom
/// rules come from the operator's file and are hot-reloaded on mtime change.
/// Builtin rules always apply before custom ones.
#[derive(Debug)]
struct Lexicon {
    builtin_rules: Vec<LexiconRule>,
    custom_rules: Vec<LexiconRule>,
    custom_path: PathBuf,
    custom_mtime: Option<SystemTime>,
    /// Canonical forms of curated protected terms (proper nouns, operator
    /// vocabulary). Used by `protected_terms_lost` to flag when an LLM or other
    /// downstream pass silently drops or mutates a protected term.
    protected_canonicals: Vec<String>,
    /// Canonical terms from the operator's custom dictionary. These feed Whisper's
    /// initial prompt after protected terms, without becoming protected-term loss
    /// sentinels.
    custom_canonicals: Vec<String>,
}

/// Process-global lexicon singleton shared by streaming, final pass, and quality lanes.
/// One rewrite table so every path normalizes operator vocabulary identically.
static GLOBAL_LEXICON: LazyLock<RwLock<Lexicon>> = LazyLock::new(|| {
    let lex = Lexicon::from_builtin();
    info!(
        "Global lexicon singleton initialized: {} rules",
        lex.rule_count()
    );
    RwLock::new(lex)
});

/// Warm the global lexicon off the caller's thread.
///
/// The singleton compiles ~14.5k rules in seconds; when the first toucher is
/// the Apple live-session thread, that compile sits between "audio stream
/// started" and "recognizer ready" and the first dictation after launch arms
/// seconds late (session a5623d55, 2026-08-12: 5.1 s). Call at startup so the
/// first recording finds the table already built. Idempotent and non-blocking;
/// concurrent first-touchers simply block on the same `LazyLock` as before.
pub fn warm_lexicon() {
    std::thread::Builder::new()
        .name("lexicon-warm".into())
        .spawn(|| {
            drop(GLOBAL_LEXICON.read());
        })
        .ok();
}

impl Lexicon {
    /// Compile the full rule set from every source, in load order.
    ///
    /// Order is load-bearing: generic programming/seed rules first, operator
    /// vocabulary next, and curated protected terms LAST among builtin sources so
    /// their brand casing wins over any lower-cased form an earlier rule produced.
    /// The operator's custom file is kept in a separate list so it can be
    /// hot-reloaded without recompiling the builtins.
    fn from_builtin() -> Self {
        let t0 = Instant::now();
        let mut builtin_rules = Vec::new();

        let t_legacy = Instant::now();
        for (label, source) in BUILTIN_LEXICONS {
            load_legacy_jsonl(source, label, &mut builtin_rules);
        }
        let legacy_ms = t_legacy.elapsed().as_millis();
        let legacy_count = builtin_rules.len();

        let t_seed = Instant::now();
        let seed_count = load_seed_jsonl(SEED_JSONL, "seed", &mut builtin_rules);
        let seed_ms = t_seed.elapsed().as_millis();

        // Operator/command vocabulary: spoken Polish UI commands + their
        // mis-hears normalize to the canonical code token. Seed format (rules
        // only) keeps these common words out of `protected_canonicals`.
        let operator_count = load_seed_jsonl(OPERATOR_VOCAB_JSONL, "operator", &mut builtin_rules);

        // Protected terms load LAST among builtin sources so their brand casing
        // wins over any generic earlier rule that produced a lower-cased form.
        let mut protected_canonicals = Vec::new();
        let protected_count = load_protected_jsonl(
            PROTECTED_TERMS_JSONL,
            "protected",
            &mut builtin_rules,
            &mut protected_canonicals,
        );

        let custom_path = Config::config_dir().join("lexicon.custom.jsonl");
        let custom_mtime = fs::metadata(&custom_path)
            .ok()
            .and_then(|m| m.modified().ok());

        let t_custom = Instant::now();
        let mut custom_rules = Vec::new();
        let mut custom_canonicals = Vec::new();
        let custom_count = load_custom_lexicon()
            .map(|content| {
                load_legacy_jsonl_with_terms(
                    &content,
                    "custom",
                    &mut custom_rules,
                    Some(&mut custom_canonicals),
                )
            })
            .unwrap_or(0);
        let custom_ms = t_custom.elapsed().as_millis();

        let total_ms = t0.elapsed().as_millis();
        let total = builtin_rules.len() + custom_count;

        if total > 0 {
            info!(
                "Loaded {} lexicon rules in {}ms (legacy={} in {}ms, seed={} in {}ms, operator={}, protected={} terms={}, custom={} in {}ms, custom_path={})",
                total,
                total_ms,
                legacy_count,
                legacy_ms,
                seed_count,
                seed_ms,
                operator_count,
                protected_count,
                protected_canonicals.len(),
                custom_count,
                custom_ms,
                custom_path.display(),
            );
        } else {
            warn!(
                "No lexicon rules loaded from lexicon sources (custom_path={})",
                custom_path.display()
            );
        }

        Self {
            builtin_rules,
            custom_rules,
            custom_path,
            custom_mtime,
            protected_canonicals,
            custom_canonicals,
        }
    }

    /// Re-read the custom lexicon file when its mtime changed; a no-op otherwise.
    /// Lets the operator teach a correction and have the next utterance honour it
    /// without restarting the app.
    fn maybe_reload(&mut self) {
        let current_mtime = fs::metadata(&self.custom_path)
            .ok()
            .and_then(|m| m.modified().ok());
        if current_mtime == self.custom_mtime {
            return;
        }
        self.custom_rules.clear();
        self.custom_canonicals.clear();
        let custom_count = fs::read_to_string(&self.custom_path)
            .ok()
            .filter(|c| !c.trim().is_empty())
            .map(|content| {
                load_legacy_jsonl_with_terms(
                    &content,
                    "custom",
                    &mut self.custom_rules,
                    Some(&mut self.custom_canonicals),
                )
            })
            .unwrap_or(0);
        self.custom_mtime = current_mtime;
        info!(
            "Hot-reloaded {} custom lexicon rules (total={}, custom_path={})",
            custom_count,
            self.rule_count(),
            self.custom_path.display(),
        );
    }

    /// Run every rule over `text`, builtins before custom rules. Rules are applied
    /// in sequence, so a later rule sees the output of an earlier one.
    fn apply(&self, text: &str) -> String {
        let t0 = Instant::now();
        let mut out = text.to_string();
        let mut matches = 0u32;
        for rule in self.builtin_rules.iter().chain(self.custom_rules.iter()) {
            if rule.pattern.is_match(&out) {
                out = rule
                    .pattern
                    .replace_all(&out, rule.replacement.as_str())
                    .to_string();
                matches += 1;
            }
        }
        let apply_ms = t0.elapsed().as_millis();
        if apply_ms > 50 {
            debug!(
                "Lexicon apply: {}ms ({} rules, {} matches, {} chars)",
                apply_ms,
                self.rule_count(),
                matches,
                text.len()
            );
        }
        out
    }

    /// Total number of active rules (builtin + custom), for logging.
    fn rule_count(&self) -> usize {
        self.builtin_rules.len() + self.custom_rules.len()
    }

    /// Domain-vocabulary hint for this rule set: protected terms first, then the
    /// operator's custom canonicals, trimmed to the Whisper prompt budget.
    fn whisper_initial_prompt(&self) -> Option<String> {
        build_whisper_initial_prompt(
            &self.protected_canonicals,
            &self.custom_canonicals,
            WHISPER_INITIAL_PROMPT_TOKEN_BUDGET,
        )
    }
}

/// Take the write lock and hot-reload the singleton's custom rules if the file
/// changed. Panics on a poisoned lock: a half-written lexicon would silently
/// corrupt every later transcript, so failing loudly is the honest outcome.
fn maybe_reload_global_lexicon() {
    let mut lexicon = GLOBAL_LEXICON
        .write()
        .expect("global lexicon write lock poisoned");
    lexicon.maybe_reload();
}

/// Apply the singleton's rules under a read lock, without reloading first.
fn apply_global_lexicon(text: &str) -> String {
    let lexicon = GLOBAL_LEXICON
        .read()
        .expect("global lexicon read lock poisoned");
    lexicon.apply(text)
}

/// Deterministically apply the global lexicon (builtin + seed + protected +
/// custom) to `text`, hot-reloading the custom file if it changed.
///
/// This is the single deterministic protected-vocabulary pass. It is safe to run
/// at any layer (it only rewrites registered mispronunciations to their
/// canonical form) and is idempotent for canonical output. Use it to re-assert
/// operator vocabulary AFTER a non-deterministic stage such as an LLM
/// formatting/assistive pass, which can otherwise silently corrupt proper nouns
/// (e.g. "Loctree" -> "Luxury").
pub fn apply_lexicon(text: &str) -> String {
    maybe_reload_global_lexicon();
    apply_global_lexicon(text)
}

/// Build the domain-vocabulary hint fed into Whisper's `initial_prompt`.
///
/// Protected terms are selected before custom dictionary terms, duplicates are
/// removed case-insensitively, and the final string is trimmed to the Whisper
/// prompt budget before decoding begins.
pub fn build_whisper_initial_prompt(
    protected_terms: &[String],
    custom_terms: &[String],
    token_budget: usize,
) -> Option<String> {
    if token_budget == 0 {
        return None;
    }

    let mut seen = HashSet::new();
    let mut selected = Vec::new();
    let mut used_tokens = 1usize; // `Vocabulary:`

    for term in protected_terms.iter().chain(custom_terms.iter()) {
        let term = term.trim();
        if term.is_empty() {
            continue;
        }
        if !seen.insert(term.to_lowercase()) {
            continue;
        }

        let term_tokens = estimated_prompt_tokens(term) + 1; // term plus separator/punctuation
        if used_tokens + term_tokens > token_budget {
            break;
        }

        used_tokens += term_tokens;
        selected.push(term.to_string());
    }

    (!selected.is_empty())
        .then(|| format!("{WHISPER_INITIAL_PROMPT_PREFIX} {}.", selected.join("; ")))
}

/// Whether the vocabulary hint may be fed to the decoder.
///
/// The env override wins so a test or a debugging session can flip the feature
/// without touching persisted settings; otherwise the user's setting decides.
/// Off by default — an initial prompt biases decoding and must be opt-in.
pub fn stt_initial_prompt_enabled() -> bool {
    match std::env::var(STT_INITIAL_PROMPT_ENABLED_ENV) {
        Ok(value) => matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on" | "enabled"
        ),
        Err(_) => Config::load_without_keychain().stt_initial_prompt_enabled,
    }
}

/// The vocabulary hint for the current lexicon, or `None` when the feature is
/// disabled or no terms are registered. Hot-reloads the custom file first so a
/// freshly taught term can reach the very next decode.
pub fn whisper_initial_prompt() -> Option<String> {
    if !stt_initial_prompt_enabled() {
        return None;
    }
    maybe_reload_global_lexicon();
    let lexicon = GLOBAL_LEXICON
        .read()
        .expect("global lexicon read lock poisoned");
    lexicon.whisper_initial_prompt()
}

/// Coarse token estimate for prompt budgeting: one token per whitespace-separated
/// word, never zero. Deliberately an over-approximation — overshooting the budget
/// would silently truncate the prompt inside the decoder.
fn estimated_prompt_tokens(term: &str) -> usize {
    term.split_whitespace().count().max(1)
}

/// Whole-word, case-insensitive containment check for a (possibly multi-word)
/// term. Mirrors the lexicon's own matching: internal whitespace is treated
/// flexibly so "Fn Shift" matches across variable spacing.
fn contains_term_ci(haystack: &str, term: &str) -> bool {
    build_word_regex(term)
        .map(|re| re.is_match(haystack))
        .unwrap_or(false)
}

/// Report curated protected terms that were present in `before` but are missing
/// from `after` — i.e. silently dropped or mutated by a downstream stage
/// (typically an LLM formatting/assistive pass). Returns canonical forms in a
/// stable, deduplicated order so the quality loop and operator can see exactly
/// which operator vocabulary was lost.
pub fn protected_terms_lost(before: &str, after: &str) -> Vec<String> {
    let canonicals = {
        let lexicon = GLOBAL_LEXICON
            .read()
            .expect("global lexicon read lock poisoned");
        lexicon.protected_canonicals.clone()
    };

    let mut lost = Vec::new();
    for term in canonicals {
        if contains_term_ci(before, &term) && !contains_term_ci(after, &term) {
            lost.push(term);
        }
    }
    lost
}

/// Load legacy-format rows without recording their canonicals — the source is a
/// generic vocabulary, not a protected-term sentinel list.
fn load_legacy_jsonl(source: &str, label: &str, rules: &mut Vec<LexiconRule>) -> usize {
    load_legacy_jsonl_with_terms(source, label, rules, None)
}

/// Parse legacy-format rows into rewrite rules, returning how many were added.
///
/// A malformed line is warned about and skipped rather than failing the load: one
/// bad row must never cost the user the whole lexicon. Variants equal to the
/// canonical (ignoring ASCII case) are dropped as no-ops, and rows from the
/// `custom` source additionally pass the [`is_unsafe_plain_custom_rule`] filter.
/// When `canonicals` is provided, each term is recorded there for downstream
/// loss detection and prompt building.
fn load_legacy_jsonl_with_terms(
    source: &str,
    label: &str,
    rules: &mut Vec<LexiconRule>,
    mut canonicals: Option<&mut Vec<String>>,
) -> usize {
    let mut added = 0usize;
    for (idx, line) in source.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        let entry: LegacyEntry = match serde_json::from_str(line) {
            Ok(entry) => entry,
            Err(e) => {
                warn!(
                    "Lexicon line {} ({}) failed to parse: {}",
                    idx + 1,
                    label,
                    e
                );
                continue;
            }
        };

        if let Some(canonicals) = &mut canonicals
            && !canonicals.iter().any(|c| c == &entry.term)
        {
            canonicals.push(entry.term.clone());
        }

        // Merge top-level mispronunciations with extras.mispronunciations
        // (legacy seed rows store them in extras, programming.jsonl at top level)
        let mut all_mis = entry.mispronunciations;
        if let Some(extras) = entry.extras {
            all_mis.extend(extras.mispronunciations);
        }

        for mis in all_mis.iter() {
            if mis.eq_ignore_ascii_case(&entry.term) {
                continue;
            }

            if label == "custom" && is_unsafe_plain_custom_rule(&entry.term, mis) {
                debug!(
                    "Skipping unsafe custom lexicon rule {} -> {}",
                    mis, entry.term
                );
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

/// Reject custom rules that rewrite one ordinary lowercase word into a different
/// ordinary word.
///
/// Such entries usually arrive from reference diffs or inflections rather than
/// real acoustic mis-hears, and as a global STT rewrite they poison unrelated
/// speech. Diacritic-only differences ("gorączka" vs "goraczka") ARE genuine
/// mis-hears and stay allowed, as does anything containing uppercase, digits, or
/// punctuation — that shape indicates a proper noun or code token.
fn is_unsafe_plain_custom_rule(term: &str, variant: &str) -> bool {
    let term = term.trim();
    let variant = variant.trim();

    if !is_plain_lowercase_language_phrase(term) || !is_plain_lowercase_language_phrase(variant) {
        return false;
    }

    if normalized_without_polish_diacritics(term) == normalized_without_polish_diacritics(variant) {
        return false;
    }

    // A custom single-token Polish word -> different Polish word rewrite is too
    // broad for global STT postprocessing. Those entries are often reference
    // diffs or inflections, not acoustic mis-hears.
    term.split_whitespace().count() == 1 && variant.split_whitespace().count() <= 2
}

/// True when `input` is ordinary lowercase prose: letters and spaces only, at
/// least one letter, and no uppercase. Digits, punctuation, or any capital mark
/// the string as a code token or proper noun instead.
fn is_plain_lowercase_language_phrase(input: &str) -> bool {
    let mut saw_letter = false;
    let mut saw_space = false;

    for ch in input.chars() {
        if ch.is_whitespace() {
            saw_space = true;
            continue;
        }
        if !ch.is_alphabetic() || ch.is_uppercase() {
            return false;
        }
        saw_letter = true;
    }

    saw_letter && (saw_space || input.split_whitespace().count() == 1)
}

/// Fold Polish diacritics onto their ASCII base letters and lowercase the rest,
/// so two spellings that differ only by accents compare equal.
fn normalized_without_polish_diacritics(input: &str) -> String {
    input
        .to_lowercase()
        .chars()
        .map(|ch| match ch {
            'ą' => 'a',
            'ć' => 'c',
            'ę' => 'e',
            'ł' => 'l',
            'ń' => 'n',
            'ó' => 'o',
            'ś' => 's',
            'ź' | 'ż' => 'z',
            _ => ch,
        })
        .collect()
}

/// Load curated protected-term entries (legacy `term`+`mispronunciations` shape).
///
/// Differs from [`load_legacy_jsonl`] in two deliberate ways:
/// 1. A variant is skipped only when it is *exactly* equal to the canonical, so
///    case-only variants ("aicx" -> "AICX") still produce a normalization rule.
///    This is safe ONLY because the source file is hand-vetted to proper nouns.
/// 2. Each canonical is recorded in `canonicals` so the quality loop can detect
///    when a protected term is lost downstream (e.g. by an LLM rewrite).
fn load_protected_jsonl(
    source: &str,
    label: &str,
    rules: &mut Vec<LexiconRule>,
    canonicals: &mut Vec<String>,
) -> usize {
    let mut added = 0usize;
    for (idx, line) in source.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        let entry: LegacyEntry = match serde_json::from_str(line) {
            Ok(entry) => entry,
            Err(e) => {
                warn!(
                    "Protected lexicon line {} ({}) failed to parse: {}",
                    idx + 1,
                    label,
                    e
                );
                continue;
            }
        };

        if !canonicals.iter().any(|c| c == &entry.term) {
            canonicals.push(entry.term.clone());
        }

        let mut all_mis = entry.mispronunciations;
        if let Some(extras) = entry.extras {
            all_mis.extend(extras.mispronunciations);
        }

        for mis in all_mis.iter() {
            // Skip only exact duplicates; case-only differences are intentional
            // normalization rules (the whole point of this curated source).
            if mis == &entry.term {
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

/// Parse seed-format rows into rewrite rules, returning how many were added.
///
/// Unlike the legacy loader this records no canonicals, which is exactly why the
/// operator/command vocabulary uses it: those are common words, and promoting
/// them to protected terms would trip the downstream loss gate on ordinary
/// speech. Each entry carries its own matching policy (whole-word vs plain,
/// case sensitivity), and disabled entries are skipped.
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

/// Compile a case-insensitive whole-word matcher for `input`.
///
/// The term is regex-escaped, then internal spaces become `\s+` so a multi-word
/// phrase matches across variable spacing. `\b` anchors on both ends are what
/// keep "python" from corrupting "wordpython". Returns `None` for a blank input
/// or an uncompilable pattern — a bad row is skipped, never fatal.
fn build_word_regex(input: &str) -> Option<Regex> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return None;
    }
    let escaped = regex::escape(trimmed);
    let flexible = escaped.replace(' ', r"\s+");
    let pattern = format!(r"(?i)\b{}\b", flexible);
    Regex::new(&pattern).ok()
}

/// Compile an unanchored matcher for `input` — same escaping and flexible
/// whitespace as [`build_word_regex`], but without word boundaries, so it can
/// rewrite inside a longer token. Only seed entries that explicitly opt out of
/// `whole_word_only` get this.
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
        format!("(?i){}", flexible)
    };
    Regex::new(&pattern).ok()
}

/// Read the operator's `lexicon.custom.jsonl`, or `None` when it is absent or
/// blank. A read error is only warned about when the file actually exists —
/// "no custom lexicon yet" is the normal state, not a fault.
fn load_custom_lexicon() -> Option<String> {
    let path = Config::config_dir().join("lexicon.custom.jsonl");
    match fs::read_to_string(&path) {
        Ok(content) => {
            if content.trim().is_empty() {
                None
            } else {
                Some(content)
            }
        }
        Err(e) => {
            if path.exists() {
                warn!("Failed to read custom lexicon {}: {}", path.display(), e);
            }
            None
        }
    }
}

/// Duplicate-suppression state for interim streaming chunks.
///
/// While a phrase is still settling the decoder re-emits near-identical text. The
/// gate drops a chunk only when it is BOTH semantically near-identical to the last
/// one and lexically un-novel, and even then only for a bounded run
/// (`MAX_DROPS_IN_ROW`) so a genuinely repetitive speaker is never silenced.
#[derive(Debug)]
struct SemanticGate {
    last_embedding: Option<Vec<f32>>,
    last_tokens: HashSet<String>,
    drops_in_row: u8,
    similarity_threshold: f32,
    novelty_threshold: f32,
}

impl SemanticGate {
    /// Build a gate with thresholds read from the environment, falling back to
    /// the tuned defaults.
    fn new() -> Self {
        let similarity_threshold =
            env_f32("CODESCRIBE_STREAM_SIMILARITY", DEFAULT_SIMILARITY_THRESHOLD);
        let novelty_threshold = env_f32("CODESCRIBE_STREAM_NOVELTY", DEFAULT_NOVELTY_THRESHOLD);

        Self {
            last_embedding: None,
            last_tokens: HashSet::new(),
            drops_in_row: 0,
            similarity_threshold,
            novelty_threshold,
        }
    }

    /// Whether this chunk is a redundant repeat of the previous one.
    ///
    /// Fails **open**: when no embedding is available (feature disabled, text too
    /// long, or the embedder erred) the chunk is kept. Losing real speech is the
    /// worse failure, so a blind gate must never drop.
    fn should_drop(&mut self, text: &str) -> bool {
        let tokens = tokenize(text);
        if tokens.is_empty() {
            return true;
        }

        let novelty = jaccard_novelty(&self.last_tokens, &tokens);
        let Some(similarity) = self.semantic_similarity(text) else {
            return false;
        };

        if similarity >= self.similarity_threshold && novelty <= self.novelty_threshold {
            self.drops_in_row = self.drops_in_row.saturating_add(1);
            if self.drops_in_row <= MAX_DROPS_IN_ROW {
                debug!(
                    "Stream gate drop (sim={:.3}, novelty={:.3})",
                    similarity, novelty
                );
                return true;
            }
        }

        self.drops_in_row = 0;
        false
    }

    /// Record an emitted chunk as the new comparison baseline and clear the
    /// consecutive-drop counter.
    fn observe(&mut self, text: &str) {
        let tokens = tokenize(text);
        self.last_tokens = tokens.into_iter().collect();
        self.last_embedding = self.semantic_embedding(text);
        self.drops_in_row = 0;
    }

    /// Cosine similarity between this text and the last observed chunk, or `None`
    /// when either embedding is unavailable.
    fn semantic_similarity(&mut self, text: &str) -> Option<f32> {
        let new_emb = self.semantic_embedding(text)?;
        let last_emb = self.last_embedding.as_ref()?;
        Some(cosine_similarity(&new_emb, last_emb))
    }

    /// Embed `text` for the gate, or `None` when embeddings are disabled, the
    /// text exceeds `MAX_EMBED_CHARS`, or the embedder failed.
    ///
    /// Over-long text is skipped rather than truncated: comparing a truncated
    /// prefix would make the gate decide on evidence it does not actually have.
    fn semantic_embedding(&mut self, text: &str) -> Option<Vec<f32>> {
        if !embeddings_enabled() {
            return None;
        }

        // Avoid truncation affecting gate decisions; if it's too long, skip embedding.
        if text.chars().count() > MAX_EMBED_CHARS {
            return None;
        }
        let input = truncate_for_embedding(text);
        match crate::embedder::embed(&input) {
            Ok(vec) => Some(vec),
            Err(e) => {
                warn!("Failed to embed text for semantic gate: {}", e);
                None
            }
        }
    }
}

/// Per-session post-processing pipeline: lexicon, artifact cleanup, and (for
/// interim chunks only) the duplicate gate, with counters for the quality loop.
#[derive(Debug)]
pub struct StreamPostProcessor {
    gate: SemanticGate,
    stats: StreamPostProcessStats,
}

/// Counters describing what one session's post-processing actually did. Serialized
/// into quality-report entries so transcript loss is attributable to a stage.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct StreamPostProcessStats {
    pub input_chunks: u64,
    pub output_chunks: u64,
    pub dropped_chunks: u64,
    pub gate_drops: u64,
    pub suspicious_chunks: u64,
    pub lexicon_rewrites: u64,
    pub repetition_cleanups: u64,
    pub embeddings_enabled: bool,
}

impl StreamPostProcessor {
    /// Build a processor and force the global lexicon to initialize now, so the
    /// first utterance never pays the compile cost mid-speech.
    pub fn new() -> Self {
        // Touch the global singleton to trigger lazy init (if not yet initialized)
        drop(GLOBAL_LEXICON.read());
        Self {
            gate: SemanticGate::new(),
            stats: StreamPostProcessStats {
                embeddings_enabled: embeddings_enabled(),
                ..StreamPostProcessStats::default()
            },
        }
    }

    /// Process a streaming chunk — applies lexicon, cleanup, and semantic gate.
    pub fn process(&mut self, text: &str) -> Option<String> {
        self.process_internal(text, true)
    }

    /// Process a complete utterance — applies lexicon and cleanup, no semantic gate.
    /// Use this for VAD-segmented utterances where each segment is naturally distinct.
    pub fn process_utterance(&mut self, text: &str) -> Option<String> {
        self.process_internal(text, false)
    }

    /// Shared body of [`Self::process`] and [`Self::process_utterance`].
    ///
    /// Stages run in a fixed order — lexicon, artifact cleanup, whitespace
    /// normalization — and `None` means the chunk was dropped (empty input, empty
    /// result, or gated). The duplicate gate only engages when `apply_gate` is set
    /// AND the text already looks suspicious, so ordinary speech never reaches the
    /// embedder. Every branch that returns `None` also increments a counter, so a
    /// vanished transcript is always attributable.
    fn process_internal(&mut self, text: &str, apply_gate: bool) -> Option<String> {
        self.stats.input_chunks += 1;
        maybe_reload_global_lexicon();

        if text.trim().is_empty() {
            self.stats.dropped_chunks += 1;
            return None;
        }

        let mut cleaned = apply_global_lexicon(text);
        if cleaned != text {
            self.stats.lexicon_rewrites += 1;
        }

        let cleaned_after_cleanup = cleanup_artifacts(&cleaned);
        if cleaned_after_cleanup != cleaned {
            self.stats.repetition_cleanups += 1;
        }
        cleaned = cleaned_after_cleanup;
        cleaned = normalize_whitespace(&cleaned);

        if cleaned.trim().is_empty() {
            self.stats.dropped_chunks += 1;
            return None;
        }

        if apply_gate && is_suspicious(&cleaned) {
            self.stats.suspicious_chunks += 1;
            if self.gate.should_drop(&cleaned) {
                self.stats.dropped_chunks += 1;
                self.stats.gate_drops += 1;
                return None;
            }
        }

        if apply_gate {
            self.gate.observe(&cleaned);
        }
        self.stats.output_chunks += 1;
        Some(cleaned)
    }

    /// Snapshot of this session's counters.
    pub fn stats(&self) -> StreamPostProcessStats {
        self.stats.clone()
    }
}

impl Default for StreamPostProcessor {
    /// Same as [`StreamPostProcessor::new`]: touches the global lexicon on construction.
    fn default() -> Self {
        Self::new()
    }
}

/// Read an `f32` tuning knob from the environment, falling back to `default` when
/// unset or unparseable.
fn env_f32(key: &str, default: f32) -> f32 {
    std::env::var(key)
        .ok()
        .and_then(|v| v.parse::<f32>().ok())
        .unwrap_or(default)
}

/// Strict opt-in env flag: only `1` or `true` enable it; anything else, including
/// an unset variable, is `false`. Use [`env_flag`] where the default is on.
fn env_bool(key: &str) -> bool {
    std::env::var(key)
        .ok()
        .map(|v| {
            let v = v.trim();
            v == "1" || v.eq_ignore_ascii_case("true")
        })
        .unwrap_or(false)
}

/// Whether the gate may embed text. Off under `cfg(test)` unless explicitly
/// forced, so the suite never depends on model weights or GPU availability.
fn embeddings_enabled() -> bool {
    if env_bool("CODESCRIBE_STREAM_DISABLE_EMBEDDINGS") {
        return false;
    }

    if cfg!(test) && !env_bool("CODESCRIBE_STREAM_FORCE_EMBEDDINGS") {
        return false;
    }

    true
}

/// Split into lowercase word tokens, stripping leading/trailing non-alphanumerics
/// so punctuation differences do not register as novelty.
fn tokenize(text: &str) -> Vec<String> {
    text.split_whitespace()
        .map(|token| {
            token
                .trim_matches(|c: char| !c.is_alphanumeric())
                .to_lowercase()
        })
        .filter(|token| !token.is_empty())
        .collect()
}

/// Lexical novelty as one minus Jaccard overlap: `0.0` means identical token
/// sets, `1.0` completely new. An empty side counts as fully novel, so a missing
/// baseline can never justify a drop.
fn jaccard_novelty(left: &HashSet<String>, right: &[String]) -> f32 {
    if left.is_empty() || right.is_empty() {
        return 1.0;
    }

    let right_set: HashSet<String> = right.iter().cloned().collect();
    let intersection = left.intersection(&right_set).count();
    let union = left.union(&right_set).count();

    if union == 0 {
        1.0
    } else {
        1.0 - (intersection as f32 / union as f32)
    }
}

/// Cosine similarity of two embeddings, or `0.0` (treated as "not similar") when
/// either vector is empty, mismatched in length, or has zero norm.
fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    if a.is_empty() || b.is_empty() || a.len() != b.len() {
        return 0.0;
    }

    let mut dot = 0.0;
    let mut norm_a = 0.0;
    let mut norm_b = 0.0;

    for (x, y) in a.iter().zip(b.iter()) {
        dot += x * y;
        norm_a += x * x;
        norm_b += y * y;
    }

    if norm_a == 0.0 || norm_b == 0.0 {
        0.0
    } else {
        dot / (norm_a.sqrt() * norm_b.sqrt())
    }
}

/// Env flag with a caller-supplied default: an unset or blank variable keeps
/// `default`, and only the explicit off-words (`0`, `false`, `off`, `no`) disable
/// it. Counterpart to [`env_bool`] for features that ship on.
fn env_flag(name: &str, default: bool) -> bool {
    match std::env::var(name) {
        Ok(value) => {
            let trimmed = value.trim();
            if trimmed.is_empty() {
                return default;
            }
            let v = trimmed.to_ascii_lowercase();
            !(v == "0" || v == "false" || v == "off" || v == "no")
        }
        Err(_) => default,
    }
}

/// Strip known ASR artifacts: trailing `:D` emoticon bursts (on by default, and
/// only at the end of an utterance) and decoder repetition loops.
fn cleanup_artifacts(text: &str) -> String {
    // Default ON: treat trailing ":D" bursts as ASR artifacts.
    let mut out = if env_flag("CODESCRIBE_STRIP_TRAILING_SMILEY_D", true) {
        TRAILING_SMILEY_D_RE.replace(text, "").to_string()
    } else {
        text.to_string()
    };

    if crate::ai_formatting::has_repetition_loop(&out) {
        out = crate::ai_formatting::remove_simple_repetitions(&out);
    }
    out
}

/// Collapse every whitespace run to a single space and trim the ends.
fn normalize_whitespace(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Cheap heuristic for "this chunk might be decoder noise": very short, very few
/// tokens, heavily repeated tokens, or a detected repetition loop.
///
/// Only a screen, never a verdict — it decides whether the expensive duplicate
/// gate is worth running, and it also guards the final pass against a rewrite
/// that turned clean text into noise.
fn is_suspicious(text: &str) -> bool {
    if text.len() < 12 {
        return true;
    }

    let tokens = tokenize(text);
    if tokens.len() <= 3 {
        return true;
    }

    let unique = tokens.iter().collect::<HashSet<_>>();
    let ratio = unique.len() as f32 / tokens.len() as f32;
    ratio < 0.5 || crate::ai_formatting::has_repetition_loop(text)
}

/// Known filler tokens that `candidate` added and `raw` never contained, sorted
/// and deduplicated. These words are the fingerprint of a final pass drifting
/// into invented English filler over Polish speech.
fn introduced_artifact_tokens(raw: &str, candidate: &str) -> Vec<String> {
    let raw_tokens: HashSet<String> = tokenize(raw).into_iter().collect();
    let mut introduced = HashSet::new();

    for token in tokenize(candidate) {
        if !raw_tokens.contains(&token) && FINAL_PASS_ARTIFACT_TOKENS.contains(&token.as_str()) {
            introduced.insert(token);
        }
    }

    let mut introduced: Vec<String> = introduced.into_iter().collect();
    introduced.sort();
    introduced
}

/// Why a final-pass candidate must be rejected, or `None` when it is safe to ship.
///
/// Two refusals: the rewrite made previously-clean text suspicious, or it
/// introduced two or more known artifact tokens. One stray token is tolerated —
/// it can be a legitimate correction — so the threshold requires a pattern, not a
/// single coincidence. An unchanged candidate is trivially safe.
pub(crate) fn final_pass_guardrail_reason(raw: &str, candidate: &str) -> Option<String> {
    if candidate == raw {
        return None;
    }

    if is_suspicious(candidate) && !is_suspicious(raw) {
        return Some("candidate_became_suspicious".to_string());
    }

    let introduced = introduced_artifact_tokens(raw, candidate);
    if introduced.len() >= 2 {
        return Some(format!("artifact_token_drift:{}", introduced.join(",")));
    }

    None
}

/// Clamp `text` to `MAX_EMBED_CHARS` characters (not bytes, so multi-byte Polish
/// letters are never split mid-character).
fn truncate_for_embedding(text: &str) -> String {
    if text.len() <= MAX_EMBED_CHARS {
        return text.to_string();
    }

    text.chars().take(MAX_EMBED_CHARS).collect()
}

/// Hermetic unit coverage for lexicon load/reload, prompt opt-in, and final-pass guardrails.
#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;
    use std::ffi::OsString;

    /// RAII guard that restores a process env var on drop so serial tests cannot leak state.
    struct EnvRestore {
        key: &'static str,
        previous: Option<OsString>,
    }

    impl EnvRestore {
        /// Snapshot `key`'s current value (or absence) before a test mutates the environment.
        fn capture(key: &'static str) -> Self {
            Self {
                key,
                previous: std::env::var_os(key),
            }
        }
    }

    impl Drop for EnvRestore {
        /// Restore the captured env value, or remove the key if it was previously unset.
        fn drop(&mut self) {
            match &self.previous {
                Some(value) => unsafe { std::env::set_var(self.key, value) },
                None => unsafe { std::env::remove_var(self.key) },
            }
        }
    }

    /// Builtin programming lexicon rewrites Whisper mis-hears (e.g. `doker` → `Docker`).
    #[test]
    fn test_lexicon_rewrite() {
        let mut processor = StreamPostProcessor::new();
        let input = "Uzywam doker do kontenerow i mam api key do github.";
        let output = processor.process(input).expect("expected output");
        assert!(
            output.contains("Docker"),
            "expected lexicon to rewrite 'doker' -> 'Docker': {output}"
        );
    }

    /// Protected-term family collapses acoustic Loctree variants to brand casing.
    #[test]
    fn test_lexicon_rewrites_loctree_compound_variants() {
        let mut processor = StreamPostProcessor::new();
        let output = processor
            .process("Bede nagrywal cos o locktree i nagrywanie o loktree.")
            .expect("expected output");

        assert_eq!(
            output,
            "Bede nagrywal cos o Loctree i nagrywanie o Loctree."
        );
    }

    /// Empty term lists or a zero token budget produce no Whisper initial prompt.
    #[test]
    fn whisper_initial_prompt_empty_terms_returns_none() {
        assert_eq!(build_whisper_initial_prompt(&[], &[], 224), None);
        assert_eq!(
            build_whisper_initial_prompt(&["Loctree".to_string()], &[], 0),
            None
        );
    }

    /// Protected terms win order; case-insensitive duplicates are dropped once.
    #[test]
    fn whisper_initial_prompt_dedupes_case_and_prioritizes_protected_terms() {
        let protected = vec![
            "Loctree".to_string(),
            "AICX".to_string(),
            "loctree".to_string(),
        ];
        let custom = vec![
            "Codescribe".to_string(),
            "aicx".to_string(),
            "Operator Console".to_string(),
        ];

        let prompt =
            build_whisper_initial_prompt(&protected, &custom, 224).expect("expected prompt");

        assert_eq!(
            prompt,
            "Vocabulary: Loctree; AICX; Codescribe; Operator Console."
        );
    }

    /// Prompt assembly stops before exceeding the coarse token budget estimate.
    #[test]
    fn whisper_initial_prompt_truncates_to_token_budget() {
        let protected = vec![
            "Loctree".to_string(),
            "Operator Console".to_string(),
            "AICX".to_string(),
        ];

        let prompt = build_whisper_initial_prompt(&protected, &["Codescribe".to_string()], 4)
            .expect("expected truncated prompt");

        assert_eq!(prompt, "Vocabulary: Loctree.");
    }

    /// Fresh/default config must not inject an initial prompt even when terms exist.
    #[test]
    #[serial]
    fn whisper_initial_prompt_defaults_off_even_with_builtin_terms() {
        let _data_dir = EnvRestore::capture("CODESCRIBE_DATA_DIR");
        let _env_path = EnvRestore::capture("CODESCRIBE_ENV_PATH");
        let _prompt_enabled = EnvRestore::capture(STT_INITIAL_PROMPT_ENABLED_ENV);
        let temp_dir = tempfile::tempdir().expect("temp data dir");

        unsafe {
            std::env::set_var("CODESCRIBE_DATA_DIR", temp_dir.path());
            std::env::remove_var("CODESCRIBE_ENV_PATH");
            std::env::remove_var(STT_INITIAL_PROMPT_ENABLED_ENV);
        }

        assert!(!stt_initial_prompt_enabled());
        assert_eq!(
            whisper_initial_prompt(),
            None,
            "fresh/default config must not inject a Whisper initial prompt"
        );
    }

    /// Setting `CODESCRIBE_STT_INITIAL_PROMPT_ENABLED` builds a prompt with protected terms.
    #[test]
    #[serial]
    fn whisper_initial_prompt_is_opt_in() {
        let _data_dir = EnvRestore::capture("CODESCRIBE_DATA_DIR");
        let _env_path = EnvRestore::capture("CODESCRIBE_ENV_PATH");
        let _prompt_enabled = EnvRestore::capture(STT_INITIAL_PROMPT_ENABLED_ENV);
        let temp_dir = tempfile::tempdir().expect("temp data dir");

        unsafe {
            std::env::set_var("CODESCRIBE_DATA_DIR", temp_dir.path());
            std::env::remove_var("CODESCRIBE_ENV_PATH");
            std::env::set_var(STT_INITIAL_PROMPT_ENABLED_ENV, "1");
        }

        let prompt = whisper_initial_prompt().expect("opt-in prompt should be built");
        assert!(
            prompt.contains("Loctree"),
            "prompt should include protected terms"
        );
    }

    /// Repetition-loop cleanup and whitespace collapse run on every processed chunk.
    #[test]
    fn test_cleanup_and_whitespace() {
        let mut processor = StreamPostProcessor::new();
        let input = "To jest to jest to jest   bardzo  wazny \n test systemu.";
        let output = processor.process(input).expect("expected output");
        assert_eq!(output, "To jest bardzo wazny test systemu.");
    }

    /// Trailing `:D` ASR artifacts are stripped from complete utterances.
    #[test]
    fn test_strip_trailing_smiley_d() {
        let mut processor = StreamPostProcessor::new();
        let input = "Siema, czy jestes tam? :D :";
        let output = processor.process_utterance(input).expect("expected output");
        assert_eq!(output, "Siema, czy jestes tam?");
    }

    /// Short, repetitive, or loop-like text is flagged; ordinary prose is not.
    #[test]
    fn test_is_suspicious_heuristics() {
        assert!(is_suspicious("ok"));
        assert!(is_suspicious("test test test test"));
        assert!(!is_suspicious(
            "To jest normalny tekst bez powtorzen i z roznymi slowami."
        ));
    }

    /// Final-pass candidates that introduce ≥2 artifact tokens are rejected with a reason.
    #[test]
    fn test_final_pass_guardrail_rejects_artifact_token_drift() {
        let raw = "Co będę robił? Ja chyba coś nagrywam? Ja coś się... Może zhulać, ale w tym momencie myślę, że kwestia";
        let candidate = "Co będę robił? Ja chyba coś nagrywam? Ja coś going... Może zhulać, ale w tym momencie myślę, use kwestia";

        let reason = final_pass_guardrail_reason(raw, candidate).expect("expected guardrail");
        assert_eq!(reason, "artifact_token_drift:going,use");
    }

    /// Lexicon-only rewrites (no filler drift) pass the final-pass guardrail.
    #[test]
    fn test_final_pass_guardrail_allows_expected_lexicon_cleanup() {
        let raw = "Uzywam doker do github";
        let candidate = "Uzywam Docker do GitHub";

        assert_eq!(final_pass_guardrail_reason(raw, candidate), None);
    }

    /// Custom lexicon mtime change reloads rules without recompiling builtins.
    #[test]
    fn test_hot_reload_picks_up_new_rules() {
        use std::io::Write;

        let dir = tempfile::tempdir().unwrap();
        let custom_path = dir.path().join("lexicon.custom.jsonl");

        // Start with empty file
        std::fs::write(&custom_path, "").unwrap();

        // Build a Lexicon pointing at our temp file
        let mut lexicon = Lexicon {
            builtin_rules: Vec::new(),
            custom_rules: Vec::new(),
            custom_path: custom_path.clone(),
            custom_mtime: std::fs::metadata(&custom_path)
                .ok()
                .and_then(|m| m.modified().ok()),
            protected_canonicals: Vec::new(),
            custom_canonicals: Vec::new(),
        };

        // No rules yet
        assert_eq!(lexicon.apply("foobarski"), "foobarski");

        // Write a custom rule: "foobarski" -> "FooBar"
        // Need a slight delay to ensure mtime changes
        std::thread::sleep(std::time::Duration::from_millis(50));
        let mut f = std::fs::File::create(&custom_path).unwrap();
        writeln!(
            f,
            r#"{{"term":"FooBar","mispronunciations":["foobarski"]}}"#
        )
        .unwrap();
        drop(f);

        // Reload should detect mtime change and pick up new rule
        lexicon.maybe_reload();
        assert_eq!(
            lexicon.apply("mam foobarski w projekcie"),
            "mam FooBar w projekcie"
        );
        assert_eq!(lexicon.rule_count(), 1);
        assert_eq!(lexicon.custom_rules.len(), 1);
        assert_eq!(lexicon.custom_canonicals, vec!["FooBar".to_string()]);
    }

    /// Overlay correction → custom lexicon → hot-reload teaches the next transcript.
    #[test]
    #[serial]
    fn overlay_correction_chain_teaches_custom_lexicon_for_next_transcript() {
        let temp_dir = tempfile::tempdir().expect("temp data dir for quality chain");
        let _data_dir = EnvRestore::capture("CODESCRIBE_DATA_DIR");
        let temp_root = temp_dir
            .path()
            .canonicalize()
            .unwrap_or_else(|_| temp_dir.path().to_path_buf());
        unsafe {
            std::env::set_var("CODESCRIBE_DATA_DIR", &temp_root);
        }

        let candidates =
            crate::quality::overlay_quality::extract_lexicon_candidates("uni agentka", "Junie");
        assert_eq!(candidates, vec![("uni agentka".into(), "Junie".into())]);

        let quality_path = crate::quality::overlay_quality::commit_overlay_correction(
            "uni agentka",
            "uni agentka",
            "Junie",
            "overlay",
            Some("whisper-test".into()),
            Some("copy"),
        )
        .expect("commit overlay correction")
        .quality_path;
        assert!(quality_path.starts_with(&temp_root));
        assert!(quality_path.ends_with("corrections.jsonl"));

        let written = std::fs::read_to_string(&quality_path).expect("read quality record");
        let record: crate::quality::overlay_quality::QualityRecord =
            serde_json::from_str(written.lines().last().expect("quality record line"))
                .expect("parse quality record");
        assert_eq!(record.raw_text, "uni agentka");
        assert_eq!(record.delivered_text, "uni agentka");
        assert_eq!(record.edited_text, "Junie");
        let correction_id = record.logical_id();
        assert_eq!(
            record.meta.get("action").and_then(|v| v.as_str()),
            Some("copy")
        );

        let custom_path = crate::config::Config::config_dir().join("lexicon.custom.jsonl");
        let custom = std::fs::read_to_string(&custom_path).expect("read custom lexicon");
        assert!(custom.contains(r#""term":"Junie""#));
        assert!(custom.contains(r#""uni agentka""#));

        let mut custom_rules = Vec::new();
        let mut custom_canonicals = Vec::new();
        let count = load_legacy_jsonl_with_terms(
            &custom,
            "custom",
            &mut custom_rules,
            Some(&mut custom_canonicals),
        );
        assert_eq!(count, 1);
        assert_eq!(custom_canonicals, vec!["Junie".to_string()]);

        let mut lexicon = Lexicon {
            builtin_rules: Vec::new(),
            custom_rules,
            custom_path: custom_path.clone(),
            custom_mtime: std::fs::metadata(&custom_path)
                .ok()
                .and_then(|metadata| metadata.modified().ok()),
            protected_canonicals: Vec::new(),
            custom_canonicals,
        };
        assert_eq!(lexicon.apply("uni agentka"), "Junie");
        assert_eq!(
            lexicon.apply("Następny transcript: uni agentka."),
            "Następny transcript: Junie."
        );

        std::thread::sleep(std::time::Duration::from_millis(50));
        let outcome = crate::quality::overlay_quality::finalize_voice_lab_correction(
            &correction_id,
            "Junie Prime",
        )
        .expect("finalize Voice Lab correction");
        assert_eq!(outcome.pairs_learned, 1);
        assert_eq!(outcome.record.revision, record.revision + 1);
        assert_eq!(outcome.record.edited_text, "Junie Prime");

        lexicon.maybe_reload();
        assert_eq!(lexicon.custom_rules.len(), 1);
        assert_eq!(lexicon.apply("uni agentka"), "Junie Prime");
        assert_eq!(
            lexicon.apply("Następny transcript: uni agentka."),
            "Następny transcript: Junie Prime."
        );
    }

    /// Plain word→word custom rules are rejected as unsafe language-level rewrites.
    #[test]
    fn test_custom_lexicon_skips_plain_word_regression_rules() {
        let json = r#"
{"term":"zobacz","mispronunciations":["zobaczcie"]}
{"term":"robimy","mispronunciations":["zrobimy", "robi się"]}
{"term":"stary","mispronunciations":["stara"]}
"#;
        let mut custom_rules = Vec::new();

        let count = load_legacy_jsonl_with_terms(json, "custom", &mut custom_rules, None);

        assert_eq!(count, 0, "plain word-to-word custom rules are unsafe");

        let lexicon = Lexicon {
            builtin_rules: Vec::new(),
            custom_rules,
            custom_path: PathBuf::from("/nonexistent/lexicon.custom.jsonl"),
            custom_mtime: None,
            protected_canonicals: Vec::new(),
            custom_canonicals: Vec::new(),
        };
        let input = "Zobaczcie, jeśli nie zrobimy tego teraz, stara.";
        assert_eq!(lexicon.apply(input), input);
    }

    /// Diacritic-only custom rules (e.g. `zazolc` → `zażółć`) remain loadable.
    #[test]
    fn test_custom_lexicon_allows_diacritic_only_rules() {
        let json = r#"{"term":"zażółć","mispronunciations":["zazolc"]}"#;
        let mut custom_rules = Vec::new();

        let count = load_legacy_jsonl_with_terms(json, "custom", &mut custom_rules, None);

        assert_eq!(count, 1);

        let lexicon = Lexicon {
            builtin_rules: Vec::new(),
            custom_rules,
            custom_path: PathBuf::from("/nonexistent/lexicon.custom.jsonl"),
            custom_mtime: None,
            protected_canonicals: Vec::new(),
            custom_canonicals: Vec::new(),
        };
        assert_eq!(lexicon.apply("zazolc gesla jazn"), "zażółć gesla jazn");
    }

    /// Unchanged custom-file mtime makes `maybe_reload` a no-op.
    #[test]
    fn test_hot_reload_no_change_skips_reload() {
        let dir = tempfile::tempdir().unwrap();
        let custom_path = dir.path().join("lexicon.custom.jsonl");
        std::fs::write(
            &custom_path,
            r#"{"term":"Rust","mispronunciations":["rast"]}"#,
        )
        .unwrap();

        let mut lexicon = Lexicon {
            builtin_rules: Vec::new(),
            custom_rules: Vec::new(),
            custom_path: custom_path.clone(),
            custom_mtime: None, // Force initial load
            protected_canonicals: Vec::new(),
            custom_canonicals: Vec::new(),
        };

        // First reload loads the rule
        lexicon.maybe_reload();
        assert_eq!(lexicon.rule_count(), 1);
        let mtime_after = lexicon.custom_mtime;

        // Second reload with same mtime — should be a no-op
        lexicon.maybe_reload();
        assert_eq!(lexicon.rule_count(), 1);
        assert_eq!(lexicon.custom_mtime, mtime_after);
    }

    /// Reloading custom rules must never drop previously compiled builtin rules.
    #[test]
    fn test_hot_reload_preserves_builtin_rules() {
        let dir = tempfile::tempdir().unwrap();
        let custom_path = dir.path().join("lexicon.custom.jsonl");
        std::fs::write(&custom_path, "").unwrap();

        // Simulate 2 builtin rules
        let mut lexicon = Lexicon {
            builtin_rules: vec![
                LexiconRule {
                    pattern: build_word_regex("builtin1").unwrap(),
                    replacement: "BUILTIN1".to_string(),
                },
                LexiconRule {
                    pattern: build_word_regex("builtin2").unwrap(),
                    replacement: "BUILTIN2".to_string(),
                },
            ],
            custom_rules: Vec::new(),
            custom_path: custom_path.clone(),
            custom_mtime: std::fs::metadata(&custom_path)
                .ok()
                .and_then(|m| m.modified().ok()),
            protected_canonicals: Vec::new(),
            custom_canonicals: Vec::new(),
        };

        // Write custom rule
        std::thread::sleep(std::time::Duration::from_millis(50));
        std::fs::write(
            &custom_path,
            r#"{"term":"Custom","mispronunciations":["kastom"]}"#,
        )
        .unwrap();

        lexicon.maybe_reload();

        // Should have 2 builtin + 1 custom = 3 rules
        assert_eq!(lexicon.rule_count(), 3);
        // Builtin rules preserved
        assert_eq!(lexicon.apply("builtin1 builtin2"), "BUILTIN1 BUILTIN2");
        // Custom rule added
        assert_eq!(lexicon.apply("moj kastom kod"), "moj Custom kod");
    }

    /// Every `process` call applies lexicon rewrites regardless of gate history.
    #[test]
    fn test_postprocessor_always_applies_lexicon_contract() {
        // Contract: every call to process() applies lexicon rewrites
        // regardless of semantic gate state or chunk history
        let mut processor = StreamPostProcessor::new();

        // First call — lexicon should rewrite known terms
        let out1 = processor
            .process("Uzywam doker do kontenerow")
            .expect("non-empty");
        assert!(
            out1.contains("Docker"),
            "First call should apply lexicon: {out1}"
        );

        // Second call with different text — still applies lexicon
        let out2 = processor
            .process("Mam git hub repository z kodem")
            .expect("non-empty");
        assert!(
            out2.contains("GitHub"),
            "Second call should apply lexicon: {out2}"
        );
    }

    /// `process` advances session stats (and thus exercised the reload hook path).
    #[test]
    fn test_process_calls_maybe_reload() {
        // Verify that process() calls maybe_reload() by checking stats progression
        let mut processor = StreamPostProcessor::new();
        let _ = processor.process("test jeden");
        let _ = processor.process("test dwa trzy cztery");
        let stats = processor.stats();
        assert_eq!(stats.input_chunks, 2, "Both chunks should be counted");
    }

    /// Legacy rows may store mis-hears under `extras.mispronunciations`.
    #[test]
    fn test_extras_mispronunciations_format() {
        // Veterinary entries store mispronunciations in extras.mispronunciations
        let vet_json = r#"{"term":"Acepromazyna","ipa":"/x/","category":"drug","definition":"x","synonyms":[],"extras":{"mispronunciations":["acepromasyna","acepramazyna"]},"mispronunciations":[]}"#;

        let mut rules = Vec::new();
        let count = load_legacy_jsonl(vet_json, "test-vet", &mut rules);
        assert_eq!(
            count, 2,
            "Should extract 2 rules from extras.mispronunciations"
        );
        assert_eq!(rules[0].replacement, "Acepromazyna");
        assert_eq!(rules[1].replacement, "Acepromazyna");
    }

    /// Top-level and extras mispronunciations merge; case-equal variants are skipped.
    #[test]
    fn test_merged_mispronunciations() {
        // Entry with mispronunciations in both top-level and extras
        let json = r#"{"term":"Anemia","mispronunciations":["anemia"],"extras":{"mispronunciations":["abemia","amemia"]}}"#;

        let mut rules = Vec::new();
        let count = load_legacy_jsonl(json, "test-merge", &mut rules);
        // "anemia" == "Anemia" case-insensitive → skipped; "abemia" + "amemia" → 2 rules
        assert_eq!(count, 2, "Should skip case-equal + extract 2 from extras");
    }

    /// Full builtin load (incl. vet extras) yields a large compiled rule set.
    #[test]
    fn test_builtin_lexicon_loads_vet_extras() {
        // Integration test: the real builtin lexicon must produce > 798 rules now
        let lexicon = Lexicon::from_builtin();
        assert!(
            lexicon.rule_count() > 5000,
            "Expected >5000 rules with extras fix, got {}",
            lexicon.rule_count()
        );
    }

    /// Build a hermetic builtin-only lexicon (programming + seed + protected),
    /// with NO operator custom file, so protected-term regression assertions are
    /// deterministic regardless of the host's ~/.codescribe/lexicon.custom.jsonl.
    fn builtin_only_lexicon() -> Lexicon {
        let mut rules = Vec::new();
        for (label, source) in BUILTIN_LEXICONS {
            load_legacy_jsonl(source, label, &mut rules);
        }
        load_seed_jsonl(SEED_JSONL, "seed", &mut rules);
        load_seed_jsonl(OPERATOR_VOCAB_JSONL, "operator", &mut rules);
        let mut canonicals = Vec::new();
        load_protected_jsonl(
            PROTECTED_TERMS_JSONL,
            "protected",
            &mut rules,
            &mut canonicals,
        );
        Lexicon {
            builtin_rules: rules,
            custom_rules: Vec::new(),
            custom_path: PathBuf::from("/nonexistent/lexicon.custom.jsonl"),
            custom_mtime: None,
            protected_canonicals: canonicals,
            custom_canonicals: Vec::new(),
        }
    }

    /// Acoustic homophone `Luxury` and locktree variants rewrite to `Loctree`.
    #[test]
    fn test_protected_terms_loctree_not_luxury() {
        let lex = builtin_only_lexicon();
        // The reported regression: Whisper/LLM emits the acoustic homophone
        // "Luxury" for the product name. The lexicon must restore "Loctree".
        assert_eq!(
            lex.apply("Odpalam luxury na repo"),
            "Odpalam Loctree na repo"
        );
        assert_eq!(lex.apply("locktree i loktree"), "Loctree i Loctree");
        // Canonical already correct stays correct.
        assert_eq!(lex.apply("Loctree daje sight"), "Loctree daje sight");
    }

    /// Protected source normalizes brand casing (AICX, MCP, GitHub, product names).
    #[test]
    fn test_protected_terms_preserve_brand_casing() {
        let lex = builtin_only_lexicon();
        assert_eq!(lex.apply("vibe crafted"), "Vibecrafted");
        assert_eq!(lex.apply("code scribe"), "Codescribe");
        assert_eq!(lex.apply("vet coders"), "Vetcoders");
        // Case-only normalization (curated protected source only).
        assert_eq!(lex.apply("mam aicx w repo"), "mam AICX w repo");
        assert_eq!(lex.apply("przez mcp"), "przez MCP");
        assert_eq!(lex.apply("a i c x"), "AICX");
        assert_eq!(lex.apply("m c p"), "MCP");
        assert_eq!(lex.apply("github"), "GitHub");
        assert_eq!(lex.apply("git hub"), "GitHub");
    }

    /// Multi-word protected phrases rewrite as whole phrases, not partial tokens.
    #[test]
    fn test_protected_terms_multiword_phrases() {
        let lex = builtin_only_lexicon();
        assert_eq!(lex.apply("fn shift"), "Fn Shift");
        assert_eq!(lex.apply("fun shift"), "Fn Shift");
        assert_eq!(lex.apply("living intent queue"), "Living Intent Queue");
        assert_eq!(
            lex.apply("assistive talk anytime"),
            "Assistive Talk Anytime"
        );
        // Already-correct phrases are preserved verbatim.
        assert_eq!(
            lex.apply("Collapsible Tool Evidence"),
            "Collapsible Tool Evidence"
        );
    }

    /// Ordinary English/Polish without protected terms must pass through unchanged.
    #[test]
    fn test_protected_terms_do_not_overcorrect_ordinary_language() {
        let lex = builtin_only_lexicon();
        // "rest", "harmony", "diesel" exist as case-only variants in
        // programming.jsonl but the legacy loader skips case-equal variants, so
        // ordinary English/Polish must pass through untouched.
        let sentence = "I need some rest in harmony near the diesel engine";
        assert_eq!(lex.apply(sentence), sentence);
        let pl = "To jest zwykłe zdanie bez żadnych nazw własnych";
        assert_eq!(lex.apply(pl), pl);
    }

    /// Polish UI command mis-hears normalize to code tokens (clipboard, screenshot, …).
    #[test]
    fn test_polish_ui_command_phrase_preservation() {
        // Regression class: Polish UI command phrases (and their Whisper
        // mis-hears) must normalize to the canonical code token, never leak the
        // garbage mutant. The reported goblin: "schowku" -> "schopku".
        let lex = builtin_only_lexicon();
        // The reported mutant and the whole "schowek" inflection family collapse
        // to the invariant code token (clipboard never inflects in Polish).
        assert_eq!(lex.apply("wrzuć do schopku"), "wrzuć do clipboard");
        assert_eq!(lex.apply("otwórz schowek"), "otwórz clipboard");
        assert_eq!(lex.apply("wrzuć do schowka"), "wrzuć do clipboard");
        assert_eq!(lex.apply("zajrzyj do schowku"), "zajrzyj do clipboard");
        // Other operator commands normalize to their code token.
        assert_eq!(lex.apply("zrób skrinszot"), "zrób screenshot");
        assert_eq!(lex.apply("zrób zrzut ekranu"), "zrób screenshot");
        assert_eq!(lex.apply("wklej to"), "paste to");
        assert_eq!(lex.apply("pokaż zaznaczenie"), "pokaż selection");
        assert_eq!(lex.apply("zapisz transkrypt"), "zapisz transcript");
        // Ordinary text without command vocabulary is untouched.
        let plain = "To jest zwykłe zdanie o kotach i psach";
        assert_eq!(lex.apply(plain), plain);
    }

    /// `protected_terms_lost` reports canonicals present in raw but missing after a pass.
    #[test]
    fn test_protected_terms_lost_detects_corruption() {
        // Uses the GLOBAL lexicon; builtin protected canonicals (Loctree,
        // Codescribe, MCP, ...) are always present regardless of custom file.
        let lost = protected_terms_lost("I run Loctree through MCP", "I run Luxury through MCP");
        assert_eq!(lost, vec!["Loctree".to_string()]);

        // Nothing lost when the term survives.
        let none = protected_terms_lost("Codescribe is great", "Codescribe is wonderful");
        assert!(none.is_empty());
    }

    /// Whole-word `\b` boundaries must not rewrite substrings inside longer tokens.
    #[test]
    fn python_whole_word_boundary_does_not_corrupt_wordpython() {
        // Regression: build_word_regex uses \b so "Python" must not rewrite
        // the embedded sequence inside "WordPython".
        let lexicon = builtin_only_lexicon();
        // Explicit rule: pajton -> Python (programming.jsonl). Whole-word only.
        let out = lexicon.apply("WordPython and pajton rocks");
        assert!(
            out.contains("WordPython"),
            "must not corrupt WordPython, got {out}"
        );
        assert!(
            out.contains("Python"),
            "standalone pajton should become Python, got {out}"
        );
        // Stronger: applying "Python" as variant pattern must not hit WordPython.
        let re = build_word_regex("Python").expect("Python word regex");
        let rewritten = re.replace_all("WordPython", "XX");
        assert_eq!(
            rewritten, "WordPython",
            "\\bPython\\b must not match inside WordPython"
        );
    }

    /// Re-applying the lexicon on canonical text reaches a fixed point without drift.
    #[test]
    fn test_apply_lexicon_is_idempotent_on_canonical() {
        // Re-applying after an LLM pass must reach a fixed point (no oscillation /
        // corruption). Uses the GLOBAL lexicon, so we only assert robustness
        // properties that an operator custom file cannot flip: the pass converges
        // and Loctree/AICX/MCP (which no builtin/operator rule downgrades) survive.
        let once = apply_lexicon("Loctree, AICX and MCP keep working");
        let twice = apply_lexicon(&once);
        assert_eq!(once, twice, "lexicon apply must be idempotent on canonical");
        assert!(once.contains("Loctree"));
        assert!(once.contains("AICX"));
        assert!(once.contains("MCP"));
    }
}
