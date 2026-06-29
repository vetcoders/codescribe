//! Dictation / STT surface — thin UniFFI wrapper over the live codescribe
//! streaming recorder + Whisper singleton. Translates the engine's semantic
//! `EngineEvent` stream into a small foreign listener contract so the new
//! SwiftUI app can drive real microphone dictation and file transcription.
//! Filled by W3 cut #3 (sibling to `agent.rs`). Uses shared
//! `crate::{CsError, CsLanguage}`.

use std::sync::{Arc, RwLock};

use codescribe_core::audio::load_audio_file;
use codescribe_core::audio::streaming_recorder::StreamingRecorder;
use codescribe_core::pipeline::contracts::{EngineEvent, EventSink};
use codescribe_core::stt::whisper;
use tokio::sync::Mutex;

use crate::{CsError, CsLanguage};

/// Result of a one-shot file transcription.
#[derive(uniffi::Record)]
pub struct CsTranscription {
    /// Final post-processed transcript text.
    pub text: String,
    /// Detected (or requested) language code, e.g. `"pl"` / `"en"`.
    pub language: String,
}

/// Foreign callback trait — dictation events forwarded to Swift.
///
/// Distilled from the engine's richer `EngineEvent` stream:
/// - `on_preview` carries the latest interim/corrected utterance text
///   (replace-not-append semantics).
/// - `on_final` carries a completed (VAD-bounded) utterance.
/// - `on_vad_active` flips when speech starts/ends.
/// - `on_no_speech` fires when a session/utterance produced no usable speech.
/// - `on_error` carries recoverable engine warnings.
///
/// The Swift side must hop these onto the main actor.
#[uniffi::export(with_foreign)]
pub trait CsTranscriptionListener: Send + Sync {
    fn on_preview(&self, text: String);
    fn on_final(&self, text: String);
    fn on_vad_active(&self, active: bool);
    fn on_no_speech(&self, reason: String);
    fn on_error(&self, message: String);
}

/// Internal `EventSink` adapter (NOT exposed across FFI). Lives between the
/// core streaming pipeline and the foreign `CsTranscriptionListener`,
/// translating every `EngineEvent` variant into the appropriate listener call.
struct CsEventSink {
    listener: Arc<dyn CsTranscriptionListener>,
}

impl EventSink for CsEventSink {
    fn on_event(&self, event: &EngineEvent) {
        match event {
            EngineEvent::VadStart { .. } => self.listener.on_vad_active(true),
            EngineEvent::VadEnd { .. } => self.listener.on_vad_active(false),
            EngineEvent::NoSpeech { reason } => self.listener.on_no_speech(reason.clone()),
            // Interim preview and post-correction both surface as "preview"
            // (replace-not-append); streaming continues either way.
            EngineEvent::Preview { text, .. } => self.listener.on_preview(text.clone()),
            EngineEvent::Correction { text, .. } => self.listener.on_preview(text.clone()),
            EngineEvent::UtteranceFinal { text, .. } => self.listener.on_final(text.clone()),
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

/// Thin handle to the codescribe dictation engine (streaming recorder +
/// Whisper). Holds the active recorder behind an async mutex and the current
/// foreign listener behind an `RwLock`.
#[derive(uniffi::Object)]
pub struct CodescribeDictation {
    recorder: Mutex<Option<StreamingRecorder>>,
    listener: RwLock<Option<Arc<dyn CsTranscriptionListener>>>,
}

#[uniffi::export(async_runtime = "tokio")]
impl CodescribeDictation {
    #[uniffi::constructor]
    pub fn new() -> Self {
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

        let sink: Arc<dyn EventSink> = Arc::new(CsEventSink { listener });
        let mut recorder =
            StreamingRecorder::new().map_err(|e| CsError::Recording { msg: e.to_string() })?;
        recorder.set_event_sink(Some(sink));

        let language_code = language.map(|l| l.as_code().to_string());
        recorder
            .start_event_session(language_code)
            .await
            .map_err(|e| CsError::Recording { msg: e.to_string() })?;

        *self.recorder.lock().await = Some(recorder);
        Ok(())
    }

    /// Stop the active dictation session and return the accumulated transcript.
    /// Wraps `StreamingRecorder::stop` (audio/streaming_recorder.rs:145),
    /// discarding the saved WAV path.
    pub async fn stop_recording(&self) -> Result<String, CsError> {
        let mut recorder = {
            let mut guard = self.recorder.lock().await;
            guard.take().ok_or_else(|| CsError::Recording {
                msg: "no active recording to stop".to_string(),
            })?
        };
        let (transcript, _audio_path) = recorder
            .stop()
            .await
            .map_err(|e| CsError::Recording { msg: e.to_string() })?;
        Ok(transcript)
    }

    /// True while a dictation session is active.
    /// Wraps `StreamingRecorder::is_recording` (audio/streaming_recorder.rs:79).
    pub async fn is_recording(&self) -> bool {
        self.recorder
            .lock()
            .await
            .as_ref()
            .map(|recorder| recorder.is_recording())
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
