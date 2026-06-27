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

const BUILTIN_LEXICONS: &[(&str, &str)] = &[(
    "programming",
    include_str!("../../assets/programming.jsonl"),
)];
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

const DEFAULT_SIMILARITY_THRESHOLD: f32 = 0.93;
const DEFAULT_NOVELTY_THRESHOLD: f32 = 0.12;
const MAX_EMBED_CHARS: usize = 512;
const MAX_DROPS_IN_ROW: u8 = 2;
const FINAL_PASS_ARTIFACT_TOKENS: &[&str] = &["going", "use"];

lazy_static! {
    // Whisper sometimes emits trailing emoticon artifacts like ":D", ":-D", "::D", often repeated.
    // We strip them only at the end of the utterance.
    static ref TRAILING_SMILEY_D_RE: Regex =
        Regex::new(r"(?i)(?:\s*:+-?d)+(?:\s*:+\s*)*$").expect("trailing :D regex");
}

#[derive(Debug, Deserialize)]
struct LexiconExtras {
    #[serde(default)]
    mispronunciations: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct LegacyEntry {
    term: String,
    #[serde(default)]
    mispronunciations: Vec<String>,
    #[serde(default)]
    extras: Option<LexiconExtras>,
}

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

#[derive(Debug, Deserialize)]
struct SeedEntry {
    canonical: String,
    #[serde(default)]
    normalization: SeedNormalization,
}

#[derive(Debug)]
struct LexiconRule {
    pattern: Regex,
    replacement: String,
}

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
}

static GLOBAL_LEXICON: LazyLock<RwLock<Lexicon>> = LazyLock::new(|| {
    let lex = Lexicon::from_builtin();
    info!(
        "Global lexicon singleton initialized: {} rules",
        lex.rule_count()
    );
    RwLock::new(lex)
});

impl Lexicon {
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
        let custom_count = load_custom_lexicon()
            .map(|content| load_legacy_jsonl(&content, "custom", &mut custom_rules))
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
        }
    }

    fn maybe_reload(&mut self) {
        let current_mtime = fs::metadata(&self.custom_path)
            .ok()
            .and_then(|m| m.modified().ok());
        if current_mtime == self.custom_mtime {
            return;
        }
        self.custom_rules.clear();
        let custom_count = fs::read_to_string(&self.custom_path)
            .ok()
            .filter(|c| !c.trim().is_empty())
            .map(|content| load_legacy_jsonl(&content, "custom", &mut self.custom_rules))
            .unwrap_or(0);
        self.custom_mtime = current_mtime;
        info!(
            "Hot-reloaded {} custom lexicon rules (total={}, custom_path={})",
            custom_count,
            self.rule_count(),
            self.custom_path.display(),
        );
    }

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

    fn rule_count(&self) -> usize {
        self.builtin_rules.len() + self.custom_rules.len()
    }
}

fn maybe_reload_global_lexicon() {
    let mut lexicon = GLOBAL_LEXICON
        .write()
        .expect("global lexicon write lock poisoned");
    lexicon.maybe_reload();
}

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

