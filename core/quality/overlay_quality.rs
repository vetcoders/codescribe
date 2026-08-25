//! P0-D Quality loop MVP: capture user corrections from overlay FINAL transcript edits.
//! Writes quality records (raw, delivered, edited) to ~/.codescribe/quality/*.jsonl
//! Extracts lexicon candidates (delivered→edited) and appends safe rules to the
//! custom lexicon (`lexicon.custom.jsonl`) loaded by the current
//! `custom_lexicon_entries` path and its bridge/quality readers.
//!
//! Privacy: purely local, no network, no secrets, no audio.
//! No new Settings knobs (three identical human teaches by default; VoiceLab UI later).

use std::collections::{HashMap, HashSet};
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

/// Serializes every custom-lexicon rewrite in this process.
///
/// The write is read-modify-write over one file, so two unsynchronized callers
/// would each rewrite from their own snapshot and the loser's rule would vanish
/// with no error. The lock covers the whole cycle, not just the final rename.
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
/// Rule the operator typed by hand in Voice Lab.
pub const LEXICON_SOURCE_MANUAL: &str = "manual";
/// Rule brought in from an external dictionary file.
pub const LEXICON_SOURCE_IMPORT: &str = "import";
/// Fallback for rows written before provenance was stamped — unknown origin,
/// not a claim that a human wrote them.
pub const LEXICON_SOURCE_LEGACY: &str = "legacy";

/// Environment override for the number of identical human teaches required
/// before a correction pair becomes a custom-lexicon rule.
pub const LEXICON_MIN_CORRECTIONS_ENV: &str = "CODESCRIBE_LEXICON_MIN_CORRECTIONS";
/// Product default: one correction is evidence; three identical corrections
/// are a learned rule. `1` is the explicit legacy compatibility escape.
pub const DEFAULT_LEXICON_MIN_CORRECTIONS: u64 = 3;

/// Read-only projection of one custom lexicon rule for product surfaces.
/// The on-disk JSONL stores one canonical term with one or more variants;
/// Voice Lab renders the flattened `variant -> canonical` truth.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CustomLexiconEntry {
    /// The misheard form as it appears in STT output.
    pub variant: String,
    /// The term the variant is rewritten to.
    pub canonical: String,
    /// `correction` | `manual` | `import` | `legacy` (default for old rows).
    pub source: String,
}

/// On-disk shape of one `lexicon.custom.jsonl` row.
///
/// Every field but `term` is `#[serde(default)]` because this file has been
/// written by several generations of the loader: rows predating `extras` and
/// `source` must still parse, or teaching would silently drop the operator's
/// oldest rules.
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

/// Legacy nested variant list. Older writers put mispronunciations under
/// `extras`; both locations are merged on read.
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
/// Upper bound of that window: longer phrases are sentences, and a rule keyed
/// on a whole sentence would never match a second time.
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
    /// New record for one overlay edit, stamped now with a fresh
    /// `correction_id` at revision 1. Confidence fields are left empty; use
    /// [`QualityRecord::new_with_confidence`] when STT reported them.
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

    /// Full constructor, including the STT confidence signals (W11-C).
    ///
    /// An unavailable clock yields `timestamp_ms == 0` rather than a panic: the
    /// correction itself is the evidence, and refusing to record it because the
    /// system clock misbehaved would lose the operator's actual work.
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

    /// Identity used to collapse a correction and its later revisions into one
    /// row.
    ///
    /// Rows written before `correction_id` existed get a deterministic
    /// `legacy-<sha256>` derived from their immutable fields (timestamp, mode,
    /// model, raw and delivered text) — never from `edited_text`, which is
    /// exactly what a revision changes. Fields are hashed with a `0` separator
    /// so adjacent values cannot be shifted between them to collide.
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
    // One write_all per record: `writeln!` on an unbuffered File issues multiple
    // write() syscalls, and concurrent appenders interleave mid-line (observed as
    // "trailing characters" parse skips). O_APPEND + a single write keeps each
    // JSONL line atomic.
    let mut line = serde_json::to_string(record).context("serialize quality record")?;
    line.push('\n');
    f.write_all(line.as_bytes())
        .context("write quality record line")?;
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

/// Every correction record in file order, revisions included.
///
/// Unlike [`recent_quality_records`] this does not collapse to the newest
/// revision per correction — finalization needs the whole chain to pick the
/// current head and compute the next revision number.
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
    if is_function_word(v) {
        return false;
    }
    if levenshtein_chars(v, c) > MAX_PAIR_EDIT_DELTA_CHARS {
        return false;
    }
    true
}

