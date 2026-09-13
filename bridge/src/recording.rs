//! Shared recording bridge types: audio-input settings, Whisper model download,
//! the controller event listener, and microphone permission probes. Live capture
//! itself is owned exclusively by `CodescribeHotkeys`/`RecordingController`.

use std::sync::Arc;

use codescribe::presentation::status_projection::{
    PresentationStatusKind, PresentationStatusProjection,
};
use codescribe::presentation::transcript_bus::{
    ProjectedAcousticReceipt, ProjectedPresentationReceipt, ProjectedSealCoverageReceipt,
    TranscriptBusEvidenceEvent, TranscriptDelivery,
};
use codescribe_core::pipeline::contracts::{AnnotationKind, LayerSource, LayerSummary};
use cpal::traits::{DeviceTrait, HostTrait};

use crate::{CsError, application_runtime};

/// Result of a one-shot file transcription.
#[derive(uniffi::Record)]
pub struct CsTranscription {
    /// Final post-processed transcript text.
    pub text: String,
    /// Detected (or requested) language code, e.g. `"pl"` / `"en"`.
    pub language: String,
}

/// UniFFI-safe, immutable projection of one ledger-owned acoustic receipt.
/// W2 copies the matching Bus fields byte-for-byte; the bridge cannot admit,
/// reconcile, seal, or otherwise reinterpret this evidence.
#[derive(uniffi::Record, Debug, Clone, PartialEq)]
pub struct CsProjectedAcousticReceipt {
    pub acoustic_serial_version: u16,
    pub acoustic_serial: String,
    pub session_id: String,
    pub capture_epoch: u64,
    pub sample_start: u64,
    pub sample_end: u64,
    pub duration_ms: u64,
    pub energy_integral: f64,
    pub mean_rms_dbfs: f32,
    pub peak_dbfs: f32,
    pub vad_open_sample: u64,
    pub vad_close_sample: u64,
    pub evidence_calibration_version: String,
    pub word_evidence_receipts: Vec<String>,
    pub layer_decision_receipts: Vec<String>,
    pub seal_receipt: Option<String>,
    pub manual_edit_receipt: Option<String>,
    pub presentation_receipt: Option<CsProjectedPresentationReceipt>,
}

/// Presentation proof remains separate from acoustic evidence and human edits.
#[derive(uniffi::Record, Debug, Clone, PartialEq, Eq)]
pub struct CsProjectedPresentationReceipt {
    pub receipt_id: String,
    pub provenance: String,
    pub session_id: String,
    pub source_revision: u64,
    pub revision: u64,
    pub capture_epoch: u64,
    pub sample_start: u64,
    pub sample_end: u64,
    pub source_seal_receipt: String,
    pub source_label: String,
    pub left_context: String,
    pub left_context_sha256: String,
    pub shaped_text: String,
}

impl CsProjectedPresentationReceipt {
    fn from_bus_receipt(receipt: &ProjectedPresentationReceipt) -> Self {
        Self {
            receipt_id: receipt.receipt_id.clone(),
            provenance: receipt.provenance.clone(),
            session_id: receipt.session_id.clone(),
            source_revision: receipt.source_revision,
            revision: receipt.revision,
            capture_epoch: receipt.capture_epoch,
            sample_start: receipt.sample_start,
            sample_end: receipt.sample_end,
            source_seal_receipt: receipt.source_seal_receipt.clone(),
            source_label: receipt.source_label.clone(),
            left_context: receipt.left_context.clone(),
            left_context_sha256: receipt.left_context_sha256.clone(),
            shaped_text: receipt.shaped_text.clone(),
        }
    }
}

/// Typed projection of the existing Bus coverage tokens. Unknown/legacy data
/// never becomes complete. This bridge does not assess acoustic evidence.
#[derive(uniffi::Enum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum CsSealCoverageStatus {
    Unknown,
    Complete,
    Incomplete,
    Unavailable,
}

#[derive(uniffi::Enum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum CsCoverageUnavailableReason {
    Unknown,
    NotObserved,
    IdentityMismatch,
    InvalidMeasurement,
    PartialObservation,
}

#[derive(uniffi::Record, Debug, Clone, PartialEq, Eq)]
pub struct CsProjectedSealCoverageRange {
    pub sample_start: u64,
    pub sample_end: u64,
}

#[derive(uniffi::Record, Debug, Clone, PartialEq)]
pub struct CsProjectedSealCoverageReceipt {
    pub status: CsSealCoverageStatus,
    pub unavailable_reason: Option<CsCoverageUnavailableReason>,
    pub speech_samples: u64,
    pub covered_samples: u64,
    pub uncovered_speech_ranges: Vec<CsProjectedSealCoverageRange>,
    pub max_uncovered_samples: u64,
    pub incomplete_threshold_samples: u64,
    pub speech_producer: String,
    pub availability: String,
    pub observed_samples: Option<u64>,
    pub coverage_ratio: Option<f64>,
}

impl CsProjectedSealCoverageReceipt {
    fn from_bus_receipt(receipt: &ProjectedSealCoverageReceipt) -> Self {
        Self {
            status: match receipt.status.as_str() {
                "complete" => CsSealCoverageStatus::Complete,
                "incomplete" => CsSealCoverageStatus::Incomplete,
                "unavailable" => CsSealCoverageStatus::Unavailable,
                _ => CsSealCoverageStatus::Unknown,
            },
            unavailable_reason: receipt
                .unavailable_reason
                .as_deref()
                .map(|reason| match reason {
                    "not_observed" => CsCoverageUnavailableReason::NotObserved,
                    "identity_mismatch" => CsCoverageUnavailableReason::IdentityMismatch,
                    "invalid_measurement" => CsCoverageUnavailableReason::InvalidMeasurement,
                    "partial_observation" => CsCoverageUnavailableReason::PartialObservation,
                    _ => CsCoverageUnavailableReason::Unknown,
                }),
            speech_samples: receipt.speech_samples,
            covered_samples: receipt.covered_samples,
            uncovered_speech_ranges: receipt
                .uncovered_speech_ranges
                .iter()
                .map(|range| CsProjectedSealCoverageRange {
                    sample_start: range.sample_start,
                    sample_end: range.sample_end,
                })
                .collect(),
            max_uncovered_samples: receipt.max_uncovered_samples,
            incomplete_threshold_samples: receipt.incomplete_threshold_samples,
            speech_producer: receipt.speech_producer.clone(),
            availability: receipt.availability.clone(),
            observed_samples: receipt.observed_samples,
            coverage_ratio: receipt.coverage_ratio,
        }
    }
}

