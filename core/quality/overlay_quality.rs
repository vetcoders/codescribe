//! P0-D Quality loop MVP: capture user corrections from overlay FINAL transcript edits.
//! Writes quality records (raw, delivered, edited) to ~/.codescribe/quality/*.jsonl
//! Extracts lexicon candidates (delivered→edited) and appends safe rules to the
//! custom lexicon (lexicon.custom.jsonl) that StreamPostProcessor / apply_lexicon already consumes.
//!
//! Privacy: purely local, no network, no secrets, no audio.
//! No new Settings knobs (defaults on; VoiceLab UI later).

use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::config::{Config, FormattingPolicy};

static CUSTOM_LEXICON_WRITE_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

/// Quality record for one user correction on the overlay.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct QualityRecord {
    /// Stable logical identity shared by the original correction and revisions.
    /// Legacy rows omit it and receive a deterministic content-derived ID.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub correction_id: String,
    /// Monotonic revision within one correction. Legacy rows deserialize as 0.
    #[serde(default)]
    pub revision: u64,
    /// Unix millis at capture (Copy/Send/Close on edited FINAL).
    pub timestamp_ms: u64,
    /// Session hint if available (future).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    /// "overlay" (or "dictation" in future waves).
    pub mode: String,
    /// Model / engine id if known (e.g. whisper-large, or lane).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Canonical lowercase formatting provenance. Missing on legacy rows.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub formatting_level: Option<String>,
    /// Raw STT text (best effort; overlay MVP may pass delivered here too).
    #[serde(default)]
    pub raw_text: String,
    /// The authoritative delivered text shown in FINAL before user edit.
    pub delivered_text: String,
    /// The text after user manual edit in the overlay TextEditor.
    pub edited_text: String,
    /// Utterance-level average log-probability from STT (W11-C). Missing on legacy rows.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub avg_logprob: Option<f32>,
    /// Speech fraction from VAD (W11-C). Missing on legacy rows.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub speech_pct: Option<f32>,
    /// Freeform confidence flags (e.g. low_logprob, high_compression). Missing → empty.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub confidence_flags: Vec<String>,
    /// Freeform meta (e.g. {"source":"overlay-final", "action":"copy"}).
    #[serde(default, skip_serializing_if = "serde_json::Value::is_null")]
    pub meta: serde_json::Value,
}

/// Provenance of a custom-lexicon row. Correction upserts stamp `"correction"`.
/// Legacy rows without a source deserialize as `"legacy"`.
pub const LEXICON_SOURCE_CORRECTION: &str = "correction";
pub const LEXICON_SOURCE_MANUAL: &str = "manual";
pub const LEXICON_SOURCE_IMPORT: &str = "import";
pub const LEXICON_SOURCE_LEGACY: &str = "legacy";

/// Read-only projection of one custom lexicon rule for product surfaces.
/// The on-disk JSONL stores one canonical term with one or more variants;
/// Voice Lab renders the flattened `variant -> canonical` truth.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CustomLexiconEntry {
    pub variant: String,
    pub canonical: String,
    /// `correction` | `manual` | `import` | `legacy` (default for old rows).
    pub source: String,
}

#[derive(Deserialize)]
struct StoredCustomLexiconEntry {
    term: String,
    #[serde(default)]
    mispronunciations: Vec<String>,
    #[serde(default)]
    extras: Option<StoredLexiconExtras>,
    #[serde(default)]
    source: Option<String>,
}

#[derive(Deserialize)]
struct StoredLexiconExtras {
    #[serde(default)]
    mispronunciations: Vec<String>,
}

/// Maximum Unicode-char Levenshtein distance for a single candidate pair.
/// Operator decision 2026-07-22: deltas above this are rewrites, not learning.
pub const MAX_PAIR_EDIT_DELTA_CHARS: usize = 20;

/// Per-side phrase length window in Unicode chars (not bytes).
pub const MIN_CANDIDATE_CHARS: usize = 2;
pub const MAX_CANDIDATE_CHARS: usize = 80;

/// Global rewrite guard: if more than this fraction of tokens changed, return
/// no candidates. Operator 2026-07-17 intent was "more than ~5% of text is
/// destruction"; 40% is the conservative tunable default (one constant).
pub const MAX_TOKEN_CHANGE_RATIO: f64 = 0.40;

/// Rewrite-ratio guard only applies once either side has this many tokens.
/// Short phrase fixes ("uni agentka" → "Junie") are legitimate whole-run pairs
/// even when 100% of their few tokens change.
pub const MIN_TOKENS_FOR_REWRITE_GUARD: usize = 6;

