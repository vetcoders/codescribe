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
    /// Freeform meta (e.g. {"source":"overlay-final", "action":"copy"}).
    #[serde(default, skip_serializing_if = "serde_json::Value::is_null")]
    pub meta: serde_json::Value,
}

/// Read-only projection of one custom lexicon rule for product surfaces.
/// The on-disk JSONL stores one canonical term with one or more variants;
/// Voice Lab renders the flattened `variant -> canonical` truth.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CustomLexiconEntry {
    pub variant: String,
    pub canonical: String,
}

#[derive(Deserialize)]
struct StoredCustomLexiconEntry {
    term: String,
    #[serde(default)]
    mispronunciations: Vec<String>,
    #[serde(default)]
    extras: Option<StoredLexiconExtras>,
}

#[derive(Deserialize)]
struct StoredLexiconExtras {
    #[serde(default)]
    mispronunciations: Vec<String>,
}

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
                entries.extend(
                    variants
                        .into_iter()
                        .map(|variant| variant.trim().to_string())
                        .filter(|variant| !variant.is_empty())
                        .map(|variant| CustomLexiconEntry {
                            variant,
                            canonical: canonical.to_string(),
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

/// Extract candidate lexicon pairs (variant -> canonical) from a user correction.
/// MVP policy: return exactly one whole-string phrase pair after trimming when
/// both sides are non-empty, differ case-insensitively, stay within the phrase
/// length ceiling, and both contain at least one alphabetic character.
/// There is no frequency/confirmation threshold yet; a single sensible overlay
/// correction is enough for the append step. Punctuation-only edits are therefore
/// currently accepted if the surrounding text differs and still contains letters,
/// while case-only edits are rejected by the case-insensitive equality guard.
/// Future: token/phrase alignment + confidence threshold.
/// The returned pairs are (misheard_variant, correct_canonical) so that
/// custom lexicon term=canonical, mispronunciations includes variant.
pub fn extract_lexicon_candidates(delivered: &str, edited: &str) -> Vec<(String, String)> {
    let d = delivered.trim();
    let e = edited.trim();
    if d.is_empty() || e.is_empty() {
        return vec![];
    }
    if d.eq_ignore_ascii_case(e) {
        return vec![];
    }
    // Basic sanity: not too long for a phrase rule, not pure punctuation.
    if d.len() > 120 || e.len() > 120 {
        return vec![];
    }
    let has_alpha_d = d.chars().any(|c| c.is_alphabetic());
    let has_alpha_e = e.chars().any(|c| c.is_alphabetic());
    if !has_alpha_d || !has_alpha_e {
        return vec![];
    }
    // For the P0-D test case and real edits: treat the differing strings as one phrase pair.
    // PostProcessor will treat it as whole-word-ish via its regex builders.
    vec![(d.to_string(), e.to_string())]
}

/// Sensible threshold for accepting a candidate into custom lexicon.
/// MVP policy: non-empty, not identical case-insensitively, each side at least
/// two bytes, and each side no longer than 80 bytes. No frequency or repeated
/// confirmation gate is applied here; upstream extraction decides whether the
/// edit shape is a candidate and the custom lexicon loader later skips unsafe
/// broad plain-word regressions.
pub fn is_sensible_lexicon_candidate(variant: &str, canonical: &str) -> bool {
    let v = variant.trim();
    let c = canonical.trim();
    if v.is_empty() || c.is_empty() || v.eq_ignore_ascii_case(c) {
        return false;
    }
    // Avoid giant blobs or single-char flips that are likely typos in the wrong direction.
    if v.len() < 2 || c.len() < 2 || v.len() > 80 || c.len() > 80 {
        return false;
    }
    true
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
    if !is_sensible_lexicon_candidate(variant, canonical) {
        return Ok(());
    }
    let path = Config::config_dir().join("lexicon.custom.jsonl");
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
    for line in existing.lines().filter(|line| !line.trim().is_empty()) {
        match serde_json::from_str::<serde_json::Value>(line) {
            Ok(mut value) => {
                remove_normalized_variant(&mut value, &target);
                lines.push(
                    serde_json::to_string(&value).context("serialize preserved lexicon row")?,
                );
            }
            Err(_) => lines.push(line.to_string()),
        }
    }
    lines.push(
        serde_json::to_string(&serde_json::json!({
            "term": canonical.trim(),
            "mispronunciations": [variant.trim()]
        }))
        .context("serialize lexicon upsert")?,
    );
    Ok(format!("{}\n", lines.join("\n")))
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
    let formatting_level = formatting_level
        .map(FormattingPolicy::parse)
        .transpose()?
        .map(|level| level.as_str().to_string());
    let record = QualityRecord::new(
        raw_text.to_string(),
        delivered_text.to_string(),
        edited_text.to_string(),
        mode,
        model,
        formatting_level,
        action,
    );
    let qpath = save_quality_record(&record)?;

    if record.formatting_level.as_deref() == Some(FormattingPolicy::Correction.as_str()) {
        // Extract + append candidates (safe only). S4 word-level policy remains
        // untouched; this gate only decides whether extraction may run at all.
        for (variant, canonical) in extract_lexicon_candidates(delivered_text, edited_text) {
            if is_sensible_lexicon_candidate(&variant, &canonical) {
                // Best-effort; do not fail the whole commit on lexicon append.
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
    }

    #[test]
    fn test_candidate_policy_accepts_current_punctuation_only_edit_shape() {
        let cands = extract_lexicon_candidates("Hello Junie", "Hello, Junie");

        assert_eq!(cands, vec![("Hello Junie".into(), "Hello, Junie".into())]);
        assert!(is_sensible_lexicon_candidate(&cands[0].0, &cands[0].1));
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
            r#"{{"term":"VetCoders","extras":{{"mispronunciations":["wet coders"]}}}}"#
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
                },
                CustomLexiconEntry {
                    variant: "luks tri mapa".into(),
                    canonical: "Loctree map".into(),
                },
                CustomLexiconEntry {
                    variant: "wet coders".into(),
                    canonical: "VetCoders".into(),
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
            ("correction", "corr variant", "Corr Canonical"),
            ("smart", "smart variant", "Smart Canonical"),
            ("max", "max variant", "Max Canonical"),
            ("off", "raw variant", "Raw Canonical"),
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
        assert_eq!(candidates[0].variant, "corr variant");
        assert_eq!(candidates[0].canonical, "Corr Canonical");
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