/// Bridge event schema for the one reducer-owned transcript projection. It
/// carries the full render, phase, availability, terminal state, and evidence,
/// but exposes no document mutation method.
///
/// Input is `TranscriptBusEvidenceEvent`; output is the foreign listener
/// callback below. UniFFI binding regeneration is deferred to plan attestation.
#[derive(uniffi::Record, Debug, Clone, PartialEq)]
pub struct CsTranscriptProjectionEvent {
    pub schema: String,
    pub sequence: u64,
    pub emitted_at: String,
    pub session_id: String,
    pub mode: String,
    pub reducer_revision: u64,
    pub reducer_action: String,
    pub occurrence_session_id: String,
    pub capture_epoch: u64,
    pub sample_start: u64,
    pub sample_end: u64,
    pub document_index: u64,
    pub label: String,
    pub rendered_text: String,
    pub phase: String,
    pub can_paste: bool,
    pub can_insert: bool,
    pub can_copy: bool,
    pub can_retranscribe: bool,
    pub can_format: bool,
    pub terminal: bool,
    /// True only for the session's lifecycle terminal. A terminal *revision* of
    /// the document is not the end of the capture, and only this flag tells the
    /// two apart without reading an action string.
    pub lifecycle_terminal: bool,
    /// Controller-owned delivery disposition, forwarded verbatim. Swift branches
    /// on this typed state; the human-facing `label` above stays presentation and
    /// never carries control meaning.
    pub delivery: CsTranscriptDelivery,
    pub acoustic_receipts: Vec<CsProjectedAcousticReceipt>,
    pub seal_coverage: Option<CsProjectedSealCoverageReceipt>,
}

/// Swift-visible mirror of [`TranscriptDelivery`]. One variant per controller
/// disposition, so no consumer has to parse a label to learn a destination.
#[derive(uniffi::Enum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum CsTranscriptDelivery {
    /// No stop-path delivery ran for this take.
    Unattempted,
    /// Destined for the Agent composer draft of the capturing thread, and not
    /// yet admitted by it. This is an obligation, never a success claim.
    ComposerPending,
    /// A system sink accepted the text at the OS boundary.
    SinkAccepted,
    /// No sink took the text; it stays recoverable.
    Retained,
}

/// The controller-admitted identity of one capture.
///
/// Issued by `start_composer_turn_recording` and required by the conditional
/// stop. Swift holds it as opaque evidence: it proves *which* take a gesture
/// opened, so a stop can be refused when a different take now owns the mic.
#[derive(uniffi::Record, Debug, Clone, PartialEq, Eq)]
pub struct CsCaptureHandle {
    pub capture_id: String,
}

/// Typed outcome of a conditional stop. Every variant is a state the caller can
/// act on; none of them is an error string to match. A failed transport may
/// retry the same handle to join/retrieve the retained controller operation.
/// Stopped acknowledges processing, not consumption of addressed delivery.
#[derive(uniffi::Enum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum CsConditionalStop {
    /// The identity matched the live capture and the stop path ran.
    Stopped,
    /// A different capture owns the microphone. It was left running.
    ForeignCapture,
    /// Nothing is capturing. Nothing was stopped and nothing was started.
    NoLiveCapture,
    /// This capture is already inside its own stop path. Not stopped twice.
    AlreadyStopping,
    /// A tracked controller task still owes settlement. Keep capture ownership.
    Pending,
    /// No task was admitted. Keep the handle; an explicit retry is safe.
    AdmissionUnavailable,
}

/// Passive, typed product status from Rust presentation authority. This is a
/// sibling of transcript projection, not a transcript event: it carries no
/// reducer revision or acoustic evidence and exposes no repair command.
#[derive(uniffi::Record, Debug, Clone, PartialEq, Eq)]
pub struct CsPresentationStatusEvent {
    pub schema: String,
    pub emitted_at: String,
    pub session_id: Option<String>,
    pub kind: String,
    pub code: String,
    pub status_label: String,
    pub headline: String,
    pub message: String,
    pub is_error: bool,
    pub terminal: bool,
    pub calibration_version: Option<String>,
}

/// Capture-bound ephemeral paint. No document or delivery mutation is exposed.
#[derive(uniffi::Record, Debug, Clone, PartialEq, Eq)]
pub struct CsCompactProjection {
    pub session_id: String,
    pub capture_epoch: u64,
    pub sequence: u64,
    pub text: String,
    pub degraded: bool,
}

impl From<codescribe::presentation::emitter::CompactProjection> for CsCompactProjection {
    fn from(value: codescribe::presentation::emitter::CompactProjection) -> Self {
        Self {
            session_id: value.session_id,
            capture_epoch: value.capture_epoch,
            sequence: value.sequence,
            text: value.text,
            degraded: value.degraded,
        }
    }
}

impl CsProjectedAcousticReceipt {
    pub(crate) fn from_bus_receipt(receipt: &ProjectedAcousticReceipt) -> Self {
        Self {
            acoustic_serial_version: receipt.acoustic_serial_version,
            acoustic_serial: receipt.acoustic_serial.clone(),
            session_id: receipt.session_id.clone(),
            capture_epoch: receipt.capture_epoch,
            sample_start: receipt.sample_start,
            sample_end: receipt.sample_end,
            duration_ms: receipt.duration_ms,
            energy_integral: receipt.energy_integral,
            mean_rms_dbfs: receipt.mean_rms_dbfs,
            peak_dbfs: receipt.peak_dbfs,
            vad_open_sample: receipt.vad_open_sample,
            vad_close_sample: receipt.vad_close_sample,
            evidence_calibration_version: receipt.evidence_calibration_version.clone(),
            word_evidence_receipts: receipt.word_evidence_receipts.clone(),
            layer_decision_receipts: receipt.layer_decision_receipts.clone(),
            seal_receipt: receipt.seal_receipt.clone(),
            manual_edit_receipt: receipt.manual_edit_receipt.clone(),
            presentation_receipt: receipt
                .presentation_receipt
                .as_ref()
                .map(CsProjectedPresentationReceipt::from_bus_receipt),
        }
    }
}

impl CsTranscriptDelivery {
    /// One total mapping. A new Rust disposition must be given a Swift variant
    /// here rather than silently collapsing into an existing one.
    pub(crate) fn from_bus_delivery(delivery: TranscriptDelivery) -> Self {
        match delivery {
            TranscriptDelivery::Unattempted => Self::Unattempted,
            TranscriptDelivery::ComposerPending => Self::ComposerPending,
            TranscriptDelivery::SinkAccepted => Self::SinkAccepted,
            TranscriptDelivery::Retained => Self::Retained,
        }
    }
}

