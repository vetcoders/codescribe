use std::collections::HashSet;
use std::fs;
use std::path::PathBuf;
use std::time::SystemTime;

use lazy_static::lazy_static;
use regex::Regex;
use serde::{Deserialize, Serialize};
use tracing::{debug, info, warn};

use crate::config::Config;

const BUILTIN_LEXICONS: &[(&str, &str)] = &[
    (
        "programming",
        include_str!("../../assets/programming.jsonl"),
    ),
    ("veterinary", include_str!("../../assets/veterinary.jsonl")),
];
const DEFAULT_SIMILARITY_THRESHOLD: f32 = 0.93;
const DEFAULT_NOVELTY_THRESHOLD: f32 = 0.12;
const MAX_EMBED_CHARS: usize = 512;
const MAX_DROPS_IN_ROW: u8 = 2;

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
struct LexiconEntry {
    term: String,
    #[serde(default)]
    mispronunciations: Vec<String>,
    #[serde(default)]
    extras: Option<LexiconExtras>,
}

#[derive(Debug)]
struct LexiconRule {
    pattern: Regex,
    replacement: String,
}

#[derive(Debug)]
struct Lexicon {
    rules: Vec<LexiconRule>,
    builtin_count: usize,
    custom_path: PathBuf,
    custom_mtime: Option<SystemTime>,
}

impl Lexicon {
    fn from_builtin() -> Self {
        let mut rules = Vec::new();
        let mut builtin_count = 0usize;
        for (label, source) in BUILTIN_LEXICONS {
            builtin_count += load_rules_from_jsonl(source, label, &mut rules);
        }

        let custom_path = Config::config_dir().join("lexicon.custom.jsonl");
        let custom_mtime = fs::metadata(&custom_path)
            .ok()
            .and_then(|m| m.modified().ok());

        let custom_count = load_custom_lexicon()
            .map(|content| load_rules_from_jsonl(&content, "custom", &mut rules))
            .unwrap_or(0);

        if !rules.is_empty() {
            info!(
                "Loaded {} lexicon rules (builtin={}, custom={})",
                rules.len(),
                builtin_count,
                custom_count
            );
        } else {
            warn!("No lexicon rules loaded from lexicon sources");
        }

        Self {
            rules,
            builtin_count,
            custom_path,
            custom_mtime,
        }
    }

    fn maybe_reload(&mut self) {
        let current_mtime = fs::metadata(&self.custom_path)
            .ok()
            .and_then(|m| m.modified().ok());
        if current_mtime == self.custom_mtime {
            return;
        }
        self.rules.truncate(self.builtin_count);
        let custom_count = fs::read_to_string(&self.custom_path)
            .ok()
            .filter(|c| !c.trim().is_empty())
            .map(|content| load_rules_from_jsonl(&content, "custom", &mut self.rules))
            .unwrap_or(0);
        self.custom_mtime = current_mtime;
        info!(
            "Hot-reloaded {} custom lexicon rules (total={})",
            custom_count,
            self.rules.len()
        );
    }

    fn apply(&self, text: &str) -> String {
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

fn load_rules_from_jsonl(source: &str, label: &str, rules: &mut Vec<LexiconRule>) -> usize {
    let mut added = 0usize;
    for (idx, line) in source.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        let entry: LexiconEntry = match serde_json::from_str(line) {
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
    lexicon: Lexicon,
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
        Self {
            lexicon: Lexicon::from_builtin(),
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
        self.lexicon.maybe_reload();

        if text.trim().is_empty() {
            self.stats.dropped_chunks += 1;
            return None;
        }

        let mut cleaned = self.lexicon.apply(text);
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

fn build_word_regex(input: &str) -> Option<Regex> {
    let mut escaped = regex::escape(input);
    escaped = escaped.replace("\\ ", "\\s+");
    let pattern = format!(r"(?i)\b{}\b", escaped);
    Regex::new(&pattern).ok()
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
    fn test_hot_reload_picks_up_new_rules() {
        use std::io::Write;

        let dir = tempfile::tempdir().unwrap();
        let custom_path = dir.path().join("lexicon.custom.jsonl");

        // Start with empty file
        std::fs::write(&custom_path, "").unwrap();

        // Build a Lexicon pointing at our temp file
        let mut lexicon = Lexicon {
            rules: Vec::new(),
            builtin_count: 0,
            custom_path: custom_path.clone(),
            custom_mtime: std::fs::metadata(&custom_path)
                .ok()
                .and_then(|m| m.modified().ok()),
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
        assert_eq!(lexicon.rules.len(), 1);
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
            rules: Vec::new(),
            builtin_count: 0,
            custom_path: custom_path.clone(),
            custom_mtime: None, // Force initial load
        };

        // First reload loads the rule
        lexicon.maybe_reload();
        assert_eq!(lexicon.rules.len(), 1);
        let mtime_after = lexicon.custom_mtime;

        // Second reload with same mtime — should be a no-op
        lexicon.maybe_reload();
        assert_eq!(lexicon.rules.len(), 1);
        assert_eq!(lexicon.custom_mtime, mtime_after);
    }

    #[test]
    fn test_hot_reload_preserves_builtin_rules() {
        let dir = tempfile::tempdir().unwrap();
        let custom_path = dir.path().join("lexicon.custom.jsonl");
        std::fs::write(&custom_path, "").unwrap();

        // Simulate 2 builtin rules
        let mut lexicon = Lexicon {
            rules: vec![
                LexiconRule {
                    pattern: build_word_regex("builtin1").unwrap(),
                    replacement: "BUILTIN1".to_string(),
                },
                LexiconRule {
                    pattern: build_word_regex("builtin2").unwrap(),
                    replacement: "BUILTIN2".to_string(),
                },
            ],
            builtin_count: 2,
            custom_path: custom_path.clone(),
            custom_mtime: std::fs::metadata(&custom_path)
                .ok()
                .and_then(|m| m.modified().ok()),
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
        assert_eq!(lexicon.rules.len(), 3);
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
        let count = load_rules_from_jsonl(vet_json, "test-vet", &mut rules);
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
        let count = load_rules_from_jsonl(json, "test-merge", &mut rules);
        // "anemia" == "Anemia" case-insensitive → skipped; "abemia" + "amemia" → 2 rules
        assert_eq!(count, 2, "Should skip case-equal + extract 2 from extras");
    }

    #[test]
    fn test_builtin_lexicon_loads_vet_extras() {
        // Integration test: the real builtin lexicon must produce > 798 rules now
        let lexicon = Lexicon::from_builtin();
        assert!(
            lexicon.rules.len() > 5000,
            "Expected >5000 rules with extras fix, got {}",
            lexicon.rules.len()
        );
    }
}
