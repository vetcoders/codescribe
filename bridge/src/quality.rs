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
    CustomLexiconEntry, QualityRecord, custom_lexicon_entries, recent_quality_records,
};

use crate::CsError;

/// UI-safe projection of a persisted overlay correction.
#[derive(uniffi::Record, Debug, Clone, PartialEq, Eq)]
pub struct CsQualityRecord {
    pub raw_text: String,
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
            raw_text: record.raw_text,
            edited_text: record.edited_text,
            action,
            timestamp_ms: record.timestamp_ms,
        }
    }
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
) -> Result<(), CsError> {
    // Delegate to core. Model/mode are best-effort for MVP (overlay always).
    // action carried for meta (over-correct for P2-03: "captureQualityIfEdited gubi action").
    codescribe_core::quality::overlay_quality::commit_overlay_correction(
        &raw_text,
        &delivered_text,
        &edited_text,
        "overlay",
        None,
        Some(&action),
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
            timestamp_ms: 42,
            session_id: None,
            mode: "overlay".into(),
            model: None,
            raw_text: "raw".into(),
            delivered_text: "delivered".into(),
            edited_text: "edited".into(),
            meta: serde_json::json!({ "action": "copy" }),
        };

        assert_eq!(
            CsQualityRecord::from(record),
            CsQualityRecord {
                raw_text: "raw".into(),
                edited_text: "edited".into(),
                action: "copy".into(),
                timestamp_ms: 42,
            }
        );
    }
}
