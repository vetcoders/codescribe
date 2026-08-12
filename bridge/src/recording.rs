//! Dictation / STT surface — thin UniFFI wrapper over the live codescribe
//! streaming recorder + Whisper singleton. Translates the engine's semantic
//! `EngineEvent` stream into a small foreign listener contract so the new
//! SwiftUI app can drive real microphone dictation and file transcription.
//! Filled by W3 cut #3 (sibling to `agent.rs`). Uses shared
//! `crate::{CsError, CsLanguage}`.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex as StdMutex, RwLock};
use std::time::{Duration, Instant};

use codescribe::os::tray_status::{self, TrayStatus};
use codescribe_core::asr_session::GatewaySessionAvailability;
use codescribe_core::audio::load_audio_file;
use codescribe_core::audio::streaming_recorder::StreamingRecorder;
use codescribe_core::config::{FinalPassRoutingMode, UserSettings};
use codescribe_core::pipeline::contracts::{
    AnnotationKind, EngineEvent, EventSink, FileTranscriptionOptions, LayerSource, LayerSummary,
};
use codescribe_core::stt::{TailGapBoundary, resolve_tail_gap_boundary, whisper};
use cpal::traits::{DeviceTrait, HostTrait};
use tokio::sync::Mutex;
use tracing::{info, warn};

use crate::{CsError, CsLanguage};

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

/// Accumulates finalized utterance text for the composer voice-note return,
/// mirroring core's crate-private `SessionTranscriptCollector` discipline
/// (skip empty, single-space join, trimmed). The same `CsEventSink` that
/// forwards engine events to Swift feeds each `UtteranceFinal` here, so
/// `stop_recording` can compose the return AFTER the streaming session's
/// completion signal fires — reusing existing finalization, not a new channel.
#[derive(Default)]
struct ComposerTranscript {
    text: StdMutex<String>,
    utterances: AtomicU64,
    /// End timestamp of the last committed utterance — the audio boundary Smart
    /// mode gap-fills from. Mirrors `SessionTelemetrySink` in the controller lane.
    committed_through_secs: StdMutex<Option<f32>>,
}

impl ComposerTranscript {
    /// Advance the committed audio boundary (monotonic max: an out-of-order
    /// final never rewinds it). Called for **every** `UtteranceFinal`, including
    /// empty ones — that audio is adjudicated even when it carried no text, so
    /// a tail gap-fill must not transcribe it again.
    fn note_committed_through(&self, end_ts: f32) {
        if !end_ts.is_finite() {
            return;
        }
        let mut guard = self
            .committed_through_secs
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        *guard = Some(match *guard {
            Some(current) if current >= end_ts => current,
            _ => end_ts,
        });
    }

    /// Committed audio boundary, or `None` when no final sealed any audio yet.
    fn committed_through_secs(&self) -> Option<f32> {
        *self
            .committed_through_secs
            .lock()
            .unwrap_or_else(|e| e.into_inner())
    }

    /// Append one finalized utterance (Layer 0 committed text). Empty/whitespace
    /// finals are ignored so trailing silence never widens the transcript.
    fn append_final(&self, text: &str) {
        let trimmed = text.trim();
        if trimmed.is_empty() {
            return;
        }
        let mut buf = self.text.lock().unwrap_or_else(|e| e.into_inner());
        if !buf.is_empty() {
            buf.push(' ');
        }
        buf.push_str(trimmed);
        self.utterances.fetch_add(1, Ordering::Relaxed);
    }

    /// Current composed transcript and the number of utterances that fed it.
    fn snapshot(&self) -> (String, u64) {
        let text = self.text.lock().unwrap_or_else(|e| e.into_inner()).clone();
        (text, self.utterances.load(Ordering::Relaxed))
    }
}

/// Wait budget for `stop_recording` to compose its return: it covers BOTH the
/// streaming drain AND the delivery-grade final pass over the saved WAV.
/// Proportional to recording length (STT work scales with audio) but clamped so
/// the composer UI never hangs indefinitely if the scheduler stalls (e.g.
/// thermal throttling): the floor covers a cold commit + short final pass, the
/// cap bounds the worst case. On exhaustion the streaming splice is returned as
/// a fallback, so overrun degrades quality, never correctness.
fn compose_stop_timeout(elapsed: Duration) -> Duration {
    /// Minimum drain budget so a cold commit + short final pass still fits.
    const FLOOR: Duration = Duration::from_secs(8);
    /// Hard upper bound so a stalled scheduler never hangs the composer forever.
    const CAP: Duration = Duration::from_secs(30);
    elapsed.mul_f32(0.6).clamp(FLOOR, CAP)
}

/// Which transcript `stop_recording` returned, for the stop breadcrumb.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ComposerTranscriptSource {
    /// Live floor + whole-WAV Whisper gap-fill (doctrine: never full-replace).
    /// Also covers whisper-only when the stream produced nothing.
    MergedLiveWhisper,
    /// Smart tail gap-fill APPENDED to the committed streaming floor. The tail is
    /// a bare fragment, never diffed against committed text (append-only doctrine).
    TailGapAppend,
    /// Spliced streaming `UtteranceFinal` chunks (final pass unavailable/empty).
    StreamingFallback,
}

impl ComposerTranscriptSource {
    /// Stable log token for the stop breadcrumb.
    fn label(self) -> &'static str {
        match self {
            Self::MergedLiveWhisper => "merged_live_whisper",
            Self::TailGapAppend => "tail_gap_append",
            Self::StreamingFallback => "streaming_fallback",
        }
    }
}

