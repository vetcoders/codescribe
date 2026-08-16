//! Shared recording bridge types: audio-input settings, Whisper model download,
//! the controller event listener, and microphone permission probes. Live capture
//! itself is owned exclusively by `CodescribeHotkeys`/`RecordingController`.

use std::sync::Arc;

use codescribe_core::pipeline::contracts::{AnnotationKind, LayerSource, LayerSummary};
use cpal::traits::{DeviceTrait, HostTrait};

use crate::CsError;

/// Result of a one-shot file transcription.
#[derive(uniffi::Record)]
pub struct CsTranscription {
    /// Final post-processed transcript text.
    pub text: String,
    /// Detected (or requested) language code, e.g. `"pl"` / `"en"`.
    pub language: String,
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
#[uniffi::export(async_runtime = "tokio")]
pub async fn download_whisper_model(
    listener: Option<Arc<dyn CsWhisperDownloadListener>>,
) -> Result<CsWhisperModelStatus, CsError> {
    tokio::task::spawn_blocking(move || {
        let path =
            codescribe_core::config::models::download_default_whisper_model(|file, done, total| {
                if let Some(ref listener) = listener {
                    listener.on_progress(
                        file.to_string(),
                        done,
                        total.map(|t| t as i64).unwrap_or(-1),
                    );
                }
            })
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
}

/// Foreign callback trait — dictation events forwarded to Swift.
///
/// Distilled from the engine's richer `EngineEvent` stream:
/// - `on_preview` carries the latest interim/corrected utterance text
///   (replace-not-append semantics).
/// - `on_final` carries a completed (VAD-bounded) utterance together with its
///   `utterance_id`, so committed sinks can stamp the segment identity that
///   later `on_replace_range` / `on_insert_annotation` patches target.
/// - `on_vad_active` flips when speech starts/ends.
/// - `on_no_speech` fires when a session/utterance produced no usable speech.
/// - `on_error` carries recoverable engine warnings.
///
/// The Swift side must hop these onto the main actor.
#[uniffi::export(with_foreign)]
pub trait CsTranscriptionListener: Send + Sync {
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
    /// Latest interim text for the utterance in flight. Replace-not-append: each
    /// call supersedes the previous preview rather than extending it.
    fn on_preview(&self, text: String);
    /// An already-previewed utterance was revised; `previous_text` is what the
    /// surface currently shows, so it can locate and swap the right span.
    fn on_correction(&self, text: String, previous_text: String);
    /// Completed VAD-bounded utterance. Optional STT quality fields feed the
    /// overlay confidence badge + quality-loop meta (LL-D); empty when unknown.
    fn on_final(
        &self,
        utterance_id: u64,
        text: String,
        avg_logprob: Option<f32>,
        speech_pct: Option<f32>,
        confidence_flags: Vec<String>,
    );
    /// Bounded patch of an already-committed utterance: replace `[start, end)`
    /// within the segment stamped `utterance_id`. `source` names the layer that
    /// produced it, so the surface can attribute or style the edit.
    fn on_replace_range(
        &self,
        utterance_id: u64,
        start: u64,
        end: u64,
        text: String,
        source: CsLayerSource,
    );
    /// Insert an annotation (hesitation pause, paralingual marker) at `position`
    /// inside the segment stamped `utterance_id`, without replacing any text.
    fn on_insert_annotation(
        &self,
        utterance_id: u64,
        position: u64,
        text: String,
        kind: CsAnnotationKind,
    );
    /// Insert a context-bucket marker at the global transcript character
    /// position captured when the agent combo was pressed.
    fn on_context_marker(&self, position: u64, marker: String);
    /// The session closed; `layer_summary` carries the per-layer edit counters.
    fn on_session_finalised(&self, session_id: String, layer_summary: CsLayerSummary);
    /// Authoritative post-stop transcript (LocalFinalPass `final_formatted_text`):
    /// the SAME clean text that is pasted/delivered and written to history. Surfaces
    /// fire it once per dictation stop so the overlay FINAL matches delivery/Copy.
    fn on_final_transcript_ready(&self, text: String);
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
