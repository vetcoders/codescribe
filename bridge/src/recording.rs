//! Dictation / STT surface — thin UniFFI wrapper over the live codescribe
//! streaming recorder + Whisper singleton. Translates the engine's semantic
//! `EngineEvent` stream into a small foreign listener contract so the new
//! SwiftUI app can drive real microphone dictation and file transcription.
//! Filled by W3 cut #3 (sibling to `agent.rs`). Uses shared
//! `crate::{CsError, CsLanguage}`.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex as StdMutex, RwLock};
use std::time::{Duration, Instant};

use codescribe_core::audio::load_audio_file;
use codescribe_core::audio::streaming_recorder::StreamingRecorder;
use codescribe_core::pipeline::contracts::{
    AnnotationKind, EngineEvent, EventSink, LayerSource, LayerSummary,
};
use codescribe_core::stt::whisper;
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
    fn on_preview(&self, text: String);
    fn on_correction(&self, text: String, previous_text: String);
    fn on_final(&self, utterance_id: u64, text: String);
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
    fn on_session_finalised(&self, session_id: String, layer_summary: CsLayerSummary);
    /// Authoritative post-stop transcript (LocalFinalPass `final_formatted_text`):
    /// the SAME clean text that is pasted/delivered and written to history. Surfaces
    /// fire it once per dictation stop so the overlay FINAL matches delivery/Copy.
    fn on_final_transcript_ready(&self, text: String);
    fn on_vad_active(&self, active: bool);
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

/// Wait budget for `stop_recording` to drain in-flight transcription before
/// composing the return. Proportional to recording length (residual STT work
/// scales with audio) but clamped so the composer UI never hangs indefinitely
/// if the scheduler stalls (e.g. thermal throttling): the floor covers a cold
/// commit + tail patch on a short note, the cap bounds the worst case.
fn compose_stop_timeout(elapsed: Duration) -> Duration {
    const FLOOR: Duration = Duration::from_secs(8);
    const CAP: Duration = Duration::from_secs(30);
    elapsed.mul_f32(0.6).clamp(FLOOR, CAP)
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
                utterance_id, text, ..
            } => {
                // Compose the composer return here: the streaming recorder's own
                // transcript buffer is never filled on this path.
                self.transcript.append_final(text);
                self.listener.on_final(*utterance_id, text.clone());
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
/// finalized-text accumulator its event sink feeds, and the wall-clock start
/// used to size the stop-drain timeout.
struct ActiveSession {
    recorder: StreamingRecorder,
    transcript: Arc<ComposerTranscript>,
    started_at: Instant,
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
            .start_event_session(language_code)
            .await
            .map_err(|e| CsError::Recording { msg: e.to_string() })?;

        *self.recorder.lock().await = Some(ActiveSession {
            recorder,
            transcript,
            started_at: Instant::now(),
        });
        listener.on_recording_started();
        Ok(())
    }

    /// Stop the active dictation session and return the composed transcript.
    ///
    /// `StreamingRecorder::stop` (audio/streaming_recorder.rs) is the completion
    /// signal: it stops the audio stream, then joins the transcription session
    /// task, which only finishes AFTER every `UtteranceFinal` has been emitted
    /// synchronously into `CsEventSink` (our accumulator). So by the time stop
    /// returns cleanly, the accumulator holds the full transcript.
    ///
    /// That join is otherwise unbounded, so we cap it: a stalled scheduler must
    /// not freeze the composer UI. On timeout we return whatever utterances
    /// finalised so far and log a WARN breadcrumb. The streaming recorder's own
    /// transcript buffer is intentionally ignored here — it stays empty on the
    /// composer path (nothing fills it), so the accumulated `UtteranceFinal`
    /// stream is the authoritative source.
    pub async fn stop_recording(&self) -> Result<String, CsError> {
        let mut session = {
            let mut guard = self.recorder.lock().await;
            guard.take().ok_or_else(|| CsError::Recording {
                msg: "no active recording to stop".to_string(),
            })?
        };

        let budget = compose_stop_timeout(session.started_at.elapsed());
        let transcript = Arc::clone(&session.transcript);

        let timed_out = match tokio::time::timeout(budget, session.recorder.stop()).await {
            Ok(Ok(_)) => false,
            Ok(Err(e)) => return Err(CsError::Recording { msg: e.to_string() }),
            Err(_elapsed) => true,
        };

        let (text, utterances) = transcript.snapshot();
        let chars = text.chars().count();
        if timed_out {
            warn!(
                target: "composer-dictation",
                utterances,
                chars,
                budget_ms = budget.as_millis() as u64,
                "composer voice-note stop timed out; returning partial transcript"
            );
        } else {
            info!(
                target: "composer-dictation",
                utterances,
                chars,
                "composer voice-note stop composed transcript"
            );
        }

        if let Ok(guard) = self.listener.read()
            && let Some(listener) = guard.as_ref()
        {
            listener.on_recording_stopped();
        }
        Ok(text)
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

    /// Captures the payload of the single listener call we assert on.
    #[derive(Default)]
    struct CapturingListener {
        final_calls: StdMutex<Vec<(u64, String)>>,
    }

    impl CsTranscriptionListener for CapturingListener {
        fn on_recording_preparing(&self) {}
        fn on_recording_started(&self) {}
        fn on_recording_stopped(&self) {}
        fn on_preview(&self, _text: String) {}
        fn on_correction(&self, _text: String, _previous_text: String) {}
        fn on_final(&self, utterance_id: u64, text: String) {
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
        fn on_session_finalised(&self, _session_id: String, _layer_summary: CsLayerSummary) {}
        fn on_final_transcript_ready(&self, _text: String) {}
        fn on_vad_active(&self, _active: bool) {}
        fn on_no_speech(&self, _reason: String) {}
        fn on_error(&self, _message: String) {}
    }

    /// The bridge must forward `utterance_id` on `UtteranceFinal` so committed
    /// sinks can stamp segment identity that later `ReplaceRange` patches target.
    /// Regression guard for the W3 keystone (identity flow into committed text).
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
        // Short note: floored so a cold commit + tail patch still fits.
        assert_eq!(
            compose_stop_timeout(Duration::from_secs(3)),
            Duration::from_secs(8)
        );
        // Mid-length: proportional (20s * 0.6 = 12s) inside the band.
        assert_eq!(
            compose_stop_timeout(Duration::from_secs(20)),
            Duration::from_secs(12)
        );
        // Long note: capped so the UI never waits unboundedly.
        assert_eq!(
            compose_stop_timeout(Duration::from_secs(300)),
            Duration::from_secs(30)
        );
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
