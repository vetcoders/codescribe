use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

use anyhow::Result;
use fastembed::{EmbeddingModel, TextEmbedding, TextInitOptions};
use regex::Regex;
use serde::Deserialize;
use tracing::{debug, info, warn};

use crate::config::Config;

const BUILTIN_LEXICON: &str = include_str!("../assets/programming.jsonl");
const DEFAULT_SIMILARITY_THRESHOLD: f32 = 0.93;
const DEFAULT_NOVELTY_THRESHOLD: f32 = 0.12;
const MAX_EMBED_CHARS: usize = 512;
const MAX_DROPS_IN_ROW: u8 = 2;

static EMBEDDER: OnceLock<Mutex<Option<TextEmbedding>>> = OnceLock::new();

#[derive(Debug, Deserialize)]
struct LexiconEntry {
    term: String,
    mispronunciations: Vec<String>,
}

#[derive(Debug)]
struct LexiconRule {
    pattern: Regex,
    replacement: String,
}

#[derive(Debug, Default)]
struct Lexicon {
    rules: Vec<LexiconRule>,
}

impl Lexicon {
    fn from_builtin() -> Self {
        let mut rules = Vec::new();

        for (idx, line) in BUILTIN_LEXICON.lines().enumerate() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }

            let entry: LexiconEntry = match serde_json::from_str(line) {
                Ok(entry) => entry,
                Err(e) => {
                    warn!("Lexicon line {} failed to parse: {}", idx + 1, e);
                    continue;
                }
            };

            for mis in entry.mispronunciations.iter() {
                if mis.eq_ignore_ascii_case(&entry.term) {
                    continue;
                }

                if let Some(pattern) = build_word_regex(mis) {
                    rules.push(LexiconRule {
                        pattern,
                        replacement: entry.term.clone(),
                    });
                }
            }
        }

        if !rules.is_empty() {
            info!("Loaded {} lexicon rules (builtin)", rules.len());
        } else {
            warn!("No lexicon rules loaded from builtin lexicon");
        }

        Self { rules }
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
        let embedder = EMBEDDER.get_or_init(|| Mutex::new(None));
        let mut guard = embedder.lock().ok()?;

        if guard.is_none() {
            match init_embedder() {
                Ok(model) => {
                    *guard = Some(model);
                }
                Err(e) => {
                    warn!("Failed to initialize BGEM3 embedder: {}", e);
                    return None;
                }
            }
        }

        let model = guard.as_mut()?;
        let input = truncate_for_embedding(text);
        let embeddings = model.embed(vec![input.as_str()], None).ok()?;
        embeddings.into_iter().next()
    }
}

#[derive(Debug)]
pub struct StreamPostProcessor {
    lexicon: Lexicon,
    gate: SemanticGate,
}

impl StreamPostProcessor {
    pub fn new() -> Self {
        Self {
            lexicon: Lexicon::from_builtin(),
            gate: SemanticGate::new(),
        }
    }

    pub fn process(&mut self, text: &str) -> Option<String> {
        if text.trim().is_empty() {
            return None;
        }

        let mut cleaned = self.lexicon.apply(text);
        cleaned = cleanup_artifacts(&cleaned);
        cleaned = normalize_whitespace(&cleaned);

        if cleaned.trim().is_empty() {
            return None;
        }

        if is_suspicious(&cleaned) && self.gate.should_drop(&cleaned) {
            return None;
        }

        self.gate.observe(&cleaned);
        Some(cleaned)
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

fn cleanup_artifacts(text: &str) -> String {
    if crate::ai_formatting::has_repetition_loop(text) {
        return crate::ai_formatting::remove_simple_repetitions(text);
    }
    text.to_string()
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

fn init_embedder() -> Result<TextEmbedding> {
    let cache_dir = embedding_cache_dir();
    std::fs::create_dir_all(&cache_dir)?;

    info!("Initializing BGEM3 embedder (this can take a while on first run)");
    let options = TextInitOptions::new(EmbeddingModel::BGEM3)
        .with_max_length(256)
        .with_cache_dir(cache_dir)
        .with_show_download_progress(true);

    TextEmbedding::try_new(options)
}

fn embedding_cache_dir() -> PathBuf {
    Config::config_dir().join("embeddings")
}

fn truncate_for_embedding(text: &str) -> String {
    if text.len() <= MAX_EMBED_CHARS {
        return text.to_string();
    }

    text.chars().take(MAX_EMBED_CHARS).collect()
}
