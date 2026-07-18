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
    CustomLexiconEntry, QualityRecord, commit_overlay_correction_with_level,
    custom_lexicon_entries, finalize_voice_lab_correction, recent_quality_records,
};

use crate::CsError;

/// UI-safe projection of a persisted overlay correction.
#[derive(uniffi::Record, Debug, Clone, PartialEq, Eq)]
pub struct CsQualityRecord {
    pub id: String,
    pub revision: u64,
    pub raw_text: String,
    pub variant: String,
    pub edited_text: String,
    pub action: String,
    pub timestamp_ms: u64,
}

impl From<QualityRecord> for CsQualityRecord {
    fn from(record: QualityRecord) -> Self {
        let action = record
            .meta
            .get("action")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("unknown")
            .to_string();
        Self {
            id: record.logical_id(),
            revision: record.revision,
            raw_text: record.raw_text,
            variant: record.delivered_text,
            edited_text: record.edited_text,
            action,
            timestamp_ms: record.timestamp_ms,
        }
    }
}

/// Finalize one correction through the core's revision + atomic lexicon
/// transaction and return the refreshed resolved record.
#[uniffi::export]
pub fn quality_finalize_correction(
    correction_id: String,
    canonical: String,
) -> Result<CsQualityRecord, CsError> {
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
}

impl From<CustomLexiconEntry> for CsLexiconEntry {
    fn from(entry: CustomLexiconEntry) -> Self {
        Self {
            variant: entry.variant,
            canonical: entry.canonical,
        }
    }
}

#[uniffi::export]
pub fn commit_overlay_quality_record(
    raw_text: String,
    delivered_text: String,
    edited_text: String,
    action: String,
    formatting_level: String,
) -> Result<(), CsError> {
    // Delegate to core. Model/mode are best-effort for MVP (overlay always).
    // action carried for meta (over-correct for P2-03: "captureQualityIfEdited gubi action").
    commit_overlay_correction_with_level(
        &raw_text,
        &delivered_text,
        &edited_text,
        "overlay",
        None,
        Some(&action),
        Some(&formatting_level),
    )
    .map(|_path| ())
    .map_err(|e| CsError::Quality {
        msg: format!("quality commit failed: {}", e),
    })
}

/// Read the newest persisted corrections, newest first. Missing storage is an
/// empty list; genuine I/O failures cross the bridge as a quality error.
#[uniffi::export]
pub fn quality_recent_records(limit: u64) -> Result<Vec<CsQualityRecord>, CsError> {
    let limit = usize::try_from(limit).map_err(|error| CsError::Quality {
        msg: format!("quality record limit is invalid: {error}"),
    })?;
    recent_quality_records(limit)
        .map(|records| records.into_iter().map(Into::into).collect())
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

#[cfg(test)]
mod tests {
    use super::*;

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
            meta: serde_json::json!({ "action": "copy" }),
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
                timestamp_ms: 42,
            }
        );
    }

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

        let result = commit_overlay_quality_record(
            "synthetic raw".into(),
            "synthetic variant".into(),
            "synthetic canonical".into(),
            "copy".into(),
            "creative".into(),
        );
        let records = recent_quality_records(10).expect("read committed quality record");
        let lexicon = custom_lexicon_entries().expect("read custom lexicon");

        match previous {
            // SAFETY: restore the exact process environment captured above.
            Some(value) => unsafe { std::env::set_var("CODESCRIBE_DATA_DIR", value) },
            // SAFETY: the variable was absent before this serial test.
            None => unsafe { std::env::remove_var("CODESCRIBE_DATA_DIR") },
        }
        std::fs::remove_dir_all(&temp_root).expect("remove temp quality root");

        result.expect("bridge commit");
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].formatting_level.as_deref(), Some("max"));
        assert!(
            lexicon.is_empty(),
            "Max evidence must not teach the lexicon"
        );
    }

    #[test]
    fn commit_overlay_quality_record_rejects_unknown_level_before_write() {
        let error = commit_overlay_quality_record(
            "raw".into(),
            "variant".into(),
            "canonical".into(),
            "close".into(),
            "mystery".into(),
        )
        .expect_err("unknown level must be rejected");

        assert!(error.to_string().contains("unknown FORMATTING_LEVEL"));
    }
}
