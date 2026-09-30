//! P0-D: thin UniFFI surface for the quality/correction loop.
//! One module per concern (matches bridge discipline). Does NOT bloat recording.rs.
//!
//! Exposes commit for overlay FINAL edits:
//!   - capture (raw, delivered, edited)
//!   - write quality record JSONL
//!   - feed safe lexicon candidates to the custom lexicon consumed by PostProcessor
//!
//! Privacy: local disk only.

use codescribe_core::quality::overlay_quality::{
    CustomLexiconEntry, DictionaryTeachResult, OverlayCorrectionCommit, OverlayCorrectionInput,
    QualityRecord, VoiceLabSaveOutcome, commit_overlay_correction, custom_lexicon_entries,
    finalize_voice_lab_correction, recent_quality_listing, teach_dictionary_from_store, teach_span,
};

use crate::CsError;

/// Result of an overlay quality commit — honest learn count for the acknowledgement toast.
#[derive(uniffi::Record, Debug, Clone, PartialEq, Eq)]
pub struct CsQualityCommitResult {
    /// Lexicon pairs actually upserted (0 when evidence-only or filtered out).
    pub pairs_learned: u32,
    /// True when no custom-lexicon pair was learned: non-teach evidence,
    /// filtered edits, or an explicit teach still below its N-correction gate.
    pub evidence_only: bool,
    /// Ready-to-show overlay toast text ("Saved — N pair(s) learned" / "Saved as evidence").
    pub acknowledgement: String,
    /// Structured progress for one normalized lexical pair.
    pub teach_seen: Option<u64>,
    pub teach_required: Option<u64>,
}

impl From<OverlayCorrectionCommit> for CsQualityCommitResult {
    /// Core commit → toast payload (pairs, evidence flag, ready acknowledgement).
    fn from(commit: OverlayCorrectionCommit) -> Self {
        let progress = commit.confirmation_progress();
        Self {
            pairs_learned: commit.pairs_learned,
            evidence_only: commit.evidence_only,
            acknowledgement: commit.acknowledgement_message(),
            teach_seen: progress.map(|value| value.0),
            teach_required: progress.map(|value| value.1),
        }
    }
}

/// UI-safe projection of a persisted overlay correction.
#[derive(uniffi::Record, Debug, Clone, PartialEq)]
pub struct CsQualityRecord {
    pub id: String,
    pub revision: u64,
    pub raw_text: String,
    pub variant: String,
    pub edited_text: String,
    pub action: String,
    pub edit_provenance: Option<String>,
    pub timestamp_ms: u64,
    pub avg_logprob: Option<f32>,
    pub speech_pct: Option<f32>,
    pub confidence_flags: Vec<String>,
}

impl From<QualityRecord> for CsQualityRecord {
    /// Flatten a stored record: logical id, delivered→variant, action from meta.
    fn from(record: QualityRecord) -> Self {
        let action = record
            .meta
            .get("action")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("unknown")
            .to_string();
        let edit_provenance = record
            .meta
            .get("edit_provenance")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string);
        Self {
            id: record.logical_id(),
            revision: record.revision,
            raw_text: record.raw_text,
            variant: record.delivered_text,
            edited_text: record.edited_text,
            action,
            edit_provenance,
            timestamp_ms: record.timestamp_ms,
            avg_logprob: record.avg_logprob,
            speech_pct: record.speech_pct,
            confidence_flags: record.confidence_flags,
        }
    }
}

/// Result of a Voice Lab save: the human revision is persisted whenever this
/// crosses the bridge as `Ok`; learning telemetry never gates the save.
#[derive(uniffi::Record, Debug, Clone, PartialEq)]
pub struct CsVoiceLabSaveResult {
    pub record: CsQualityRecord,
    /// Word pairs actually upserted into the custom lexicon (0 is honest).
    pub pairs_learned: u32,
    /// Set when the revision saved but the lexicon write failed (I/O only).
    pub lexicon_error: Option<String>,
}

