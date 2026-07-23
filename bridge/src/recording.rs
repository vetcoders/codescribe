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
use codescribe_core::audio::load_audio_file;
use codescribe_core::audio::streaming_recorder::StreamingRecorder;
use codescribe_core::pipeline::contracts::{
    AnnotationKind, EngineEvent, EventSink, FileTranscriptionOptions, LayerSource, LayerSummary,
};
use codescribe_core::stt::whisper;
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

fn normalized_device_name(device: Option<&str>) -> Option<String> {
    device
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(str::to_owned)
}

fn device_is_available(configured_device: Option<&str>, devices: &[String]) -> bool {
    let Some(configured_device) = normalized_device_name(configured_device) else {
        return true;
    };
    let configured_lower = configured_device.to_lowercase();
    devices.iter().any(|device| {
        *device == configured_device || device.to_lowercase().contains(&configured_lower)
    })
}

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
    fn on_recording_preparing(&self);
    fn on_recording_started(&self);
    fn on_recording_stopped(&self);
    /// Capture ended and the controller entered `Busy` (final transcription pass).
    /// Fired BEFORE `on_recording_stopped` (which lands on the terminal Idle) so a
    /// hotkey hold-release / toggle stop can show a distinct "transcribing" phase
    /// instead of leaving the live-capture UI up while the final pass runs. The
    /// Swift-driven Finish path enters that phase itself; this is the native-path
    /// counterpart. Surfaces with no post-capture phase may leave it a no-op.
    fn on_recording_finalising(&self);
    fn on_preview(&self, text: String);
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
    fn on_replace_range(
        &self,
        utterance_id: u64,
        start: u64,
        end: u64,
        text: String,
        source: CsLayerSource,
    );
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
    fn on_session_finalised(&self, session_id: String, layer_summary: CsLayerSummary);
    /// Authoritative post-stop transcript (LocalFinalPass `final_formatted_text`):
    /// the SAME clean text that is pasted/delivered and written to history. Surfaces
    /// fire it once per dictation stop so the overlay FINAL matches delivery/Copy.
    fn on_final_transcript_ready(&self, text: String);
    fn on_vad_active(&self, active: bool);
    /// Live microphone input level: RMS of one captured audio block (linear,
    /// 0..~1). Fires continuously (~40–50 Hz) while a controller dictation
    /// session records, so the overlay waveform can track the real voice.
    /// Surfaces without a level meter may leave it a no-op.
    fn on_audio_level(&self, rms: f32);
    fn on_no_speech(&self, reason: String);
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
}

impl ComposerTranscript {
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
    const FLOOR: Duration = Duration::from_secs(8);
    const CAP: Duration = Duration::from_secs(30);
    elapsed.mul_f32(0.6).clamp(FLOOR, CAP)
}

/// Which transcript `stop_recording` returned, for the stop breadcrumb.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ComposerTranscriptSource {
    /// Delivery-grade whole-WAV final pass (matches the hotkey/overlay quality).
    FinalPass,
    /// Spliced streaming `UtteranceFinal` chunks (final pass unavailable/empty).
    StreamingFallback,
}

impl ComposerTranscriptSource {
    fn label(self) -> &'static str {
        match self {
            Self::FinalPass => "final_pass",
            Self::StreamingFallback => "streaming_fallback",
        }
    }
}

/// Pick the composer return: the whole-WAV final pass wins whenever it produced
/// non-empty text (it decodes the recording as one continuous utterance, so it
/// avoids the mid-word cut artifacts of the streaming splice); otherwise the
/// streaming accumulation is the fallback authority. Both inputs are trimmed.
fn select_composer_transcript(
    final_pass: Option<&str>,
    streaming: &str,
) -> (String, ComposerTranscriptSource) {
    if let Some(text) = final_pass {
        let trimmed = text.trim();
        if !trimmed.is_empty() {
            return (trimmed.to_string(), ComposerTranscriptSource::FinalPass);
        }
    }
    (
        streaming.trim().to_string(),
        ComposerTranscriptSource::StreamingFallback,
    )
}