impl CsTranscriptProjectionEvent {
    pub(crate) fn from_bus_event(event: &TranscriptBusEvidenceEvent) -> Self {
        Self {
            schema: event.schema.clone(),
            sequence: event.sequence,
            emitted_at: event.emitted_at.clone(),
            session_id: event.session_id.clone(),
            mode: format!("{:?}", event.mode).to_lowercase(),
            reducer_revision: event.reducer_revision,
            reducer_action: event.reducer_action.clone(),
            occurrence_session_id: event.occurrence_session_id.clone(),
            capture_epoch: event.capture_epoch,
            sample_start: event.sample_start,
            sample_end: event.sample_end,
            document_index: event.document_index,
            label: event.label.clone(),
            rendered_text: event.rendered_text.clone(),
            phase: event.phase.as_str().to_string(),
            can_paste: event.can_paste,
            can_insert: event.can_insert,
            can_copy: event.can_copy,
            can_retranscribe: event.can_retranscribe,
            can_format: event.can_format,
            terminal: event.terminal,
            lifecycle_terminal: event.lifecycle_terminal,
            delivery: CsTranscriptDelivery::from_bus_delivery(event.delivery),
            seal_coverage: event
                .seal_coverage
                .as_ref()
                .map(CsProjectedSealCoverageReceipt::from_bus_receipt),
            acoustic_receipts: event
                .acoustic_receipts
                .iter()
                .map(CsProjectedAcousticReceipt::from_bus_receipt)
                .collect(),
        }
    }
}

impl CsPresentationStatusEvent {
    pub(crate) fn from_projection(event: &PresentationStatusProjection) -> Self {
        let kind = match event.kind {
            PresentationStatusKind::AdmissionRefused => "admission_refused",
            PresentationStatusKind::CalibrationSucceeded => "calibration_succeeded",
            PresentationStatusKind::CalibrationFailed => "calibration_failed",
        };
        Self {
            schema: event.schema.clone(),
            emitted_at: event.emitted_at.clone(),
            session_id: event.session_id.clone(),
            kind: kind.to_string(),
            code: event.code.clone(),
            status_label: event.status_label.clone(),
            headline: event.headline.clone(),
            message: event.message.clone(),
            is_error: event.is_error,
            terminal: event.terminal,
            calibration_version: event.calibration_version.clone(),
        }
    }
}

/// Live audio-input resolution used by Settings. `runtime_device` is resolved
/// from the same cpal host and matching policy as `Recorder::start`: a
/// configured exact/substring match wins, otherwise the current system default
/// is the honest fallback. It is intentionally a snapshot, not a second store.
#[derive(uniffi::Record, Debug, Clone, PartialEq, Eq)]
pub struct CsAudioInputSnapshot {
    pub devices: Vec<String>,
    pub configured_device: Option<String>,
    pub runtime_device: Option<String>,
    pub configured_device_available: bool,
    pub fallback_to_default: bool,
    /// False when settings.json and the recorder's process-env selector differ.
    /// The UI must then show the current runtime device, not the saved wish.
    pub runtime_configuration_matches: bool,
}

/// Admission readiness of the next product recording, projected for Settings,
/// the overlay, and the tray. `ready == false` carries exactly one blocker
/// (`code` + `message` with the action) — the same verdict the controller
/// applies before it opens a microphone. Never a second decision.
#[derive(uniffi::Record, Debug, Clone, PartialEq)]
pub struct CsAdmissionReadiness {
    pub ready: bool,
    /// `admission_granted` or the blocker code (`admission_*`).
    pub code: String,
    /// User-readable explanation + action (empty when granted).
    pub message: String,
    pub device_name: Option<String>,
    pub sample_rate: Option<u32>,
    pub calibration_version: Option<String>,
    /// Loader verdict on the calibration artifact: `sealed` / `missing` / `refused`.
    pub calibration_status: String,
    pub calibration_path: String,
    pub calibrated_devices: Vec<String>,
    /// Effective value after the optional power-user override.
    pub seal_lane_armed: bool,
    /// Persisted Settings › Audio value before an override.
    pub seal_lane_setting_armed: bool,
    /// `settings` or `env_override`.
    pub seal_lane_source: String,
    pub seal_lane_env: String,
}

/// What a guided calibration measured and stored (levels and counts only).
#[derive(uniffi::Record, Debug, Clone, PartialEq)]
pub struct CsEnergyCalibrationReport {
    pub device_name: String,
    pub sample_rate: u32,
    pub measured_seconds: f32,
    pub active_speech_median_dbfs: f32,
    pub noise_floor_dbfs: Option<f32>,
    pub peak_dbfs: f32,
    pub existence_threshold_dbfs: f32,
    pub version: String,
    pub path: String,
}

/// Whether local Whisper weights are ready (embedded or on-disk). Used by
/// Settings → Dictation so users can download the model without a fat DMG.
#[derive(uniffi::Record, Debug, Clone, PartialEq, Eq)]
pub struct CsWhisperModelStatus {
    pub available: bool,
    pub embedded: bool,
    pub path: Option<String>,
    pub model_id: String,
    pub repo: String,
    pub size_hint: String,
}

impl From<codescribe_core::config::models::WhisperModelStatus> for CsWhisperModelStatus {
    /// Map core Whisper readiness into the FFI record Settings displays.
    fn from(s: codescribe_core::config::models::WhisperModelStatus) -> Self {
        Self {
            available: s.available,
            embedded: s.embedded,
            path: s.path,
            model_id: s.model_id,
            repo: s.repo,
            size_hint: s.size_hint,
        }
    }
}

/// Progress callbacks for Settings Whisper download (large, multi-file).
/// `bytes_total` is `-1` when the server did not send Content-Length.
#[uniffi::export(with_foreign)]
pub trait CsWhisperDownloadListener: Send + Sync {
    /// One progress tick for `file`. `bytes_total` is `-1` when the server sent no
    /// Content-Length, so the UI must fall back to an indeterminate indicator.
    fn on_progress(&self, file: String, bytes_done: u64, bytes_total: i64);
    /// The whole model is on disk at `path`.
    fn on_complete(&self, path: String);
}

/// Snapshot Whisper availability without constructing a dictation session.
#[uniffi::export]
pub fn whisper_model_status() -> CsWhisperModelStatus {
    CsWhisperModelStatus::from(codescribe_core::config::models::whisper_model_status())
}

/// Download the default Whisper model (idempotent if already complete).
#[uniffi::export]
pub async fn download_whisper_model(
    listener: Option<Arc<dyn CsWhisperDownloadListener>>,
) -> Result<CsWhisperModelStatus, CsError> {
    application_runtime::run(async move {
        tokio::task::spawn_blocking(move || {
            let path = codescribe_core::config::models::download_default_whisper_model(
                |file, done, total| {
                    if let Some(ref listener) = listener {
                        listener.on_progress(
                            file.to_string(),
                            done,
                            total.map(|t| t as i64).unwrap_or(-1),
                        );
                    }
                },
            )
            .map_err(|e| CsError::Recording { msg: e.to_string() })?;
            if let Some(ref listener) = listener {
                listener.on_complete(path.display().to_string());
            }
            Ok(CsWhisperModelStatus::from(
                codescribe_core::config::models::whisper_model_status(),
            ))
        })
        .await
        .map_err(|e| CsError::Recording {
            msg: format!("download_whisper_model join error: {e}"),
        })?
    })
    .await?
}