/// Pick the composer return.
///
/// Overlay doctrine (AGENTS.md law): the live streaming assembly is the floor
/// of truth — a non-empty whole-WAV final pass never replaces it, it merges as
/// gap-fill via `merge_live_whisper` (substitution disagreements keep live, so
/// a collapsing file-STT final can no longer blank or shrink a real stream,
/// and an inflated stream is never swapped for a shorter Whisper guess).
/// Empty/absent final falls back to the streaming splice. Both inputs trimmed.
fn select_composer_transcript(
    final_pass: Option<&str>,
    streaming: &str,
) -> (String, ComposerTranscriptSource) {
    let streaming = streaming.trim();
    if let Some(text) = final_pass {
        let trimmed = text.trim();
        if !trimmed.is_empty() {
            let merged = codescribe_core::quality::merge_live_whisper(streaming, trimmed);
            return (merged.text, ComposerTranscriptSource::MergedLiveWhisper);
        }
    }
    (
        streaming.to_string(),
        ComposerTranscriptSource::StreamingFallback,
    )
}

/// Compose the composer return from the planned final pass — the plan decides
/// HOW the Whisper text is allowed to meet the live floor.
///
/// - `TailGap` (Smart): the Whisper text is a **bare tail** of the uncommitted
///   audio, not a transcript of the whole session. It is APPENDED via the shared
///   core primitive (`codescribe_core::stt::append_tail_gap`), which keeps the
///   committed text as an untouched prefix and only dedups repeated preview words
///   from the tail side. Feeding a bare tail to `merge_live_whisper` (as this lane
///   used to) turns the boundary Delete+Insert pair into a Substitute that keeps
///   live and DISCARDS the whisper token — measurable gap-fill word loss.
/// - `FullFile` (Always): a whole-WAV transcript, which is exactly what
///   `merge_live_whisper` is built for — merge as gap-fill over the live floor.
/// - `SkipStreaming` (Off / Smart-without-boundary): no final pass exists;
///   the streaming splice is the answer.
fn compose_composer_transcript(
    plan: ComposerFinalPassPlan,
    final_pass: Option<&str>,
    streaming: &str,
) -> (String, ComposerTranscriptSource) {
    match plan {
        ComposerFinalPassPlan::TailGap(_) => {
            let streaming = streaming.trim();
            let tail = final_pass.map(str::trim).unwrap_or_default();
            if tail.is_empty() {
                return (
                    streaming.to_string(),
                    ComposerTranscriptSource::StreamingFallback,
                );
            }
            (
                codescribe_core::stt::append_tail_gap(streaming, tail),
                ComposerTranscriptSource::TailGapAppend,
            )
        }
        ComposerFinalPassPlan::FullFile | ComposerFinalPassPlan::SkipStreaming => {
            select_composer_transcript(final_pass, streaming)
        }
    }
}

/// What the composer stop lane is allowed to run over the saved WAV, per
/// `FINAL_PASS_MODE` (operator law 2026-08-05).
#[derive(Debug, Clone, Copy, PartialEq)]
enum ComposerFinalPassPlan {
    /// Always only: re-transcribe the whole file.
    FullFile,
    /// Smart: transcribe the uncommitted tail from this boundary and append it.
    TailGap(f32),
    /// Off — or Smart without usable commit evidence: no Whisper at all; the
    /// streaming splice is the answer.
    SkipStreaming,
}

impl ComposerFinalPassPlan {
    /// Stable log token for the chosen plan (the `TailGap` boundary is logged
    /// separately, so variants with payloads still map to one flat name).
    fn label(self) -> &'static str {
        match self {
            Self::FullFile => "full_file",
            Self::TailGap(_) => "tail_gap",
            Self::SkipStreaming => "skip_streaming",
        }
    }
}

/// Route the composer stop lane by mode — the same law the controller lane obeys.
///
/// Always is the ONLY mode permitted a full-file re-pass; Off runs zero Whisper
/// on the stop path; Smart delegates its boundary question to the shared core
/// guard so a missing boundary can never degrade into a whole-file pass landing
/// on committed text.
fn composer_final_pass_plan(
    mode: FinalPassRoutingMode,
    committed_through_secs: Option<f32>,
    streaming_is_empty: bool,
) -> ComposerFinalPassPlan {
    match mode {
        FinalPassRoutingMode::Always => ComposerFinalPassPlan::FullFile,
        FinalPassRoutingMode::Off => ComposerFinalPassPlan::SkipStreaming,
        FinalPassRoutingMode::Smart => {
            match resolve_tail_gap_boundary(committed_through_secs, streaming_is_empty) {
                TailGapBoundary::From(secs) => ComposerFinalPassPlan::TailGap(secs),
                TailGapBoundary::WholeSessionBootstrap => ComposerFinalPassPlan::TailGap(0.0),
                TailGapBoundary::Skip => ComposerFinalPassPlan::SkipStreaming,
            }
        }
    }
}

/// Run the planned final pass over the saved WAV.
///
/// `FullFile` mirrors the controller's toggle-stop adjudicator
/// (`transcribe_file_verdict` with default options); `TailGap` transcribes only
/// the uncommitted tail (append-only doctrine); `SkipStreaming` never touches
/// Whisper. Blocking work runs off the async runtime and is bounded by the
/// shared `deadline`; any failure/timeout/absent-WAV/empty text yields `None`
/// so the caller falls back to the streaming splice.
async fn run_final_pass(
    plan: ComposerFinalPassPlan,
    audio_path: Option<PathBuf>,
    language: Option<String>,
    deadline: tokio::time::Instant,
) -> Option<String> {
    if matches!(plan, ComposerFinalPassPlan::SkipStreaming) {
        return None;
    }
    let path = audio_path?;
    let job = tokio::task::spawn_blocking(move || match plan {
        ComposerFinalPassPlan::TailGap(from_secs) => {
            codescribe_core::stt::whisper_tail_gap_transcribe_file(
                &path,
                from_secs,
                language.as_deref(),
            )
            .map(|raw| raw.text)
        }
        // Always — the ONLY mode permitted a whole-file re-pass.
        ComposerFinalPassPlan::FullFile => whisper::transcribe_file_verdict(
            &path,
            language.as_deref(),
            FileTranscriptionOptions::default(),
        )
        .map(|verdict| verdict.text),
        // Returned above; kept explicit so a FUTURE plan variant is a compile
        // error here instead of silently routing into the full-file re-pass.
        ComposerFinalPassPlan::SkipStreaming => Ok(String::new()),
    });
    match tokio::time::timeout_at(deadline, job).await {
        Ok(Ok(Ok(text))) if !text.trim().is_empty() => Some(text),
        Ok(Ok(Ok(_))) => None,
        Ok(Ok(Err(e))) => {
            warn!(target: "composer-dictation", error = %e, "final pass transcription failed");
            None
        }
        Ok(Err(e)) => {
            warn!(target: "composer-dictation", error = %e, "final pass task join failed");
            None
        }
        Err(_elapsed) => {
            warn!(target: "composer-dictation", "final pass timed out; using streaming fallback");
            None
        }
    }
}