/// Bare high-frequency Polish/English function words never form a sane
/// mispronunciation variant on their own: a rule keyed on one of these fires
/// on virtually every utterance (the "jest" -> "rozwiązanie dostępne"
/// poisoning). Multi-word variants containing them remain allowed.
fn is_function_word(variant: &str) -> bool {
    // One flat set — "a" and "to" are shared between Polish and English, so
    // splitting the list by language duplicates them (review P3-02).
    /// High-frequency PL/EN function words rejected as bare lexicon variants.
    /// Multi-word phrases that merely contain one of these stay eligible.
    const FUNCTION_WORDS: &[&str] = &[
        "a", "ale", "an", "and", "are", "być", "by", "co", "czy", "do", "go", "i", "in", "is",
        "it", "jak", "jest", "jestem", "jesteś", "już", "ma", "mam", "mi", "na", "nie", "no", "o",
        "od", "of", "on", "or", "po", "się", "są", "tak", "tam", "te", "ten", "the", "to", "tu",
        "w", "was", "z", "za", "że",
    ];
    let folded: String = variant.chars().flat_map(char::to_lowercase).collect();
    FUNCTION_WORDS.contains(&folded.as_str())
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

/// Comparison key for alignment: full Unicode lowercase, so `Żółw` and `żółw`
/// align as the same token instead of registering as a substitution.
fn token_key(token: &str) -> String {
    token
        .chars()
        .flat_map(char::to_lowercase)
        .collect::<String>()
}

/// Length of the longest common token subsequence — the "how much survived the
/// edit" number behind the global rewrite guard. Two rolling rows only; the
/// alignment itself is reconstructed separately by [`aligned_replace_runs`].
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
    /// LCS backtrack step while rebuilding word-level replace runs.
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

/// Edit distance in Unicode chars. Chars, not bytes: a Polish diacritic costs
/// two bytes, and byte distance would reject `łódź`-shaped fixes that are one
/// character apart.
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

/// Insert a promoted lexical pair once, even when detached overlay quality
/// tasks reach the third-confirmation boundary concurrently.
fn insert_promoted_correction_once(variant: &str, canonical: &str) -> Result<bool> {
    let _write_guard = CUSTOM_LEXICON_WRITE_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .map_err(|_| anyhow::anyhow!("custom lexicon write lock was poisoned"))?;
    let already_present = custom_lexicon_entries()?.iter().any(|entry| {
        normalized_variant(&entry.variant) == normalized_variant(variant)
            && entry.canonical.trim() == canonical.trim()
    });
    if already_present {
        return Ok(false);
    }
    upsert_correction_in_custom_lexicon_unlocked(variant, canonical)?;
    Ok(true)
}

/// Single-pair upsert body. Caller must already hold [`CUSTOM_LEXICON_WRITE_LOCK`].
fn upsert_correction_in_custom_lexicon_unlocked(variant: &str, canonical: &str) -> Result<()> {
    upsert_corrections_unlocked(std::slice::from_ref(&(variant, canonical)))
}

/// Upsert MANY correction rules in one pass: one read, one rewrite, one atomic
/// write — instead of a full read + reparse + fsync per pair.
///
/// Teaching replays every stored correction, so the per-pair path made the cost
/// quadratic in the lexicon and multiplied fsyncs by the candidate count, all
/// while holding the global write lock (and, from the Voice Lab button, the main
/// thread). Result is identical to applying the pairs in order — later pairs
/// still supersede earlier mappings of the same variant, because they run
/// through the same rewrite, just in memory.
pub fn upsert_corrections_in_custom_lexicon(pairs: &[(&str, &str)]) -> Result<()> {
    if pairs.is_empty() {
        return Ok(());
    }
    let _write_guard = CUSTOM_LEXICON_WRITE_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .map_err(|_| anyhow::anyhow!("custom lexicon write lock was poisoned"))?;
    upsert_corrections_unlocked(pairs)
}

/// Shared upsert body: gate every pair, fold them into one in-memory rewrite,
/// then perform a single atomic replace.
///
/// Caller must already hold [`CUSTOM_LEXICON_WRITE_LOCK`]. Returns `Ok(())`
/// without touching the file when no pair survives the gate.
fn upsert_corrections_unlocked(pairs: &[(&str, &str)]) -> Result<()> {
    assert_test_data_dir_isolated("upsert_correction_in_custom_lexicon");
    let accepted: Vec<(&str, &str)> = pairs
        .iter()
        .copied()
        .filter(|(variant, canonical)| is_sensible_lexicon_candidate(variant, canonical))
        .collect();
    if accepted.is_empty() {
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
    let mut rewritten = existing;
    for (variant, canonical) in accepted {
        rewritten = rewrite_custom_lexicon(&rewritten, variant, canonical)?;
    }
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

/// Matching key for "is this the same variant?" during a rewrite. Trim plus
/// lowercase, so re-teaching `Junie` after `junie` supersedes the old row
/// instead of stacking a second rule beside it.
fn normalized_variant(value: &str) -> String {
    value.trim().to_lowercase()
}

/// Read the human-teach threshold from exactly one place.
///
/// An absent, empty, malformed, or zero value fails closed to the product law:
/// three identical corrections. This is deliberately read at each teach so the
/// registered hot environment override takes effect without restarting.
fn lexicon_min_corrections() -> u64 {
    std::env::var(LEXICON_MIN_CORRECTIONS_ENV)
        .ok()
        .and_then(|value| value.trim().parse::<u64>().ok())
        .filter(|count| *count > 0)
        .unwrap_or(DEFAULT_LEXICON_MIN_CORRECTIONS)
}

/// True only for persisted records created by a human lexicon-teach gesture.
/// Copy, close, send, speech-gap, formatter-only, and bulk/replay paths remain
/// evidence or explicit operator promotion respectively; none become history
/// for the automatic N-correction gate.
fn record_is_human_lexicon_teach(record: &QualityRecord) -> bool {
    if record
        .meta
        .get("edit_provenance")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        == Some("manual_human")
    {
        return true;
    }
    let action = record
        .meta
        .get("action")
        .and_then(serde_json::Value::as_str);
    match action {
        Some("teach-span") | Some("teach-dictionary") => true,
        Some("edit") => {
            record
                .meta
                .get("source")
                .and_then(serde_json::Value::as_str)
                .map(str::trim)
                == Some("voice-lab")
        }
        _ => false,
    }
}

/// Count distinct quality records that teach exactly this pair. A record with
/// the same aligned pair twice is still one human teach, not two votes.
fn identical_human_teach_count(records: &[QualityRecord], variant: &str, canonical: &str) -> u64 {
    let target_variant = normalized_variant(variant);
    let target_canonical = canonical.trim();

    records
        .iter()
        .filter(|record| record_is_human_lexicon_teach(record))
        .filter(|record| {
            let learning_source = if record.raw_text.trim().is_empty() {
                &record.delivered_text
            } else {
                &record.raw_text
            };
            extract_lexicon_candidates(learning_source, &record.edited_text)
                .into_iter()
                .any(|(seen_variant, seen_canonical)| {
                    normalized_variant(&seen_variant) == target_variant
                        && seen_canonical.trim() == target_canonical
                })
        })
        .map(QualityRecord::logical_id)
        .collect::<HashSet<_>>()
        .len() as u64
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PendingLexiconTeach {
    seen: u64,
    required: u64,
}

#[derive(Debug, Default)]
struct LexiconTeachPromotion {
    promoted: Vec<(String, String)>,
    progress: Vec<PendingLexiconTeach>,
}

/// Classify candidates after their quality record has been saved. The third
/// matching record is therefore included in the count and performs the first
/// upsert; later matching records refresh the existing row through the normal
/// write primitive.
fn classify_human_lexicon_teaches(
    candidates: &[(String, String)],
) -> Result<LexiconTeachPromotion> {
    let records = all_quality_records()?;
    let existing = custom_lexicon_entries()?;
    let required = lexicon_min_corrections();
    let mut seen_pairs = HashSet::new();
    let mut result = LexiconTeachPromotion::default();

    for (variant, canonical) in candidates {
        if !is_sensible_lexicon_candidate(variant, canonical) {
            continue;
        }
        let normalized_pair = (normalized_variant(variant), canonical.trim().to_string());
        if !seen_pairs.insert(normalized_pair) {
            continue;
        }

        let seen = identical_human_teach_count(&records, variant, canonical);
        let already_promoted = existing.iter().any(|entry| {
            normalized_variant(&entry.variant) == normalized_variant(variant)
                && entry.canonical.trim() == canonical.trim()
        });
        if seen >= required && !already_promoted {
            result
                .promoted
                .push((variant.trim().to_string(), canonical.trim().to_string()));
        }
        result.progress.push(PendingLexiconTeach {
            seen: seen.min(required),
            required,
        });
    }

    Ok(result)
}

/// Strip `target` from a row's variant lists, both the top-level
/// `mispronunciations` and the legacy `extras` nest.
///
/// Non-string array members are kept: this walks untyped JSON so an unfamiliar
/// row shape is preserved rather than quietly pruned.
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

/// Produce the new lexicon file contents with exactly one active mapping for
/// `variant`.
///
/// Order of operations matters: every prior mapping of the variant is removed
/// first, husk rows left with no variants are dropped, and only then is the new
/// row appended — otherwise the loader could see two rules for one variant and
/// the winner would depend on read order. Rows that fail to parse are passed
/// through verbatim; a rewrite is not the place to discard data it cannot read.
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

/// True when a row has no usable variant left in either location — a husk that
/// would otherwise accumulate in the file forever (W11-B).
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

/// Replace a file's contents atomically: write a unique temp beside it, fsync,
/// rename over the target, then fsync the directory so the rename itself is
/// durable. A crash leaves either the old file or the new one, never a partial.
///
/// `rename` is injected so tests can fail that exact step and prove the
/// previous bytes survive. The temp name carries pid and UUID, so concurrent
/// processes cannot collide, and the error path removes it.
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

/// Result of a successful overlay-quality commit (evidence always; learn only
/// on an explicit teach gesture, never on overlay copy/close).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OverlayCorrectionCommit {
    /// The `corrections.jsonl` the evidence line was appended to.
    pub quality_path: PathBuf,
    /// Lexicon pairs actually upserted from this commit (0 when evidence-only or filtered).
    pub pairs_learned: u32,
    /// True when this commit left no custom-lexicon rule written.
    pub evidence_only: bool,
    /// Manual confirmation progress, retained for the acknowledgement toast.
    /// Each tuple is `(identical_teaches_seen, required_teaches)`.
    lexicon_teach_progress: Vec<PendingLexiconTeach>,
}

impl OverlayCorrectionCommit {
    /// Structured single-pair progress for bridge/UI consumers.
    pub fn confirmation_progress(&self) -> Option<(u64, u64)> {
        (self.lexicon_teach_progress.len() == 1).then(|| {
            let progress = &self.lexicon_teach_progress[0];
            (progress.seen, progress.required)
        })
    }

    /// Honest post-edit acknowledgement for the overlay toast (operator UX, LL-E).
    pub fn acknowledgement_message(&self) -> String {
        if !self.lexicon_teach_progress.is_empty() {
            let learned = match self.pairs_learned {
                0 => "Saved as evidence".to_string(),
                1 => "Saved — 1 pair learned".to_string(),
                count => format!("Saved — {count} pairs learned"),
            };
            if self.lexicon_teach_progress.len() == 1 {
                let progress = &self.lexicon_teach_progress[0];
                return format!(
                    "{learned} — {}/{} manual confirmations",
                    progress.seen, progress.required
                );
            }
            let progress = self
                .lexicon_teach_progress
                .iter()
                .map(|pending| format!("{}/{}", pending.seen, pending.required))
                .collect::<Vec<_>>()
                .join(", ");
            return format!(
                "{learned} — {} pairs pending ({progress})",
                self.lexicon_teach_progress.len()
            );
        }

        if self.evidence_only || self.pairs_learned == 0 {
            "Saved as evidence".to_string()
        } else if self.pairs_learned == 1 {
            "Saved — 1 pair learned".to_string()
        } else {
            format!("Saved — {} pairs learned", self.pairs_learned)
        }
    }
}

/// Overlay copy/close/send never writes `lexicon.custom.jsonl`.
/// Highlighted-span teach (`action=teach-span`) still does. Speech-gap
/// teach (`teach-span-gap`) stays evidence-only.
fn overlay_commit_teaches_lexicon(_mode: &str, action: Option<&str>) -> bool {
    matches!(action, Some("teach-span") | Some("teach-dictionary"))
}

fn edit_provenance_is_manual(edit_provenance: Option<&str>) -> bool {
    edit_provenance.map(str::trim) == Some("manual_human")
}

/// High-level: save the quality record for the overlay edit AND feed lexicon candidates.
/// Called from bridge (and tests). Returns path + honest pairs-learned count.
/// `action` (e.g. "copy", "send", "close") is carried into meta for future analytics (P2-03 triage over-correct).
pub fn commit_overlay_correction(
    raw_text: &str,
    delivered_text: &str,
    edited_text: &str,
    mode: &str,
    model: Option<String>,
    action: Option<&str>,
) -> Result<OverlayCorrectionCommit> {
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
) -> Result<OverlayCorrectionCommit> {
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
/// fields recorded on the quality JSONL line (W11-C / LL-D).
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
) -> Result<OverlayCorrectionCommit> {
    commit_overlay_correction_with_provenance(
        raw_text,
        delivered_text,
        edited_text,
        mode,
        model,
        action,
        formatting_level,
        None,
        avg_logprob,
        speech_pct,
        confidence_flags,
    )
}

/// Persist one overlay receipt while keeping delivery action separate from the
/// explicit editor provenance that alone may vote in the three-confirmation gate.
#[allow(clippy::too_many_arguments)]
pub fn commit_overlay_correction_with_provenance(
    raw_text: &str,
    delivered_text: &str,
    edited_text: &str,
    mode: &str,
    model: Option<String>,
    action: Option<&str>,
    formatting_level: Option<&str>,
    edit_provenance: Option<&str>,
    avg_logprob: Option<f32>,
    speech_pct: Option<f32>,
    confidence_flags: Vec<String>,
) -> Result<OverlayCorrectionCommit> {
    let formatting_level = formatting_level
        .map(FormattingPolicy::parse)
        .transpose()?
        .map(|level| level.as_str().to_string());
    // Overlay copy/close/send is evidence. Teaching from that diff is how
    // 2026-08-17 learned "pisanie Żyda" → "mi się nie wydaje" and "w 3 4" →
    // "Dwa Trzy Cztery Pięć". Lexicon grows only on an explicit teach gesture
    // (highlighted span / Voice Lab), never from a formatting-level flag.
    let teaches =
        overlay_commit_teaches_lexicon(mode, action) || edit_provenance_is_manual(edit_provenance);
    let mut record = QualityRecord::new_with_confidence(
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
    if let Some(provenance) = edit_provenance
        .map(str::trim)
        .filter(|value| !value.is_empty())
        && let Some(meta) = record.meta.as_object_mut()
    {
        meta.insert(
            "edit_provenance".to_string(),
            serde_json::Value::String(provenance.to_string()),
        );
    }
    let qpath = save_quality_record(&record)?;

    let mut pairs_learned = 0u32;
    let mut lexicon_teach_progress = Vec::new();
    if teaches {
        // Learn what the recognizer actually heard, not punctuation/casing or
        // rewrites introduced by the formatter/parser between STT and overlay.
        let learning_source = if raw_text.trim().is_empty() {
            delivered_text
        } else {
            raw_text
        };
        // Word-level extraction may yield several pairs. Only candidates taught
        // by enough identical human records may reach the write primitive.
        match classify_human_lexicon_teaches(&extract_lexicon_candidates(
            learning_source,
            edited_text,
        )) {
            Ok(promotion) => {
                lexicon_teach_progress = promotion.progress;
                for (variant, canonical) in promotion.promoted {
                    match insert_promoted_correction_once(&variant, &canonical) {
                        Ok(true) => {
                            pairs_learned = pairs_learned.saturating_add(1);
                            tracing::info!(
                                "quality: added lexicon candidate {} -> {}",
                                variant,
                                canonical
                            );
                        }
                        Ok(false) => {}
                        Err(e) => {
                            tracing::warn!(
                                "quality: failed to append lexicon candidate {} -> {}: {}",
                                variant,
                                canonical,
                                e
                            );
                        }
                    }
                }
            }
            Err(error) => tracing::warn!(
                "quality: could not count prior human teaches after saving evidence: {error:#}"
            ),
        }
    }
    Ok(OverlayCorrectionCommit {
        quality_path: qpath,
        pairs_learned,
        evidence_only: !teaches || pairs_learned == 0,
        lexicon_teach_progress,
    })
}

/// One-click Teach from a highlighted canvas span.
///
/// Lexicon-corrected spans upsert the known variant→canonical pair through
/// the existing Correction path. Speech-gap pustki are evidence-only: there
/// is no word to teach until a human supplies one in Voice Lab.
pub fn teach_span(variant: &str, canonical: &str, kind: &str) -> Result<OverlayCorrectionCommit> {
    match kind {
        "speech_gap" => commit_overlay_correction_with_confidence(
            variant,
            variant,
            if canonical.trim().is_empty() {
                "∅"
            } else {
                canonical
            },
            "overlay-span",
            None,
            Some("teach-span-gap"),
            Some(FormattingPolicy::Off.as_str()),
            None,
            None,
            vec!["speech_gap".to_string()],
        ),
        _ => commit_overlay_correction_with_confidence(
            variant,
            variant,
            canonical,
            "overlay-span",
            None,
            Some("teach-span"),
            Some(FormattingPolicy::Correction.as_str()),
            None,
            None,
            vec!["lexicon_corrected".to_string()],
        ),
    }
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
        let pairs: Vec<(&str, &str)> = results
            .iter()
            .map(|candidate| (candidate.variant.as_str(), candidate.canonical.as_str()))
            .collect();
        upsert_corrections_in_custom_lexicon(&pairs)?;
        for candidate in &mut results {
            candidate.applied = true;
        }
    }
    Ok(results)
}

/// One dry-run / apply row from [`replay_corrections_through_extractor`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ReplayCandidate {
    /// 1-based line in the replayed corrections file, for operator traceability.
    pub line: usize,
    /// Logical id of the record this pair came from.
    pub correction_id: String,
    /// Misheard form.
    pub variant: String,
    /// Term it would be rewritten to.
    pub canonical: String,
    /// False in a dry run; true once the batch upsert succeeded.
    pub applied: bool,
}

/// Result of promoting store evidence (corrections + proposed) into the live dictionary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DictionaryTeachResult {
    /// Pairs upserted from quality corrections.jsonl (Correction level).
    pub from_corrections: u32,
    /// Pairs upserted from lexicon.custom.proposed.jsonl.
    pub from_proposed: u32,
    /// Flattened custom-lexicon rows after teach (variant→canonical).
    pub total_rules: u32,
    /// Subset with source=correction after teach.
    pub rules_from_correction_source: u32,
}

/// Teach the live custom dictionary from quality store evidence.
///
/// 1. Replay `quality/corrections.jsonl` through the extractor (`apply=true`).
/// 2. Promote any rows in `lexicon.custom.proposed.jsonl` into the live file.
///
/// Idempotent: re-running only upserts missing/updated pairs. This is the product
/// "Teach" button so Dictionary stops showing "0 rules learned" while evidence sits idle.
pub fn teach_dictionary_from_store() -> Result<DictionaryTeachResult> {
    let config_dir = Config::config_dir();
    let corrections_path = config_dir.join("quality").join("corrections.jsonl");
    let proposed_path = config_dir.join("lexicon.custom.proposed.jsonl");

    let mut from_corrections = 0u32;
    if corrections_path.exists() {
        let table = replay_corrections_through_extractor(&corrections_path, true)?;
        from_corrections = table.iter().filter(|c| c.applied).count() as u32;
    }

    let mut from_proposed = 0u32;
    let mut proposed_pairs: Vec<(String, String)> = Vec::new();
    if proposed_path.exists() {
        // nosemgrep: rust.actix.path-traversal.tainted-path.tainted-path -- local config_dir path only
        let file = File::open(&proposed_path)
            .with_context(|| format!("open proposed lexicon {}", proposed_path.display()))?;
        for (index, line) in BufReader::new(file).lines().enumerate() {
            let line = line.with_context(|| format!("read proposed line {}", index + 1))?;
            if line.trim().is_empty() {
                continue;
            }
            let value: serde_json::Value = match serde_json::from_str(&line) {
                Ok(v) => v,
                Err(e) => {
                    tracing::warn!("teach: skip malformed proposed line {}: {}", index + 1, e);
                    continue;
                }
            };
            let term = value
                .get("term")
                .and_then(|v| v.as_str())
                .map(str::trim)
                .filter(|s| !s.is_empty());
            let Some(canonical) = term else { continue };
            let variants: Vec<String> = value
                .get("mispronunciations")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|x| x.as_str())
                        .map(|s| s.trim().to_string())
                        .filter(|s| !s.is_empty())
                        .collect()
                })
                .unwrap_or_default();
            for variant in variants {
                if !is_sensible_lexicon_candidate(&variant, canonical) {
                    continue;
                }
                proposed_pairs.push((variant, canonical.to_string()));
            }
        }

        // One write for the whole proposed file, same as the replay path above.
        let pairs: Vec<(&str, &str)> = proposed_pairs
            .iter()
            .map(|(variant, canonical)| (variant.as_str(), canonical.as_str()))
            .collect();
        match upsert_corrections_in_custom_lexicon(&pairs) {
            Ok(()) => from_proposed = pairs.len() as u32,
            Err(e) => tracing::warn!("teach: failed to apply proposed rules: {:#}", e),
        }
    }

    let entries = custom_lexicon_entries()?;
    let rules_from_correction_source = entries
        .iter()
        .filter(|e| e.source == LEXICON_SOURCE_CORRECTION)
        .count() as u32;

    Ok(DictionaryTeachResult {
        from_corrections,
        from_proposed,
        total_rules: entries.len() as u32,
        rules_from_correction_source,
    })
}