impl From<VoiceLabSaveOutcome> for CsVoiceLabSaveResult {
    /// Core Voice Lab outcome → FFI: saved record plus honest learn telemetry.
    fn from(outcome: VoiceLabSaveOutcome) -> Self {
        Self {
            record: outcome.record.into(),
            pairs_learned: outcome.pairs_learned,
            lexicon_error: outcome.lexicon_error,
        }
    }
}

/// Finalize one correction: the revision always saves; word-level lexicon
/// pairs are derived and gated individually. `Err` means the SAVE failed.
#[uniffi::export]
pub fn quality_finalize_correction(
    correction_id: String,
    canonical: String,
) -> Result<CsVoiceLabSaveResult, CsError> {
    finalize_voice_lab_correction(&correction_id, &canonical)
        .map(Into::into)
        .map_err(|error| CsError::Quality {
            msg: format!("Voice Lab correction update failed: {error:#}"),
        })
}

/// UI-safe flattened custom lexicon row (`variant -> canonical`).
#[derive(uniffi::Record, Debug, Clone, PartialEq, Eq)]
pub struct CsLexiconEntry {
    pub variant: String,
    pub canonical: String,
    /// `correction` | `manual` | `import` | `legacy`
    pub source: String,
}

impl From<CustomLexiconEntry> for CsLexiconEntry {
    /// Core lexicon row → UniFFI (variant, canonical, source tag).
    fn from(entry: CustomLexiconEntry) -> Self {
        Self {
            variant: entry.variant,
            canonical: entry.canonical,
            source: entry.source,
        }
    }
}

/// Persist one overlay correction: the quality record always lands, while lexicon
/// learning is gated by explicit teach action plus the N-correction threshold.
///
/// Word pairs can be proposed by an explicit Teach gesture, a legacy
/// `manual_human` edit, or a reducer-authenticated `user-edit-*` receipt; an
/// auto-format result without that provenance is only evidence. Automatic
/// promotion waits until the same normalized pair reaches the configured
/// correction threshold. The separate Dictionary Teach command is an explicit
/// bulk-promotion override. An unrecognised `formatting_level` is rejected
/// before anything is written.
///
/// The confidence fields (`avg_logprob`, `speech_pct`, `confidence_flags`) are
/// stored alongside the text so later analysis can correlate corrections with how
/// unsure the engine was.
/// The nine columns of one quality receipt, carried as one record so the
/// Swift overlay states them by name and the export needs no argument
/// telescope. Mode and model stay bridge-owned ("overlay", none).
#[derive(uniffi::Record)]
pub struct CsOverlayCorrectionInput {
    pub raw_text: String,
    pub delivered_text: String,
    pub edited_text: String,
    pub action: String,
    pub formatting_level: String,
    pub edit_provenance: Option<String>,
    pub avg_logprob: Option<f32>,
    pub speech_pct: Option<f32>,
    pub confidence_flags: Vec<String>,
}

#[uniffi::export]
pub fn commit_overlay_quality_record(
    input: CsOverlayCorrectionInput,
) -> Result<CsQualityCommitResult, CsError> {
    // Delegate to core. Model/mode are best-effort for MVP (overlay always).
    // action carried for meta (over-correct for P2-03: "captureQualityIfEdited gubi action").
    commit_overlay_correction(OverlayCorrectionInput {
        raw_text: input.raw_text,
        delivered_text: input.delivered_text,
        edited_text: input.edited_text,
        mode: "overlay".to_string(),
        model: None,
        action: Some(input.action),
        formatting_level: Some(input.formatting_level),
        edit_provenance: input.edit_provenance,
        avg_logprob: input.avg_logprob,
        speech_pct: input.speech_pct,
        confidence_flags: input.confidence_flags,
    })
    .map(Into::into)
    .map_err(|e| CsError::Quality {
        msg: format!("quality commit failed: {}", e),
    })
}