fn load_legacy_jsonl(source: &str, label: &str, rules: &mut Vec<LexiconRule>) -> usize {
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

        // Merge top-level mispronunciations with extras.mispronunciations
        // (veterinary.jsonl stores them in extras, programming.jsonl at top level)
        let mut all_mis = entry.mispronunciations;
        if let Some(extras) = entry.extras {
            all_mis.extend(extras.mispronunciations);
        }

        for mis in all_mis.iter() {
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

#[derive(Debug)]
struct SemanticGate {
    last_embedding: Option<Vec<f32>>,
    last_tokens: HashSet<String>,
    drops_in_row: u8,
    similarity_threshold: f32,
    novelty_threshold: f32,
}

impl SemanticGate {
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

    fn observe(&mut self, text: &str) {
        let tokens = tokenize(text);
        self.last_tokens = tokens.into_iter().collect();
        self.last_embedding = self.semantic_embedding(text);
        self.drops_in_row = 0;
    }

    fn semantic_similarity(&mut self, text: &str) -> Option<f32> {
        let new_emb = self.semantic_embedding(text)?;
        let last_emb = self.last_embedding.as_ref()?;
        Some(cosine_similarity(&new_emb, last_emb))
    }

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

#[derive(Debug)]
pub struct StreamPostProcessor {
    gate: SemanticGate,
    stats: StreamPostProcessStats,
}

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

    pub fn stats(&self) -> StreamPostProcessStats {
        self.stats.clone()
    }
}

impl Default for StreamPostProcessor {
    fn default() -> Self {
        Self::new()
    }
}

fn env_f32(key: &str, default: f32) -> f32 {
    std::env::var(key)
        .ok()
        .and_then(|v| v.parse::<f32>().ok())
        .unwrap_or(default)
}

fn env_bool(key: &str) -> bool {
    std::env::var(key)
        .ok()
        .map(|v| {
            let v = v.trim();
            v == "1" || v.eq_ignore_ascii_case("true")
        })
        .unwrap_or(false)
}

fn embeddings_enabled() -> bool {
    if env_bool("CODESCRIBE_STREAM_DISABLE_EMBEDDINGS") {
        return false;
    }

    if cfg!(test) && !env_bool("CODESCRIBE_STREAM_FORCE_EMBEDDINGS") {
        return false;
    }

    true
}

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

fn normalize_whitespace(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

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

fn truncate_for_embedding(text: &str) -> String {
    if text.len() <= MAX_EMBED_CHARS {
        return text.to_string();
    }

    text.chars().take(MAX_EMBED_CHARS).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn test_cleanup_and_whitespace() {
        let mut processor = StreamPostProcessor::new();
        let input = "To jest to jest to jest   bardzo  wazny \n test systemu.";
        let output = processor.process(input).expect("expected output");
        assert_eq!(output, "To jest bardzo wazny test systemu.");
    }

    #[test]
    fn test_strip_trailing_smiley_d() {
        let mut processor = StreamPostProcessor::new();
        let input = "Siema, czy jestes tam? :D :";
        let output = processor.process_utterance(input).expect("expected output");
        assert_eq!(output, "Siema, czy jestes tam?");
    }

    #[test]
    fn test_is_suspicious_heuristics() {
        assert!(is_suspicious("ok"));
        assert!(is_suspicious("test test test test"));
        assert!(!is_suspicious(
            "To jest normalny tekst bez powtorzen i z roznymi slowami."
        ));
    }

    #[test]
    fn test_final_pass_guardrail_rejects_artifact_token_drift() {
        let raw = "Co będę robił? Ja chyba coś nagrywam? Ja coś się... Może zhulać, ale w tym momencie myślę, że kwestia";
        let candidate = "Co będę robił? Ja chyba coś nagrywam? Ja coś going... Może zhulać, ale w tym momencie myślę, use kwestia";

        let reason = final_pass_guardrail_reason(raw, candidate).expect("expected guardrail");
        assert_eq!(reason, "artifact_token_drift:going,use");
    }

    #[test]
    fn test_final_pass_guardrail_allows_expected_lexicon_cleanup() {
        let raw = "Uzywam doker do github";
        let candidate = "Uzywam Docker do GitHub";

        assert_eq!(final_pass_guardrail_reason(raw, candidate), None);
    }

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
    }

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

    #[test]
    fn test_process_calls_maybe_reload() {
        // Verify that process() calls maybe_reload() by checking stats progression
        let mut processor = StreamPostProcessor::new();
        let _ = processor.process("test jeden");
        let _ = processor.process("test dwa trzy cztery");
        let stats = processor.stats();
        assert_eq!(stats.input_chunks, 2, "Both chunks should be counted");
    }

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

    #[test]
    fn test_merged_mispronunciations() {
        // Entry with mispronunciations in both top-level and extras
        let json = r#"{"term":"Anemia","mispronunciations":["anemia"],"extras":{"mispronunciations":["abemia","amemia"]}}"#;

        let mut rules = Vec::new();
        let count = load_legacy_jsonl(json, "test-merge", &mut rules);
        // "anemia" == "Anemia" case-insensitive → skipped; "abemia" + "amemia" → 2 rules
        assert_eq!(count, 2, "Should skip case-equal + extract 2 from extras");
    }

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
        }
    }

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

    #[test]
    fn test_protected_terms_preserve_brand_casing() {
        let lex = builtin_only_lexicon();
        assert_eq!(lex.apply("vibe crafted"), "Vibecrafted");
        assert_eq!(lex.apply("code scribe"), "CodeScribe");
        assert_eq!(lex.apply("vet coders"), "VetCoders");
        // Case-only normalization (curated protected source only).
        assert_eq!(lex.apply("mam aicx w repo"), "mam AICX w repo");
        assert_eq!(lex.apply("przez mcp"), "przez MCP");
        assert_eq!(lex.apply("a i c x"), "AICX");
        assert_eq!(lex.apply("m c p"), "MCP");
        assert_eq!(lex.apply("github"), "GitHub");
        assert_eq!(lex.apply("git hub"), "GitHub");
    }

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

    #[test]
    fn test_protected_terms_lost_detects_corruption() {
        // Uses the GLOBAL lexicon; builtin protected canonicals (Loctree,
        // CodeScribe, MCP, ...) are always present regardless of custom file.
        let lost = protected_terms_lost("I run Loctree through MCP", "I run Luxury through MCP");
        assert_eq!(lost, vec!["Loctree".to_string()]);

        // Nothing lost when the term survives.
        let none = protected_terms_lost("CodeScribe is great", "CodeScribe is wonderful");
        assert!(none.is_empty());
    }

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