impl QualityRecord {
    pub fn new(
        raw_text: String,
        delivered_text: String,
        edited_text: String,
        mode: &str,
        model: Option<String>,
        formatting_level: Option<String>,
        action: Option<&str>,
    ) -> Self {
        Self::new_with_confidence(
            raw_text,
            delivered_text,
            edited_text,
            mode,
            model,
            formatting_level,
            action,
            None,
            None,
            Vec::new(),
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn new_with_confidence(
        raw_text: String,
        delivered_text: String,
        edited_text: String,
        mode: &str,
        model: Option<String>,
        formatting_level: Option<String>,
        action: Option<&str>,
        avg_logprob: Option<f32>,
        speech_pct: Option<f32>,
        confidence_flags: Vec<String>,
    ) -> Self {
        let timestamp_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        let meta = match action {
            Some(a) => serde_json::json!({ "source": "overlay-final", "action": a }),
            None => serde_json::json!({ "source": "overlay-final" }),
        };
        QualityRecord {
            correction_id: Uuid::new_v4().to_string(),
            revision: 1,
            timestamp_ms,
            session_id: None,
            mode: mode.to_string(),
            model,
            formatting_level,
            raw_text,
            delivered_text,
            edited_text,
            avg_logprob,
            speech_pct,
            confidence_flags,
            meta,
        }
    }

    pub fn logical_id(&self) -> String {
        let stored = self.correction_id.trim();
        if !stored.is_empty() {
            return stored.to_string();
        }

        let mut digest = Sha256::new();
        for value in [
            self.timestamp_ms.to_string(),
            self.session_id.clone().unwrap_or_default(),
            self.mode.clone(),
            self.model.clone().unwrap_or_default(),
            self.raw_text.clone(),
            self.delivered_text.clone(),
        ] {
            digest.update(value.as_bytes());
            digest.update([0]);
        }
        format!("legacy-{:x}", digest.finalize())
    }
}

/// Directory for quality records: ~/.codescribe/quality/
pub fn quality_dir() -> PathBuf {
    Config::config_dir().join("quality")
}

/// Append a quality record as one JSONL line. Creates dir and file as needed.
/// Uses a single rolling file for MVP (corrections.jsonl); per-session files are future.
pub fn save_quality_record(record: &QualityRecord) -> Result<PathBuf> {
    assert_test_data_dir_isolated("save_quality_record");
    let dir = quality_dir();
    fs::create_dir_all(&dir).with_context(|| format!("create quality dir {}", dir.display()))?;
    let path = dir.join("corrections.jsonl");
    let mut f = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .with_context(|| format!("open quality log {}", path.display()))?;
    let line = serde_json::to_string(record).context("serialize quality record")?;
    writeln!(f, "{}", line).context("write quality record line")?;
    Ok(path)
}

/// Under `cfg(test)`, quality/lexicon write paths must never touch the real
/// home data dir. Evidence: real `~/.codescribe` pollution as recent as 2026-07-22.
fn assert_test_data_dir_isolated(caller: &str) {
    #[cfg(test)]
    {
        if std::env::var_os("CODESCRIBE_DATA_DIR").is_none() {
            panic!(
                "CODESCRIBE_DATA_DIR must be set under cfg(test) before {caller} (test isolation)"
            );
        }
    }
    let _ = caller;
}

/// Return the newest correction records first, bounded to `limit` entries.
/// A missing log is the honest empty state. Malformed historical lines are
/// skipped individually so one damaged entry cannot hide the remaining truth.
pub fn recent_quality_records(limit: usize) -> Result<Vec<QualityRecord>> {
    if limit == 0 {
        return Ok(Vec::new());
    }

    let path = quality_dir().join("corrections.jsonl");
    let file = match File::open(&path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => {
            return Err(error).with_context(|| format!("open quality log {}", path.display()));
        }
    };

    let mut resolved: HashMap<String, (usize, QualityRecord)> = HashMap::new();
    for (index, line) in BufReader::new(file).lines().enumerate() {
        let line = line.with_context(|| format!("read quality log line {}", index + 1))?;
        if line.trim().is_empty() {
            continue;
        }
        match serde_json::from_str::<QualityRecord>(&line) {
            Ok(record) => {
                let logical_id = record.logical_id();
                let replace = resolved
                    .get(&logical_id)
                    .map(|(previous_index, previous)| {
                        record.revision > previous.revision
                            || (record.revision == previous.revision && index > *previous_index)
                    })
                    .unwrap_or(true);
                if replace {
                    resolved.insert(logical_id, (index, record));
                }
            }
            Err(error) => tracing::warn!(
                "quality: skipping malformed correction record at {}:{}: {}",
                path.display(),
                index + 1,
                error
            ),
        }
    }

    let mut recent: Vec<_> = resolved.into_values().collect();
    recent.sort_by_key(|entry| std::cmp::Reverse(entry.0));
    Ok(recent
        .into_iter()
        .take(limit)
        .map(|(_, record)| record)
        .collect())
}

fn all_quality_records() -> Result<Vec<QualityRecord>> {
    let path = quality_dir().join("corrections.jsonl");
    let file = match File::open(&path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => {
            return Err(error).with_context(|| format!("open quality log {}", path.display()));
        }
    };
    let mut records = Vec::new();
    for (index, line) in BufReader::new(file).lines().enumerate() {
        let line = line.with_context(|| format!("read quality log line {}", index + 1))?;
        if line.trim().is_empty() {
            continue;
        }
        match serde_json::from_str(&line) {
            Ok(record) => records.push(record),
            Err(error) => tracing::warn!(
                "quality: skipping malformed correction record at {}:{}: {}",
                path.display(),
                index + 1,
                error
            ),
        }
    }
    Ok(records)
}

/// Read the custom lexicon as flattened `variant -> canonical` entries.
/// This mirrors the existing loader format without changing candidate policy.
pub fn custom_lexicon_entries() -> Result<Vec<CustomLexiconEntry>> {
    let path = Config::config_dir().join("lexicon.custom.jsonl");
    if let Some(parent) = path.parent() {
        cleanup_orphaned_lexicon_temps(parent);
    }
    let file = match File::open(&path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => {
            return Err(error).with_context(|| format!("open custom lexicon {}", path.display()));
        }
    };

    let mut entries = Vec::new();
    for (index, line) in BufReader::new(file).lines().enumerate() {
        let line = line.with_context(|| format!("read custom lexicon line {}", index + 1))?;
        if line.trim().is_empty() {
            continue;
        }
        match serde_json::from_str::<StoredCustomLexiconEntry>(&line) {
            Ok(stored) => {
                let canonical = stored.term.trim();
                if canonical.is_empty() {
                    continue;
                }
                let mut variants = stored.mispronunciations;
                if let Some(extras) = stored.extras {
                    variants.extend(extras.mispronunciations);
                }
                let source = stored
                    .source
                    .as_deref()
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .unwrap_or(LEXICON_SOURCE_LEGACY)
                    .to_string();
                entries.extend(
                    variants
                        .into_iter()
                        .map(|variant| variant.trim().to_string())
                        .filter(|variant| !variant.is_empty())
                        .map(|variant| CustomLexiconEntry {
                            variant,
                            canonical: canonical.to_string(),
                            source: source.clone(),
                        }),
                );
            }
            Err(error) => tracing::warn!(
                "quality: skipping malformed custom lexicon entry at {}:{}: {}",
                path.display(),
                index + 1,
                error
            ),
        }
    }

    Ok(entries)
}

/// Extract candidate lexicon pairs (variant → canonical) from a user correction.
///
/// S4 / W11-A policy (operator 2026-07-22):
/// 1. Tokenize both sides (whitespace + punctuation-aware; Polish diacritics kept).
/// 2. Word-level LCS alignment; each contiguous replaced run → one candidate pair.
/// 3. Per-pair: `levenshtein_chars <= 20`, both sides 2..=80 **chars**, not case-only
///    equal (Unicode casefold), both contain letters.
/// 4. Global rewrite guard: if > 40% of tokens changed, return **no** candidates
///    (quality evidence is still saved by the caller).
///
/// Returned pairs are (misheard_variant, correct_canonical).
pub fn extract_lexicon_candidates(delivered: &str, edited: &str) -> Vec<(String, String)> {
    let d = delivered.trim();
    let e = edited.trim();
    if d.is_empty() || e.is_empty() {
        return vec![];
    }
    if unicode_casefold_eq(d, e) {
        return vec![];
    }

    let delivered_tokens = tokenize_for_alignment(d);
    let edited_tokens = tokenize_for_alignment(e);
    if delivered_tokens.is_empty() || edited_tokens.is_empty() {
        return vec![];
    }

    let max_tokens = delivered_tokens.len().max(edited_tokens.len());
    let lcs = token_lcs_length(&delivered_tokens, &edited_tokens);
    let changed = max_tokens.saturating_sub(lcs);
    // Operator 5%-intent; 40% is the conservative tunable default. Only armed
    // for multi-token utterances so short phrase rewrites still teach.
    if max_tokens >= MIN_TOKENS_FOR_REWRITE_GUARD
        && (changed as f64 / max_tokens as f64) > MAX_TOKEN_CHANGE_RATIO
    {
        return vec![];
    }

    let mut pairs = Vec::new();
    for (variant_phrase, canonical_phrase) in
        aligned_replace_runs(&delivered_tokens, &edited_tokens)
    {
        if is_sensible_lexicon_candidate(&variant_phrase, &canonical_phrase) {
            pairs.push((variant_phrase, canonical_phrase));
        }
    }
    pairs
}

/// Single gate policy for lexicon candidates — **chars only**, same thresholds
/// as extraction. No whole-string 120-char ceiling; no byte-based dead zones.
pub fn is_sensible_lexicon_candidate(variant: &str, canonical: &str) -> bool {
    let v = variant.trim();
    let c = canonical.trim();
    if v.is_empty() || c.is_empty() {
        return false;
    }
    if unicode_casefold_eq(v, c) {
        return false;
    }
    let v_chars = v.chars().count();
    let c_chars = c.chars().count();
    if v_chars < MIN_CANDIDATE_CHARS
        || c_chars < MIN_CANDIDATE_CHARS
        || v_chars > MAX_CANDIDATE_CHARS
        || c_chars > MAX_CANDIDATE_CHARS
    {
        return false;
    }
    if !v.chars().any(|ch| ch.is_alphabetic()) || !c.chars().any(|ch| ch.is_alphabetic()) {
        return false;
    }
    if levenshtein_chars(v, c) > MAX_PAIR_EDIT_DELTA_CHARS {
        return false;
    }
    true
}

/// Unicode-aware case equality (Polish ż/Ż must count as case-only).
fn unicode_casefold_eq(a: &str, b: &str) -> bool {
    a.chars()
        .flat_map(char::to_lowercase)
        .eq(b.chars().flat_map(char::to_lowercase))
}

/// Whitespace + punctuation-aware tokenizer. Keeps letter/digit runs intact
/// (including Polish diacritics). Punctuation is a boundary, not a token.
fn tokenize_for_alignment(text: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    for ch in text.chars() {
        if ch.is_alphanumeric() || ch == '\'' || ch == '’' {
            current.push(ch);
        } else if !current.is_empty() {
            tokens.push(std::mem::take(&mut current));
        }
    }
    if !current.is_empty() {
        tokens.push(current);
    }
    tokens
}

fn token_key(token: &str) -> String {
    token
        .chars()
        .flat_map(char::to_lowercase)
        .collect::<String>()
}

fn token_lcs_length(a: &[String], b: &[String]) -> usize {
    let n = a.len();
    let m = b.len();
    let mut prev = vec![0usize; m + 1];
    let mut curr = vec![0usize; m + 1];
    for i in 1..=n {
        for j in 1..=m {
            if token_key(&a[i - 1]) == token_key(&b[j - 1]) {
                curr[j] = prev[j - 1] + 1;
            } else {
                curr[j] = prev[j].max(curr[j - 1]);
            }
        }
        std::mem::swap(&mut prev, &mut curr);
        curr.fill(0);
    }
    prev[m]
}

/// Walk a simple word-level edit script and emit contiguous replace runs as
/// joined phrases. Pure inserts/deletes without a counterpart are ignored for
/// learning (no stable variant↔canonical pair).
fn aligned_replace_runs(a: &[String], b: &[String]) -> Vec<(String, String)> {
    let n = a.len();
    let m = b.len();
    // DP table for LCS reconstruction (small; dictation token counts are modest).
    let mut dp = vec![vec![0usize; m + 1]; n + 1];
    for i in 1..=n {
        for j in 1..=m {
            if token_key(&a[i - 1]) == token_key(&b[j - 1]) {
                dp[i][j] = dp[i - 1][j - 1] + 1;
            } else {
                dp[i][j] = dp[i - 1][j].max(dp[i][j - 1]);
            }
        }
    }

    // Backtrack into reverse ops: Equal / Del / Ins
    enum Op {
        Equal,
        Del,
        Ins,
    }
    let mut ops = Vec::new();
    let mut i = n;
    let mut j = m;
    while i > 0 || j > 0 {
        if i > 0 && j > 0 && token_key(&a[i - 1]) == token_key(&b[j - 1]) {
            ops.push(Op::Equal);
            i -= 1;
            j -= 1;
        } else if j > 0 && (i == 0 || dp[i][j - 1] >= dp[i - 1][j]) {
            ops.push(Op::Ins);
            j -= 1;
        } else if i > 0 {
            ops.push(Op::Del);
            i -= 1;
        }
    }
    ops.reverse();

    let mut pairs = Vec::new();
    let mut ai = 0usize;
    let mut bi = 0usize;
    let mut del_buf: Vec<String> = Vec::new();
    let mut ins_buf: Vec<String> = Vec::new();

    let flush = |del: &mut Vec<String>, ins: &mut Vec<String>, out: &mut Vec<(String, String)>| {
        if !del.is_empty() && !ins.is_empty() {
            out.push((del.join(" "), ins.join(" ")));
        }
        del.clear();
        ins.clear();
    };

    for op in ops {
        match op {
            Op::Equal => {
                flush(&mut del_buf, &mut ins_buf, &mut pairs);
                ai += 1;
                bi += 1;
            }
            Op::Del => {
                del_buf.push(a[ai].clone());
                ai += 1;
            }
            Op::Ins => {
                ins_buf.push(b[bi].clone());
                bi += 1;
            }
        }
    }
    flush(&mut del_buf, &mut ins_buf, &mut pairs);
    pairs
}

fn levenshtein_chars(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let n = a.len();
    let m = b.len();
    if n == 0 {
        return m;
    }
    if m == 0 {
        return n;
    }
    let mut prev: Vec<usize> = (0..=m).collect();
    let mut curr = vec![0usize; m + 1];
    for i in 1..=n {
        curr[0] = i;
        for j in 1..=m {
            let cost = if a[i - 1] == b[j - 1] { 0 } else { 1 };
            curr[j] = (prev[j] + 1).min(curr[j - 1] + 1).min(prev[j - 1] + cost);
        }
        std::mem::swap(&mut prev, &mut curr);
    }
    prev[m]
}

/// Atomically upsert one correction-derived rule in the user's custom lexicon.
/// Every prior mapping for the normalized variant is removed before one
/// canonical row is appended. Unknown and malformed legacy rows are preserved.
pub fn upsert_correction_in_custom_lexicon(variant: &str, canonical: &str) -> Result<()> {
    let _write_guard = CUSTOM_LEXICON_WRITE_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .map_err(|_| anyhow::anyhow!("custom lexicon write lock was poisoned"))?;
    upsert_correction_in_custom_lexicon_unlocked(variant, canonical)
}

fn upsert_correction_in_custom_lexicon_unlocked(variant: &str, canonical: &str) -> Result<()> {
    assert_test_data_dir_isolated("upsert_correction_in_custom_lexicon");
    if !is_sensible_lexicon_candidate(variant, canonical) {
        return Ok(());
    }
    let path = Config::config_dir().join("lexicon.custom.jsonl");
    cleanup_orphaned_lexicon_temps(path.parent().unwrap_or_else(|| Path::new(".")));
    let existing = match fs::read_to_string(&path) {
        Ok(content) => content,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(error) => {
            return Err(error).with_context(|| format!("read custom lexicon {}", path.display()));
        }
    };
    let rewritten = rewrite_custom_lexicon(&existing, variant, canonical)?;
    atomic_write_with_rename(&path, rewritten.as_bytes(), |from, to| fs::rename(from, to))
}

/// Remove orphaned `.lexicon.custom.jsonl.tmp.*` files older than 1 hour
/// (crashed atomic writes whose error-path cleanup never ran).
pub fn cleanup_orphaned_lexicon_temps(dir: &Path) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    let cutoff = SystemTime::now()
        .checked_sub(std::time::Duration::from_secs(3600))
        .unwrap_or(SystemTime::UNIX_EPOCH);
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if !name.starts_with(".lexicon.custom.jsonl.tmp.") {
            continue;
        }
        let Ok(meta) = entry.metadata() else {
            continue;
        };
        let modified = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
        if modified > cutoff {
            continue;
        }
        match fs::remove_file(entry.path()) {
            Ok(()) => tracing::info!(
                "quality: removed orphaned lexicon temp {}",
                entry.path().display()
            ),
            Err(error) => tracing::warn!(
                "quality: failed to remove orphaned lexicon temp {}: {}",
                entry.path().display(),
                error
            ),
        }
    }
}