/// Dictionary listing over the bridge: real corrections plus the count of
/// takes that changed nothing and recorded no telemetry (Founder report
/// 2026-09-30 — whole untouched takes padded the corrections list).
#[derive(uniffi::Record, Debug, Clone, PartialEq)]
pub struct CsQualityListing {
    pub records: Vec<CsQualityRecord>,
    pub unchanged_takes: u64,
}

/// Read the newest persisted corrections, newest first, with the
/// unchanged-take count alongside. Missing storage is an empty listing;
/// genuine I/O failures cross the bridge as a quality error.
#[uniffi::export]
pub fn quality_recent_listing(limit: u64) -> Result<CsQualityListing, CsError> {
    let limit = usize::try_from(limit).map_err(|error| CsError::Quality {
        msg: format!("quality record limit is invalid: {error}"),
    })?;
    recent_quality_listing(limit)
        .map(|listing| CsQualityListing {
            records: listing.corrections.into_iter().map(Into::into).collect(),
            unchanged_takes: listing.unchanged_takes,
        })
        .map_err(|error| CsError::Quality {
            msg: format!("quality records read failed: {error}"),
        })
}

/// Read the live custom lexicon as flattened `variant -> canonical` rows.
#[uniffi::export]
pub fn lexicon_custom_entries() -> Result<Vec<CsLexiconEntry>, CsError> {
    custom_lexicon_entries()
        .map(|entries| entries.into_iter().map(Into::into).collect())
        .map_err(|error| CsError::Quality {
            msg: format!("custom lexicon read failed: {error}"),
        })
}

/// Result of Dictionary "Teach" — promote corrections + proposed into live lexicon.
#[derive(uniffi::Record, Debug, Clone, PartialEq, Eq)]
pub struct CsDictionaryTeachResult {
    pub from_corrections: u32,
    pub from_proposed: u32,
    pub total_rules: u32,
    pub rules_from_correction_source: u32,
}

impl From<DictionaryTeachResult> for CsDictionaryTeachResult {
    /// Core Teach counts → UniFFI counters for the Dictionary panel toast.
    fn from(value: DictionaryTeachResult) -> Self {
        Self {
            from_corrections: value.from_corrections,
            from_proposed: value.from_proposed,
            total_rules: value.total_rules,
            rules_from_correction_source: value.rules_from_correction_source,
        }
    }
}

/// Teach live dictionary from quality store (corrections.jsonl + proposed.jsonl).
/// Product: Dictionary panel "Teach" so rules leave the udawany 0-learned state.
#[uniffi::export]
pub fn quality_teach_dictionary_from_store() -> Result<CsDictionaryTeachResult, CsError> {
    teach_dictionary_from_store()
        .map(Into::into)
        .map_err(|error| CsError::Quality {
            msg: format!("dictionary teach failed: {error:#}"),
        })
}

/// One-click Teach from a highlighted span. Reuses the existing quality +
/// custom-lexicon writers — no new disk root, no new permission.
#[uniffi::export]
pub fn quality_teach_span(
    variant: String,
    canonical: String,
    kind: String,
) -> Result<CsQualityCommitResult, CsError> {
    teach_span(&variant, &canonical, &kind)
        .map(Into::into)
        .map_err(|error| CsError::Quality {
            msg: format!("span teach failed: {error:#}"),
        })
}

/// Bridge quality projections and commit gate contracts (level normalize/reject).
#[cfg(test)]
mod tests {
    use super::*;

