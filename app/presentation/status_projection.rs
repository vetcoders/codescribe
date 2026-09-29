//! Typed, non-transcript product status projected over the existing IPC stream.
//!
//! These events explain why capture did not start, or how guided calibration
//! ended. They share the controller's presentation transport but never enter
//! `TranscriptBusEvidenceEvent`: no occurrence, acoustic receipt, reducer
//! revision, or replay row is invented for a status that is not transcript
//! truth.

use serde::{Deserialize, Serialize};

pub const PRESENTATION_STATUS_SCHEMA: &str = "codescribe.presentation-status.v1";

/// Product-visible status classes. The serialized spelling is the stable Swift
/// and diagnostic contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PresentationStatusKind {
    AdmissionRefused,
    CalibrationSucceeded,
    CalibrationFailed,
}

/// One passive status card. Rust owns all wording and classification; Swift
/// paints these values and exposes no repair action.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PresentationStatusProjection {
    pub schema: String,
    pub emitted_at: String,
    pub session_id: Option<String>,
    pub kind: PresentationStatusKind,
    pub code: String,
    pub status_label: String,
    pub headline: String,
    pub message: String,
    pub is_error: bool,
    pub terminal: bool,
    pub calibration_version: Option<String>,
}

impl PresentationStatusProjection {
    pub fn admission_refused(
        session_id: Option<String>,
        code: impl Into<String>,
        message: impl Into<String>,
    ) -> Self {
        let code = code.into();
        Self {
            schema: PRESENTATION_STATUS_SCHEMA.to_string(),
            emitted_at: now_rfc3339(),
            session_id,
            kind: PresentationStatusKind::AdmissionRefused,
            status_label: "recording blocked".to_string(),
            headline: admission_headline(&code).to_string(),
            code,
            message: message.into(),
            is_error: true,
            terminal: true,
            calibration_version: None,
        }
    }

    pub fn calibration_succeeded(version: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            schema: PRESENTATION_STATUS_SCHEMA.to_string(),
            emitted_at: now_rfc3339(),
            session_id: None,
            kind: PresentationStatusKind::CalibrationSucceeded,
            code: "calibration_succeeded".to_string(),
            status_label: "calibrated".to_string(),
            headline: "Microphone calibration saved".to_string(),
            message: message.into(),
            is_error: false,
            terminal: true,
            calibration_version: Some(version.into()),
        }
    }

    pub fn calibration_failed(message: impl Into<String>) -> Self {
        Self {
            schema: PRESENTATION_STATUS_SCHEMA.to_string(),
            emitted_at: now_rfc3339(),
            session_id: None,
            kind: PresentationStatusKind::CalibrationFailed,
            code: "calibration_failed".to_string(),
            status_label: "calibration failed".to_string(),
            headline: "Microphone calibration failed".to_string(),
            message: message.into(),
            is_error: true,
            terminal: true,
            calibration_version: None,
        }
    }
}

fn admission_headline(code: &str) -> &'static str {
    match code {
        "admission_calibration_missing" | "admission_calibration_no_profile" => {
            "Microphone calibration required"
        }
        "admission_calibration_refused" | "admission_calibration_unusable" => {
            "Stored microphone calibration cannot be used"
        }
        "admission_microphone_permission_unavailable" => "Microphone access is unavailable",
        "admission_speech_recognition_unavailable" => "Speech Recognition is unavailable",
        "admission_capture_device_unavailable" => "No microphone is available",
        "admission_seal_lane_disarmed" => "Recording admission is disabled",
        "admission_seal_vad_unavailable" => "Speech detection is unavailable",
        _ => "Recording could not start",
    }
}

fn now_rfc3339() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn admission_status_is_actionable_without_claiming_transcript_truth() {
        let status = PresentationStatusProjection::admission_refused(
            Some("session-1".to_string()),
            "admission_calibration_unusable",
            "capture generation changed — Re-run Calibrate microphone in Settings › Audio",
        );

        assert_eq!(status.schema, PRESENTATION_STATUS_SCHEMA);
        assert_eq!(status.kind, PresentationStatusKind::AdmissionRefused);
        assert_eq!(status.status_label, "recording blocked");
        assert!(status.message.contains("Settings › Audio"));
        assert!(status.is_error);
        assert!(status.terminal);
        assert_eq!(status.calibration_version, None);
    }

    #[test]
    fn calibration_outcomes_keep_success_version_and_failure_reason() {
        let success = PresentationStatusProjection::calibration_succeeded(
            "cal2-built-in-microphone-1",
            "Built-in Microphone at 48000 Hz is ready for recording.",
        );
        assert_eq!(success.kind, PresentationStatusKind::CalibrationSucceeded);
        assert_eq!(
            success.calibration_version.as_deref(),
            Some("cal2-built-in-microphone-1")
        );
        assert!(!success.is_error);

        let failure = PresentationStatusProjection::calibration_failed(
            "calibration_capture_failed: microphone disconnected",
        );
        assert_eq!(failure.kind, PresentationStatusKind::CalibrationFailed);
        assert!(failure.message.contains("microphone disconnected"));
        assert!(failure.is_error);
        assert_eq!(failure.calibration_version, None);
    }
}