/// Internal `EventSink` adapter (NOT exposed across FFI). Lives between the
/// core streaming pipeline and the foreign `CsTranscriptionListener`,
/// translating every `EngineEvent` variant into the appropriate listener call.
struct CsEventSink {
    listener: Arc<dyn CsTranscriptionListener>,
    /// Composer-side accumulator: `stop_recording` reads its snapshot for the
    /// return value (the Swift `on_final` callback is a no-op on this path).
    transcript: Arc<ComposerTranscript>,
}

impl EventSink for CsEventSink {
    /// Translate one core `EngineEvent` into the foreign listener contract and
    /// accumulate finals for the composer return path.
    fn on_event(&self, event: &EngineEvent) {
        match event {
            EngineEvent::VadStart { .. } => self.listener.on_vad_active(true),
            EngineEvent::VadEnd { .. } => self.listener.on_vad_active(false),
            EngineEvent::NoSpeech { reason } => self.listener.on_no_speech(reason.clone()),
            EngineEvent::Preview { text, .. } => self.listener.on_preview(text.clone()),
            EngineEvent::Correction {
                text,
                previous_text,
                ..
            } => self
                .listener
                .on_correction(text.clone(), previous_text.clone()),
            EngineEvent::UtteranceFinal {
                utterance_id,
                text,
                end_ts,
                avg_logprob,
                vad_speech_pct,
                confidence_flags,
                ..
            } => {
                // Compose the composer return here: the streaming recorder's own
                // transcript buffer is never filled on this path.
                self.transcript.append_final(text);
                self.transcript.note_committed_through(*end_ts);
                let flags: Vec<String> = confidence_flags.iter().map(ToString::to_string).collect();
                self.listener.on_final(
                    *utterance_id,
                    text.clone(),
                    *avg_logprob,
                    *vad_speech_pct,
                    flags,
                );
            }
            EngineEvent::ReplaceRange {
                utterance_id,
                start,
                end,
                text,
                source,
            } => self.listener.on_replace_range(
                *utterance_id,
                *start as u64,
                *end as u64,
                text.clone(),
                (*source).into(),
            ),
            EngineEvent::InsertAnnotation {
                utterance_id,
                position,
                text,
                kind,
            } => self.listener.on_insert_annotation(
                *utterance_id,
                *position as u64,
                text.clone(),
                kind.into(),
            ),
            EngineEvent::SessionFinalised {
                session_id,
                layer_summary,
            } => self
                .listener
                .on_session_finalised(session_id.clone(), layer_summary.into()),
            // Recoverable engine warning — surface as a non-fatal error string.
            // Deliberately does NOT touch the tray: `TrayStatus::Error` means
            // "backend not available", and a warning about degraded transcript
            // quality leaves the backend fully alive. Painting the tray red here
            // told the operator the engine had died while it was still running.
            EngineEvent::Warning { code, message } => {
                self.listener.on_error(format!("{code}: {message}"))
            }
            // Engine-internal bookkeeping (dropped content, session stats) has no
            // listener surface; intentionally ignored.
            EngineEvent::Drop { .. } | EngineEvent::Stats { .. } => {}
        }
    }
}

/// Resolve the Whisper language hint for a manual voice-note session.
///
/// An explicit caller choice wins; `None` falls back to the persisted
/// `WHISPER_LANGUAGE` setting (mirroring the hotkey path in
/// `RecordingController`) rather than forcing blind auto-detect — the latter
/// mis-guessed `en`/`ru` on short manual notes. `Auto` collapses to `None`
/// (genuine auto-detect) via `whisper_hint`, never the literal `"auto"` code.
/// Uses `load_without_keychain` so opening the composer mic never triggers a
/// Keychain prompt.
fn resolve_language_hint(language: Option<CsLanguage>) -> Option<String> {
    match language {
        Some(lang) => codescribe_core::config::Language::from(lang).whisper_hint(),
        None => codescribe_core::config::Config::load_without_keychain()
            .whisper_language
            .whisper_hint(),
    }
    .map(str::to_string)
}

/// One live composer voice-note session: the streaming recorder plus the
/// finalized-text accumulator its event sink feeds, the wall-clock start used to
/// size the stop timeout, and the resolved Whisper language hint reused for the
/// stop-time final pass (kept so it honours the persisted setting exactly like
/// the start-time streaming session).
struct ActiveSession {
    recorder: StreamingRecorder,
    transcript: Arc<ComposerTranscript>,
    started_at: Instant,
    language_hint: Option<String>,
}

/// Thin handle to the codescribe dictation engine (streaming recorder +
/// Whisper). Holds the active session behind an async mutex and the current
/// foreign listener behind an `RwLock`.
#[derive(uniffi::Object)]
pub struct CodescribeDictation {
    recorder: Mutex<Option<ActiveSession>>,
    listener: RwLock<Option<Arc<dyn CsTranscriptionListener>>>,
}