/// Trim a device name and collapse blank/whitespace-only values to `None`, so an
/// empty setting reads as "no preference" rather than a device named `""`.
fn normalized_device_name(device: Option<&str>) -> Option<String> {
    device
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(str::to_owned)
}

/// Whether the configured device is present among live inputs, using the same
/// exact-then-substring policy as `Recorder::start`. No configured device counts
/// as available: the system default always exists.
fn device_is_available(configured_device: Option<&str>, devices: &[String]) -> bool {
    let Some(configured_device) = normalized_device_name(configured_device) else {
        return true;
    };
    let configured_lower = configured_device.to_lowercase();
    devices.iter().any(|device| {
        *device == configured_device || device.to_lowercase().contains(&configured_lower)
    })
}

/// Resolve which input the recorder will actually use, mirroring `Recorder::start`.
///
/// Returns `(resolved_device, configured_device_available, fell_back_to_default)`.
/// A configured name wins on exact match, then on case-insensitive substring; an
/// unplugged device degrades to the system default and is reported as a fallback
/// so the UI can say so instead of showing a device that is not recording.
fn resolve_audio_input_state(
    configured_device: Option<&str>,
    devices: &[String],
    default_device: Option<&str>,
) -> (Option<String>, bool, bool) {
    let configured_device = configured_device
        .map(str::trim)
        .filter(|name| !name.is_empty());
    let Some(configured_device) = configured_device else {
        return (default_device.map(str::to_owned), true, false);
    };

    let configured_lower = configured_device.to_lowercase();
    if let Some(device) = devices.iter().find(|device| {
        *device == configured_device || device.to_lowercase().contains(&configured_lower)
    }) {
        return (Some(device.clone()), true, false);
    }

    (default_device.map(str::to_owned), false, true)
}

/// Enumerate live input hardware and resolve the effective recorder device.
/// Failures cross the bridge as one `CsError::Recording` concern; no device
/// names are persisted here.
#[uniffi::export]
pub fn audio_input_snapshot() -> Result<CsAudioInputSnapshot, CsError> {
    let configured_device = codescribe_core::config::UserSettings::load().audio_input_device;
    // Recorder::start reads this process value directly. It is the actual
    // selector for the current app lifetime, while `configured_device` is the
    // freshly-persisted choice for the next launch.
    let runtime_preference =
        normalized_device_name(std::env::var("AUDIO_INPUT_DEVICE").ok().as_deref());
    let host = cpal::default_host();
    let default_device = host
        .default_input_device()
        .and_then(|device| device.description().ok())
        .map(|description| description.to_string());

    let mut devices: Vec<String> = host
        .input_devices()
        .map_err(|error| CsError::Recording {
            msg: format!("failed to enumerate audio input devices: {error}"),
        })?
        .filter_map(|device| device.description().ok())
        .map(|description| description.to_string())
        .collect();

    if let Some(ref default_device) = default_device
        && !devices.contains(default_device)
    {
        devices.push(default_device.clone());
    }
    devices.sort_unstable_by_key(|name| name.to_lowercase());
    devices.dedup();

    let (runtime_device, _, fallback_to_default) = resolve_audio_input_state(
        runtime_preference.as_deref(),
        &devices,
        default_device.as_deref(),
    );
    let configured_device_available = device_is_available(configured_device.as_deref(), &devices);
    let runtime_configuration_matches =
        normalized_device_name(configured_device.as_deref()) == runtime_preference;

    Ok(CsAudioInputSnapshot {
        devices,
        configured_device,
        runtime_device,
        configured_device_available,
        fallback_to_default,
        runtime_configuration_matches,
    })
}

/// Bridge-safe source for bounded transcript replacement events.
#[derive(uniffi::Enum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum CsLayerSource {
    TailPatch,
    Lexicon,
    InlineLlm,
    FinalBam,
}

impl From<LayerSource> for CsLayerSource {
    /// Map a core layer tag onto the bridge-safe UniFFI enum variant.
    fn from(source: LayerSource) -> Self {
        match source {
            LayerSource::TailPatch => Self::TailPatch,
            LayerSource::Lexicon => Self::Lexicon,
            LayerSource::InlineLlm => Self::InlineLlm,
            LayerSource::FinalBam => Self::FinalBam,
        }
    }
}

/// Bridge-safe annotation kind. `label` is set for paralingual annotations.
#[derive(uniffi::Record, Debug, Clone, PartialEq, Eq)]
pub struct CsAnnotationKind {
    pub kind: String,
    pub label: Option<String>,
}

impl From<&AnnotationKind> for CsAnnotationKind {
    /// Flatten core annotation kinds into a stringly FFI record Swift can switch.
    fn from(kind: &AnnotationKind) -> Self {
        match kind {
            AnnotationKind::HesitationPause => Self {
                kind: "hesitation_pause".to_string(),
                label: None,
            },
            AnnotationKind::Paralingual { label } => Self {
                kind: "paralingual".to_string(),
                label: Some(label.clone()),
            },
        }
    }
}

/// Session-end counters emitted with `SessionFinalised`.
#[derive(uniffi::Record, Debug, Clone, PartialEq, Eq)]
pub struct CsLayerSummary {
    pub tail_patch_replacements: u64,
    pub lexicon_replacements: u64,
    pub inline_llm_replacements: u64,
    pub final_bam_replacements: u64,
    pub annotations_inserted: u64,
}

impl From<&LayerSummary> for CsLayerSummary {
    /// Copy session-end layer counters into the FFI summary record.
    fn from(summary: &LayerSummary) -> Self {
        Self {
            tail_patch_replacements: summary.tail_patch_replacements,
            lexicon_replacements: summary.lexicon_replacements,
            inline_llm_replacements: summary.inline_llm_replacements,
            final_bam_replacements: summary.final_bam_replacements,
            annotations_inserted: summary.annotations_inserted,
        }
    }
}

/// Path prefixes pick the Retranscribe pass:
/// - `hq:` or no prefix — Full HQ file pass (`transcribe_file_verdict`)
/// - `cloud:` — Cloud pass (`transcribe_cloud` with Settings STT credentials)
pub(crate) async fn transcribe_session_file(path: String) -> Result<CsTranscription, CsError> {
    let (pass, file_path) = split_retranscribe_path(&path);
    match pass {
        RetranscribePass::Hq => tokio::task::spawn_blocking(move || transcribe_file_hq(file_path))
            .await
            .map_err(|e| CsError::Recording {
                msg: format!("transcribe_file task join error: {e}"),
            })?,
        RetranscribePass::Cloud => transcribe_file_cloud(file_path).await,
    }
}

/// `~/.codescribe/last_session.wav` when the last stop retained audio.
pub(crate) fn last_session_audio_path() -> Option<String> {
    let dest = codescribe_core::config::Config::config_dir().join("last_session.wav");
    dest.exists().then(|| dest.to_string_lossy().into_owned())
}