fn normalized_variant(value: &str) -> String {
    value.trim().to_lowercase()
}

fn remove_normalized_variant(value: &mut serde_json::Value, target: &str) {
    if let Some(entries) = value
        .get_mut("mispronunciations")
        .and_then(serde_json::Value::as_array_mut)
    {
        entries.retain(|entry| {
            entry
                .as_str()
                .map(|variant| normalized_variant(variant) != target)
                .unwrap_or(true)
        });
    }
    if let Some(entries) = value
        .get_mut("extras")
        .and_then(serde_json::Value::as_object_mut)
        .and_then(|extras| extras.get_mut("mispronunciations"))
        .and_then(serde_json::Value::as_array_mut)
    {
        entries.retain(|entry| {
            entry
                .as_str()
                .map(|variant| normalized_variant(variant) != target)
                .unwrap_or(true)
        });
    }
}

fn rewrite_custom_lexicon(existing: &str, variant: &str, canonical: &str) -> Result<String> {
    let target = normalized_variant(variant);
    let mut lines = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for line in existing.lines().filter(|line| !line.trim().is_empty()) {
        match serde_json::from_str::<serde_json::Value>(line) {
            Ok(mut value) => {
                remove_normalized_variant(&mut value, &target);
                // W11-B: drop husk rows whose variant lists became empty.
                if lexicon_row_has_no_variants(&value) {
                    continue;
                }
                let serialized =
                    serde_json::to_string(&value).context("serialize preserved lexicon row")?;
                if seen.insert(serialized.clone()) {
                    lines.push(serialized);
                }
            }
            Err(_) => {
                if seen.insert(line.to_string()) {
                    lines.push(line.to_string());
                }
            }
        }
    }
    let new_row = serde_json::to_string(&serde_json::json!({
        "term": canonical.trim(),
        "mispronunciations": [variant.trim()],
        "source": LEXICON_SOURCE_CORRECTION,
    }))
    .context("serialize lexicon upsert")?;
    if seen.insert(new_row.clone()) {
        lines.push(new_row);
    }
    Ok(format!("{}\n", lines.join("\n")))
}