#[uniffi::export(async_runtime = "tokio")]
impl CodescribeDictation {
    /// Build an idle dictation handle and initialize logging. No microphone or
    /// model work happens here — call `set_listener` then `start_recording`.
    #[uniffi::constructor]
    pub fn new() -> Self {
        codescribe::logging::init_logging();
        Self {
            recorder: Mutex::new(None),
            listener: RwLock::new(None),
        }
    }

    /// Register (or replace) the foreign listener that receives dictation
    /// events. Must be called before `start_recording`.
    pub fn set_listener(&self, listener: Arc<dyn CsTranscriptionListener>) {
        if let Ok(mut guard) = self.listener.write() {
            *guard = Some(listener);
        }
    }

    /// Optionally warm Whisper weights. Runs on a blocking thread because model
    /// load touches the GPU and can take seconds.
    ///
    /// When the live engine is Apple, Whisper is **gap-fill only** (file final /
    /// emergency recovery). Missing weights must never refuse recording start —
    /// we log an honest degraded-mode note and return `Ok(())`. Candle-live
    /// still requires a model and surfaces load errors.
    /// Wraps `whisper::init` (stt/whisper/singleton.rs).
    pub async fn init_model(&self) -> Result<(), CsError> {
        let apple_live = codescribe::stt::active_engine_is_apple();
        let result = tokio::task::spawn_blocking(whisper::init)
            .await
            .map_err(|e| CsError::Recording {
                msg: format!("init_model task join error: {e}"),
            })?;
        match result {
            Ok(()) => Ok(()),
            Err(e) if apple_live => {
                tracing::warn!("no Whisper gap fill this session (Apple live continues): {e:#}");
                Ok(())
            }
            Err(e) => Err(CsError::Recording { msg: e.to_string() }),
        }
    }

    /// True when the Whisper engine is currently loaded. May flip back to
    /// `false` after idle-unload; the next transcription reloads transparently.
    /// Wraps `whisper::is_initialized` (stt/whisper/singleton.rs:207).
    pub fn is_model_loaded(&self) -> bool {
        whisper::is_initialized()
    }

    /// Whether the default Whisper weights are on disk / embedded (not necessarily loaded).
    pub fn whisper_model_ready_status(&self) -> CsWhisperModelStatus {
        CsWhisperModelStatus::from(codescribe_core::config::models::whisper_model_status())
    }

    /// Start microphone dictation. Builds a `CsEventSink` from the registered
    /// listener, wires it into a fresh `StreamingRecorder`, and starts the
    /// event-based transcription session.
    ///
    /// Wraps `StreamingRecorder::new` (audio/streaming_recorder.rs:25),
    /// `set_event_sink` (:74) and `start_event_session` (:87). Errors if no
    /// listener was set (the core pipeline requires an event sink).
    pub async fn start_recording(&self, language: Option<CsLanguage>) -> Result<(), CsError> {
        let listener = self
            .listener
            .read()
            .map_err(|_| CsError::Recording {
                msg: "listener lock poisoned".to_string(),
            })?
            .clone()
            .ok_or_else(|| CsError::Recording {
                msg: "set_listener(...) must be called before start_recording".to_string(),
            })?;

        let transcript = Arc::new(ComposerTranscript::default());
        let sink: Arc<dyn EventSink> = Arc::new(CsEventSink {
            listener: Arc::clone(&listener),
            transcript: Arc::clone(&transcript),
        });
        let mut recorder =
            StreamingRecorder::new().map_err(|e| CsError::Recording { msg: e.to_string() })?;
        recorder.set_event_sink(Some(sink));
        recorder.configure_layer1(
            &UserSettings::load(),
            GatewaySessionAvailability::Unavailable,
        );

        // Manual voice-note: the composer's Stop click is the source of truth,
        // exactly like the hotkey hold's key-up (see `RecordingController`
        // hold-start, which also sets `auto_silence = false`). The legacy
        // `RecorderConfig` defaults to `auto_silence = true`, which auto-stops the
        // stream after ~0.3s of silence and chops a single spoken note into
        // fragments the commit-VAD then rejects as "no speech". Disable it so the
        // user — not the VAD — ends the recording.
        recorder.recorder.config.auto_silence = false;

        let language_code = resolve_language_hint(language);
        recorder
            .start_event_session(language_code.clone())
            .await
            .map_err(|e| CsError::Recording { msg: e.to_string() })?;

        *self.recorder.lock().await = Some(ActiveSession {
            recorder,
            transcript,
            started_at: Instant::now(),
            language_hint: language_code,
        });
        tray_status::update_tray_status(TrayStatus::Listening);
        listener.on_recording_started();
        Ok(())
    }