enum RetranscribePass {
    Hq,
    Cloud,
}

fn split_retranscribe_path(path: &str) -> (RetranscribePass, String) {
    if let Some(rest) = path.strip_prefix("cloud:") {
        (RetranscribePass::Cloud, rest.to_string())
    } else if let Some(rest) = path.strip_prefix("hq:") {
        (RetranscribePass::Hq, rest.to_string())
    } else {
        (RetranscribePass::Hq, path.to_string())
    }
}

fn transcribe_file_hq(path: String) -> Result<CsTranscription, CsError> {
    let verdict = codescribe_core::stt::transcribe_file_verdict(std::path::Path::new(&path), None)
        .map_err(|e| CsError::Recording { msg: e.to_string() })?;
    Ok(CsTranscription {
        text: verdict.text,
        language: "und".to_string(),
    })
}

fn cloud_file_lane(
    config: &codescribe_core::config::Config,
) -> Result<codescribe_core::stt::lanes::ResolvedSttLane, CsError> {
    let lane = config
        .stt_lane(codescribe_core::stt::lanes::SttLane::File)
        .ok_or_else(|| CsError::Recording {
            msg: "Cloud pass needs a file transcription endpoint (Providers › Speech-to-text)"
                .into(),
        })?;
    if lane.key_missing() {
        return Err(CsError::Recording {
            msg: "Cloud pass needs STT_FILE_API_KEY for this endpoint".into(),
        });
    }
    Ok(lane)
}

async fn transcribe_file_cloud(path: String) -> Result<CsTranscription, CsError> {
    let lane = cloud_file_lane(&codescribe_core::config::Config::load())?;
    let verdict = codescribe::client::transcribe_cloud(
        std::path::Path::new(&path),
        None,
        &lane.endpoint,
        lane.api_key.as_deref().unwrap_or_default(),
    )
    .await
    .map_err(|e| CsError::Recording { msg: e.to_string() })?;
    Ok(CsTranscription {
        text: verdict.text,
        language: "und".to_string(),
    })
}

#[cfg(test)]
mod retranscribe_tests {
    use super::*;

    #[test]
    fn retranscribe_path_prefixes_select_hq_or_cloud() {
        assert!(matches!(
            split_retranscribe_path("/tmp/last_session.wav"),
            (RetranscribePass::Hq, path) if path == "/tmp/last_session.wav"
        ));
        assert!(matches!(
            split_retranscribe_path("hq:/tmp/last_session.wav"),
            (RetranscribePass::Hq, path) if path == "/tmp/last_session.wav"
        ));
        assert!(matches!(
            split_retranscribe_path("cloud:/tmp/last_session.wav"),
            (RetranscribePass::Cloud, path) if path == "/tmp/last_session.wav"
        ));
    }

    /// Real instrument, not a seam: both overlay Retranscribe passes over a
    /// real session WAV. HQ = local Whisper file pass; Cloud = the File STT
    /// lane with the operator's real endpoint and key (`Config::load`, may
    /// open Keychain). Prints the verdicts so a human can read them.
    ///
    ///   PROOF_WAV=~/.codescribe/sessions/<id>.wav \
    ///   cargo test -p codescribe-ffi -- --ignored --nocapture retranscribe_passes_on_real_session_audio
    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "real audio + real STT credentials"]
    async fn retranscribe_passes_on_real_session_audio() {
        let wav = std::env::var("PROOF_WAV").unwrap_or_else(|_| {
            codescribe_core::config::Config::config_dir()
                .join("last_session.wav")
                .to_string_lossy()
                .into_owned()
        });
        assert!(
            std::path::Path::new(&wav).exists(),
            "PROOF_WAV missing: {wav}"
        );

        let hq = transcribe_session_file(format!("hq:{wav}"))
            .await
            .expect("HQ file pass");
        eprintln!("HQ pass: {} chars: {:?}", hq.text.chars().count(), hq.text);
        assert!(
            !hq.text.trim().is_empty(),
            "HQ pass returned no text for real speech"
        );

        match transcribe_session_file(format!("cloud:{wav}")).await {
            Ok(cloud) => {
                eprintln!(
                    "Cloud pass: {} chars: {:?}",
                    cloud.text.chars().count(),
                    cloud.text
                );
                assert!(
                    !cloud.text.trim().is_empty(),
                    "Cloud pass answered with no text for real speech"
                );
            }
            Err(err) => panic!("Cloud pass failed: {err}"),
        }
    }

    #[test]
    fn cloud_pass_reads_the_file_lane_only() {
        let mut config = codescribe_core::config::Config {
            stt_live_endpoint: Some("wss://api.libraxis.cloud/v1/audio/transcribe".into()),
            stt_live_api_key: Some("live-only-key".into()),
            ..Default::default()
        };
        assert!(
            cloud_file_lane(&config)
                .unwrap_err()
                .to_string()
                .contains("file transcription endpoint")
        );
        config.stt_file_endpoint =
            Some("https://api.libraxis.cloud/v1/audio/transcriptions".into());
        assert!(
            cloud_file_lane(&config)
                .unwrap_err()
                .to_string()
                .contains("STT_FILE_API_KEY")
        );
        config.stt_file_api_key = Some("file-only-key".into());
        let lane = cloud_file_lane(&config).unwrap();
        assert_eq!(Some(&lane.endpoint), config.stt_file_endpoint.as_ref());
        assert_eq!(lane.api_key.as_deref(), Some("file-only-key"));
        config.stt_file_api_key = None;
        config.stt_file_endpoint = Some("http://127.0.0.1:8444/v1/audio/transcriptions".into());
        let lane = cloud_file_lane(&config).unwrap();
        assert_eq!(
            lane.endpoint,
            "http://127.0.0.1:8444/v1/audio/transcriptions"
        );
        assert_eq!(
            codescribe_core::stt::request_vocabulary::codescribe_stt_vocabulary_form_part(
                &lane.endpoint
            ),
            Some(("vocabulary", "programming"))
        );
    }
}