/// Run the delivery-grade final pass over the saved WAV, mirroring the
/// controller's toggle-stop adjudicator (`transcribe_file_verdict` with default
/// options). Blocking Whisper work runs off the async runtime and is bounded by
/// the shared `deadline`; any failure/timeout/absent-WAV yields `None` so the
/// caller falls back to the streaming splice.
async fn run_final_pass(
    audio_path: Option<PathBuf>,
    language: Option<String>,
    deadline: tokio::time::Instant,
) -> Option<String> {
    let path = audio_path?;
    let job = tokio::task::spawn_blocking(move || {
        whisper::transcribe_file_verdict(
            &path,
            language.as_deref(),
            FileTranscriptionOptions::default(),
        )
    });
    match tokio::time::timeout_at(deadline, job).await {
        Ok(Ok(Ok(verdict))) => Some(verdict.text),
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
                avg_logprob,
                vad_speech_pct,
                confidence_flags,
                ..
            } => {
                // Compose the composer return here: the streaming recorder's own
                // transcript buffer is never filled on this path.
                self.transcript.append_final(text);
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
            EngineEvent::Warning { code, message } => {
                tray_status::update_tray_status(TrayStatus::Error);
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

    /// Load the Whisper engine (idempotent). Runs on a blocking thread because
    /// model load touches the GPU and can take seconds.
    /// Wraps `whisper::init` (stt/whisper/singleton.rs:199).
    pub async fn init_model(&self) -> Result<(), CsError> {
        tokio::task::spawn_blocking(whisper::init)
            .await
            .map_err(|e| CsError::Recording {
                msg: format!("init_model task join error: {e}"),
            })?
            .map_err(|e| CsError::Recording { msg: e.to_string() })
    }

    /// True when the Whisper engine is currently loaded. May flip back to
    /// `false` after idle-unload; the next transcription reloads transparently.
    /// Wraps `whisper::is_initialized` (stt/whisper/singleton.rs:207).
    pub fn is_model_loaded(&self) -> bool {
        whisper::is_initialized()
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
    /// 2. A delivery-grade final pass re-transcribes the whole saved WAV, the
    ///    same `transcribe_file_verdict` adjudicator the hotkey/overlay
    ///    toggle-stop uses. Decoding the recording as one continuous utterance
    ///    avoids the mid-word cut artifacts of the spliced streaming chunks, so
    ///    its text is the quality the overlay delivers.
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

        // Phase 2: delivery-grade final pass over the whole WAV; the streaming
        // splice remains the fallback authority.
        let (streaming_text, _utterances) = transcript.snapshot();
        let final_pass_text = run_final_pass(audio_path, language_hint, deadline).await;

        let final_pass_chars = final_pass_text
            .as_deref()
            .map(|t| t.trim().chars().count())
            .unwrap_or(0);
        let (text, source) =
            select_composer_transcript(final_pass_text.as_deref(), &streaming_text);

        info!(
            target: "composer-dictation",
            source = source.label(),
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex as StdMutex;

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
        fn on_recording_preparing(&self) {}
        fn on_recording_started(&self) {}
        fn on_recording_stopped(&self) {}
        fn on_recording_finalising(&self) {}
        fn on_preview(&self, _text: String) {}
        fn on_correction(&self, _text: String, _previous_text: String) {}
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
        fn on_replace_range(
            &self,
            _utterance_id: u64,
            _start: u64,
            _end: u64,
            _text: String,
            _source: CsLayerSource,
        ) {
        }
        fn on_insert_annotation(
            &self,
            _utterance_id: u64,
            _position: u64,
            _text: String,
            _kind: CsAnnotationKind,
        ) {
        }
        fn on_context_marker(&self, _position: u64, _marker: String) {}
        fn on_session_finalised(&self, _session_id: String, _layer_summary: CsLayerSummary) {}
        fn on_final_transcript_ready(&self, _text: String) {}
        fn on_vad_active(&self, _active: bool) {}
        fn on_audio_level(&self, _rms: f32) {}
        fn on_no_speech(&self, _reason: String) {}
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

    /// The stop-drain budget scales with recording length but is clamped so the
    /// composer UI can never hang indefinitely on a stalled scheduler.
    #[test]
    fn compose_stop_timeout_scales_and_clamps() {
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

    /// A non-empty final pass is the delivery-grade winner over the streaming
    /// splice; both sides are trimmed on the way out.
    #[test]
    fn select_composer_transcript_prefers_final_pass() {
        let (text, source) = select_composer_transcript(Some("  raz dwa trzy  "), "raz dwa tszy");
        assert_eq!(text, "raz dwa trzy");
        assert_eq!(source, ComposerTranscriptSource::FinalPass);
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