    /// Stop the active dictation session and return the composed transcript.
    ///
    /// Two-phase, within one shared budget (`compose_stop_timeout`):
    ///
    /// 1. `StreamingRecorder::stop` is the completion signal — it stops the
    ///    audio stream, joins the transcription task (which only finishes AFTER
    ///    every `UtteranceFinal` has been emitted synchronously into our
    ///    accumulator), and saves the WAV. So the streaming splice is complete
    ///    once stop returns cleanly.
    /// 2. The final pass `FINAL_PASS_MODE` permits (`composer_final_pass_plan`,
    ///    same law as the controller lane): **Always** re-transcribes the whole
    ///    saved WAV with the `transcribe_file_verdict` adjudicator the
    ///    hotkey/overlay toggle-stop uses; **Smart** transcribes only the audio
    ///    after the last committed utterance and merges it as gap-fill;
    ///    **Off** runs no Whisper at all and streaming is final.
    ///
    /// The final pass wins whenever it yields non-empty text; the streaming
    /// splice is the fallback for a failed/timed-out/empty final pass (or a
    /// drain timeout, where no WAV is composed). Either way the UI never hangs:
    /// the shared budget bounds both phases and overrun degrades quality, not
    /// correctness. The streaming recorder's own transcript buffer is ignored —
    /// it stays empty on this path.
    pub async fn stop_recording(&self) -> Result<String, CsError> {
        let mut session = {
            let mut guard = self.recorder.lock().await;
            guard.take().ok_or_else(|| CsError::Recording {
                msg: "no active recording to stop".to_string(),
            })?
        };

        let budget = compose_stop_timeout(session.started_at.elapsed());
        let deadline = tokio::time::Instant::now() + budget;
        let transcript = Arc::clone(&session.transcript);
        let language_hint = session.language_hint.clone();
        self.notify_recording_finalising();

        // Phase 1: drain the streaming session and recover the saved WAV path.
        let audio_path = match tokio::time::timeout_at(deadline, session.recorder.stop()).await {
            Ok(Ok((_streaming_buf, audio_path))) => audio_path,
            Ok(Err(e)) => {
                tray_status::update_tray_status(TrayStatus::Error);
                return Err(CsError::Recording { msg: e.to_string() });
            }
            Err(_elapsed) => {
                // Drain overran the budget — no WAV to adjudicate; return the
                // streaming finals accumulated so far.
                let (streaming_text, utterances) = transcript.snapshot();
                let text = streaming_text.trim().to_string();
                warn!(
                    target: "composer-dictation",
                    source = ComposerTranscriptSource::StreamingFallback.label(),
                    utterances,
                    streaming_chars = text.chars().count(),
                    budget_ms = budget.as_millis() as u64,
                    "composer voice-note stop drain timed out; returning streaming fallback"
                );
                self.notify_recording_stopped();
                return Ok(text);
            }
        };

        // Phase 2: the final pass `FINAL_PASS_MODE` permits — full file under
        // Always, uncommitted-tail gap-fill under Smart, nothing under Off. The
        // streaming splice remains the fallback authority in every mode.
        let (streaming_text, _utterances) = transcript.snapshot();
        let mode = codescribe_core::config::final_pass_routing_mode();
        let plan = composer_final_pass_plan(
            mode,
            transcript.committed_through_secs(),
            streaming_text.trim().is_empty(),
        );
        info!(
            target: "composer-dictation",
            mode = mode.as_str(),
            plan = plan.label(),
            committed_through_secs = transcript.committed_through_secs(),
            "composer voice-note stop final-pass plan"
        );
        let final_pass_text = run_final_pass(plan, audio_path, language_hint, deadline).await;

        let final_pass_chars = final_pass_text
            .as_deref()
            .map(|t| t.trim().chars().count())
            .unwrap_or(0);
        let (text, source) =
            compose_composer_transcript(plan, final_pass_text.as_deref(), &streaming_text);

        info!(
            target: "composer-dictation",
            source = source.label(),
            plan = plan.label(),
            final_pass_chars,
            streaming_chars = streaming_text.trim().chars().count(),
            "composer voice-note stop composed transcript"
        );

        self.notify_recording_stopped();
        Ok(text)
    }

    /// Fire the foreign `on_recording_stopped` callback if a listener is set.
    fn notify_recording_stopped(&self) {
        tray_status::update_tray_status(TrayStatus::Idle);
        if let Ok(guard) = self.listener.read()
            && let Some(listener) = guard.as_ref()
        {
            listener.on_recording_stopped();
        }
    }

    /// Fire the foreign `on_recording_finalising` callback and publish processing.
    fn notify_recording_finalising(&self) {
        tray_status::update_tray_status(TrayStatus::Thinking);
        if let Ok(guard) = self.listener.read()
            && let Some(listener) = guard.as_ref()
        {
            listener.on_recording_finalising();
        }
    }

    /// True while a dictation session is active.
    /// Wraps `StreamingRecorder::is_recording` (audio/streaming_recorder.rs:79).
    pub async fn is_recording(&self) -> bool {
        self.recorder
            .lock()
            .await
            .as_ref()
            .map(|session| session.recorder.is_recording())
            .unwrap_or(false)
    }

    /// Transcribe an existing audio file. Loads + decodes the file, detects the
    /// language, then runs Whisper. All blocking work runs off the async runtime.
    ///
    /// Wraps `audio::load_audio_file` (audio/loader.rs:10),
    /// `whisper::detect_language` (stt/whisper/singleton.rs:249) and
    /// `whisper::transcribe` (stt/whisper/singleton.rs:214).
    pub async fn transcribe_file(&self, path: String) -> Result<CsTranscription, CsError> {
        tokio::task::spawn_blocking(move || -> Result<CsTranscription, CsError> {
            let path = std::path::PathBuf::from(path);
            let (samples, sample_rate) =
                load_audio_file(&path).map_err(|e| CsError::Recording { msg: e.to_string() })?;
            let language = whisper::detect_language(&samples, sample_rate)
                .map_err(|e| CsError::Recording { msg: e.to_string() })?;
            let text = whisper::transcribe(&samples, sample_rate, Some(language.as_str()))
                .map_err(|e| CsError::Recording { msg: e.to_string() })?;
            Ok(CsTranscription { text, language })
        })
        .await
        .map_err(|e| CsError::Recording {
            msg: format!("transcribe_file task join error: {e}"),
        })?
    }
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

/// Dictation-bridge unit coverage: audio-input resolution, event-sink identity
/// flow, composer commit boundaries, and final-pass plan mode truth.
#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex as StdMutex;