/// Outcome of one Voice Lab save. When this is `Ok`, the human revision is
/// persisted — learning telemetry rides alongside, it never gates the save.
#[derive(Debug, Clone, PartialEq)]
pub struct VoiceLabSaveOutcome {
    /// The resolved revision now exposed by the Voice Lab projection.
    pub record: QualityRecord,
    /// Word-level pairs actually upserted into the custom lexicon.
    pub pairs_learned: u32,
    /// Set when the revision saved but the lexicon write failed (I/O only).
    /// A failed pair derivation must never veto the human's text.
    pub lexicon_error: Option<String>,
}

/// Finalize the canonical value of one learned correction. Two separate
/// transactions, in human-authority order:
///
/// 1. The superseding revision is appended unconditionally (validation is
///    ID shape + non-empty canonical only — saving a human edit is not a
///    lexicon candidacy question).
/// 2. Word-level pairs are derived from `raw_text -> canonical` (falling back
///    to delivered text only for legacy records that never captured raw STT).
///    Each pair needs the configured number of identical saved human teaches;
///    only promoted pairs enter one atomic lexicon rewrite. A failed rewrite
///    leaves the previous lexicon bytes intact and is reported via
///    `lexicon_error`, never as `Err`.
pub fn finalize_voice_lab_correction(
    correction_id: &str,
    canonical: &str,
) -> Result<VoiceLabSaveOutcome> {
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
    if current.edited_text.trim() == canonical {
        return Ok(VoiceLabSaveOutcome {
            record: current.clone(),
            pairs_learned: 0,
            lexicon_error: None,
        });
    }

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
    save_quality_record(&revision).context("append finalized correction revision")?;

    // Dictionary learning is a WER correction loop, not a formatter-training
    // loop. Comparing the formatted delivery against the human correction would
    // teach punctuation/casing/LLM rewrites as if Whisper had heard them.
    let learning_source = if current.raw_text.trim().is_empty() {
        &current.delivered_text
    } else {
        &current.raw_text
    };
    let pairs = derive_lexicon_pairs(learning_source, canonical);
    let mut pairs_learned = 0u32;
    let mut lexicon_error = None;
    match classify_human_lexicon_teaches(&pairs) {
        Ok(promotion) if !promotion.promoted.is_empty() => {
            let borrowed: Vec<(&str, &str)> = promotion
                .promoted
                .iter()
                .map(|(variant, canonical)| (variant.as_str(), canonical.as_str()))
                .collect();
            match upsert_corrections_in_custom_lexicon(&borrowed) {
                Ok(()) => pairs_learned = borrowed.len() as u32,
                Err(error) => {
                    tracing::error!(
                        "quality: voice lab lexicon learn failed after revision save: {error:#}"
                    );
                    lexicon_error = Some(format!("{error:#}"));
                }
            }
        }
        Ok(_) => {}
        Err(error) => {
            tracing::error!(
                "quality: voice lab could not count prior human teaches after revision save: {error:#}"
            );
            lexicon_error = Some(format!("{error:#}"));
        }
    }

    Ok(VoiceLabSaveOutcome {
        record: revision,
        pairs_learned,
        lexicon_error,
    })
}

