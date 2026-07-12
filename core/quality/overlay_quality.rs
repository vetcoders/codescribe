//! P0-D Quality loop MVP: capture user corrections from overlay FINAL transcript edits.
//! Writes quality records (raw, delivered, edited) to ~/.codescribe/quality/*.jsonl
//! Extracts lexicon candidates (delivered→edited) and appends safe rules to the
//! custom lexicon (lexicon.custom.jsonl) that StreamPostProcessor / apply_lexicon already consumes.
//!
//! Privacy: purely local, no network, no secrets, no audio.
//! No new Settings knobs (defaults on; VoiceLab UI later).

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::config::Config;

/// Quality record for one user correction on the overlay.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QualityRecord {
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

impl QualityRecord {
    pub fn new(
        raw_text: String,
        delivered_text: String,
        edited_text: String,
        mode: &str,
        model: Option<String>,
    ) -> Self {
        let timestamp_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        QualityRecord {
            timestamp_ms,
            session_id: None,
            mode: mode.to_string(),
            model,
            raw_text,
            delivered_text,
            edited_text,
            meta: serde_json::json!({ "source": "overlay-final" }),
        }
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

/// Extract candidate lexicon pairs (variant -> canonical) from a user correction.
/// MVP: whole-string when they differ and are short/sensible.
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
/// MVP: length + not identical after norm; caller can tighten.
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

/// Append a correction-derived rule to the user's custom lexicon file.
/// Format matches what load_legacy_jsonl_with_terms expects for "custom":
///   {"term": "<canonical>", "mispronunciations": ["<variant>"]}
/// The loader already skips unsafe plain-word regressions for custom.
pub fn append_correction_to_custom_lexicon(variant: &str, canonical: &str) -> Result<()> {
    if !is_sensible_lexicon_candidate(variant, canonical) {
        return Ok(());
    }
    let path = Config::config_dir().join("lexicon.custom.jsonl");
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).ok();
    }
    let entry = serde_json::json!({
        "term": canonical,
        "mispronunciations": [variant]
    });
    let mut f = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .with_context(|| format!("open custom lexicon {}", path.display()))?;
    let line = serde_json::to_string(&entry).context("serialize lexicon entry")?;
    writeln!(f, "{}", line).context("append lexicon rule")?;
    Ok(())
}

/// High-level: save the quality record for the overlay edit AND feed lexicon candidates.
/// Called from bridge (and tests). Returns the quality file path on success.
pub fn commit_overlay_correction(
    raw_text: &str,
    delivered_text: &str,
    edited_text: &str,
    mode: &str,
    model: Option<String>,
) -> Result<PathBuf> {
    let record = QualityRecord::new(
        raw_text.to_string(),
        delivered_text.to_string(),
        edited_text.to_string(),
        mode,
        model,
    );
    let qpath = save_quality_record(&record)?;

    // Extract + append candidates (safe only).
    for (variant, canonical) in extract_lexicon_candidates(delivered_text, edited_text) {
        if is_sensible_lexicon_candidate(&variant, &canonical) {
            // Best-effort; do not fail the whole commit on lexicon append.
            if let Err(e) = append_correction_to_custom_lexicon(&variant, &canonical) {
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
    Ok(qpath)
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
        unsafe {
            std::env::set_var("CODESCRIBE_DATA_DIR", temp_dir.path());
        }

        let p = commit_overlay_correction(
            "raw raw",
            "uni agentka here",
            "Junie here",
            "overlay",
            Some("whisper".into()),
        )
        .expect("commit should succeed");
        assert!(p.ends_with("corrections.jsonl"));
        // Proof of isolation: the quality file landed under the overridden DATA_DIR
        // (config_dir + quality_dir respect it; real ~/.codescribe untouched).
        assert!(
            p.starts_with(temp_dir.path()),
            "quality record path must be under the CODESCRIBE_DATA_DIR temp for isolation (got: {})",
            p.display()
        );
    }
}