    /// Configured match, unavailable fallback, and default-only paths stay honest.
    #[test]
    fn audio_input_resolution_reports_live_match_and_unavailable_fallback() {
        let devices = vec![
            "MacBook Pro Microphone".to_string(),
            "USB Studio Mic".to_string(),
        ];

        assert_eq!(
            resolve_audio_input_state(Some("Studio Mic"), &devices, Some("MacBook Pro Microphone"),),
            (Some("USB Studio Mic".to_string()), true, false)
        );
        assert_eq!(
            resolve_audio_input_state(
                Some("Unplugged Mic"),
                &devices,
                Some("MacBook Pro Microphone"),
            ),
            (Some("MacBook Pro Microphone".to_string()), false, true)
        );
        assert_eq!(
            resolve_audio_input_state(None, &devices, Some("MacBook Pro Microphone")),
            (Some("MacBook Pro Microphone".to_string()), true, false)
        );
        assert!(device_is_available(Some("Studio Mic"), &devices));
        assert!(!device_is_available(Some("Unplugged Mic"), &devices));
    }

    /// Captures the payload of the single listener call we assert on.
    #[derive(Default)]
    struct CapturingListener {
        final_calls: StdMutex<Vec<(u64, String)>>,
    }

    impl CsTranscriptionListener for CapturingListener {
        /// Lifecycle prepare — unused by sink identity tests.
        fn on_recording_preparing(&self) {}
        /// Lifecycle start — unused by sink identity tests.
        fn on_recording_started(&self) {}
        /// Lifecycle stop — unused by sink identity tests.
        fn on_recording_stopped(&self) {}
        /// Lifecycle finalising — unused by sink identity tests.
        fn on_recording_finalising(&self) {}
        /// Interim preview text — unused by sink identity tests.
        fn on_preview(&self, _text: String) {}
        /// Live correction text — unused by sink identity tests.
        fn on_correction(&self, _text: String, _previous_text: String) {}
        /// Record each final so tests can assert utterance_id + text together.
        fn on_final(
            &self,
            utterance_id: u64,
            text: String,
            _avg_logprob: Option<f32>,
            _speech_pct: Option<f32>,
            _confidence_flags: Vec<String>,
        ) {
            self.final_calls.lock().unwrap().push((utterance_id, text));
        }
        /// Bounded replace events — unused by the capture fixture.
        fn on_replace_range(
            &self,
            _utterance_id: u64,
            _start: u64,
            _end: u64,
            _text: String,
            _source: CsLayerSource,
        ) {
        }
        /// Inline annotations — unused by the capture fixture.
        fn on_insert_annotation(
            &self,
            _utterance_id: u64,
            _position: u64,
            _text: String,
            _kind: CsAnnotationKind,
        ) {
        }
        /// Context markers — unused by the capture fixture.
        fn on_context_marker(&self, _position: u64, _marker: String) {}
        /// Session-end summary — unused by the capture fixture.
        fn on_session_finalised(&self, _session_id: String, _layer_summary: CsLayerSummary) {}
        /// Delivery-grade final transcript — unused by the capture fixture.
        fn on_final_transcript_ready(&self, _text: String) {}
        /// VAD active flips — unused by the capture fixture.
        fn on_vad_active(&self, _active: bool) {}
        /// RMS level ticks — unused by the capture fixture.
        fn on_audio_level(&self, _rms: f32) {}
        /// No-speech notices — unused by the capture fixture.
        fn on_no_speech(&self, _reason: String) {}
        /// Recoverable engine errors — unused by the capture fixture.
        fn on_error(&self, _message: String) {}
    }

    /// Build a minimal `UtteranceFinal` event with the given identity/text.
    fn utterance_final(utterance_id: u64, text: &str) -> EngineEvent {
        EngineEvent::UtteranceFinal {
            utterance_id,
            text: text.to_string(),
            raw_text: text.to_string(),
            start_ts: 0.0,
            end_ts: 1.0,
            segments: Vec::new(),
            vad_speech_pct: None,
            avg_logprob: None,
            compression_ratio: None,
            quality_gate_dropped: false,
            confidence_flags: Vec::new(),
        }
    }

    /// The bridge must forward `utterance_id` on `UtteranceFinal` so committed
    /// sinks can stamp segment identity that later `ReplaceRange` patches target.
    /// Regression guard for the W3 keystone (identity flow into committed text).
    #[test]
    fn utterance_final_forwards_utterance_id() {
        let listener = Arc::new(CapturingListener::default());
        let sink = CsEventSink {
            listener: listener.clone(),
            transcript: Arc::new(ComposerTranscript::default()),
        };

        sink.on_event(&utterance_final(7, "ala ma kota"));

        let calls = listener.final_calls.lock().unwrap();
        assert_eq!(
            calls.as_slice(),
            &[(7, "ala ma kota".to_string())],
            "on_final must receive the utterance_id from UtteranceFinal"
        );
    }

    /// The composer return is composed from the finalized utterance stream: the
    /// sink must accumulate each `UtteranceFinal` (space-joined, empties skipped)
    /// so `stop_recording` never returns an empty transcript after real speech.
    /// Regression guard for the "audio + STT work but final is empty" bug.
    #[test]
    fn cs_event_sink_accumulates_final_transcript() {
        let listener = Arc::new(CapturingListener::default());
        let transcript = Arc::new(ComposerTranscript::default());
        let sink = CsEventSink {
            listener: listener.clone(),
            transcript: Arc::clone(&transcript),
        };

        sink.on_event(&utterance_final(1, "  no to  "));
        sink.on_event(&utterance_final(2, "")); // empty final must not widen text
        sink.on_event(&utterance_final(3, "dobra teraz"));

        let (text, utterances) = transcript.snapshot();
        assert_eq!(text, "no to dobra teraz");
        assert_eq!(
            utterances, 2,
            "empty final must not count toward utterances"
        );
    }