/// Word-level lexicon pairs for one human revision: aligned replace runs of
/// `delivered -> canonical`, each pair individually gated. Delegates to
/// [`extract_lexicon_candidates`] — same aligner, same per-pair policy.
pub fn derive_lexicon_pairs(delivered: &str, canonical: &str) -> Vec<(String, String)> {
    extract_lexicon_candidates(delivered, canonical)
}

/// Hermetic quality-loop + lexicon-policy tests; isolate via CODESCRIBE_DATA_DIR.
#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;
    use std::ffi::OsString;

    /// Restores one process env var on drop so serial tests leave the host clean.
    struct EnvRestore {
        key: &'static str,
        previous: Option<OsString>,
    }

    impl EnvRestore {
        /// Snapshot the current value (or absence) of `key` before a test mutates it.
        fn capture(key: &'static str) -> Self {
            Self {
                key,
                previous: std::env::var_os(key),
            }
        }
    }

    impl Drop for EnvRestore {
        /// Put the captured env binding back; safe only under #[serial] exclusive access.
        fn drop(&mut self) {
            match &self.previous {
                Some(value) => unsafe { std::env::set_var(self.key, value) },
                None => unsafe { std::env::remove_var(self.key) },
            }
        }
    }

    /// Short multi-word mishearing collapses to one variant→canonical pair.
    #[test]
    fn test_extract_candidates_basic() {
        let cands = extract_lexicon_candidates("uni agentka", "Junie");
        assert_eq!(cands.len(), 1);
        assert_eq!(cands[0], ("uni agentka".to_string(), "Junie".to_string()));
    }

    /// Identical sides and empty inputs yield no teachable candidates.
    #[test]
    fn test_extract_ignores_identical_and_empty() {
        assert!(extract_lexicon_candidates("foo", "foo").is_empty());
        assert!(extract_lexicon_candidates("", "bar").is_empty());
        assert!(extract_lexicon_candidates("bar", "").is_empty());
    }

    /// Case-only changes (incl. Polish diacritic casefold) are not lexicon pairs.
    #[test]
    fn test_candidate_policy_rejects_case_only_edits() {
        assert!(extract_lexicon_candidates("junie", "Junie").is_empty());
        assert!(!is_sensible_lexicon_candidate("junie", "Junie"));
        // Polish diacritics must use Unicode casefold, not ASCII.
        assert!(extract_lexicon_candidates("żaba", "Żaba").is_empty());
        assert!(!is_sensible_lexicon_candidate("żaba", "Żaba"));
    }

    /// Punctuation-only edits share tokens after stripping, so no pair is emitted.
    #[test]
    fn test_candidate_policy_rejects_punctuation_only_edit_shape() {
        // Word-level alignment: same tokens after stripping punctuation → no pair.
        assert!(extract_lexicon_candidates("Hello Junie", "Hello, Junie").is_empty());
    }

    /// Near-total rewrites trip the global token-change guard and stay evidence-only.
    #[test]
    fn test_candidate_policy_rejects_long_sentence_rewrites() {
        let delivered = "uni agentka ".repeat(12);
        let edited = "Junie ".repeat(24);

        assert!(extract_lexicon_candidates(&delivered, &edited).is_empty());
        assert!(!is_sensible_lexicon_candidate(&delivered, "Junie"));
    }

    /// Multi-word phonetic-ish phrases remain legal when within length and delta caps.
    #[test]
    fn test_candidate_policy_accepts_multi_word_phrase_pairs() {
        let cands = extract_lexicon_candidates("luks tri mapa", "Loctree map");

        assert_eq!(cands, vec![("luks tri mapa".into(), "Loctree map".into())]);
        assert!(is_sensible_lexicon_candidate(&cands[0].0, &cands[0].1));
    }

    /// Bare function words are poisoned as variants; multi-word phrases containing them pass.
    #[test]
    fn test_candidate_policy_rejects_bare_function_word_variants() {
        // The 2026-07-30 lexicon poisoning: a rule keyed on bare "jest" fired
        // on virtually every Polish utterance. Function words never form a
        // sane variant on their own, regardless of the paired term.
        assert!(!is_sensible_lexicon_candidate(
            "jest",
            "rozwiązanie dostępne"
        ));
        assert!(!is_sensible_lexicon_candidate("to", "Loctree"));
        assert!(!is_sensible_lexicon_candidate("The", "Vibecrafted"));
        // ...but a multi-word variant CONTAINING a function word stays legal.
        assert!(is_sensible_lexicon_candidate(
            "jest w kurted",
            "jest w iCurt"
        ));
    }

    /// Non-phonetic substitutions stay legal — product feature, not a similarity filter.
    #[test]
    fn test_candidate_policy_keeps_arbitrary_substitutions_legal() {
        // Deliberate product behavior: non-phonetic substitutions are a
        // feature ("zaznaczenie" -> "selection"), so only the function-word
        // guard and the absolute edit cap bound the pair space.
        assert!(is_sensible_lexicon_candidate("zaznaczenie", "selection"));
        assert!(is_sensible_lexicon_candidate("Wycrafted", "Vibecrafted"));
        assert!(is_sensible_lexicon_candidate("Bonarki", "binarki"));
    }

    /// Per-side char window: below MIN or above MAX candidate length is rejected.
    #[test]
    fn test_sensible_rejects_too_short_or_long() {
        assert!(!is_sensible_lexicon_candidate("a", "b"));
        assert!(!is_sensible_lexicon_candidate(&"x".repeat(100), "y"));
    }

    /// Long dictation with one local word fix still extracts that single pair.
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

    /// ~500-char body with a five-char local fix yields exactly one aligned pair.
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

    /// Full token rewrite exceeds the change-ratio guard and returns no pairs.
    #[test]
    fn total_rewrite_yields_zero_pairs() {
        let delivered = "alpha beta gamma delta epsilon zeta eta theta";
        let edited = "one two three four five six seven eight";
        assert!(extract_lexicon_candidates(delivered, edited).is_empty());
    }

    /// Levenshtein boundary: delta 20 is teachable; 21 is rejected by the pair gate.
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

    /// Disjoint local substitutions emit one candidate pair per replaced run.
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
    fn teach_span_requires_three_identical_corrections_and_gap_is_evidence_only() {
        let temp_dir = tempfile::tempdir().expect("temp");
        let _guard = EnvRestore::capture("CODESCRIBE_DATA_DIR");
        let temp_root = temp_dir.path().canonicalize().unwrap();
        unsafe {
            std::env::set_var("CODESCRIBE_DATA_DIR", &temp_root);
        }

        let first = super::teach_span("uni agentka", "Junie", "lexicon_corrected")
            .expect("first teach lexicon span");
        assert_eq!(first.pairs_learned, 0);
        assert!(first.evidence_only);
        assert_eq!(
            first.acknowledgement_message(),
            "Saved as evidence — 1/3 manual confirmations"
        );
        assert_eq!(
            audit_line_count(),
            1,
            "first teach persists quality evidence"
        );
        let lexicon_path = Config::config_dir().join("lexicon.custom.jsonl");
        assert!(
            !lexicon_path.exists(),
            "one identical teach is evidence, not a rule"
        );

        let second = super::teach_span("UNI AGENTKA", "Junie", "lexicon_corrected")
            .expect("second teach lexicon span");
        assert_eq!(second.pairs_learned, 0);
        assert!(second.evidence_only);
        assert_eq!(
            second.acknowledgement_message(),
            "Saved as evidence — 2/3 manual confirmations"
        );
        assert!(
            !lexicon_path.exists(),
            "two identical teaches are still evidence"
        );

        let learned = super::teach_span("uni agentka", "Junie", "lexicon_corrected")
            .expect("third teach lexicon span");
        assert_eq!(learned.pairs_learned, 1);
        assert!(!learned.evidence_only);
        assert_eq!(
            learned.acknowledgement_message(),
            "Saved — 1 pair learned — 3/3 manual confirmations"
        );
        let entries = custom_lexicon_entries().expect("learned custom lexicon");
        assert!(entries.iter().any(|entry| {
            entry.variant == "uni agentka"
                && entry.canonical == "Junie"
                && entry.source == LEXICON_SOURCE_CORRECTION
        }));
        let refreshed = super::teach_span("UNI AGENTKA", "Junie", "lexicon_corrected")
            .expect("later identical teach leaves the promoted rule alone");
        assert_eq!(refreshed.pairs_learned, 0, "promotion occurs exactly once");
        assert_eq!(
            refreshed.acknowledgement_message(),
            "Saved as evidence — 3/3 manual confirmations"
        );
        assert_eq!(
            custom_lexicon_entries()
                .unwrap()
                .iter()
                .filter(|entry| normalized_variant(&entry.variant) == "uni agentka")
                .count(),
            1,
            "re-teaching after promotion must not stack or rewrite duplicate rows"
        );

        for _ in 0..3 {
            let gap = super::teach_span("brak", "uzupełnienie", "speech_gap")
                .expect("speech-gap evidence");
            assert_eq!(gap.pairs_learned, 0);
            assert!(gap.evidence_only);
        }
        assert!(
            !custom_lexicon_entries()
                .unwrap()
                .iter()
                .any(|entry| entry.variant == "brak"),
            "speech-gap records never vote toward a lexicon rule"
        );
    }

    #[test]
    #[serial]
    fn ordinary_manual_overlay_edits_vote_three_times_and_promote_once() {
        let temp_dir = tempfile::tempdir().expect("temp");
        let _guard = EnvRestore::capture("CODESCRIBE_DATA_DIR");
        let temp_root = temp_dir.path().canonicalize().unwrap();
        unsafe { std::env::set_var("CODESCRIBE_DATA_DIR", &temp_root) };

        let commit = |action: &str, provenance: Option<&str>| {
            commit_overlay_correction_with_provenance(
                "ajwo",
                "ajwo",
                "Iwo",
                "overlay",
                None,
                Some(action),
                Some("correction"),
                provenance,
                None,
                None,
                Vec::new(),
            )
            .unwrap()
        };

        let first = commit("copy", Some("manual_human"));
        assert_eq!(first.confirmation_progress(), Some((1, 3)));
        assert_eq!(first.pairs_learned, 0);
        let second = commit("paste", Some("manual_human"));
        assert_eq!(second.confirmation_progress(), Some((2, 3)));
        assert_eq!(second.pairs_learned, 0);
        let third = commit("close", Some("manual_human"));
        assert_eq!(third.confirmation_progress(), Some((3, 3)));
        assert_eq!(third.pairs_learned, 1);

        let delivery_only = commit("copy", None);
        assert_eq!(delivery_only.confirmation_progress(), None);
        assert_eq!(delivery_only.pairs_learned, 0);
        assert_eq!(
            custom_lexicon_entries()
                .unwrap()
                .iter()
                .filter(|entry| normalized_variant(&entry.variant) == "ajwo")
                .count(),
            1,
            "the third distinct manual act promotes once; delivery actions do not vote"
        );

        let records = all_quality_records().unwrap();
        assert_eq!(
            records[0]
                .meta
                .get("action")
                .and_then(serde_json::Value::as_str),
            Some("copy")
        );
        assert_eq!(
            records[0]
                .meta
                .get("edit_provenance")
                .and_then(serde_json::Value::as_str),
            Some("manual_human")
        );
    }

    #[test]
    fn repeated_revision_of_one_correction_id_is_one_vote() {
        let mut first = QualityRecord::new(
            "ajwo".into(),
            "ajwo".into(),
            "Iwo".into(),
            "overlay",
            None,
            Some("correction".into()),
            Some("copy"),
        );
        first
            .meta
            .as_object_mut()
            .unwrap()
            .insert("edit_provenance".into(), "manual_human".into());
        let mut revision = first.clone();
        revision.revision = 2;
        assert_eq!(
            identical_human_teach_count(&[first, revision], "ajwo", "Iwo"),
            1
        );
    }

    #[test]
    #[serial]
    fn concurrent_promotion_insert_reports_exactly_one_new_rule() {
        let temp_dir = tempfile::tempdir().expect("temp");
        let _guard = EnvRestore::capture("CODESCRIBE_DATA_DIR");
        let temp_root = temp_dir.path().canonicalize().unwrap();
        unsafe { std::env::set_var("CODESCRIBE_DATA_DIR", &temp_root) };

        let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
        let handles = (0..2)
            .map(|_| {
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    barrier.wait();
                    insert_promoted_correction_once("ajwo", "Iwo").unwrap()
                })
            })
            .collect::<Vec<_>>();
        let inserted = handles
            .into_iter()
            .map(|handle| handle.join().unwrap())
            .filter(|inserted| *inserted)
            .count();
        assert_eq!(inserted, 1);
        assert_eq!(custom_lexicon_entries().unwrap().len(), 1);
    }

    /// E2E: long-dictation commit learns one pair and stamps correction provenance.
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

        let evidence = commit_overlay_correction(
            &delivered,
            &delivered,
            &edited,
            "overlay",
            Some("whisper".into()),
            Some("copy"),
        )
        .expect("commit long dictation evidence");
        assert_eq!(evidence.pairs_learned, 0);
        let first = teach_span("zaznaczenie", "selection", "lexicon_corrected")
            .expect("first explicit teach of the one-word fix");
        assert_eq!(first.pairs_learned, 0);
        let second = teach_span("ZAZNACZENIE", "selection", "lexicon_corrected")
            .expect("second explicit teach of the one-word fix");
        assert_eq!(second.pairs_learned, 0);
        let commit = teach_span("zaznaczenie", "selection", "lexicon_corrected")
            .expect("third explicit teach of the one-word fix");
        assert_eq!(commit.pairs_learned, 1);
        assert_eq!(
            commit.acknowledgement_message(),
            "Saved — 1 pair learned — 3/3 manual confirmations"
        );

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

    /// A different canonical is a different vote, even when the STT variant is identical.
    #[test]
    #[serial]
    fn same_variant_with_different_canonical_does_not_promote_the_first_pair() {
        let temp_dir = tempfile::tempdir().expect("temp");
        let _guard = EnvRestore::capture("CODESCRIBE_DATA_DIR");
        let temp_root = temp_dir.path().canonicalize().unwrap();
        unsafe {
            std::env::set_var("CODESCRIBE_DATA_DIR", &temp_root);
        }

        teach_span("zazdroszczę", "życzliwość", "lexicon_corrected").unwrap();
        teach_span("ZAZDROSZCZĘ", "życzliwość", "lexicon_corrected").unwrap();
        let different = teach_span("zazdroszczę", "współczucie", "lexicon_corrected")
            .expect("different canonical is its own pair");

        assert_eq!(different.pairs_learned, 0);
        assert!(
            custom_lexicon_entries().unwrap().is_empty(),
            "two X teaches plus one Y teach must not promote X"
        );
    }

    /// The per-utterance Dictionary teach action uses the same three-correction
    /// gate as a highlighted span; it is not the bulk proposed-file promotion.
    #[test]
    #[serial]
    fn overlay_teach_dictionary_action_requires_three_identical_corrections() {
        let temp_dir = tempfile::tempdir().expect("temp");
        let _guard = EnvRestore::capture("CODESCRIBE_DATA_DIR");
        let temp_root = temp_dir.path().canonicalize().unwrap();
        unsafe {
            std::env::set_var("CODESCRIBE_DATA_DIR", &temp_root);
        }

        for expected in [0, 0, 1] {
            let outcome = commit_overlay_correction(
                "kubernetis",
                "kubernetis",
                "Kubernetes",
                "overlay",
                None,
                Some("teach-dictionary"),
            )
            .expect("dictionary teach gesture");
            assert_eq!(outcome.pairs_learned, expected);
        }
        assert!(
            custom_lexicon_entries()
                .unwrap()
                .iter()
                .any(|entry| { entry.variant == "kubernetis" && entry.canonical == "Kubernetes" })
        );
    }

    /// Overlay copy and close remain evidence lines, never hidden votes toward teach N.
    #[test]
    #[serial]
    fn overlay_copy_and_close_do_not_increment_the_human_teach_counter() {
        let temp_dir = tempfile::tempdir().expect("temp");
        let _guard = EnvRestore::capture("CODESCRIBE_DATA_DIR");
        let temp_root = temp_dir.path().canonicalize().unwrap();
        unsafe {
            std::env::set_var("CODESCRIBE_DATA_DIR", &temp_root);
        }

        teach_span("pansiwe", "Pensieve", "lexicon_corrected").unwrap();
        for action in ["copy", "close"] {
            let evidence = commit_overlay_correction(
                "pansiwe",
                "pansiwe",
                "Pensieve",
                "overlay",
                None,
                Some(action),
            )
            .expect("copy/close evidence");
            assert_eq!(evidence.pairs_learned, 0);
            assert!(evidence.evidence_only);
        }
        let second =
            teach_span("pansiwe", "Pensieve", "lexicon_corrected").expect("second actual teach");

        assert_eq!(second.pairs_learned, 0);
        assert!(
            custom_lexicon_entries().unwrap().is_empty(),
            "copy and close must not turn the second explicit teach into a rule"
        );
    }

    /// Empty and emptied lexicon rows are dropped on the next rewrite (W11-B husks).
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

    /// Under cfg(test), quality writes panic if CODESCRIBE_DATA_DIR is unset.
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

    /// Legacy JSONL omits confidence fields; new records round-trip them intact.
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

    /// New upserts stamp source=correction; pre-provenance rows still deserialize.
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

    /// Replay dry-run keeps only local teachable pairs; apply writes the lexicon.
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

    /// Commit under DATA_DIR isolation writes quality + meta and may teach pairs.
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
        // pattern used elsewhere in test-only configuration guards. Process-env mutation
        // is the documented way to drive CODESCRIBE_DATA_DIR for hermetic isolation tests.
        unsafe {
            std::env::set_var("CODESCRIBE_DATA_DIR", &temp_root);
        }

        let commit = commit_overlay_correction(
            "uni agentka here",
            "uni agentka here",
            "Junie here",
            "overlay",
            Some("whisper".into()),
            Some("test"),
        )
        .expect("commit should succeed");
        let p = commit.quality_path.clone();
        assert!(p.ends_with("corrections.jsonl"));
        assert_eq!(
            commit.pairs_learned, 0,
            "overlay copy is evidence, not a lexicon teacher"
        );
        assert!(commit.evidence_only);
        assert_eq!(commit.acknowledgement_message(), "Saved as evidence");
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
            rec.raw_text, "uni agentka here",
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
    /// Distinct raw_text and action variants land correctly in quality meta.
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
        .expect("commit copy action")
        .quality_path;
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
        .expect("commit send")
        .quality_path;
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

    /// Over-long edits still append quality evidence with zero lexicon growth.
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
        let commit = commit_overlay_correction(
            &long,
            "delivered long",
            &long,
            "overlay",
            None,
            Some("close"),
        )
        .expect("quality record even for long (lexicon guard separate)");
        assert_eq!(commit.pairs_learned, 0);
        assert_eq!(commit.acknowledgement_message(), "Saved as evidence");
        let p = commit.quality_path;
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

    /// Voice Lab read surface projects newest records and flattened lexicon rows.
    #[test]
    #[serial]
    fn test_voice_lab_read_surface_returns_live_records_and_lexicon_entries() {
        let temp_dir = tempfile::tempdir().expect("temp data dir for read surface");
        let _guard = EnvRestore::capture("CODESCRIBE_DATA_DIR");
        let _min_guard = EnvRestore::capture(LEXICON_MIN_CORRECTIONS_ENV);
        let temp_root = temp_dir
            .path()
            .canonicalize()
            .unwrap_or_else(|_| temp_dir.path().to_path_buf());
        // SAFETY: this test is serial and EnvRestore restores process state.
        unsafe {
            std::env::set_var("CODESCRIBE_DATA_DIR", &temp_root);
            // This read-projection fixture is intentionally a one-write
            // custom-lexicon store fixture, not a product-threshold test.
            std::env::set_var(LEXICON_MIN_CORRECTIONS_ENV, "1");
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

        teach_span("raw one", "Junie", "lexicon_corrected").expect("explicit teach first pair");
        teach_span("raw two", "Loctree map", "lexicon_corrected")
            .expect("explicit teach second pair");
        let lexicon_path = Config::config_dir().join("lexicon.custom.jsonl");
        let mut lexicon_file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&lexicon_path)
            .expect("open custom lexicon for legacy extras fixture");
        writeln!(
            lexicon_file,
            r#"{{"term":"Vetcoders","extras":{{"mispronunciations":["wet coders"]}}}}"#
        )
        .expect("append legacy extras fixture");

        let lexicon = custom_lexicon_entries().expect("custom lexicon entries");
        assert_eq!(
            lexicon,
            vec![
                CustomLexiconEntry {
                    variant: "raw one".into(),
                    canonical: "Junie".into(),
                    source: LEXICON_SOURCE_CORRECTION.into(),
                },
                CustomLexiconEntry {
                    variant: "raw two".into(),
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

    /// Pre-id rows share a stable legacy-<hash> logical_id across deserializations.
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

    /// Known formatting levels serialize canonically; missing level stays None.
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

    /// All overlay copy/close levels append evidence; none auto-teach lexicon.
    #[test]
    #[serial]
    fn overlay_copy_records_every_level_and_never_teaches_lexicon() {
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
        assert!(
            candidates.is_empty(),
            "overlay copy must not write lexicon.custom.jsonl, got {candidates:?}"
        );
    }

    /// Live 2026-08-17: a human C-card correction must not invent "Meksyku" rules.
    #[test]
    #[serial]
    fn overlay_correction_of_garbled_take_is_evidence_only() {
        let temp_dir = tempfile::tempdir().expect("temp quality root");
        let _guard = EnvRestore::capture("CODESCRIBE_DATA_DIR");
        let temp_root = temp_dir.path().canonicalize().unwrap();
        unsafe { std::env::set_var("CODESCRIBE_DATA_DIR", &temp_root) };

        let outcome = commit_overlay_correction(
            "A to jest pierwsze w oknie nie wybu słów tylko poprawiamy lokal power Meksyku.",
            "A to jest pierwsze w oknie nie wybu słów tylko poprawiamy lokal power Meksyku.",
            "Apple jest pierwszy, Whisper poprawia w oknie, nie wyjebujemy słów, tylko poprawiamy. Local power, leksykon.",
            "overlay",
            None,
            Some("copy"),
        )
        .expect("quality evidence");
        assert_eq!(outcome.pairs_learned, 0);
        assert!(outcome.evidence_only);
        assert_eq!(outcome.acknowledgement_message(), "Saved as evidence");
        assert!(custom_lexicon_entries().expect("lexicon").is_empty());
    }

    /// Learning keys on raw STT text, not the formatter's delivered surface.
    #[test]
    #[serial]
    fn correction_learning_uses_raw_stt_not_formatted_delivery() {
        let temp_dir = tempfile::tempdir().expect("temp quality root");
        let _guard = EnvRestore::capture("CODESCRIBE_DATA_DIR");
        let _min_guard = EnvRestore::capture(LEXICON_MIN_CORRECTIONS_ENV);
        let temp_root = temp_dir.path().canonicalize().unwrap();
        unsafe {
            std::env::set_var("CODESCRIBE_DATA_DIR", &temp_root);
            std::env::set_var(LEXICON_MIN_CORRECTIONS_ENV, "1");
        };

        let outcome = teach_span("rawvariant", "RawCanonical", "lexicon_corrected")
            .expect("explicit teach from raw STT");

        assert_eq!(outcome.pairs_learned, 1);
        let entries = custom_lexicon_entries().expect("custom lexicon");
        assert!(
            entries.iter().any(|entry| {
                entry.variant == "rawvariant" && entry.canonical == "RawCanonical"
            })
        );
        assert!(
            !entries
                .iter()
                .any(|entry| entry.variant == "formattervariant")
        );
    }

    /// Voice Lab finalize still teaches from raw_text, not formatted delivery.
    #[test]
    #[serial]
    fn voice_lab_revision_keeps_raw_stt_as_dictionary_source() {
        let temp_dir = tempfile::tempdir().expect("temp quality root");
        let _guard = EnvRestore::capture("CODESCRIBE_DATA_DIR");
        let _min_guard = EnvRestore::capture(LEXICON_MIN_CORRECTIONS_ENV);
        let temp_root = temp_dir.path().canonicalize().unwrap();
        unsafe {
            std::env::set_var("CODESCRIBE_DATA_DIR", &temp_root);
            // This fixture isolates the raw-source writer behavior; the product
            // threshold itself is covered by the three-save Voice Lab test.
            std::env::set_var(LEXICON_MIN_CORRECTIONS_ENV, "1");
        };

        commit_overlay_correction_with_level(
            "rawvariant",
            "formattervariant",
            "FirstCanonical",
            "overlay",
            None,
            Some("copy"),
            Some("correction"),
        )
        .expect("seed correction");
        let id = recent_quality_records(1).unwrap()[0].logical_id();

        finalize_voice_lab_correction(&id, "RawCanonical").expect("revise from Voice Lab");
        let entries = custom_lexicon_entries().expect("custom lexicon");
        assert!(
            entries.iter().any(|entry| {
                entry.variant == "rawvariant" && entry.canonical == "RawCanonical"
            })
        );
        assert!(
            !entries
                .iter()
                .any(|entry| entry.variant == "formattervariant")
        );
    }

    /// Finalize appends a revision and collapses duplicate variant mappings to one.
    #[test]
    #[serial]
    fn finalizing_correction_appends_revision_and_leaves_one_active_mapping() {
        let temp_dir = tempfile::tempdir().expect("temp data dir for Voice Lab edit");
        let _guard = EnvRestore::capture("CODESCRIBE_DATA_DIR");
        let _min_guard = EnvRestore::capture(LEXICON_MIN_CORRECTIONS_ENV);
        let temp_root = temp_dir.path().canonicalize().unwrap();
        unsafe {
            std::env::set_var("CODESCRIBE_DATA_DIR", &temp_root);
            // This regression is the one-write supersession fixture, not the
            // product threshold contract.
            std::env::set_var(LEXICON_MIN_CORRECTIONS_ENV, "1");
        }

        let quality_path = commit_overlay_correction(
            "uni agentka",
            "uni agentka",
            "Junie",
            "overlay",
            None,
            Some("copy"),
        )
        .expect("initial correction")
        .quality_path;
        let original = recent_quality_records(10).expect("initial projection")[0].clone();
        let id = original.logical_id();

        let lexicon_path = Config::config_dir().join("lexicon.custom.jsonl");
        let mut duplicate = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&lexicon_path)
            .expect("open duplicate fixture");
        writeln!(
            duplicate,
            r#"{{"term":"Stale","mispronunciations":[" UNI AGENTKA "]}}"#
        )
        .expect("append stale duplicate");
        drop(duplicate);

        let outcome = finalize_voice_lab_correction(&id, "Junie Prime")
            .expect("finalize canonical correction");
        assert_eq!(outcome.pairs_learned, 1);
        assert_eq!(outcome.lexicon_error, None);
        let revised = outcome.record;
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

    /// Voice Lab revisions are human teaches too, but each save gets one vote.
    #[test]
    #[serial]
    fn voice_lab_requires_three_identical_human_saves_before_learning() {
        let temp_dir = tempfile::tempdir().expect("temp data dir for Voice Lab threshold");
        let _guard = EnvRestore::capture("CODESCRIBE_DATA_DIR");
        let temp_root = temp_dir.path().canonicalize().unwrap();
        unsafe {
            std::env::set_var("CODESCRIBE_DATA_DIR", &temp_root);
        }

        let ids = (0..3)
            .map(|_| seed_voice_lab_record("uni agentka", "uni agentka"))
            .collect::<Vec<_>>();
        for (index, id) in ids.iter().enumerate() {
            let outcome =
                finalize_voice_lab_correction(id, "Junie").expect("Voice Lab human revision saves");
            assert_eq!(outcome.pairs_learned, if index == 2 { 1 } else { 0 });
            assert_eq!(outcome.lexicon_error, None);
        }
        assert!(custom_lexicon_entries().unwrap().iter().any(|entry| {
            entry.variant == "uni agentka"
                && entry.canonical == "Junie"
                && entry.source == LEXICON_SOURCE_CORRECTION
        }));
    }

    /// Invalid values cannot silently relax the sealed product default.
    #[test]
    #[serial]
    fn lexicon_min_corrections_fails_closed_to_three() {
        let _guard = EnvRestore::capture(LEXICON_MIN_CORRECTIONS_ENV);

        unsafe { std::env::remove_var(LEXICON_MIN_CORRECTIONS_ENV) };
        assert_eq!(lexicon_min_corrections(), 3);
        for invalid in ["", "0", "not-a-number"] {
            unsafe { std::env::set_var(LEXICON_MIN_CORRECTIONS_ENV, invalid) };
            assert_eq!(lexicon_min_corrections(), 3, "{invalid:?} must fail closed");
        }
        unsafe { std::env::set_var(LEXICON_MIN_CORRECTIONS_ENV, "1") };
        assert_eq!(lexicon_min_corrections(), 1);
    }

    /// One committed record inside an isolated data dir; returns its logical ID.
    fn seed_voice_lab_record(delivered: &str, edited: &str) -> String {
        commit_overlay_correction(delivered, delivered, edited, "overlay", None, Some("copy"))
            .expect("seed correction");
        recent_quality_records(1).expect("seed projection")[0].logical_id()
    }

    /// Count lines in the isolated corrections.jsonl audit log.
    fn audit_line_count() -> usize {
        fs::read_to_string(quality_dir().join("corrections.jsonl"))
            .expect("read audit")
            .lines()
            .count()
    }

    /// Paragraph-length human revision always persists; learning is separate.
    #[test]
    #[serial]
    fn paragraph_length_edit_always_saves_the_human_revision() {
        // The 2026-07-28 failing shape: a ~500-char delivered text with a
        // slightly longer human revision died on the whole-edit lexicon gate
        // before anything was persisted. Saving is not learning.
        let temp_dir = tempfile::tempdir().expect("temp data dir");
        let _guard = EnvRestore::capture("CODESCRIBE_DATA_DIR");
        let temp_root = temp_dir.path().canonicalize().unwrap();
        unsafe {
            std::env::set_var("CODESCRIBE_DATA_DIR", &temp_root);
        }

        let filler = "w badaniu klinicznym stwierdzono prawidłowy stan ogólny oraz dobrą kondycję pacjenta po zabiegu ";
        let delivered = format!(
            "{}pansiwe pozostaje lekiem pierwszego wyboru",
            filler.repeat(5)
        );
        let canonical = format!(
            "{}Pensieve pozostaje lekiem pierwszego wyboru u tego pacjenta",
            filler.repeat(5)
        );
        assert!(
            delivered.chars().count() >= 500,
            "failing shape is paragraph-length"
        );
        assert!(canonical.chars().count() > delivered.chars().count());

        let id = seed_voice_lab_record(&delivered, &delivered);
        let outcome = finalize_voice_lab_correction(&id, &canonical)
            .expect("paragraph-length human revision must save");

        assert_eq!(outcome.record.edited_text, canonical.trim());
        assert_eq!(outcome.lexicon_error, None);
        assert_eq!(outcome.pairs_learned, 0, "one save is still evidence");
        assert_eq!(audit_line_count(), 2, "revision appended, nothing replaced");
        assert_eq!(
            recent_quality_records(1).unwrap()[0].edited_text,
            canonical.trim()
        );
    }

    /// Mixed edits teach only sensible pairs; long insane sides are filtered alone.
    #[test]
    #[serial]
    fn pairs_are_gated_individually_not_as_one_edit() {
        let temp_dir = tempfile::tempdir().expect("temp data dir");
        let _guard = EnvRestore::capture("CODESCRIBE_DATA_DIR");
        let _min_guard = EnvRestore::capture(LEXICON_MIN_CORRECTIONS_ENV);
        let temp_root = temp_dir.path().canonicalize().unwrap();
        unsafe {
            std::env::set_var("CODESCRIBE_DATA_DIR", &temp_root);
            // This is an extractor/write-primitive fixture; the product
            // threshold itself is covered separately below.
            std::env::set_var(LEXICON_MIN_CORRECTIONS_ENV, "1");
        }

        let insane = "a".repeat(90); // above MAX_CANDIDATE_CHARS — rejected per-pair
        let delivered = "pacjent otrzymał pansiwe rano a wieczorem podano mu chaoswort przed kolejnym badaniem kontrolnym";
        let canonical = format!(
            "pacjent otrzymał Pensieve rano a wieczorem podano mu {insane} przed kolejnym badaniem kontrolnym"
        );

        let id = seed_voice_lab_record(delivered, delivered);
        let outcome =
            finalize_voice_lab_correction(&id, &canonical).expect("mixed edit still saves");

        assert_eq!(outcome.pairs_learned, 1);
        let entries = custom_lexicon_entries().expect("lexicon projection");
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].variant, "pansiwe");
        assert_eq!(entries[0].canonical, "Pensieve");
    }

    /// Whitespace-only revision saves audit with zero pairs and no lexicon touch.
    #[test]
    #[serial]
    fn whitespace_only_edit_saves_with_zero_pairs_and_untouched_lexicon() {
        let temp_dir = tempfile::tempdir().expect("temp data dir");
        let _guard = EnvRestore::capture("CODESCRIBE_DATA_DIR");
        let temp_root = temp_dir.path().canonicalize().unwrap();
        unsafe {
            std::env::set_var("CODESCRIBE_DATA_DIR", &temp_root);
        }

        let id = seed_voice_lab_record("uni agentka", "uni agentka");
        let outcome = finalize_voice_lab_correction(&id, "uni  agentka")
            .expect("whitespace-only revision saves");

        assert_eq!(outcome.pairs_learned, 0);
        assert_eq!(outcome.lexicon_error, None);
        assert_eq!(audit_line_count(), 2);
        assert!(
            !Config::config_dir().join("lexicon.custom.jsonl").exists(),
            "zero-pair edit must not touch the lexicon"
        );
    }

    /// Lexicon upsert failure reports lexicon_error but never blocks the human save.
    #[test]
    #[serial]
    fn lexicon_write_failure_never_vetoes_the_human_save() {
        let temp_dir = tempfile::tempdir().expect("temp data dir");
        let _guard = EnvRestore::capture("CODESCRIBE_DATA_DIR");
        let _min_guard = EnvRestore::capture(LEXICON_MIN_CORRECTIONS_ENV);
        let temp_root = temp_dir.path().canonicalize().unwrap();
        unsafe {
            std::env::set_var("CODESCRIBE_DATA_DIR", &temp_root);
            // Force the writer path: this fixture verifies that an I/O failure
            // after an eligible promotion cannot veto the human revision.
            std::env::set_var(LEXICON_MIN_CORRECTIONS_ENV, "1");
        }

        let id = seed_voice_lab_record("uni agentka", "uni agentka");
        // Injected lexicon failure: a directory where the JSONL file must be
        // makes the upsert's read fail while the revision append still works.
        fs::create_dir_all(Config::config_dir().join("lexicon.custom.jsonl"))
            .expect("occupy lexicon path");

        let outcome = finalize_voice_lab_correction(&id, "Junie")
            .expect("human save must survive a lexicon write failure");

        assert_eq!(outcome.record.edited_text, "Junie");
        assert_eq!(outcome.pairs_learned, 0);
        assert!(outcome.lexicon_error.is_some(), "failure reported honestly");
        assert_eq!(audit_line_count(), 2);
        assert_eq!(recent_quality_records(1).unwrap()[0].edited_text, "Junie");
    }

    /// Injected rename failure leaves prior lexicon bytes and cleans the temp file.
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

    /// Isolate config + data dirs into a fresh tempdir and return it, so a test
    /// that teaches never reads or writes the operator's real lexicon.
    fn isolated_config_dir(guard: &EnvRestore) -> tempfile::TempDir {
        let _ = guard;
        let temp_dir = tempfile::tempdir().expect("temp");
        let temp_root = temp_dir.path().canonicalize().unwrap();
        unsafe {
            std::env::set_var("CODESCRIBE_DATA_DIR", &temp_root);
        }
        fs::create_dir_all(Config::config_dir().join("quality")).unwrap();
        temp_dir
    }

    /// Batch multi-pair upsert equals sequential upserts, including supersession.
    #[test]
    #[serial]
    fn batch_upsert_matches_sequential_upserts_row_for_row() {
        // The batch path exists to collapse N reads/writes into one; it earns
        // that only if the resulting file is byte-identical to the old loop,
        // including later pairs superseding earlier mappings of a variant.
        let pairs = [
            ("kubernetis", "Kubernetes"),
            ("dokier", "Docker"),
            ("kubernetis", "K8s"), // supersedes the first mapping
        ];
        let seed = r#"{"term":"Keep","mispronunciations":["keep-var"]}
"#;

        let sequential = {
            let _guard = EnvRestore::capture("CODESCRIBE_DATA_DIR");
            let temp = isolated_config_dir(&_guard);
            let path = Config::config_dir().join("lexicon.custom.jsonl");
            fs::write(&path, seed).unwrap();
            for (variant, canonical) in pairs {
                upsert_correction_in_custom_lexicon(variant, canonical).unwrap();
            }
            let bytes = fs::read_to_string(&path).unwrap();
            drop(temp);
            bytes
        };

        let batched = {
            let _guard = EnvRestore::capture("CODESCRIBE_DATA_DIR");
            let temp = isolated_config_dir(&_guard);
            let path = Config::config_dir().join("lexicon.custom.jsonl");
            fs::write(&path, seed).unwrap();
            upsert_corrections_in_custom_lexicon(&pairs).unwrap();
            let bytes = fs::read_to_string(&path).unwrap();
            drop(temp);
            bytes
        };

        assert_eq!(batched, sequential);
        assert!(batched.contains("K8s"), "last mapping wins: {batched}");
        assert!(
            !batched.contains(r#"{"term":"Kubernetes""#),
            "superseded mapping must be gone: {batched}"
        );
        assert!(
            batched.contains("Keep"),
            "unrelated rows survive: {batched}"
        );
    }

    /// Teach promotes proposed rules and counts only rows that actually landed.
    #[test]
    #[serial]
    fn teach_promotes_proposed_rules_and_counts_only_what_landed() {
        // First core-level coverage of teach_dictionary_from_store: before this,
        // the only test of the Teach button was a Swift mock, so nothing proved
        // the rules actually reached the lexicon.
        let _guard = EnvRestore::capture("CODESCRIBE_DATA_DIR");
        let _temp = isolated_config_dir(&_guard);
        let config_dir = Config::config_dir();

        fs::write(
            config_dir.join("lexicon.custom.proposed.jsonl"),
            concat!(
            r#"{"term":"Kubernetes","mispronunciations":["kubernetis","kubernetys"],"source":"correction"}"#,
            "\n",
            r#"{"term":"Docker","mispronunciations":["dokier"],"source":"correction"}"#,
            "\n",
            "\n",
            "{ this line is not json\n",
            r#"{"mispronunciations":["orphan"]}"#,
            "\n",
            ),
        )
            .unwrap();

        let result = teach_dictionary_from_store().expect("teach");

        // Malformed and term-less rows are skipped, not counted and not fatal.
        assert_eq!(result.from_proposed, 3);
        assert_eq!(result.from_corrections, 0);

        let written = fs::read_to_string(config_dir.join("lexicon.custom.jsonl")).unwrap();
        for expected in ["kubernetis", "kubernetys", "dokier"] {
            assert!(
                written.contains(expected),
                "missing {expected} in {written}"
            );
        }
        assert!(
            !written.contains("orphan"),
            "row without a term must not land"
        );

        let entries = custom_lexicon_entries().unwrap();
        assert_eq!(result.total_rules, entries.len() as u32);
        assert_eq!(
            result.rules_from_correction_source,
            entries
                .iter()
                .filter(|e| e.source == LEXICON_SOURCE_CORRECTION)
                .count() as u32
        );
    }

    /// Empty store teach is a zero-count success, not an error path.
    #[test]
    #[serial]
    fn teach_on_empty_store_is_a_no_op_not_an_error() {
        let _guard = EnvRestore::capture("CODESCRIBE_DATA_DIR");
        let _temp = isolated_config_dir(&_guard);

        let result = teach_dictionary_from_store().expect("teach on empty store");
        assert_eq!(result.from_proposed, 0);
        assert_eq!(result.from_corrections, 0);
        assert_eq!(result.total_rules, 0);
    }
}