    /// Projection maps id/revision/texts, meta.action, and confidence fields.
    #[test]
    fn quality_record_projection_maps_live_fields_and_action() {
        let record = QualityRecord {
            correction_id: "correction-42".into(),
            revision: 3,
            timestamp_ms: 42,
            session_id: None,
            mode: "overlay".into(),
            model: None,
            formatting_level: Some("smart".into()),
            raw_text: "raw".into(),
            delivered_text: "delivered".into(),
            edited_text: "edited".into(),
            avg_logprob: Some(-0.5),
            speech_pct: Some(0.8),
            confidence_flags: vec!["low_logprob".into()],
            meta: serde_json::json!({
                "action": "copy",
                "edit_provenance": "manual_human"
            }),
        };

        assert_eq!(
            CsQualityRecord::from(record),
            CsQualityRecord {
                id: "correction-42".into(),
                revision: 3,
                raw_text: "raw".into(),
                variant: "delivered".into(),
                edited_text: "edited".into(),
                action: "copy".into(),
                edit_provenance: Some("manual_human".into()),
                timestamp_ms: 42,
                avg_logprob: Some(-0.5),
                speech_pct: Some(0.8),
                confidence_flags: vec!["low_logprob".into()],
            }
        );
    }

    /// `creative` normalizes to max evidence; lexicon stays empty (no teach).
    #[test]
    #[serial_test::serial]
    fn commit_overlay_quality_record_normalizes_level_and_keeps_max_out_of_lexicon() {
        let temp_dir = std::env::temp_dir().join(format!(
            "codescribe-bridge-quality-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("system time")
                .as_nanos()
        ));
        std::fs::create_dir_all(&temp_dir).expect("create temp quality root");
        let previous = std::env::var_os("CODESCRIBE_DATA_DIR");
        let temp_root = temp_dir
            .canonicalize()
            .expect("canonical temp quality root");
        // SAFETY: this serial test owns the process-level data-root override and
        // restores its exact previous value before returning.
        unsafe { std::env::set_var("CODESCRIBE_DATA_DIR", &temp_root) };

        let result = commit_overlay_quality_record(CsOverlayCorrectionInput {
            raw_text: "synthetic raw".into(),
            delivered_text: "synthetic variant".into(),
            edited_text: "synthetic canonical".into(),
            action: "copy".into(),
            formatting_level: "creative".into(),
            edit_provenance: None,
            avg_logprob: Some(-1.2),
            speech_pct: Some(0.75),
            confidence_flags: vec!["test_flag".into()],
        });
        let records = recent_quality_listing(10)
            .expect("read committed quality record")
            .corrections;
        let lexicon = custom_lexicon_entries().expect("read custom lexicon");

        match previous {
            // SAFETY: restore the exact process environment captured above.
            Some(value) => unsafe { std::env::set_var("CODESCRIBE_DATA_DIR", value) },
            // SAFETY: the variable was absent before this serial test.
            None => unsafe { std::env::remove_var("CODESCRIBE_DATA_DIR") },
        }
        std::fs::remove_dir_all(&temp_root).expect("remove temp quality root");

        let commit = result.expect("bridge commit");
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].formatting_level.as_deref(), Some("max"));
        assert_eq!(records[0].avg_logprob, Some(-1.2));
        assert_eq!(records[0].speech_pct, Some(0.75));
        assert_eq!(records[0].confidence_flags, vec!["test_flag".to_string()]);
        assert_eq!(commit.pairs_learned, 0);
        assert!(commit.evidence_only);
        assert_eq!(commit.acknowledgement, "Saved as evidence");
        assert!(
            lexicon.is_empty(),
            "Max evidence must not teach the lexicon"
        );
    }

    /// Unknown formatting_level fails closed before any quality write lands.
    #[test]
    fn commit_overlay_quality_record_rejects_unknown_level_before_write() {
        let error = commit_overlay_quality_record(CsOverlayCorrectionInput {
            raw_text: "raw".into(),
            delivered_text: "variant".into(),
            edited_text: "canonical".into(),
            action: "close".into(),
            formatting_level: "mystery".into(),
            edit_provenance: None,
            avg_logprob: None,
            speech_pct: None,
            confidence_flags: vec![],
        })
        .expect_err("unknown level must be rejected");

        assert!(error.to_string().contains("unknown FORMATTING_LEVEL"));
    }
}