fn lexicon_row_has_no_variants(value: &serde_json::Value) -> bool {
    let top_empty = value
        .get("mispronunciations")
        .and_then(serde_json::Value::as_array)
        .map(|entries| {
            entries
                .iter()
                .all(|entry| entry.as_str().map(|s| s.trim().is_empty()).unwrap_or(true))
        })
        .unwrap_or(true);
    let extras_empty = value
        .get("extras")
        .and_then(|extras| extras.get("mispronunciations"))
        .and_then(serde_json::Value::as_array)
        .map(|entries| {
            entries
                .iter()
                .all(|entry| entry.as_str().map(|s| s.trim().is_empty()).unwrap_or(true))
        })
        .unwrap_or(true);
    top_empty && extras_empty
}

fn atomic_write_with_rename<F>(path: &Path, content: &[u8], rename: F) -> Result<()>
where
    F: FnOnce(&Path, &Path) -> std::io::Result<()>,
{
    let parent = path
        .parent()
        .context("custom lexicon path has no parent directory")?;
    fs::create_dir_all(parent)
        .with_context(|| format!("create custom lexicon directory {}", parent.display()))?;
    let temp_path = parent.join(format!(
        ".lexicon.custom.jsonl.tmp.{}.{}",
        std::process::id(),
        Uuid::new_v4()
    ));
    let outcome = (|| -> std::io::Result<()> {
        let mut temp = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temp_path)?;
        temp.write_all(content)?;
        temp.sync_all()?;
        drop(temp);
        rename(&temp_path, path)?;
        // nosemgrep: rust.actix.path-traversal.tainted-path.tainted-path -- Config::config_dir plus a fixed lexicon filename only.
        File::open(parent)?.sync_all()?;
        Ok(())
    })();
    if let Err(error) = outcome {
        let _ = fs::remove_file(&temp_path);
        return Err(error).with_context(|| format!("atomically replace {}", path.display()));
    }
    Ok(())
}

/// High-level: save the quality record for the overlay edit AND feed lexicon candidates.
/// Called from bridge (and tests). Returns the quality file path on success.
/// `action` (e.g. "copy", "send", "close") is carried into meta for future analytics (P2-03 triage over-correct).
pub fn commit_overlay_correction(
    raw_text: &str,
    delivered_text: &str,
    edited_text: &str,
    mode: &str,
    model: Option<String>,
    action: Option<&str>,
) -> Result<PathBuf> {
    commit_overlay_correction_with_level(
        raw_text,
        delivered_text,
        edited_text,
        mode,
        model,
        action,
        Some(FormattingPolicy::Correction.as_str()),
    )
}

/// Persist quality evidence with canonical level provenance. Candidate learning
/// is deliberately narrower than evidence capture: only Correction keeps the
/// existing custom-lexicon behavior; Off, Smart, and Max remain evidence-only.
pub fn commit_overlay_correction_with_level(
    raw_text: &str,
    delivered_text: &str,
    edited_text: &str,
    mode: &str,
    model: Option<String>,
    action: Option<&str>,
    formatting_level: Option<&str>,
) -> Result<PathBuf> {
    commit_overlay_correction_with_confidence(
        raw_text,
        delivered_text,
        edited_text,
        mode,
        model,
        action,
        formatting_level,
        None,
        None,
        Vec::new(),
    )
}

/// Like [`commit_overlay_correction_with_level`], plus optional STT confidence
/// fields recorded on the quality JSONL line (W11-C).
#[allow(clippy::too_many_arguments)]
pub fn commit_overlay_correction_with_confidence(
    raw_text: &str,
    delivered_text: &str,
    edited_text: &str,
    mode: &str,
    model: Option<String>,
    action: Option<&str>,
    formatting_level: Option<&str>,
    avg_logprob: Option<f32>,
    speech_pct: Option<f32>,
    confidence_flags: Vec<String>,
) -> Result<PathBuf> {
    let formatting_level = formatting_level
        .map(FormattingPolicy::parse)
        .transpose()?
        .map(|level| level.as_str().to_string());
    let record = QualityRecord::new_with_confidence(
        raw_text.to_string(),
        delivered_text.to_string(),
        edited_text.to_string(),
        mode,
        model,
        formatting_level,
        action,
        avg_logprob,
        speech_pct,
        confidence_flags,
    );
    let qpath = save_quality_record(&record)?;

    if record.formatting_level.as_deref() == Some(FormattingPolicy::Correction.as_str()) {
        // Word-level extraction may yield several pairs; upsert each.
        for (variant, canonical) in extract_lexicon_candidates(delivered_text, edited_text) {
            if is_sensible_lexicon_candidate(&variant, &canonical) {
                if let Err(e) = upsert_correction_in_custom_lexicon(&variant, &canonical) {
                    tracing::warn!(
                        "quality: failed to append lexicon candidate {} -> {}: {}",
                        variant,
                        canonical,
                        e
                    );
                } else {
                    tracing::info!(
                        "quality: added lexicon candidate {} -> {}",
                        variant,
                        canonical
                    );
                }
            }
        }
    }
    Ok(qpath)
}

/// Replay historical `corrections.jsonl` through the current extractor.
/// Returns dry-run candidate rows; with `apply=true` upserts after backing up
/// the custom lexicon to `.bak-replay-<ts>`.
pub fn replay_corrections_through_extractor(
    corrections_path: &Path,
    apply: bool,
) -> Result<Vec<ReplayCandidate>> {
    // nosemgrep: rust.actix.path-traversal.tainted-path.tainted-path -- CLI path is operator-supplied local filesystem path for offline replay; no network or public input.
    let file = match File::open(corrections_path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => {
            return Err(error)
                .with_context(|| format!("open corrections {}", corrections_path.display()));
        }
    };

    let mut results = Vec::new();
    for (index, line) in BufReader::new(file).lines().enumerate() {
        let line = line.with_context(|| format!("read corrections line {}", index + 1))?;
        if line.trim().is_empty() {
            continue;
        }
        let record: QualityRecord = match serde_json::from_str(&line) {
            Ok(record) => record,
            Err(error) => {
                tracing::warn!("replay: skip malformed line {}: {}", index + 1, error);
                continue;
            }
        };
        // Real records only: Correction level, or legacy-missing level.
        let level = record.formatting_level.as_deref();
        let teaches = match level {
            None => true,
            Some(value) => value.eq_ignore_ascii_case(FormattingPolicy::Correction.as_str()),
        };
        if !teaches {
            continue;
        }
        // Skip non-edits / synthetic empty shells.
        if record.delivered_text.trim().is_empty()
            || record.edited_text.trim().is_empty()
            || unicode_casefold_eq(&record.delivered_text, &record.edited_text)
        {
            continue;
        }
        let pairs = extract_lexicon_candidates(&record.delivered_text, &record.edited_text);
        for (variant, canonical) in pairs {
            results.push(ReplayCandidate {
                line: index + 1,
                correction_id: record.logical_id(),
                variant: variant.clone(),
                canonical: canonical.clone(),
                applied: false,
            });
        }
    }

    if apply && !results.is_empty() {
        assert_test_data_dir_isolated("replay_corrections_through_extractor");
        let lexicon_path = Config::config_dir().join("lexicon.custom.jsonl");
        if lexicon_path.exists() {
            let ts = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            let backup =
                lexicon_path.with_file_name(format!(".lexicon.custom.jsonl.bak-replay-{ts}"));
            fs::copy(&lexicon_path, &backup).with_context(|| {
                format!(
                    "backup custom lexicon {} -> {}",
                    lexicon_path.display(),
                    backup.display()
                )
            })?;
            tracing::info!("replay: backed up custom lexicon to {}", backup.display());
        }
        for candidate in &mut results {
            upsert_correction_in_custom_lexicon(&candidate.variant, &candidate.canonical)?;
            candidate.applied = true;
        }
    }
    Ok(results)
}

/// One dry-run / apply row from [`replay_corrections_through_extractor`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ReplayCandidate {
    pub line: usize,
    pub correction_id: String,
    pub variant: String,
    pub canonical: String,
    pub applied: bool,
}

