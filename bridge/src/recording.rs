//! Shared recording bridge types: audio-input settings, Whisper model download,
//! the controller event listener, and microphone permission probes. Live capture
//! itself is owned exclusively by `CodescribeHotkeys`/`RecordingController`.

use std::sync::Arc;

use codescribe::presentation::transcript_bus::{
    ProjectedAcousticReceipt, TranscriptBusEvidenceEvent,
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
}

/// Bridge event schema for one reducer-owned transcript projection. It carries
/// a rendered value and evidence but no document mutation or finality method.
///
/// W2 input: `TranscriptBusEvidenceEvent`. W2 output: the foreign listener
/// callback below. The producer and Swift consumer remain unresolved in W1;
/// UniFFI binding regeneration is explicitly deferred to W3.
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
    pub acoustic_receipts: Vec<CsProjectedAcousticReceipt>,
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
            acoustic_receipts: event
                .acoustic_receipts
                .iter()
                .map(CsProjectedAcousticReceipt::from_bus_receipt)
                .collect(),
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

async fn transcribe_file_cloud(path: String) -> Result<CsTranscription, CsError> {
    let config = codescribe_core::config::Config::load();
    let endpoint = config
        .stt_endpoint
        .clone()
        .filter(|value| !value.trim().is_empty());
    let key = config
        .stt_api_key
        .clone()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_default();
    let Some(endpoint) = endpoint else {
        return Err(CsError::Recording {
            msg: "Cloud pass needs STT_ENDPOINT".to_string(),
        });
    };
    // Same invert as Settings → Test: a stored Voice Lab socket is not a
    // multipart URL. Public HTTPS file URLs stay file.
    let endpoint = codescribe_core::stt::tail_provider::file_probe_endpoint(&endpoint);
    if codescribe_core::stt::tail_provider::stt_auth_mode(&endpoint)
        != codescribe_core::stt::tail_provider::SttAuthMode::Unauthenticated
        && key.is_empty()
    {
        return Err(CsError::Recording {
            msg: "Cloud pass needs STT_API_KEY for this endpoint".to_string(),
        });
    }
    let verdict =
        codescribe::client::transcribe_cloud(std::path::Path::new(&path), None, &endpoint, &key)
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

    #[test]
    fn cloud_pass_inverts_voice_lab_socket_to_file() {
        assert_eq!(
            codescribe_core::stt::tail_provider::file_probe_endpoint(
                "ws://127.0.0.1:8446/v1/audio/transcribe"
            ),
            "http://127.0.0.1:8444/v1/audio/transcriptions"
        );
        assert_eq!(
            codescribe_core::stt::tail_provider::file_probe_endpoint(
                "https://api.libraxis.cloud/v1/audio/transcriptions"
            ),
            "https://api.libraxis.cloud/v1/audio/transcriptions"
        );
    }

    #[test]
    fn remapped_loopback_file_url_names_programming_vocabulary() {
        let endpoint = codescribe_core::stt::tail_provider::file_probe_endpoint(
            "ws://127.0.0.1:8446/v1/audio/transcribe",
        );
        assert_eq!(endpoint, "http://127.0.0.1:8444/v1/audio/transcriptions");
        assert_eq!(
            codescribe_core::stt::request_vocabulary::codescribe_stt_vocabulary_form_part(
                &endpoint
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
    use codescribe::presentation::transcript_bus::TranscriptMode;

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
                }],
            }
        );
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