    /// Same as [`utterance_final`] but with an explicit commit boundary.
    fn utterance_final_at(utterance_id: u64, text: &str, end_ts: f32) -> EngineEvent {
        match utterance_final(utterance_id, text) {
            EngineEvent::UtteranceFinal {
                utterance_id,
                text,
                raw_text,
                start_ts,
                segments,
                vad_speech_pct,
                avg_logprob,
                compression_ratio,
                quality_gate_dropped,
                confidence_flags,
                ..
            } => EngineEvent::UtteranceFinal {
                utterance_id,
                text,
                raw_text,
                start_ts,
                end_ts,
                segments,
                vad_speech_pct,
                avg_logprob,
                compression_ratio,
                quality_gate_dropped,
                confidence_flags,
            },
            other => other,
        }
    }

    /// Smart mode needs the composer lane's committed audio boundary, exactly as
    /// the controller's `SessionTelemetrySink` tracks it: a monotonic max fold of
    /// `UtteranceFinal::end_ts` that an out-of-order final can never rewind, and
    /// that an empty final still advances (the audio IS adjudicated, it simply
    /// carried no text — so it must not be gap-filled again).
    #[test]
    fn composer_transcript_tracks_committed_through_secs() {
        let transcript = ComposerTranscript::default();
        assert_eq!(
            transcript.committed_through_secs(),
            None,
            "no finals yet ⇒ no commit evidence"
        );

        let listener = Arc::new(CapturingListener::default());
        let sink = CsEventSink {
            listener,
            transcript: Arc::new(ComposerTranscript::default()),
        };
        sink.on_event(&utterance_final_at(1, "raz", 2.5));
        sink.on_event(&utterance_final_at(2, "dwa", 7.25));
        // Out-of-order final: the boundary must not rewind.
        sink.on_event(&utterance_final_at(3, "trzy", 4.0));
        // Empty final still seals its audio.
        sink.on_event(&utterance_final_at(4, "   ", 9.5));

        assert_eq!(sink.transcript.committed_through_secs(), Some(9.5));
        assert_eq!(
            sink.transcript.snapshot().0,
            "raz dwa trzy",
            "boundary tracking must not disturb the text accumulator"
        );
    }

    /// The composer stop lane must obey `FINAL_PASS_MODE` exactly like the
    /// controller lane: Always is the ONLY mode allowed a full-file re-pass,
    /// Smart may only gap-fill the uncommitted tail, Off runs no Whisper at all.
    #[test]
    fn composer_final_pass_plan_honours_mode() {
        // Always: full file, regardless of commit evidence or canvas state.
        for (committed, empty) in [(None, false), (Some(4.0), false), (None, true)] {
            assert_eq!(
                composer_final_pass_plan(FinalPassRoutingMode::Always, committed, empty),
                ComposerFinalPassPlan::FullFile,
                "Always must full-file re-pass (committed={committed:?}, empty={empty})"
            );
        }

        // Off: zero Whisper on the stop path, streaming is final.
        for (committed, empty) in [(None, false), (Some(4.0), false), (None, true)] {
            assert_eq!(
                composer_final_pass_plan(FinalPassRoutingMode::Off, committed, empty),
                ComposerFinalPassPlan::SkipStreaming,
                "Off must never invoke Whisper (committed={committed:?}, empty={empty})"
            );
        }

        // Smart with a committed boundary: gap-fill the tail only.
        assert_eq!(
            composer_final_pass_plan(FinalPassRoutingMode::Smart, Some(6.5), false),
            ComposerFinalPassPlan::TailGap(6.5)
        );
        // Smart, no commit evidence, empty canvas: whole session is still an append.
        assert_eq!(
            composer_final_pass_plan(FinalPassRoutingMode::Smart, None, true),
            ComposerFinalPassPlan::TailGap(0.0)
        );
        // Smart, no commit evidence, non-empty canvas: a whole-file pass would land
        // on committed text — honest skip instead.
        assert_eq!(
            composer_final_pass_plan(FinalPassRoutingMode::Smart, None, false),
            ComposerFinalPassPlan::SkipStreaming
        );
        // Non-finite / non-positive boundaries carry no evidence either.
        assert_eq!(
            composer_final_pass_plan(FinalPassRoutingMode::Smart, Some(f32::NAN), false),
            ComposerFinalPassPlan::SkipStreaming
        );
        assert_eq!(
            composer_final_pass_plan(FinalPassRoutingMode::Smart, Some(0.0), false),
            ComposerFinalPassPlan::SkipStreaming
        );
    }

    /// The stop-drain budget scales with recording length but is clamped so the
    /// composer UI can never hang indefinitely on a stalled scheduler.
    #[test]
    fn compose_stop_timeout_scales_and_clamps() {
        /// Allow one-micro drift from floating proportional clamp arithmetic.
        fn assert_duration_close(actual: Duration, expected: Duration) {
            let drift = actual.abs_diff(expected);
            assert!(
                drift <= Duration::from_micros(1),
                "duration drift {drift:?} exceeded tolerance: actual={actual:?}, expected={expected:?}"
            );
        }

        // Short note: floored so a cold commit + tail patch still fits.
        assert_eq!(
            compose_stop_timeout(Duration::from_secs(3)),
            Duration::from_secs(8)
        );
        // Mid-length: proportional (20s * 0.6 = 12s) inside the band.
        assert_duration_close(
            compose_stop_timeout(Duration::from_secs(20)),
            Duration::from_secs(12),
        );
        // Long note: capped so the UI never waits unboundedly.
        assert_eq!(
            compose_stop_timeout(Duration::from_secs(300)),
            Duration::from_secs(30)
        );
    }

    /// Whisper excess fills live gaps (InsertB), never replaces the live floor.
    #[test]
    fn select_composer_transcript_merges_whisper_gap_fill() {
        let (text, source) = select_composer_transcript(Some("  raz dwa trzy cztery  "), "raz dwa");
        assert_eq!(text, "raz dwa trzy cztery");
        assert_eq!(source, ComposerTranscriptSource::MergedLiveWhisper);
    }