/// Foreign callback trait — dictation events forwarded to Swift.
///
/// `on_transcript_projection` is the sole transcript callback. Raw preview,
/// final, correction, patch, and annotation events remain on the IPC stream as
/// diagnostics; they do not cross this product-facing callback boundary.
/// - `on_vad_active` flips when speech starts/ends.
/// - `on_no_speech` fires when a session/utterance produced no usable speech.
/// - `on_error` carries recoverable engine warnings.
///
/// The Swift side must hop these onto the main actor.
#[uniffi::export(with_foreign)]
pub trait CsTranscriptionListener: Send + Sync {
    /// Immutable reducer/ledger projection. Swift may display it but cannot
    /// mutate, seal, or reinterpret transcript truth through this callback.
    fn on_transcript_projection(&self, event: CsTranscriptProjectionEvent);
    /// Typed product status. Swift may display it but receives no settings or
    /// repair command through this passive projection.
    fn on_presentation_status(&self, event: CsPresentationStatusEvent);
    /// Ordered passive compact paint from the opened recorder capture.
    fn on_compact_projection(&self, event: CsCompactProjection);
    /// The engine is spinning up capture; no audio is flowing yet.
    fn on_recording_preparing(&self);
    /// The microphone is live and utterances may start arriving.
    fn on_recording_started(&self);
    /// Terminal state of a dictation session — capture and any post-capture pass
    /// are both finished. Always the last lifecycle callback.
    fn on_recording_stopped(&self);
    /// Capture ended and the controller entered `Busy` (final transcription pass).
    /// Fired BEFORE `on_recording_stopped` (which lands on the terminal Idle) so a
    /// hotkey hold-release / toggle stop can show a distinct "transcribing" phase
    /// instead of leaving the live-capture UI up while the final pass runs. The
    /// Swift-driven Finish path enters that phase itself; this is the native-path
    /// counterpart. Surfaces with no post-capture phase may leave it a no-op.
    fn on_recording_finalising(&self);
    /// The session closed; `layer_summary` carries the per-layer edit counters.
    fn on_session_finalised(&self, session_id: String, layer_summary: CsLayerSummary);
    /// Voice activity started (`true`) or stopped (`false`).
    fn on_vad_active(&self, active: bool);
    /// Live microphone input level: RMS of one captured audio block (linear,
    /// 0..~1). Fires continuously (~40–50 Hz) while a controller dictation
    /// session records, so the overlay waveform can track the real voice.
    /// Surfaces without a level meter may leave it a no-op.
    fn on_audio_level(&self, rms: f32);
    /// A session or utterance yielded no usable speech; `reason` explains which
    /// check rejected it, so the UI can distinguish silence from a failure.
    fn on_no_speech(&self, reason: String);
    /// Recoverable engine warning. Not fatal — the session keeps running.
    fn on_error(&self, message: String);
}

/// True when microphone permission is already granted.
/// Wraps `os::permissions::check_microphone` (app/os/permissions.rs:135).
#[uniffi::export]
pub fn mic_permission_granted() -> bool {
    codescribe::os::permissions::check_microphone()
        == codescribe::os::permissions::PermissionStatus::Granted
}

/// Request microphone permission (shows the system dialog when undetermined),
/// returning whether access is granted.
/// Wraps `os::permissions::request_microphone` (app/os/permissions.rs:301).
#[uniffi::export]
pub fn request_mic_permission() -> bool {
    codescribe::os::permissions::request_microphone()
}

#[cfg(test)]
mod tests {
    use super::*;
    use codescribe::presentation::transcript_bus::{
        TranscriptDelivery, TranscriptMode, TranscriptProjectionPhase,
    };

    #[test]
    fn bus_projection_conversion_preserves_every_authority_field() {
        let event = TranscriptBusEvidenceEvent {
            schema: "codescribe.transcript-evidence.v1".to_string(),
            sequence: 7,
            emitted_at: "2026-08-27T12:00:00Z".to_string(),
            session_id: "bus-session".to_string(),
            mode: TranscriptMode::Agent,
            reducer_revision: 11,
            reducer_action: "apply_ledger_decision".to_string(),
            occurrence_session_id: "occurrence-session".to_string(),
            capture_epoch: 13,
            sample_start: 17,
            sample_end: 23,
            document_index: 29,
            label: "Iwo".to_string(),
            rendered_text: "Iwo".to_string(),
            phase: TranscriptProjectionPhase::Formatted,
            can_paste: true,
            can_insert: true,
            can_copy: true,
            can_retranscribe: true,
            can_format: true,
            terminal: true,
            lifecycle_terminal: true,
            delivery: TranscriptDelivery::ComposerPending,
            acoustic_receipts: vec![ProjectedAcousticReceipt {
                acoustic_serial_version: 2,
                acoustic_serial: "sha256:acoustic".to_string(),
                session_id: "occurrence-session".to_string(),
                capture_epoch: 13,
                sample_start: 17,
                sample_end: 23,
                duration_ms: 31,
                energy_integral: 37.5,
                mean_rms_dbfs: -41.0,
                peak_dbfs: -43.0,
                vad_open_sample: 47,
                vad_close_sample: 53,
                evidence_calibration_version: "energy-calibration.v2".to_string(),
                word_evidence_receipts: vec!["word-receipt".to_string()],
                layer_decision_receipts: vec!["layer-receipt".to_string()],
                seal_receipt: Some("seal-receipt".to_string()),
                manual_edit_receipt: Some("manual-edit-receipt".to_string()),
                presentation_receipt: None,
            }],
            seal_coverage: None,
            comparison: None,
        };

        let projected = CsTranscriptProjectionEvent::from_bus_event(&event);

        assert_eq!(
            projected,
            CsTranscriptProjectionEvent {
                schema: "codescribe.transcript-evidence.v1".to_string(),
                sequence: 7,
                emitted_at: "2026-08-27T12:00:00Z".to_string(),
                session_id: "bus-session".to_string(),
                mode: "agent".to_string(),
                reducer_revision: 11,
                reducer_action: "apply_ledger_decision".to_string(),
                occurrence_session_id: "occurrence-session".to_string(),
                capture_epoch: 13,
                sample_start: 17,
                sample_end: 23,
                document_index: 29,
                label: "Iwo".to_string(),
                rendered_text: "Iwo".to_string(),
                phase: "formatted".to_string(),
                can_paste: true,
                can_insert: true,
                can_copy: true,
                can_retranscribe: true,
                can_format: true,
                terminal: true,
                lifecycle_terminal: true,
                delivery: CsTranscriptDelivery::ComposerPending,
                seal_coverage: None,
                acoustic_receipts: vec![CsProjectedAcousticReceipt {
                    acoustic_serial_version: 2,
                    acoustic_serial: "sha256:acoustic".to_string(),
                    session_id: "occurrence-session".to_string(),
                    capture_epoch: 13,
                    sample_start: 17,
                    sample_end: 23,
                    duration_ms: 31,
                    energy_integral: 37.5,
                    mean_rms_dbfs: -41.0,
                    peak_dbfs: -43.0,
                    vad_open_sample: 47,
                    vad_close_sample: 53,
                    evidence_calibration_version: "energy-calibration.v2".to_string(),
                    word_evidence_receipts: vec!["word-receipt".to_string()],
                    layer_decision_receipts: vec!["layer-receipt".to_string()],
                    seal_receipt: Some("seal-receipt".to_string()),
                    manual_edit_receipt: Some("manual-edit-receipt".to_string()),
                    presentation_receipt: None,
                }],
            }
        );
    }

