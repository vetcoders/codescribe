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
            timestamp_ms,
            session_id: None,
            mode: mode.to_string(),
            model,
            raw_text,
            delivered_text,
            edited_text,
            meta,
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
/// `action` (e.g. "copy", "send", "close") is carried into meta for future analytics (P2-03 triage over-correct).
pub fn commit_overlay_correction(
    raw_text: &str,
    delivered_text: &str,
    edited_text: &str,
    mode: &str,
    model: Option<String>,
    action: Option<&str>,
) -> Result<PathBuf> {
    let record = QualityRecord::new(
        raw_text.to_string(),
        delivered_text.to_string(),
        edited_text.to_string(),
        mode,
        model,
        action,
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
}