    /// Substitution disagreements keep live (doctrine: floor of truth); the
    /// Whisper variant is lexicon/human territory, not a silent overwrite.
    #[test]
    fn select_composer_transcript_keeps_live_on_substitution() {
        let (text, source) = select_composer_transcript(Some("raz dwa trzy"), "raz dwa tszy");
        assert_eq!(text, "raz dwa tszy");
        assert_eq!(source, ComposerTranscriptSource::MergedLiveWhisper);
    }

    /// Collapsing file-final (Apple SFSpeech short) must not blank or shrink a
    /// real stream: merge keeps every live token, so the floor survives.
    #[test]
    fn select_composer_transcript_collapsing_final_keeps_live_floor() {
        let stream = "Im wystarczy i jeszcze sporo z freezed live assembly utterance dwa";
        let (text, source) = select_composer_transcript(Some("Im wystarczy"), stream);
        assert_eq!(text, stream.trim());
        assert_eq!(source, ComposerTranscriptSource::MergedLiveWhisper);
    }

    /// With no live stream at all, the whisper final stands alone.
    #[test]
    fn select_composer_transcript_whisper_only_when_stream_empty() {
        let (text, source) = select_composer_transcript(Some("raz dwa"), "   ");
        assert_eq!(text, "raz dwa");
        assert_eq!(source, ComposerTranscriptSource::MergedLiveWhisper);
    }

    /// An absent or empty/whitespace final pass falls back to the streaming
    /// splice so a failed adjudication never blanks a real transcript.
    #[test]
    fn select_composer_transcript_falls_back_to_streaming() {
        let (none_text, none_source) = select_composer_transcript(None, "  raz dwa  ");
        assert_eq!(none_text, "raz dwa");
        assert_eq!(none_source, ComposerTranscriptSource::StreamingFallback);

        let (empty_text, empty_source) = select_composer_transcript(Some("   \n "), "raz dwa");
        assert_eq!(empty_text, "raz dwa");
        assert_eq!(empty_source, ComposerTranscriptSource::StreamingFallback);
    }

    /// THE ONE RULE for the Smart lane: a `TailGap` result is a **bare tail**,
    /// not a full-file transcript. It must be APPENDED to the immutable
    /// committed/live streaming text — never diffed against it. `merge_live_whisper`
    /// is built for full transcripts vs the live floor: on a bare tail it turns the
    /// boundary DeleteA+InsertB pair into a Substitute that keeps live and DISCARDS
    /// the whisper token, silently losing gap-fill words.
    #[test]
    fn compose_composer_transcript_appends_tail_gap() {
        let (text, source) = compose_composer_transcript(
            ComposerFinalPassPlan::TailGap(1.5),
            Some("trzy cztery"),
            "raz dwa",
        );
        assert_eq!(
            text, "raz dwa trzy cztery",
            "tail gap-fill must be appended verbatim, not merged"
        );
        assert_eq!(source, ComposerTranscriptSource::TailGapAppend);

        // Function words are the first casualty of the merge path.
        let (clinical, _) = compose_composer_transcript(
            ComposerFinalPassPlan::TailGap(2.0),
            Some("i wymioty od rana"),
            "Pacjent ma goraczke",
        );
        assert_eq!(clinical, "Pacjent ma goraczke i wymioty od rana");

        // Overlapping preview words are deduped word-granularly, committed side untouched.
        let (deduped, _) = compose_composer_transcript(
            ComposerFinalPassPlan::TailGap(2.0),
            Some("goraczke i wymioty od rana"),
            "Pacjent ma goraczke i",
        );
        assert_eq!(deduped, "Pacjent ma goraczke i wymioty od rana");
    }

    /// `FullFile` (Always) keeps the whole-WAV merge; `SkipStreaming` never has a
    /// final pass to compose with.
    #[test]
    fn compose_composer_transcript_keeps_merge_for_full_file() {
        let (text, source) = compose_composer_transcript(
            ComposerFinalPassPlan::FullFile,
            Some("raz dwa trzy cztery"),
            "raz dwa",
        );
        assert_eq!(text, "raz dwa trzy cztery");
        assert_eq!(source, ComposerTranscriptSource::MergedLiveWhisper);

        let (skipped, skipped_source) =
            compose_composer_transcript(ComposerFinalPassPlan::SkipStreaming, None, "  raz dwa  ");
        assert_eq!(skipped, "raz dwa");
        assert_eq!(skipped_source, ComposerTranscriptSource::StreamingFallback);
    }

    /// An empty / absent tail leaves the streaming splice exactly as it stands.
    #[test]
    fn compose_composer_transcript_tail_gap_empty_falls_back() {
        for tail in [None, Some("   \n ")] {
            let (text, source) = compose_composer_transcript(
                ComposerFinalPassPlan::TailGap(1.0),
                tail,
                "  raz dwa  ",
            );
            assert_eq!(
                text, "raz dwa",
                "empty tail {tail:?} must not disturb streaming"
            );
            assert_eq!(source, ComposerTranscriptSource::StreamingFallback);
        }
    }

    /// An explicit caller language must map to its two-letter Whisper hint, and
    /// `Auto` must collapse to genuine auto-detect (`None`) — never the literal
    /// `"auto"` code, which Whisper cannot honour. Guards the manual voice-note
    /// language path so the composer respects the persisted language like the
    /// hotkey path instead of blind-guessing `en`/`ru`.
    #[test]
    fn resolve_language_hint_maps_explicit_choices() {
        assert_eq!(
            resolve_language_hint(Some(CsLanguage::Polish)),
            Some("pl".to_string())
        );
        assert_eq!(
            resolve_language_hint(Some(CsLanguage::English)),
            Some("en".to_string())
        );
        assert_eq!(
            resolve_language_hint(Some(CsLanguage::Auto)),
            None,
            "Auto must be genuine auto-detect (None), never the literal \"auto\" code"
        );
    }
}