    #[test]
    fn nonempty_ledger_shaping_survives_the_actual_bus_to_bridge_mapping() {
        use codescribe::presentation::emitter::TranscriptReducer;
        use codescribe::presentation::transcript_bus::{TranscriptBus, TranscriptSession};
        use codescribe_core::pipeline::acoustic_ledger::{
            AcousticEvidence, AcousticLedger, EnergyCalibration, ObservationIdentity,
            ObservationProducer, OccurrenceIdentity,
        };
        let mut ledger = AcousticLedger::new();
        let mut reducer = TranscriptReducer::default();
        let occurrence = OccurrenceIdentity::new("bridge-shaping", 3, 32_000, 48_000);
        let calibration = EnergyCalibration::new("bridge-fixture", 1.0, 1);
        let evidence = AcousticEvidence {
            occurrence: occurrence.clone(),
            duration_ms: 1_000.0,
            energy_integral: 10.0,
            mean_rms_dbfs: -12.0,
            peak_dbfs: -3.0,
            vad_open_sample: Some(32_000),
            vad_close_sample: Some(48_000),
            evidence_calibration_version: calibration.version.clone(),
        };
        assert!(ledger.qualify(&evidence, &calibration).is_qualified());
        let observation =
            ObservationIdentity::new(ObservationProducer::Apple, 1, 0, occurrence.clone());
        let mutation = ledger.admit(&observation, "zażółć gęślą");
        reducer
            .apply_ledger_mutation(&ledger, &observation, &mutation)
            .unwrap();
        ledger.schedule_frontier(occurrence.clone(), [ObservationProducer::Apple]);
        assert!(ledger.note_frontier_return(&occurrence, ObservationProducer::Apple));
        let seal = ledger.seal(&occurrence).unwrap().clone();
        reducer.apply_ledger_seal(&seal).unwrap();
        let revision = reducer
            .apply_incremental_shaping(&mut ledger, &occurrence)
            .unwrap();
        let temp = tempfile::tempdir().unwrap();
        let bus = TranscriptBus::open_at(
            TranscriptSession {
                session_id: "bridge-shaping".to_string(),
                mode: TranscriptMode::Agent,
                has_latched_target: false,
                latched_target_is_self: false,
            },
            temp.path().join("bus"),
            None,
        )
        .unwrap();
        let events = bus.publish_revision(&revision, &ledger);
        assert_eq!(events.len(), 1);
        let projected = CsTranscriptProjectionEvent::from_bus_event(&events[0]);
        let actual = projected.acoustic_receipts[0]
            .presentation_receipt
            .as_ref()
            .unwrap();
        let minted = &ledger.incremental_shapings()[0];
        assert_eq!(
            actual,
            &CsProjectedPresentationReceipt {
                receipt_id: minted.receipt_id.clone(),
                provenance: "light-plus".to_string(),
                session_id: "bridge-shaping".to_string(),
                source_revision: minted.source_revision,
                revision: minted.revision,
                capture_epoch: 3,
                sample_start: 32_000,
                sample_end: 48_000,
                source_seal_receipt: seal.receipt_id,
                source_label: "zażółć gęślą".to_string(),
                left_context: String::new(),
                left_context_sha256: minted.left_context_sha256.clone(),
                shaped_text: "Zażółć gęślą.".to_string(),
            }
        );
        assert_eq!(projected.rendered_text, actual.shaped_text);
        assert!(projected.acoustic_receipts[0].manual_edit_receipt.is_none());
        assert_eq!(projected.phase, "listening");
        assert!(!projected.terminal && !projected.lifecycle_terminal);
        assert_eq!(projected.delivery, CsTranscriptDelivery::Unattempted);
    }

    fn coverage_bus_fixture(
        availability: codescribe_core::audio::capture_receipt::AcousticAvailability,
    ) -> (TranscriptBusEvidenceEvent, TranscriptBusEvidenceEvent) {
        use codescribe::presentation::emitter::TranscriptReducer;
        use codescribe::presentation::transcript_bus::{TranscriptBus, TranscriptSession};
        use codescribe_core::pipeline::acoustic_ledger::{
            AcousticEvidence, AcousticLedger, EnergyCalibration, ObservationIdentity,
            ObservationProducer, OccurrenceIdentity,
        };
        let mut ledger = AcousticLedger::new();
        let mut reducer = TranscriptReducer::default();
        let occurrence = OccurrenceIdentity::new("bridge-shaping", 3, 32_000, 48_000);
        let calibration = EnergyCalibration::new("bridge-fixture", 1.0, 1);
        let evidence = AcousticEvidence {
            occurrence: occurrence.clone(),
            duration_ms: 1_000.0,
            energy_integral: 10.0,
            mean_rms_dbfs: -12.0,
            peak_dbfs: -3.0,
            vad_open_sample: Some(32_000),
            vad_close_sample: Some(48_000),
            evidence_calibration_version: calibration.version.clone(),
        };
        assert!(ledger.qualify(&evidence, &calibration).is_qualified());
        let observation =
            ObservationIdentity::new(ObservationProducer::Apple, 1, 0, occurrence.clone());
        let mutation = ledger.admit(&observation, "zażółć gęślą");
        reducer
            .apply_ledger_mutation(&ledger, &observation, &mutation)
            .unwrap();
        ledger.schedule_frontier(occurrence.clone(), [ObservationProducer::Apple]);
        assert!(ledger.note_frontier_return(&occurrence, ObservationProducer::Apple));
        let seal = ledger.seal(&occurrence).unwrap().clone();
        reducer.apply_ledger_seal(&seal).unwrap();
        reducer
            .apply_incremental_shaping(&mut ledger, &occurrence)
            .unwrap();
        let temp = tempfile::tempdir().unwrap();
        let bus = TranscriptBus::open_at(
            TranscriptSession {
                session_id: "bridge-shaping".to_string(),
                mode: TranscriptMode::Agent,
                has_latched_target: false,
                latched_target_is_self: false,
            },
            temp.path().join("bus"),
            None,
        )
        .unwrap();

        use codescribe_core::audio::capture_receipt::{
            AcousticSpeechEvidence, CaptureEvidenceIdentity,
        };
        use codescribe_core::stt::tail_provider::TailSampleRange;
        let speech = AcousticSpeechEvidence::measured(
            CaptureEvidenceIdentity::new("bridge-shaping", 3),
            "capture_energy",
            availability,
            vec![TailSampleRange {
                session: "bridge-shaping".into(),
                capture_epoch: 3,
                sample_start: 32_000,
                sample_end: 64_000,
            }],
        );
        let coverage = ledger.assess_seal_coverage("bridge-shaping", 3, &speech, 4_000);
        assert!(ledger.record_seal_coverage(coverage.clone()));
        let revision = reducer.apply_seal_coverage(&coverage, None);
        bus.publish_started();
        let events = bus.publish_revision(&revision, &ledger);
        assert_eq!(events.len(), 1);
        let terminal = bus.publish_ended(
            codescribe::presentation::transcript_bus::TranscriptSessionEndReason::CoverageRefused,
            true, TranscriptDelivery::ComposerPending,
        ).unwrap();
        assert!(bus.publish_ended(
            codescribe::presentation::transcript_bus::TranscriptSessionEndReason::CoverageRefused,
            true, TranscriptDelivery::ComposerPending,
        ).is_none(), "the producer emits one lifecycle terminal");
        (events[0].clone(), terminal)
    }