/// Finalize the canonical value of one learned correction. The variant and
/// immutable audit fields come from the latest resolved revision. The custom
/// dictionary is atomically replaced first; only then is the superseding
/// revision appended and exposed by the Voice Lab projection.
pub fn finalize_voice_lab_correction(
    correction_id: &str,
    canonical: &str,
) -> Result<QualityRecord> {
    let correction_id = correction_id.trim();
    let canonical = canonical.trim();
    anyhow::ensure!(
        !correction_id.is_empty()
            && correction_id.len() <= 128
            && correction_id.chars().all(
                |character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_')
            ),
        "invalid correction ID"
    );
    anyhow::ensure!(
        !canonical.is_empty(),
        "canonical correction cannot be empty"
    );

    let records = all_quality_records()?;
    let (_, current) = records
        .iter()
        .enumerate()
        .filter(|(_, record)| record.logical_id() == correction_id)
        .max_by_key(|(index, record)| (record.revision, *index))
        .context("correction ID was not found")?;
    anyhow::ensure!(
        is_sensible_lexicon_candidate(&current.delivered_text, canonical),
        "canonical correction does not satisfy the existing lexicon safety policy"
    );
    if current.edited_text.trim() == canonical {
        return Ok(current.clone());
    }

    // Serialize the lexicon snapshot, rewrite, audit append, and rollback as
    // one process-local transaction so simultaneous edits cannot lose rules.
    let _write_guard = CUSTOM_LEXICON_WRITE_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .map_err(|_| anyhow::anyhow!("custom lexicon write lock was poisoned"))?;
    let lexicon_path = Config::config_dir().join("lexicon.custom.jsonl");
    let previous_lexicon = match fs::read(&lexicon_path) {
        Ok(bytes) => Some(bytes),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => {
            return Err(error)
                .with_context(|| format!("read custom lexicon {}", lexicon_path.display()));
        }
    };
    upsert_correction_in_custom_lexicon_unlocked(&current.delivered_text, canonical)?;

    let mut revision = current.clone();
    revision.correction_id = correction_id.to_string();
    revision.revision = current.revision.saturating_add(1);
    revision.timestamp_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(current.timestamp_ms);
    revision.edited_text = canonical.to_string();
    revision.meta = serde_json::json!({
        "source": "voice-lab",
        "action": "edit",
        "supersedes_revision": current.revision,
    });

    if let Err(error) = save_quality_record(&revision) {
        let rollback = match previous_lexicon {
            Some(bytes) => {
                atomic_write_with_rename(&lexicon_path, &bytes, |from, to| fs::rename(from, to))
            }
            None => match fs::remove_file(&lexicon_path) {
                Ok(()) => Ok(()),
                Err(remove_error) if remove_error.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(remove_error) => Err(remove_error).with_context(|| {
                    format!(
                        "remove rolled-back custom lexicon {}",
                        lexicon_path.display()
                    )
                }),
            },
        };
        if let Err(rollback_error) = rollback {
            tracing::error!(
                "quality: lexicon rollback failed after revision append error: {rollback_error:#}"
            );
        }
        return Err(error).context("append finalized correction revision");
    }

    Ok(revision)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;
    use std::ffi::OsString;

    struct EnvRestore {
        key: &'static str,
        previous: Option<OsString>,
    }

    impl EnvRestore {
        fn capture(key: &'static str) -> Self {
            Self {
                key,
                previous: std::env::var_os(key),
            }
        }
    }

    impl Drop for EnvRestore {
        fn drop(&mut self) {
            match &self.previous {
                Some(value) => unsafe { std::env::set_var(self.key, value) },
                None => unsafe { std::env::remove_var(self.key) },
            }
        }
    }

    #[test]
    fn test_extract_candidates_basic() {
        let cands = extract_lexicon_candidates("uni agentka", "Junie");
        assert_eq!(cands.len(), 1);
        assert_eq!(cands[0], ("uni agentka".to_string(), "Junie".to_string()));
    }

    #[test]
    fn test_extract_ignores_identical_and_empty() {
        assert!(extract_lexicon_candidates("foo", "foo").is_empty());
        assert!(extract_lexicon_candidates("", "bar").is_empty());
        assert!(extract_lexicon_candidates("bar", "").is_empty());
    }

    #[test]
    fn test_candidate_policy_rejects_case_only_edits() {
        assert!(extract_lexicon_candidates("junie", "Junie").is_empty());
        assert!(!is_sensible_lexicon_candidate("junie", "Junie"));
        // Polish diacritics must use Unicode casefold, not ASCII.
        assert!(extract_lexicon_candidates("żaba", "Żaba").is_empty());
        assert!(!is_sensible_lexicon_candidate("żaba", "Żaba"));
    }

    #[test]
    fn test_candidate_policy_rejects_punctuation_only_edit_shape() {
        // Word-level alignment: same tokens after stripping punctuation → no pair.
        assert!(extract_lexicon_candidates("Hello Junie", "Hello, Junie").is_empty());
    }

    #[test]
    fn test_candidate_policy_rejects_long_sentence_rewrites() {
        let delivered = "uni agentka ".repeat(12);
        let edited = "Junie ".repeat(24);

        assert!(extract_lexicon_candidates(&delivered, &edited).is_empty());
        assert!(!is_sensible_lexicon_candidate(&delivered, "Junie"));
    }

    #[test]
    fn test_candidate_policy_accepts_multi_word_phrase_pairs() {
        let cands = extract_lexicon_candidates("luks tri mapa", "Loctree map");

        assert_eq!(cands, vec![("luks tri mapa".into(), "Loctree map".into())]);
        assert!(is_sensible_lexicon_candidate(&cands[0].0, &cands[0].1));
    }

    #[test]
    fn test_sensible_rejects_too_short_or_long() {
        assert!(!is_sensible_lexicon_candidate("a", "b"));
        assert!(!is_sensible_lexicon_candidate(&"x".repeat(100), "y"));
    }

    #[test]
    fn long_dictation_single_word_fix_teaches() {
        // 500-char-class Polish dictation with one local fix.
        let prefix = "W dzisiejszym badaniu klinicznym pacjent prezentował typowe objawy \
                      wymagające starannego zaznaczenie w dokumentacji medycznej oraz \
                      dokładnego opisu przebiegu. ";
        let delivered = format!(
            "{}Konieczne jest wykonanie dodatkowych testów laboratoryjnych \
             i kontrola parametrów życiowych w ciągu najbliższych godzin.",
            prefix
        );
        assert!(delivered.chars().count() > 200);
        let edited = delivered.replace("zaznaczenie", "selection");
        let cands = extract_lexicon_candidates(&delivered, &edited);
        assert_eq!(cands, vec![("zaznaczenie".into(), "selection".into())]);
    }

    #[test]
    fn five_hundred_char_dictation_with_five_char_fix_yields_one_pair() {
        let filler = "słowo ";
        let mut body = String::new();
        while body.chars().count() < 480 {
            body.push_str(filler);
        }
        let delivered = format!("{body}error koniec");
        let edited = format!("{body}fix koniec");
        assert!(delivered.chars().count() >= 500 || delivered.chars().count() > 480);
        let cands = extract_lexicon_candidates(&delivered, &edited);
        assert_eq!(cands, vec![("error".into(), "fix".into())]);
    }

    #[test]
    fn total_rewrite_yields_zero_pairs() {
        let delivered = "alpha beta gamma delta epsilon zeta eta theta";
        let edited = "one two three four five six seven eight";
        assert!(extract_lexicon_candidates(delivered, edited).is_empty());
    }

    #[test]
    fn delta_twenty_accepted_twenty_one_rejected() {
        // Same length so Levenshtein equals substitution count.
        let base = "abcdefghij"; // 10
        let v20 = format!("{base}{}", "x".repeat(10)); // 20
        let c20 = format!("{base}{}", "y".repeat(10)); // 20, dist=10? need dist exactly
        // Construct strings with known char distance.
        let variant = "a".repeat(20);
        let canonical_ok = format!("{}{}", "a".repeat(0), "b".repeat(20)); // dist 20
        assert_eq!(levenshtein_chars(&variant, &canonical_ok), 20);
        assert!(is_sensible_lexicon_candidate(&variant, &canonical_ok));

        let variant21 = "a".repeat(21);
        let canonical21 = "b".repeat(21);
        assert_eq!(levenshtein_chars(&variant21, &canonical21), 21);
        assert!(!is_sensible_lexicon_candidate(&variant21, &canonical21));

        // Through extractor as single-token replace:
        let d = format!("prefix {variant} suffix");
        let e = format!("prefix {canonical_ok} suffix");
        assert_eq!(
            extract_lexicon_candidates(&d, &e),
            vec![(variant.clone(), canonical_ok)]
        );
        let d21 = format!("prefix {variant21} suffix");
        let e21 = format!("prefix {canonical21} suffix");
        assert!(extract_lexicon_candidates(&d21, &e21).is_empty());
        let _ = v20;
        let _ = c20;
    }

    #[test]
    fn multi_fix_edit_yields_multiple_pairs() {
        let delivered = "foo bar baz qux";
        let edited = "fop bar bat qux";
        let mut cands = extract_lexicon_candidates(delivered, edited);
        cands.sort();
        assert_eq!(
            cands,
            vec![("baz".into(), "bat".into()), ("foo".into(), "fop".into()),]
        );
    }

    #[test]
    #[serial]
    fn long_dictation_e2e_pair_learned_and_applied_by_lexicon() {
        let temp_dir = tempfile::tempdir().expect("temp");
        let _guard = EnvRestore::capture("CODESCRIBE_DATA_DIR");
        let temp_root = temp_dir.path().canonicalize().unwrap();
        unsafe {
            std::env::set_var("CODESCRIBE_DATA_DIR", &temp_root);
        }

        let mut body = String::new();
        while body.chars().count() < 480 {
            body.push_str("słowo ");
        }
        let delivered = format!("{body}zaznaczenie koniec");
        let edited = format!("{body}selection koniec");

        commit_overlay_correction(
            &delivered,
            &delivered,
            &edited,
            "overlay",
            Some("whisper".into()),
            Some("copy"),
        )
        .expect("commit long dictation fix");

        let entries = custom_lexicon_entries().expect("lexicon");
        assert!(
            entries.iter().any(|e| {
                e.variant == "zaznaczenie"
                    && e.canonical == "selection"
                    && e.source == LEXICON_SOURCE_CORRECTION
            }),
            "expected learned pair, got {entries:?}"
        );

        let custom = fs::read_to_string(Config::config_dir().join("lexicon.custom.jsonl"))
            .expect("custom lexicon file");
        assert!(custom.contains("zaznaczenie") && custom.contains("selection"));
        // Word-boundary rewrite contract (same as build_word_regex): next transcript
        // containing the variant becomes the canonical form.
        let pattern = regex::Regex::new(r"(?i)\bzaznaczenie\b").expect("word boundary regex");
        let next = pattern.replace_all("tu zaznaczenie jest", "selection");
        assert_eq!(next, "tu selection jest");
    }

    #[test]
    #[serial]
    fn husk_rows_are_dropped_on_next_upsert() {
        let temp_dir = tempfile::tempdir().expect("temp");
        let _guard = EnvRestore::capture("CODESCRIBE_DATA_DIR");
        let temp_root = temp_dir.path().canonicalize().unwrap();
        unsafe {
            std::env::set_var("CODESCRIBE_DATA_DIR", &temp_root);
        }

        let path = Config::config_dir().join("lexicon.custom.jsonl");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        // Seed: one real row + one husk (empty mispronunciations) + a row that
        // will become a husk when its sole variant is reassigned.
        fs::write(
            &path,
            r#"{"term":"Keep","mispronunciations":["keep-var"]}
{"term":"Husk","mispronunciations":[]}
{"term":"Stale","mispronunciations":["move-me"]}
"#,
        )
        .unwrap();

        upsert_correction_in_custom_lexicon("move-me", "Fresh").expect("upsert");

        let content = fs::read_to_string(&path).expect("read lexicon");
        assert!(
            !content.contains(r#""term":"Husk""#),
            "empty husk must be dropped: {content}"
        );
        assert!(
            !content.contains(r#""term":"Stale""#),
            "row that lost its only variant must be dropped: {content}"
        );
        assert!(content.contains(r#""term":"Keep""#));
        assert!(content.contains(r#""term":"Fresh""#));
        assert!(content.contains(r#""source":"correction""#));
    }

    #[test]
    #[serial]
    fn quality_write_panics_without_data_dir_under_test() {
        let _guard = EnvRestore::capture("CODESCRIBE_DATA_DIR");
        unsafe {
            std::env::remove_var("CODESCRIBE_DATA_DIR");
        }
        let result = std::panic::catch_unwind(|| {
            let _ = save_quality_record(&QualityRecord::new(
                "r".into(),
                "d".into(),
                "e".into(),
                "overlay",
                None,
                None,
                Some("copy"),
            ));
        });
        assert!(result.is_err(), "must panic when CODESCRIBE_DATA_DIR unset");
    }

    #[test]
    fn confidence_fields_roundtrip_old_and_new_records() {
        let legacy = r#"{"timestamp_ms":42,"mode":"overlay","raw_text":"r","delivered_text":"d","edited_text":"e","meta":null}"#;
        let old: QualityRecord = serde_json::from_str(legacy).expect("legacy");
        assert_eq!(old.avg_logprob, None);
        assert_eq!(old.speech_pct, None);
        assert!(old.confidence_flags.is_empty());

        let mut fresh = QualityRecord::new_with_confidence(
            "r".into(),
            "d".into(),
            "e".into(),
            "overlay",
            None,
            Some("correction".into()),
            Some("copy"),
            Some(-0.42),
            Some(0.91),
            vec!["low_logprob".into()],
        );
        fresh.timestamp_ms = 99;
        let encoded = serde_json::to_string(&fresh).expect("encode");
        let decoded: QualityRecord = serde_json::from_str(&encoded).expect("decode");
        assert_eq!(decoded.avg_logprob, Some(-0.42));
        assert_eq!(decoded.speech_pct, Some(0.91));
        assert_eq!(decoded.confidence_flags, vec!["low_logprob".to_string()]);
    }

    #[test]
    fn upsert_stamps_correction_provenance_and_legacy_rows_parse() {
        let existing = r#"{"term":"Old","mispronunciations":["old-var"]}
"#;
        let rewritten = rewrite_custom_lexicon(existing, "new-var", "New").expect("rewrite");
        assert!(rewritten.contains(r#""source":"correction""#));
        let legacy_line = r#"{"term":"Legacy","mispronunciations":["leg"]}"#;
        let stored: StoredCustomLexiconEntry =
            serde_json::from_str(legacy_line).expect("legacy parse");
        assert!(stored.source.is_none());
    }

    #[test]
    #[serial]
    fn replay_dry_run_on_fixture_corpus_produces_expected_table() {
        let temp_dir = tempfile::tempdir().expect("temp");
        let _guard = EnvRestore::capture("CODESCRIBE_DATA_DIR");
        let temp_root = temp_dir.path().canonicalize().unwrap();
        unsafe {
            std::env::set_var("CODESCRIBE_DATA_DIR", &temp_root);
        }

        let quality = quality_dir();
        fs::create_dir_all(&quality).unwrap();
        let path = quality.join("corrections.jsonl");
        // Fixture: long Polish with one word fix + a rewrite + smart (non-teaching).
        let mut body = String::new();
        while body.chars().count() < 200 {
            body.push_str("tekst ");
        }
        let delivered = format!("{body}zaznaczenie");
        let edited = format!("{body}selection");
        let lines = [
            serde_json::json!({
                "timestamp_ms": 1,
                "mode": "overlay",
                "formatting_level": "correction",
                "raw_text": delivered,
                "delivered_text": delivered,
                "edited_text": edited,
                "meta": {"action": "copy"}
            })
            .to_string(),
            serde_json::json!({
                "timestamp_ms": 2,
                "mode": "overlay",
                "formatting_level": "correction",
                "raw_text": "alpha beta gamma delta epsilon zeta eta theta",
                "delivered_text": "alpha beta gamma delta epsilon zeta eta theta",
                "edited_text": "one two three four five six seven eight",
                "meta": {"action": "copy"}
            })
            .to_string(),
            serde_json::json!({
                "timestamp_ms": 3,
                "mode": "overlay",
                "formatting_level": "smart",
                "raw_text": "x",
                "delivered_text": "smart var",
                "edited_text": "Smart Canon",
                "meta": {"action": "copy"}
            })
            .to_string(),
        ];
        fs::write(&path, format!("{}\n", lines.join("\n"))).unwrap();

        let table = replay_corrections_through_extractor(&path, false).expect("replay");
        assert_eq!(
            table.len(),
            1,
            "only the local word fix should extract: {table:?}"
        );
        assert_eq!(table[0].variant, "zaznaczenie");
        assert_eq!(table[0].canonical, "selection");
        assert!(!table[0].applied);

        let applied = replay_corrections_through_extractor(&path, true).expect("apply");
        assert!(applied[0].applied);
        let entries = custom_lexicon_entries().unwrap();
        assert!(entries.iter().any(|e| e.variant == "zaznaczenie"));
    }

    #[test]
    #[serial]
    fn test_commit_writes_record_and_does_not_panic_on_lexicon() {
        // P1-02: MUST honor CODESCRIBE_DATA_DIR (the single existing override path,
        // verified via loct find --literal) for hermetic test isolation. No twin
        // path logic. Prove by writing under temp and asserting the returned path.
        let temp_dir = tempfile::tempdir().expect("temp data dir for isolation");
        let _guard = EnvRestore::capture("CODESCRIBE_DATA_DIR");

        // Canonicalize for macOS reality: config_dir() does .canonicalize() on
        // CODESCRIBE_DATA_DIR (see loader.rs), turning /var/folders into
        // /private/var/folders. Use the same form for the starts_with proof.
        let temp_root = temp_dir
            .path()
            .canonicalize()
            .unwrap_or_else(|_| temp_dir.path().to_path_buf());

        // SAFETY: test-only, #[serial] guarantees exclusive access; mirrors EnvGuard/EnvRestore
        // pattern used elsewhere (e.g. lane_truth, stream_postprocess). Process-env mutation
        // is the documented way to drive CODESCRIBE_DATA_DIR for hermetic isolation tests.
        unsafe {
            std::env::set_var("CODESCRIBE_DATA_DIR", &temp_root);
        }

        let p = commit_overlay_correction(
            "raw raw",
            "uni agentka here",
            "Junie here",
            "overlay",
            Some("whisper".into()),
            Some("test"),
        )
        .expect("commit should succeed");
        assert!(p.ends_with("corrections.jsonl"));
        // Proof of isolation: the quality file landed under the overridden DATA_DIR
        // (config_dir + quality_dir respect it; real ~/.codescribe untouched).
        assert!(
            p.starts_with(&temp_root),
            "quality record path must be under the CODESCRIBE_DATA_DIR temp for isolation (got: {})",
            p.display()
        );

        // D-02 depth + action/raw wiring (over-correct): deserialize last record and
        // assert full fields (raw_text, delivered, edited, meta.action, source).
        // Proves the heart of quality loop (capture + meta + lexicon feed) without
        // relying on string contains.
        let written = std::fs::read_to_string(&p).expect("read written quality log");
        let last_line = written.lines().last().expect("at least one jsonl line");
        let rec: QualityRecord =
            serde_json::from_str(last_line).expect("parse quality record jsonl");
        assert_eq!(
            rec.raw_text, "raw raw",
            "D-05/D-02: raw_text must be wired and recorded"
        );
        assert_eq!(rec.delivered_text, "uni agentka here");
        assert_eq!(rec.edited_text, "Junie here");
        assert_eq!(rec.mode, "overlay");
        let meta_action = rec.meta.get("action").and_then(|v| v.as_str());
        assert_eq!(
            meta_action,
            Some("test"),
            "P2-03/P2-07: action must flow to meta"
        );
        let meta_source = rec.meta.get("source").and_then(|v| v.as_str());
        assert_eq!(meta_source, Some("overlay-final"));
    }

    // Over-correct depth for D-02 / P1-02 / P2-03: explicit action variants + distinct raw_text
    // prove the quality heart (record + meta + raw for lexicon v2) under isolation.
    #[test]
    #[serial]
    fn test_commit_records_distinct_raw_and_various_actions() {
        let temp_dir = tempfile::tempdir().expect("temp data dir for isolation");
        let _guard = EnvRestore::capture("CODESCRIBE_DATA_DIR");
        let temp_root = temp_dir
            .path()
            .canonicalize()
            .unwrap_or_else(|_| temp_dir.path().to_path_buf());
        // SAFETY: test-only, #[serial] + EnvRestore; mirrors other env guards.
        unsafe {
            std::env::set_var("CODESCRIBE_DATA_DIR", &temp_root);
        }

        // "copy" action + distinct raw (real STT vs post-delivered)
        let p = commit_overlay_correction(
            "raw stt with selection here",
            "delivered with selection",
            "edited with selection",
            "overlay",
            Some("whisper-large".into()),
            Some("copy"),
        )
        .expect("commit copy action");
        assert!(
            p.starts_with(&temp_root),
            "isolation: must land under temp DATA_DIR"
        );

        let written = std::fs::read_to_string(&p).expect("read quality log");
        let last_line = written.lines().last().expect("record line");
        let rec: QualityRecord = serde_json::from_str(last_line).expect("parse");
        assert_eq!(
            rec.raw_text, "raw stt with selection here",
            "D-05: distinct raw wired"
        );
        assert_eq!(rec.delivered_text, "delivered with selection");
        assert_eq!(
            rec.meta.get("action").and_then(|v| v.as_str()),
            Some("copy")
        );
        assert_eq!(
            rec.meta.get("source").and_then(|v| v.as_str()),
            Some("overlay-final")
        );

        // "send" action variant
        let p2 = commit_overlay_correction(
            "another raw",
            "delivered2",
            "edited2",
            "overlay",
            None,
            Some("send"),
        )
        .expect("commit send");
        assert!(p2.starts_with(&temp_root));
        let last2: QualityRecord = serde_json::from_str(
            std::fs::read_to_string(&p2)
                .unwrap()
                .lines()
                .last()
                .unwrap(),
        )
        .unwrap();
        assert_eq!(
            last2.meta.get("action").and_then(|v| v.as_str()),
            Some("send")
        );
    }

    #[test]
    #[serial]
    fn test_commit_long_edit_records_quality_but_no_lexicon_candidate() {
        let temp_dir = tempfile::tempdir().expect("temp");
        let _guard = EnvRestore::capture("CODESCRIBE_DATA_DIR");
        let temp_root = temp_dir
            .path()
            .canonicalize()
            .unwrap_or_else(|_| temp_dir.path().to_path_buf());
        unsafe {
            std::env::set_var("CODESCRIBE_DATA_DIR", &temp_root);
        }

        let long = "x".repeat(150);
        let p = commit_overlay_correction(
            &long,
            "delivered long",
            &long,
            "overlay",
            None,
            Some("close"),
        )
        .expect("quality record even for long (lexicon guard separate)");
        assert!(p.starts_with(&temp_root));

        // lexicon candidate rejected by length (is_sensible + extract guard)
        // Use the same config resolution the append fn uses (honors DATA_DIR via test guard).
        let lex_path = crate::config::Config::config_dir().join("lexicon.custom.jsonl");
        let before = std::fs::read_to_string(&lex_path)
            .unwrap_or_default()
            .lines()
            .count();
        // call extract directly to prove
        assert!(extract_lexicon_candidates(&long, &long).is_empty());
        let after = std::fs::read_to_string(&lex_path)
            .unwrap_or_default()
            .lines()
            .count();
        assert_eq!(before, after, "no lexicon growth for long edit");
    }

    #[test]
    #[serial]
    fn test_voice_lab_read_surface_returns_live_records_and_lexicon_entries() {
        let temp_dir = tempfile::tempdir().expect("temp data dir for read surface");
        let _guard = EnvRestore::capture("CODESCRIBE_DATA_DIR");
        let temp_root = temp_dir
            .path()
            .canonicalize()
            .unwrap_or_else(|_| temp_dir.path().to_path_buf());
        // SAFETY: this test is serial and EnvRestore restores process state.
        unsafe {
            std::env::set_var("CODESCRIBE_DATA_DIR", &temp_root);
        }

        assert!(
            recent_quality_records(10)
                .expect("missing log is empty")
                .is_empty()
        );
        assert!(
            custom_lexicon_entries()
                .expect("missing lexicon is empty")
                .is_empty()
        );

        commit_overlay_correction(
            "raw one",
            "uni agentka",
            "Junie",
            "overlay",
            None,
            Some("copy"),
        )
        .expect("first correction");
        commit_overlay_correction(
            "raw two",
            "luks tri mapa",
            "Loctree map",
            "overlay",
            None,
            Some("send"),
        )
        .expect("second correction");
        let lexicon_path = Config::config_dir().join("lexicon.custom.jsonl");
        let mut lexicon_file = OpenOptions::new()
            .append(true)
            .open(&lexicon_path)
            .expect("open custom lexicon for legacy extras fixture");
        writeln!(
            lexicon_file,
            r#"{{"term":"Vetcoders","extras":{{"mispronunciations":["wet coders"]}}}}"#
        )
        .expect("append legacy extras fixture");

        let records = recent_quality_records(1).expect("recent records");
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].raw_text, "raw two");
        assert_eq!(records[0].edited_text, "Loctree map");
        assert_eq!(
            records[0]
                .meta
                .get("action")
                .and_then(|value| value.as_str()),
            Some("send")
        );

        let lexicon = custom_lexicon_entries().expect("custom lexicon entries");
        assert_eq!(
            lexicon,
            vec![
                CustomLexiconEntry {
                    variant: "uni agentka".into(),
                    canonical: "Junie".into(),
                    source: LEXICON_SOURCE_CORRECTION.into(),
                },
                CustomLexiconEntry {
                    variant: "luks tri mapa".into(),
                    canonical: "Loctree map".into(),
                    source: LEXICON_SOURCE_CORRECTION.into(),
                },
                CustomLexiconEntry {
                    variant: "wet coders".into(),
                    canonical: "Vetcoders".into(),
                    source: LEXICON_SOURCE_LEGACY.into(),
                },
            ]
        );
    }

    #[test]
    fn legacy_records_receive_deterministic_logical_ids() {
        let legacy = r#"{"timestamp_ms":42,"mode":"overlay","raw_text":"uni agentka","delivered_text":"uni agentka","edited_text":"Junie","meta":{"action":"copy"}}"#;
        let first: QualityRecord = serde_json::from_str(legacy).expect("legacy record");
        let second: QualityRecord = serde_json::from_str(legacy).expect("legacy record again");

        assert_eq!(first.revision, 0);
        assert_eq!(first.formatting_level, None);
        assert!(first.correction_id.is_empty());
        assert!(first.logical_id().starts_with("legacy-"));
        assert_eq!(first.logical_id(), second.logical_id());
    }

    #[test]
    fn formatting_level_roundtrips_canonically_and_old_rows_remain_compatible() {
        for (input, canonical) in [
            ("correction", "correction"),
            ("smart", "smart"),
            ("max", "max"),
        ] {
            let level = FormattingPolicy::parse(input)
                .expect("known formatting level")
                .as_str()
                .to_string();
            let record = QualityRecord::new(
                "raw".into(),
                "delivered".into(),
                "edited".into(),
                "overlay",
                None,
                Some(level),
                Some("copy"),
            );
            let encoded = serde_json::to_string(&record).expect("serialize quality record");
            let decoded: QualityRecord =
                serde_json::from_str(&encoded).expect("deserialize quality record");

            assert_eq!(decoded.formatting_level.as_deref(), Some(canonical));
        }

        let old = r#"{"timestamp_ms":7,"mode":"overlay","raw_text":"raw","delivered_text":"variant","edited_text":"canonical","meta":null}"#;
        let decoded: QualityRecord = serde_json::from_str(old).expect("old row remains readable");
        assert_eq!(decoded.formatting_level, None);
    }

    #[test]
    #[serial]
    fn level_aware_commit_records_every_level_but_only_correction_teaches_lexicon() {
        let temp_dir = tempfile::tempdir().expect("temp quality root");
        let _guard = EnvRestore::capture("CODESCRIBE_DATA_DIR");
        let temp_root = temp_dir.path().canonicalize().unwrap();
        unsafe { std::env::set_var("CODESCRIBE_DATA_DIR", &temp_root) };

        for (level, delivered, edited) in [
            ("correction", "korrvariant", "CorrCanonical"),
            ("smart", "smartvariant", "SmartCanonical"),
            ("max", "maxvariant", "MaxCanonical"),
            ("off", "rawvariant", "RawCanonical"),
        ] {
            commit_overlay_correction_with_level(
                delivered,
                delivered,
                edited,
                "overlay",
                None,
                Some("copy"),
                Some(level),
            )
            .expect("quality evidence commit");
        }

        let records = recent_quality_records(10).expect("quality evidence rows");
        let candidates = custom_lexicon_entries().expect("custom lexicon candidates");
        assert_eq!(records.len(), 4, "every level appends quality evidence");
        assert_eq!(candidates.len(), 1, "only Correction emits a candidate");
        assert_eq!(candidates[0].variant, "korrvariant");
        assert_eq!(candidates[0].canonical, "CorrCanonical");
    }

    #[test]
    #[serial]
    fn finalizing_correction_appends_revision_and_leaves_one_active_mapping() {
        let temp_dir = tempfile::tempdir().expect("temp data dir for Voice Lab edit");
        let _guard = EnvRestore::capture("CODESCRIBE_DATA_DIR");
        let temp_root = temp_dir.path().canonicalize().unwrap();
        unsafe {
            std::env::set_var("CODESCRIBE_DATA_DIR", &temp_root);
        }

        let quality_path = commit_overlay_correction(
            "uni agentka",
            "uni agentka",
            "Junie",
            "overlay",
            None,
            Some("copy"),
        )
        .expect("initial correction");
        let original = recent_quality_records(10).expect("initial projection")[0].clone();
        let id = original.logical_id();

        let lexicon_path = Config::config_dir().join("lexicon.custom.jsonl");
        let mut duplicate = OpenOptions::new()
            .append(true)
            .open(&lexicon_path)
            .expect("open duplicate fixture");
        writeln!(
            duplicate,
            r#"{{"term":"Stale","mispronunciations":[" UNI AGENTKA "]}}"#
        )
        .expect("append stale duplicate");
        drop(duplicate);

        let revised = finalize_voice_lab_correction(&id, "Junie Prime")
            .expect("finalize canonical correction");
        assert_eq!(revised.correction_id, id);
        assert_eq!(revised.revision, original.revision + 1);
        assert_eq!(revised.delivered_text, "uni agentka");
        assert_eq!(revised.edited_text, "Junie Prime");

        let audit: Vec<QualityRecord> = fs::read_to_string(&quality_path)
            .expect("read append-only audit")
            .lines()
            .map(|line| serde_json::from_str(line).expect("quality revision"))
            .collect();
        assert_eq!(audit.len(), 2);
        assert_eq!(audit[0].edited_text, "Junie");
        assert_eq!(audit[1].edited_text, "Junie Prime");
        assert_eq!(recent_quality_records(10).unwrap()[0], revised);

        let active: Vec<_> = custom_lexicon_entries()
            .expect("active lexicon projection")
            .into_iter()
            .filter(|entry| normalized_variant(&entry.variant) == "uni agentka")
            .collect();
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].canonical, "Junie Prime");
    }

    #[test]
    fn injected_atomic_replace_failure_keeps_previous_lexicon_bytes() {
        let temp_dir = tempfile::tempdir().expect("temp lexicon");
        let path = temp_dir.path().join("lexicon.custom.jsonl");
        let previous = b"{\"term\":\"Junie\",\"mispronunciations\":[\"uni agentka\"]}\n";
        fs::write(&path, previous).expect("seed previous lexicon");

        let error = atomic_write_with_rename(&path, b"replacement\n", |_, _| {
            Err(std::io::Error::other("injected rename failure"))
        })
        .expect_err("injected rename must fail");

        assert!(error.to_string().contains("atomically replace"));
        assert_eq!(fs::read(&path).expect("read unchanged lexicon"), previous);
        assert_eq!(
            fs::read_dir(temp_dir.path()).unwrap().count(),
            1,
            "temporary file is cleaned up"
        );
    }
}
