//! P0-D: thin UniFFI surface for the quality/correction loop.
//! One module per concern (matches bridge discipline). Does NOT bloat recording.rs.
//!
//! Exposes commit for overlay FINAL edits:
//!   - capture (raw, delivered, edited)
//!   - write quality record JSONL
//!   - feed safe lexicon candidates to the custom lexicon consumed by PostProcessor
//!
//! Privacy: local disk only.

use crate::CsError;

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

// Future: CsQualityRecord record type + richer meta if needed.
// For MVP strings keep surface tiny and wiring trivial from Swift.

#[cfg(test)]
mod tests {
    // Bridge tests are light; real behavior tested via core + integration.
}