    #[test]
    fn incomplete_coverage_crosses_real_bus_and_bridge_with_lifecycle_order() {
        use codescribe_core::audio::capture_receipt::AcousticAvailability;
        let (document, terminal) = coverage_bus_fixture(AcousticAvailability::Observed {
            observed_samples: 64_000,
        });
        assert_eq!(terminal.reducer_revision, document.reducer_revision);
        assert_eq!(terminal.capture_epoch, document.capture_epoch);
        assert!(terminal.sequence > document.sequence);
        let projected = CsTranscriptProjectionEvent::from_bus_event(&terminal);
        let coverage = projected.seal_coverage.unwrap();
        assert_eq!(coverage.status, CsSealCoverageStatus::Incomplete);
        assert_eq!(coverage.unavailable_reason, None);
        assert_eq!(coverage.coverage_ratio, Some(0.5));
        assert_eq!(coverage.speech_samples, 32_000);
        assert_eq!(coverage.covered_samples, 16_000);
        assert_eq!(
            coverage.uncovered_speech_ranges,
            vec![CsProjectedSealCoverageRange {
                sample_start: 48_000,
                sample_end: 64_000
            }]
        );
        assert_eq!(coverage.max_uncovered_samples, 16_000);
        assert_eq!(coverage.incomplete_threshold_samples, 4_000);
        assert_eq!(coverage.observed_samples, Some(64_000));
        assert_eq!(coverage.speech_producer, "capture_energy");
        assert_eq!(coverage.availability, "observed");
        assert_eq!(projected.rendered_text, document.rendered_text);
        assert_eq!(projected.phase, "coverage_refused");
        assert!(projected.lifecycle_terminal);
        assert_eq!(projected.delivery, CsTranscriptDelivery::ComposerPending);
    }

    #[test]
    fn every_unavailable_coverage_reason_crosses_real_bus_and_bridge_without_ratio() {
        use codescribe_core::audio::capture_receipt::AcousticAvailability;
        for (availability, expected) in [
            (
                AcousticAvailability::NotObserved,
                CsCoverageUnavailableReason::NotObserved,
            ),
            (
                AcousticAvailability::IdentityMismatch,
                CsCoverageUnavailableReason::IdentityMismatch,
            ),
            (
                AcousticAvailability::InvalidMeasurement {
                    valid_samples: 32_000,
                },
                CsCoverageUnavailableReason::InvalidMeasurement,
            ),
            (
                AcousticAvailability::Discontinuous {
                    observed_samples: 32_000,
                },
                CsCoverageUnavailableReason::PartialObservation,
            ),
        ] {
            let (document, terminal) = coverage_bus_fixture(availability);
            let projected = CsTranscriptProjectionEvent::from_bus_event(&terminal);
            let coverage = projected.seal_coverage.unwrap();
            assert_eq!(coverage.status, CsSealCoverageStatus::Unavailable);
            assert_eq!(coverage.unavailable_reason, Some(expected));
            assert_eq!(coverage.coverage_ratio, None);
            assert_eq!(coverage.observed_samples, None);
            assert_eq!(coverage.speech_samples, 0);
            assert_eq!(coverage.covered_samples, 0);
            assert!(coverage.uncovered_speech_ranges.is_empty());
            assert_eq!(coverage.max_uncovered_samples, 0);
            assert_eq!(coverage.incomplete_threshold_samples, 4_000);
            assert_eq!(coverage.speech_producer, "capture_energy");
            assert_eq!(coverage.availability, availability.as_str());
            assert_eq!(projected.rendered_text, document.rendered_text);
            assert_eq!(projected.phase, "coverage_refused");
            assert_eq!(projected.delivery, CsTranscriptDelivery::ComposerPending);
        }
    }

    #[test]
    fn legacy_and_unknown_coverage_never_become_complete() {
        use codescribe_core::audio::capture_receipt::AcousticAvailability;
        let (_, mut event) = coverage_bus_fixture(AcousticAvailability::NotObserved);
        event.seal_coverage = None;
        assert!(
            CsTranscriptProjectionEvent::from_bus_event(&event)
                .seal_coverage
                .is_none()
        );
        let (_, mut event) = coverage_bus_fixture(AcousticAvailability::NotObserved);
        event.seal_coverage.as_mut().unwrap().status = "future_status".into();
        event.seal_coverage.as_mut().unwrap().unavailable_reason = Some("future_reason".into());
        let coverage = CsTranscriptProjectionEvent::from_bus_event(&event)
            .seal_coverage
            .unwrap();
        assert_eq!(coverage.status, CsSealCoverageStatus::Unknown);
        assert_eq!(
            coverage.unavailable_reason,
            Some(CsCoverageUnavailableReason::Unknown)
        );
        assert_eq!(coverage.coverage_ratio, None);
    }

    #[test]
    fn presentation_status_conversion_preserves_rust_owned_copy_and_classification() {
        let event = PresentationStatusProjection::admission_refused(
            Some("session-1".to_string()),
            "admission_calibration_unusable",
            "capture generation changed — Re-run Calibrate microphone in Settings › Audio",
        );

        let projected = CsPresentationStatusEvent::from_projection(&event);

        assert_eq!(projected.schema, "codescribe.presentation-status.v1");
        assert_eq!(projected.session_id.as_deref(), Some("session-1"));
        assert_eq!(projected.kind, "admission_refused");
        assert_eq!(projected.code, "admission_calibration_unusable");
        assert_eq!(projected.status_label, "recording blocked");
        assert!(projected.message.contains("Settings › Audio"));
        assert!(projected.is_error);
        assert!(projected.terminal);
        assert_eq!(projected.calibration_version, None);
    }

    #[test]
    fn audio_input_resolution_reports_live_match_and_unavailable_fallback() {
        let devices = vec![
            "MacBook Pro Microphone".to_string(),
            "USB Studio Mic".to_string(),
        ];
        assert_eq!(
            resolve_audio_input_state(Some("Studio Mic"), &devices, Some("MacBook Pro Microphone")),
            (Some("USB Studio Mic".to_string()), true, false)
        );
        assert_eq!(
            resolve_audio_input_state(
                Some("Unplugged Mic"),
                &devices,
                Some("MacBook Pro Microphone")
            ),
            (Some("MacBook Pro Microphone".to_string()), false, true)
        );
        assert!(device_is_available(Some("Studio Mic"), &devices));
        assert!(!device_is_available(Some("Unplugged Mic"), &devices));
    }
}
