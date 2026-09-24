//! Live capture wired to the streaming transcription pipeline.
//!
//! [`StreamingRecorder`] owns a [`Recorder`] and forwards every captured block
//! down a bounded channel to `transcription_session`, which emits `EngineEvent`s
//! to a caller-supplied sink. The channel is deliberately deep
//! (`AUDIO_BACKLOG_CHUNKS`): a cold Whisper load happens *behind* it, so the
//! user's first words queue up instead of being dropped while the model loads.
//!
//! Shutdown is ordered and matters. Stopping capture is not enough — the session
//! task has to drain, and the presentation layer ticks on its own task, so both
//! `stop` paths wait for the transcript to stop growing (bounded to three
//! seconds) before releasing the sink. Dropping the sink early truncates the
//! tail of the delivered text.

use crate::asr_session::recorder::{RecorderLifecycleHandle, recorder_lifecycle_channel};
use crate::audio::recorder::{Recorder, RecorderConfig};
use crate::config::{RuntimeSettingsSnapshot, UserSettings};
use crate::pipeline::acoustic_ledger::AcousticLedger;
#[cfg(test)]
use crate::pipeline::acoustic_ledger::SealCoverageReceipt;
use crate::pipeline::contracts::{EngineEvent, EventSink};
use crate::pipeline::streaming::{
    SessionConfig, TailPatchSessionReceipt, collect_buffered_engine_events_with_config,
    stream_log_path, transcription_session,
};
use anyhow::{Context, Result, anyhow};
use std::sync::Arc;
use std::sync::Mutex as StdMutex;
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::sync::{Mutex, mpsc, oneshot};
use tokio::task::JoinHandle;
use tracing::{debug, info, warn};

/// How many turns one capture gesture owns.
///
/// This is per-take capture intent, never a stored preference and never a mode
/// flag. It is decided by the surface that opened the microphone and frozen for
/// exactly that take: the hands-free lanes keep the utterance-epoch contract
/// they always had, and one composer gesture owns one explicit turn.
///
/// It deliberately does **not** describe destination, engine, or acoustic
/// evidence. Silero segmentation, ledger qualification, and Layer 1 tail repair
/// are unaffected by either variant — only the UI-visible epoch lifecycle and
/// the *live* paid formatter lane read this.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CaptureTurnIntent {
    /// Hotkey toggle, hold, tray, and the assistive overlay.
    ///
    /// Trailing silence past the configured threshold closes an utterance
    /// epoch, and every sealed occurrence may reach the live formatter as it
    /// happens. This is the pre-existing behaviour of every non-composer lane.
    #[default]
    HandsFree,
    /// One composer gesture, one explicit take.
    ///
    /// Silence never closes the take — only an explicit stop does — and no
    /// paid formatting is launched per silence-delimited fragment. The turn is
    /// formatted once at terminal processing instead.
    SingleTurn,
}

impl CaptureTurnIntent {
    /// The per-take utterance silence threshold this intent asks the recorder
    /// for, given the configured hands-free value.
    ///
    /// `None` is the pipeline's legacy contract: one continuous stream for the
    /// whole take, no epoch decisions at all. It is the *engine lifecycle* that
    /// rests, not the VAD — Silero keeps running for identity and tail repair.
    pub fn utterance_silence_sec(self, configured_sec: f32) -> Option<f32> {
        match self {
            Self::HandsFree => Some(configured_sec),
            Self::SingleTurn => None,
        }
    }

    /// Whether a sealed occurrence may open a paid formatter slot *while the
    /// take is still live*.
    ///
    /// A one-turn take collects the whole turn and formats it once at terminal
    /// processing, so per-fragment provider calls are refused at the arming
    /// seam rather than deduplicated after the fact.
    pub const fn schedules_live_formatting(self) -> bool {
        matches!(self, Self::HandsFree)
    }

    /// Whether the take may be formatted once when it terminates.
    ///
    /// Exactly the complement of [`Self::schedules_live_formatting`]: a
    /// hands-free take has already paid per occurrence and must not be charged
    /// a second time at stop.
    pub const fn formats_once_at_terminal(self) -> bool {
        matches!(self, Self::SingleTurn)
    }
}

/// Transport admission only: never invoke the executor while opening audio.
fn live_max_capability(
    intent: CaptureTurnIntent,
    enabled: bool,
    policy: crate::config::FormattingPolicy,
    agent: Option<&Arc<dyn crate::ai_formatting::FormattingAgent>>,
) -> Option<Arc<dyn crate::ai_formatting::FormattingAgent>> {
    if intent.schedules_live_formatting()
        && enabled
        && policy == crate::config::FormattingPolicy::Max
    {
        agent.cloned()
    } else {
        None
    }
}

/// Ledger refusal of the terminal transcript after a successful capture stop.
///
/// Raised by [`StreamingRecorder::stop`] when the acoustic ledger cannot
/// authenticate an issued terminal seal. Complete coverage is not finality.
/// The capture itself succeeded — `audio_path` is the take WAV
/// already written to disk — which is why this is a typed error rather than a
/// string: the stop path must retain that audio and close the take instead of
/// reporting a recorder failure.
#[derive(Debug, Clone)]
pub struct TerminalSealRefused {
    pub finality: crate::pipeline::acoustic_ledger::TerminalFinalityRefusal,
    pub audio_path: Option<std::path::PathBuf>,
    /// The committed live document at refusal time. The ledger refused the
    /// seal, not the words: the controller may still hand this text to the
    /// user as a degraded stop-path delivery. It is not a seal witness and is
    /// never written back into the ledger or history as one.
    pub committed_text: String,
}

impl std::fmt::Display for TerminalSealRefused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "terminal transcript refused: {}",
            self.finality.reason().as_str()
        )?;
        if let Some(receipt) = self.finality.coverage() {
            write!(
                f,
                " ({}/{} samples covered; measured max gap {}; threshold {})",
                receipt.covered_samples,
                receipt.speech_samples,
                receipt.max_uncovered_samples,
                receipt.incomplete_threshold_samples
            )?;
        }
        Ok(())
    }
}

impl std::error::Error for TerminalSealRefused {}

/// Producer-owned evidence after capture/archive or transcription-task failure.
/// A saved WAV is recovery material, never proof of a successful transcript.
/// There is deliberately no text field: the shared render buffer alone cannot
/// authenticate a committed revision after a worker fails.
#[derive(Debug)]
pub struct CaptureStopFailure {
    pub session_id: Option<String>,
    pub capture_epoch: u64,
    pub audio_path: Option<std::path::PathBuf>,
    pub cause: anyhow::Error,
    /// If archive finalization and the task both failed, keep both causes.
    pub task_failure: Option<anyhow::Error>,
}

impl std::fmt::Display for CaptureStopFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "capture processing failed: {:#}; committed text unavailable",
            self.cause
        )?;
        if let Some(path) = &self.audio_path {
            write!(f, "; source WAV retained at {}", path.display())?;
        } else {
            write!(f, "; no finalized WAV receipt")?;
        }
        if let Some(error) = &self.task_failure {
            write!(f, "; transcription task also failed: {error:#}")?;
        }
        Ok(())
    }
}

impl std::error::Error for CaptureStopFailure {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(self.cause.as_ref())
    }
}

// Keep enough raw audio queued to survive a cold Whisper load without dropping
// the user's first words. The STT session drains this backlog once the model is ready.
/// Channel depth for cold Whisper load: first words queue instead of drop.
const AUDIO_BACKLOG_CHUNKS: usize = 2048;

/// Engine that owns the live canvas for every session this recorder starts.
///
/// `transcription_session` has exactly one live route (Apple progressive), so
/// the label is a constant today. It is still the recorder's fact, not the
/// controller's: a future runtime engine switch changes this owner and the
/// stop path keeps reporting whatever the session actually ran.
pub const LIVE_STREAMING_ENGINE_LABEL: &str = "live_apple";

/// Content-free witness returned by the production PCM replay seam.
#[derive(Debug)]
pub struct ProductionSessionReplay {
    /// Ordered event stream emitted by the same session implementation as live capture.
    pub events: Vec<EngineEvent>,
    /// The exact ledger mutated by the replayed production session. Consumers
    /// project these receipts; they must not reconstruct transcript authority
    /// from the legacy text events beside them.
    pub acoustic_ledger: Arc<StdMutex<AcousticLedger>>,
    /// Whether recording-start policy armed a Layer 1 provider before the
    /// single-use decision was consumed by the session.
    pub layer1_armed: bool,
    /// Engine that actually owned the live canvas for this replay session.
    pub streaming_engine_label: String,
    /// Typed local tail-patch arming and bounded-drain evidence emitted by the
    /// production session, when that session reached finality.
    pub tail_patch_receipt: Option<TailPatchSessionReceipt>,
}

/// Replay fixture PCM through the production recording-session cone.
///
/// The only differing boundary is PCM ingress: 100 ms in-memory chunks replace
/// CoreAudio callback blocks. Decision construction, `SessionConfig`, session
/// semantics, Layer 1 fan-out, VAD, Apple/Whisper events, and shutdown drainage
/// all remain owned by the same production symbols as microphone capture.
pub async fn replay_production_session(
    samples: &[f32],
    sample_rate: u32,
    language: Option<String>,
    settings: &UserSettings,
) -> Result<ProductionSessionReplay> {
    let runtime_settings = Arc::new(
        crate::config::Config::load_runtime_snapshot_without_keychain()
            .map_err(|error| anyhow!("runtime settings snapshot refused: {error:?}"))?,
    );
    let acoustic_ledger = Arc::new(StdMutex::new(AcousticLedger::new()));
    let (layer1, _decision_receipt) = crate::asr_session::layer1_decision(&runtime_settings);
    let layer1_armed = layer1.is_armed();
    // `transcription_session` has one live canvas route: Apple progressive.
    // Report the route we actually enter; never reconstruct it through the
    // deleted global engine selector.
    let streaming_engine_label = LIVE_STREAMING_ENGINE_LABEL.to_string();
    let utterance_silence_sec = settings.toggle_silence_sec.filter(|&sec| sec >= 0.5);
    let config = SessionConfig {
        session_id: uuid::Uuid::new_v4().to_string(),
        capture_epoch: 1,
        runtime_settings,
        live_formatting_agent: None,
        acoustic_ledger: acoustic_ledger.clone(),
        sample_rate,
        capture_device_name: None,
        language,
        stream_log_path: None,
        utterance_silence_sec,
        // Replay reproduces a hands-free recording: the composer take is a UI
        // gesture with no offline equivalent, so this seam never fabricates one.
        capture_turn: CaptureTurnIntent::HandsFree,
        layer1,
        lifecycle_events: None,
        terminal_audio: None,
        last_window_closed: None,
    };
    let events = collect_buffered_engine_events_with_config(samples, config).await?;
    let tail_patch_receipt = TailPatchSessionReceipt::from_events(&events);
    Ok(ProductionSessionReplay {
        events,
        acoustic_ledger,
        layer1_armed,
        streaming_engine_label,
        tail_patch_receipt,
    })
}

/// A recording session that transcribes while it captures.
///
/// Configure the sink and any callbacks first, then call
/// [`StreamingRecorder::start_event_session`]; the sink is cleared on stop, so
/// it must be set again for each session.
pub struct StreamingRecorder {
    pub recorder: Recorder,
    transcript_buffer: Arc<Mutex<String>>,
    transcription_handle: Option<JoinHandle<()>>,
    sample_rate: u32,
    utterance_callback: Option<Arc<dyn Fn(String) + Send + Sync>>,
    utterance_silence_sec: Option<f32>,
    /// How many turns the next/active take owns. Set by the surface that opened
    /// the microphone and cleared back to the default between sessions, so a
    /// one-turn composer take can never leak into the next hands-free one.
    capture_turn: CaptureTurnIntent,
    /// Counter for audio chunks dropped due to channel backpressure.
    dropped_chunks: Arc<AtomicU64>,
    /// Sink used by `start_event_session`. Caller must configure it explicitly.
    event_sink: Option<Arc<dyn EventSink>>,
    /// Per-block input level tap: receives the RMS of every captured audio
    /// block (linear, 0..~1). Runs on the CoreAudio callback thread — keep it
    /// cheap and non-blocking (a broadcast send, an atomic store).
    level_callback: Option<Arc<dyn Fn(f32) + Send + Sync>>,
    /// O(1) host lifecycle signal for the currently active session.
    lifecycle_handle: Option<RecorderLifecycleHandle>,
    /// Session-frozen runtime truth. Set once by the controller before start.
    runtime_settings: Option<Arc<RuntimeSettingsSnapshot>>,
    /// Bound by the host after session authority; cleared on each new bind.
    live_formatting_agent: Option<Arc<dyn crate::ai_formatting::FormattingAgent>>,
    /// The one ledger instance shared by PCM capture, engines, and reducer.
    acoustic_ledger: Option<Arc<StdMutex<AcousticLedger>>>,
    /// Controller-owned session identity bound with the ledger.
    authority_session_id: Option<String>,
    /// Last capture-open epoch issued for the currently bound session.
    /// Zero means this bind has not successfully opened capture yet.
    capture_epoch: u64,
    captured_samples: Arc<AtomicU64>,
    terminal_audio_sender: Option<
        std::sync::mpsc::Sender<
            Result<crate::pipeline::streaming::live_audio_buffer::FinalizedPcmArchive, String>,
        >,
    >,
    last_window_closed: Option<oneshot::Receiver<()>>,
}

impl StreamingRecorder {
    /// Build a streaming recorder over a default-configured [`Recorder`].
    ///
    /// No sink is attached yet, so `start_event_session` will refuse until
    /// [`Self::set_event_sink`] is called.
    pub fn new() -> Result<Self> {
        let recorder = Recorder::new()?;
        let sample_rate = recorder.config.sample_rate;

        Ok(Self {
            recorder,
            transcript_buffer: Arc::new(Mutex::new(String::new())),
            transcription_handle: None,
            sample_rate,
            utterance_callback: None,
            utterance_silence_sec: None,
            capture_turn: CaptureTurnIntent::HandsFree,
            dropped_chunks: Arc::new(AtomicU64::new(0)),
            event_sink: None,
            level_callback: None,
            lifecycle_handle: None,
            runtime_settings: None,
            live_formatting_agent: None,
            acoustic_ledger: None,
            authority_session_id: None,
            capture_epoch: 0,
            captured_samples: Arc::new(AtomicU64::new(0)),
            terminal_audio_sender: None,
            last_window_closed: None,
        })
    }

    /// Build a streaming recorder over a specific [`RecorderConfig`].
    ///
    /// The configured sample rate is only provisional — it is corrected to the
    /// device's actual rate once the stream opens.
    pub fn with_config(config: RecorderConfig) -> Result<Self> {
        let sample_rate = config.sample_rate;
        let recorder = Recorder::with_config(config)?;

        Ok(Self {
            recorder,
            transcript_buffer: Arc::new(Mutex::new(String::new())),
            transcription_handle: None,
            sample_rate,
            utterance_callback: None,
            utterance_silence_sec: None,
            capture_turn: CaptureTurnIntent::HandsFree,
            dropped_chunks: Arc::new(AtomicU64::new(0)),
            event_sink: None,
            level_callback: None,
            lifecycle_handle: None,
            runtime_settings: None,
            live_formatting_agent: None,
            acoustic_ledger: None,
            authority_session_id: None,
            capture_epoch: 0,
            captured_samples: Arc::new(AtomicU64::new(0)),
            terminal_audio_sender: None,
            last_window_closed: None,
        })
    }

    /// Bind the next capture to one immutable settings snapshot and one ledger.
    pub fn bind_session_authority(
        &mut self,
        session_id: String,
        runtime_settings: Arc<RuntimeSettingsSnapshot>,
    ) -> Arc<StdMutex<AcousticLedger>> {
        let acoustic_ledger = Arc::new(StdMutex::new(AcousticLedger::new()));
        self.capture_epoch = 0;
        self.authority_session_id = Some(session_id);
        self.runtime_settings = Some(runtime_settings);
        self.live_formatting_agent = None;
        self.acoustic_ledger = Some(Arc::clone(&acoustic_ledger));
        acoustic_ledger
    }

    /// Supply the existing host executor without creating another Agent session.
    /// The streaming lane still owes explicit whole-instruction admission.
    pub fn set_live_formatting_agent(
        &mut self,
        agent: Option<Arc<dyn crate::ai_formatting::FormattingAgent>>,
    ) {
        self.live_formatting_agent = agent;
    }

    /// Borrow the ledger handle already bound for the next/active session.
    pub fn acoustic_ledger_handle(&self) -> Option<Arc<StdMutex<AcousticLedger>>> {
        self.acoustic_ledger.as_ref().map(Arc::clone)
    }

    /// Identity bound at capture open, not a latest-file or UI-slot lookup.
    pub fn capture_identity(&self) -> (Option<&str>, u64) {
        (self.authority_session_id.as_deref(), self.capture_epoch)
    }

    /// Store a per-utterance text callback.
    ///
    /// Note: the stored value is currently never read by this type — completed
    /// utterances reach consumers as `EngineEvent`s through the event sink
    /// instead. The live per-utterance callback is the one on the presentation
    /// emitter, not this one.
    pub fn set_utterance_callback(&mut self, callback: Option<Arc<dyn Fn(String) + Send + Sync>>) {
        self.utterance_callback = callback;
    }

    /// Override how much trailing silence closes an utterance.
    ///
    /// Read when the session starts and passed into `SessionConfig`, so it has
    /// to be set before [`Self::start_event_session`]. `None` keeps the
    /// pipeline default.
    pub fn set_utterance_silence_sec(&mut self, silence_sec: Option<f32>) {
        self.utterance_silence_sec = silence_sec;
    }

    /// Declare how many turns the next take owns.
    ///
    /// Read when the session starts and frozen into `SessionConfig`, so it has
    /// to be set before [`Self::start_event_session`]. The controller resets it
    /// to [`CaptureTurnIntent::HandsFree`] between sessions: a one-turn
    /// composer take must never widen into the hands-free take that follows it.
    pub fn set_capture_turn_intent(&mut self, capture_turn: CaptureTurnIntent) {
        self.capture_turn = capture_turn;
    }

    /// The capture intent frozen for the next/active take.
    pub fn capture_turn_intent(&self) -> CaptureTurnIntent {
        self.capture_turn
    }

    /// Set the per-block input-level tap consumed by UI meters (overlay
    /// waveform). Configure before `start_event_session`; cleared alongside the
    /// other callbacks between sessions.
    pub fn set_level_callback(&mut self, callback: Option<Arc<dyn Fn(f32) + Send + Sync>>) {
        self.level_callback = callback;
    }

    /// Returns a cloned handle to the transcript buffer.
    ///
    /// Shared delivery buffer. Only committed reducer projections may write it;
    /// previews are ephemeral paint and `stop()` only reads the accumulated
    /// committed rendering.
    pub fn transcript_buffer_handle(&self) -> Arc<Mutex<String>> {
        self.transcript_buffer.clone()
    }

    /// Set the event sink for the unified pipeline.
    pub fn set_event_sink(&mut self, sink: Option<Arc<dyn EventSink>>) {
        self.event_sink = sink;
    }

    /// Clone the active session sink so controller-owned presentation events
    /// can enter the same ordered reducer/fanout as engine events.
    pub fn event_sink_handle(&self) -> Option<Arc<dyn EventSink>> {
        self.event_sink.clone()
    }

    /// Returns true when the underlying recorder still has an active audio stream.
    pub fn is_recording(&self) -> bool {
        self.recorder.is_active()
    }

    /// Notify the active transcription task that the host crossed sleep/wake.
    ///
    /// No active capture is a normal no-op. This method only enqueues a typed
    /// boundary; the session loop owns the fail-closed Layer 1 transition.
    pub fn note_sleep_wake(&self) -> bool {
        self.recorder.is_active()
            && self
                .lifecycle_handle
                .as_ref()
                .is_some_and(RecorderLifecycleHandle::note_sleep_wake)
    }

    /// Engine label of the live route this recorder's sessions run on.
    ///
    /// The stop path publishes this as the serving truth for Settings
    /// "Active STT"; it must never be reconstructed from the configured
    /// `stt_engine` preference (see `controller::serving_status`).
    pub fn streaming_engine_label(&self) -> &'static str {
        LIVE_STREAMING_ENGINE_LABEL
    }

    /// Start recording with the new event-based pipeline.
    ///
    /// Uses `transcription_session` which emits `EngineEvent`s to the configured
    /// `event_sink`.
    pub async fn start_event_session(&mut self, language: Option<String>) -> Result<()> {
        let event_sink = self.event_sink.clone().ok_or_else(|| {
            anyhow!(
                "start_event_session requires event_sink (set_event_sink(Some(...)) before start)"
            )
        })?;
        let session_id = self
            .authority_session_id
            .clone()
            .ok_or_else(|| anyhow!("start_event_session requires bound session authority"))?;
        let runtime_settings = self
            .runtime_settings
            .as_ref()
            .map(Arc::clone)
            .ok_or_else(|| anyhow!("start_event_session requires RuntimeSettingsSnapshot"))?;
        let acoustic_ledger = self
            .acoustic_ledger
            .as_ref()
            .map(Arc::clone)
            .ok_or_else(|| anyhow!("start_event_session requires AcousticLedger"))?;
        let next_capture_epoch = self.capture_epoch.checked_add(1).ok_or_else(|| {
            anyhow!("capture epoch overflow: no unused epoch remains for the bound session")
        })?;

        // Clear previous transcript and reset drop counter
        *self.transcript_buffer.lock().await = String::new();
        self.dropped_chunks.store(0, Ordering::Relaxed);
        self.captured_samples.store(0, Ordering::Relaxed);

        // Create channel for audio chunks. This is intentionally larger than a
        // normal live queue: cold STT initialization happens behind this buffer.
        let (tx, rx) = mpsc::channel::<Vec<f32>>(AUDIO_BACKLOG_CHUNKS);

        // Setup callback to send audio data
        let dropped = Arc::clone(&self.dropped_chunks);
        let level_callback = self.level_callback.clone();
        let captured_samples = Arc::clone(&self.captured_samples);
        self.recorder.set_callback(Box::new(move |data| {
            captured_samples.fetch_add(data.len() as u64, Ordering::Relaxed);
            if let Some(ref level_cb) = level_callback {
                level_cb(block_rms(data));
            }
            if let Err(_e) = tx.try_send(data.to_vec()) {
                let n = dropped.fetch_add(1, Ordering::Relaxed);
                if n == 0 || (n + 1).is_multiple_of(50) {
                    tracing::warn!("Audio callback: channel full, dropped {} chunk(s)", n + 1);
                }
            }
        }));

        // Start actual audio stream
        self.recorder.start().await?;
        self.capture_epoch = next_capture_epoch;
        event_sink.on_capture_opened(&session_id, next_capture_epoch);

        // Update sample rate to match real input stream
        let actual_sample_rate = self.recorder.actual_sample_rate();
        let capture_device_name = self.recorder.last_input_device().map(str::to_owned);
        crate::audio::capture_receipt::publish_open_capture_path(
            crate::audio::capture_receipt::CapturePathMeta::from_open_path(
                actual_sample_rate,
                self.recorder.last_native_channels(),
                self.recorder.last_input_device(),
            ),
        );
        if actual_sample_rate != self.sample_rate {
            info!(
                "StreamingRecorder sample_rate updated: config={}Hz -> actual={}Hz",
                self.sample_rate, actual_sample_rate
            );
            self.sample_rate = actual_sample_rate;
        }

        let log_path = stream_log_path();
        let utterance_silence_sec = self.utterance_silence_sec;
        let capture_turn = self.capture_turn;
        let live_formatting_agent = live_max_capability(
            capture_turn,
            runtime_settings.values().ai_formatting_enabled,
            runtime_settings.formatting_policy(),
            self.live_formatting_agent.as_ref(),
        );

        let (layer1, _decision_receipt) = crate::asr_session::layer1_decision(&runtime_settings);
        let (lifecycle_handle, lifecycle_events) = recorder_lifecycle_channel();
        self.lifecycle_handle = Some(lifecycle_handle);
        let (terminal_tx, terminal_rx) = std::sync::mpsc::channel();
        self.terminal_audio_sender = Some(terminal_tx);
        let (last_window_tx, last_window_rx) = oneshot::channel();
        self.last_window_closed = Some(last_window_rx);
        self.transcription_handle = Some(tokio::spawn(async move {
            transcription_session(
                rx,
                event_sink,
                SessionConfig {
                    session_id,
                    capture_epoch: next_capture_epoch,
                    runtime_settings,
                    live_formatting_agent,
                    acoustic_ledger,
                    sample_rate: actual_sample_rate,
                    capture_device_name,
                    language,
                    stream_log_path: log_path,
                    utterance_silence_sec,
                    capture_turn,
                    layer1,
                    lifecycle_events: Some(lifecycle_events),
                    terminal_audio: Some(terminal_rx),
                    last_window_closed: Some(last_window_tx),
                },
            )
            .await;
        }));

        Ok(())
    }

    /// Stop the session and return the accumulated transcript plus the WAV path.
    ///
    /// Ordered shutdown: stop capture (which drops the sender), await the
    /// transcription task, let the presentation layer drain, then release the
    /// sink. Any chunks dropped to backpressure during the session are logged
    /// here — that counter is the signal that audio was actually lost.
    pub async fn stop(&mut self) -> Result<(String, Option<std::path::PathBuf>)> {
        info!("Stopping streaming recorder...");

        // Report any dropped audio chunks
        let drops = self.dropped_chunks.load(Ordering::Relaxed);
        if drops > 0 {
            warn!(
                "Recording session: dropped {} audio chunk(s) due to backpressure",
                drops
            );
        }

        // The all-at-once API remains for callers outside the stop delivery
        // path; the controller uses close_capture/finish_closed_capture.
        let stopped = self.recorder.stop().await;
        self.complete_stop(stopped).await
    }

    /// Close capture independently of the session drain and WAV finalization.
    pub async fn close_capture(&mut self) -> bool {
        self.recorder.close_capture().await
    }

    /// Wait only for Apple final admission and reducer delivery, never L1 or
    /// terminal archive work. A failed worker closes the channel without an ack.
    pub async fn wait_last_window_closed(&mut self, bound: std::time::Duration) -> bool {
        let Some(mut receiver) = self.last_window_closed.take() else {
            return false;
        };
        matches!(tokio::time::timeout(bound, &mut receiver).await, Ok(Ok(())))
    }

    /// Continue the owned stop tail after the microphone has closed.
    pub async fn finish_closed_capture(
        &mut self,
        was_active: bool,
    ) -> Result<(String, Option<std::path::PathBuf>)> {
        let stopped = self.recorder.finalize_closed_capture(was_active);
        self.complete_stop(stopped).await
    }

    /// The production stop tail. Tests inject only the recorder's archive
    /// outcome; notification, task join, drain and failure selection stay here.
    async fn complete_stop(
        &mut self,
        stopped: Result<Option<std::path::PathBuf>>,
    ) -> Result<(String, Option<std::path::PathBuf>)> {
        if let Some(sender) = self.terminal_audio_sender.take() {
            let receipt = match &stopped {
                Ok(Some(path)) => Ok(
                    crate::pipeline::streaming::live_audio_buffer::FinalizedPcmArchive {
                        session_id: self.authority_session_id.clone().unwrap_or_default(),
                        capture_epoch: self.capture_epoch,
                        sample_rate: self.sample_rate,
                        sample_count: self.captured_samples.load(Ordering::Relaxed),
                        path: path.clone(),
                    },
                ),
                Ok(None) => Err("capture finalized without a WAV archive".into()),
                Err(error) => Err(format!("capture archive finalization failed: {error}")),
            };
            let _ = sender.send(receipt);
        }
        self.lifecycle_handle = None;

        // 2. Wait for worker to finish processing remaining chunks
        // Borrow until joined: cancellation of a caller must not detach the
        // handle. Named controller Stop keeps this future alive across expiry.
        let task_failure = if let Some(handle) = self.transcription_handle.as_mut() {
            debug!("Waiting for transcription session task to finish...");
            handle
                .await
                .context("Transcription session task failed")
                .err()
        } else {
            None
        };
        self.transcription_handle = None;

        // 3. Drain presentation layer.
        // PresentationEmitter's BufferedEmitter tick loop runs in a separate
        // tokio task. After transcription_session sends Finish, the tick loop
        // needs time to drain queued text into transcript_buffer before we
        // drop the event sink (which aborts the tick loop via Drop).
        if self.event_sink.is_some() {
            let drain_deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(3);
            loop {
                let snapshot = self.transcript_buffer.lock().await.len();
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                if self.transcript_buffer.lock().await.len() == snapshot
                    || tokio::time::Instant::now() >= drain_deadline
                {
                    break;
                }
            }
        }
        self.event_sink = None;

        // No early return may bypass the owned shutdown tail. Archive failure
        // is primary when both operations failed; never invent a saved path.
        let (audio_path, cause, task_failure) = match stopped {
            Ok(path) => (path, task_failure, None),
            Err(error) => (None, Some(error), task_failure),
        };
        if let Some(cause) = cause {
            return Err(anyhow::Error::new(CaptureStopFailure {
                session_id: self.authority_session_id.clone(),
                capture_epoch: self.capture_epoch,
                audio_path,
                cause,
                task_failure,
            }));
        }

        let transcript = self.transcript_buffer.lock().await.clone();
        // A gesture shorter than one complete speech window can contain PCM
        // while producing no Silero/ledger observation. It is still a take,
        // but cannot owe a terminal transcript receipt. A longer unobserved
        // capture remains a processing refusal.
        let captured_samples = self.captured_samples.load(Ordering::Relaxed);
        let empty_capture = captured_samples <= u64::from(self.sample_rate) * 3 / 10
            && transcript.is_empty()
            && self.acoustic_ledger.as_ref().is_none_or(|ledger| {
                ledger
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .has_no_capture_facts()
            });
        if empty_capture {
            return Ok((transcript, audio_path));
        }
        let finality = self.authority_session_id.as_deref().and_then(|session| {
            self.acoustic_ledger.as_ref().map(|ledger| {
                ledger
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .terminal_finality(session, self.capture_epoch)
            })
        });
        match finality {
            Some(crate::pipeline::acoustic_ledger::TerminalFinality::Sealed(_)) => {
                Ok((transcript, audio_path))
            }
            Some(crate::pipeline::acoustic_ledger::TerminalFinality::ObservedSilence(_))
                if transcript.is_empty() =>
            {
                Ok((transcript, audio_path))
            }
            Some(crate::pipeline::acoustic_ledger::TerminalFinality::Refused(finality)) => {
                Err(anyhow::Error::new(TerminalSealRefused {
                    finality,
                    audio_path,
                    committed_text: transcript,
                }))
            }
            _ => Err(anyhow::Error::new(CaptureStopFailure {
                session_id: self.authority_session_id.clone(),
                capture_epoch: self.capture_epoch,
                audio_path,
                cause: anyhow!("recording terminal authority unavailable or inconsistent"),
                task_failure: None,
            })),
        }
    }

    /// Stop the session and return only the transcript.
    ///
    /// Same ordered shutdown as [`Self::stop`], but the WAV path is dropped.
    /// The file itself is still written by the recorder — this discards the
    /// handle, it does not suppress the write.
    pub async fn stop_and_discard_path(&mut self) -> Result<String> {
        let (transcript, _audio_path) = self.stop().await?;
        Ok(transcript)
    }
}

/// RMS of one captured audio block (linear, 0..~1 for full-scale input).
/// Cheap enough for the CoreAudio callback thread (one pass + one sqrt).
fn block_rms(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    // Accumulate in f64 so a malformed/out-of-range f32 block cannot overflow
    // the sum. Non-finite device samples are treated as silence; NaN/Inf must
    // never cross the typed audio-level transport into Swift.
    let sum_sq = samples.iter().fold(0.0_f64, |sum, sample| {
        let sample = if sample.is_finite() {
            f64::from(*sample)
        } else {
            0.0
        };
        sum + sample * sample
    });
    (sum_sq / samples.len() as f64).sqrt() as f32
}

/// Unit and opt-in e2e probes for RMS meters, VAD index sync, and corpus WER.
#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::chunker::{SpeechEvent, SpeechSession, VadGateMode};
    use crate::audio::load_audio_file;
    use crate::vad;
    use serial_test::serial;
    use std::fs;
    use tokio::time::Duration;

    struct TransportOnlyAgent;

    #[async_trait::async_trait]
    impl crate::ai_formatting::FormattingAgent for TransportOnlyAgent {
        async fn execute(
            &self,
            _turn_id: &str,
            _text: &str,
            _settings: &RuntimeSettingsSnapshot,
        ) -> Result<String> {
            panic!("transport must not execute an instruction");
        }
    }

    #[test]
    fn live_max_transport_preserves_owner_and_excludes_other_policies() {
        use crate::config::FormattingPolicy;
        let agent: Arc<dyn crate::ai_formatting::FormattingAgent> = Arc::new(TransportOnlyAgent);
        for intent in [CaptureTurnIntent::HandsFree, CaptureTurnIntent::SingleTurn] {
            for enabled in [false, true] {
                for policy in [
                    FormattingPolicy::Off,
                    FormattingPolicy::Correction,
                    FormattingPolicy::Smart,
                    FormattingPolicy::Max,
                ] {
                    let selected = live_max_capability(intent, enabled, policy, Some(&agent));
                    let admitted = intent == CaptureTurnIntent::HandsFree
                        && enabled
                        && policy == FormattingPolicy::Max;
                    assert_eq!(selected.is_some(), admitted);
                    if let Some(selected) = selected {
                        assert!(Arc::ptr_eq(&selected, &agent));
                    }
                }
            }
        }
        assert!(
            live_max_capability(
                CaptureTurnIntent::HandsFree,
                true,
                FormattingPolicy::Max,
                None,
            )
            .is_none()
        );
    }

    /// Empty/silence/full-scale blocks map to the 0 / 0 / ~1 energy ladder meters use.
    #[test]
    fn block_rms_measures_signal_energy() {
        assert_eq!(block_rms(&[]), 0.0, "empty block must read as silence");
        assert_eq!(block_rms(&[0.0; 512]), 0.0, "digital silence is 0 RMS");
        let full_scale = block_rms(&[1.0, -1.0, 1.0, -1.0]);
        assert!(
            (full_scale - 1.0).abs() < 1e-6,
            "full-scale square wave must read ~1.0, got {full_scale}"
        );
        let half = block_rms(&[0.5, -0.5, 0.5, -0.5]);
        assert!(
            (half - 0.5).abs() < 1e-6,
            "half-scale square wave must read ~0.5, got {half}"
        );
    }

    /// Quiet < loud stays finite; NaN/Inf capture samples must not poison level transport.
    #[test]
    fn block_rms_orders_quiet_and_loud_finite_levels() {
        let silence = block_rms(&[0.0; 512]);
        let quiet = block_rms(&[0.01, -0.01, 0.01, -0.01]);
        let loud = block_rms(&[0.8, -0.8, 0.8, -0.8]);

        assert!(silence.is_finite() && quiet.is_finite() && loud.is_finite());
        assert!(
            silence < quiet && quiet < loud,
            "expected monotonic energy, got silence={silence}, quiet={quiet}, loud={loud}"
        );
        assert_eq!(
            block_rms(&[f32::NAN, f32::INFINITY, f32::NEG_INFINITY]),
            0.0,
            "non-finite capture samples must not poison the meter transport"
        );
    }

    /// Delivery probe for the selected real input. During the nine-second run,
    /// keep 0-3s silent, speak quietly during 3-6s, then loudly during 6-9s.
    /// The test is ignored by default because it requires TCC microphone access
    /// and a human-marked acoustic sequence.
    #[tokio::test]
    #[ignore = "requires selected microphone + TCC and silence/quiet/loud operator input"]
    async fn real_input_rms_probe() {
        if !env_bool("CODESCRIBE_E2E_MIC") {
            eprintln!("Skipping real RMS probe (set CODESCRIBE_E2E_MIC=1 to enable)");
            return;
        }

        let started = std::time::Instant::now();
        let (level_tx, level_rx) = std::sync::mpsc::sync_channel::<(f32, f32)>(1024);
        let mut recorder = Recorder::new().expect("Failed to initialize selected microphone");
        recorder.set_callback(Box::new(move |samples| {
            let _ = level_tx.try_send((started.elapsed().as_secs_f32(), block_rms(samples)));
        }));

        eprintln!("RMS probe: 0-3s SILENCE, 3-6s QUIET SPEECH, 6-9s LOUD SPEECH");
        recorder
            .start()
            .await
            .expect("Failed to start selected microphone");
        tokio::time::sleep(Duration::from_secs(9)).await;
        let audio_path = recorder
            .stop()
            .await
            .expect("Failed to stop selected microphone");
        if let Some(path) = audio_path {
            let _ = std::fs::remove_file(path);
        }

        let mut windows = [Vec::<f32>::new(), Vec::<f32>::new(), Vec::<f32>::new()];
        for (elapsed, rms) in level_rx.try_iter() {
            assert!(rms.is_finite(), "real input emitted non-finite RMS: {rms}");
            let index = (elapsed / 3.0).floor() as usize;
            if let Some(window) = windows.get_mut(index) {
                window.push(rms);
            }
        }

        let means = windows.map(|window| {
            assert!(
                !window.is_empty(),
                "real input probe window captured no blocks"
            );
            window.iter().copied().sum::<f32>() / window.len() as f32
        });
        eprintln!(
            "RMS probe means: silence={:.6}, quiet={:.6}, loud={:.6}",
            means[0], means[1], means[2]
        );
        assert!(
            means[0] < means[1] && means[1] < means[2],
            "selected input did not produce ordered silence/quiet/loud energy: {means:?}"
        );
    }

    /// Five-minute delivery probe for capture/backpressure stability. This runs
    /// the production `StreamingRecorder` path against the selected input and
    /// reports callback, engine-event, and dropped-chunk counters. It is opt-in
    /// because it needs TCC microphone access and intentionally holds the real
    /// audio device for the full acceptance interval.
    #[tokio::test]
    #[ignore = "requires selected microphone + TCC and a five-minute foreground run"]
    async fn sustained_real_input_pressure_probe() {
        if !env_bool("CODESCRIBE_E2E_MIC") {
            eprintln!("Skipping sustained mic probe (set CODESCRIBE_E2E_MIC=1 to enable)");
            return;
        }

        let duration_sec = env_f32("CODESCRIBE_E2E_SUSTAIN_SEC", 300.0).max(300.0);
        let level_blocks = Arc::new(AtomicU64::new(0));
        let non_finite_levels = Arc::new(AtomicU64::new(0));
        let level_blocks_for_callback = Arc::clone(&level_blocks);
        let non_finite_for_callback = Arc::clone(&non_finite_levels);
        let sink = Arc::new(crate::pipeline::sinks::CollectorEventSink::new());
        let mut recorder = StreamingRecorder::new().expect("Failed to initialize selected input");
        recorder.set_level_callback(Some(Arc::new(move |rms| {
            level_blocks_for_callback.fetch_add(1, Ordering::Relaxed);
            if !rms.is_finite() {
                non_finite_for_callback.fetch_add(1, Ordering::Relaxed);
            }
        })));
        recorder.set_event_sink(Some(sink.clone()));

        eprintln!("Sustained mic probe: recording selected input for {duration_sec:.0}s");
        recorder
            .start_event_session(None)
            .await
            .expect("Failed to start streaming recorder");
        tokio::time::sleep(Duration::from_secs_f32(duration_sec)).await;
        let (_transcript, audio_path) = recorder
            .stop()
            .await
            .expect("Failed to stop streaming recorder");
        if let Some(path) = audio_path {
            let _ = std::fs::remove_file(path);
        }

        let levels = level_blocks.load(Ordering::Relaxed);
        let invalid = non_finite_levels.load(Ordering::Relaxed);
        let drops = recorder.dropped_chunks.load(Ordering::Relaxed);
        let events = sink.events().len();
        eprintln!(
            "Sustained mic counters: level_blocks={levels}, non_finite={invalid}, dropped_chunks={drops}, engine_events={events}"
        );
        assert!(levels > 0, "selected input produced no capture callbacks");
        assert_eq!(invalid, 0, "real input emitted non-finite RMS levels");
        assert_eq!(drops, 0, "sustained recording dropped audio chunks");
    }

    /// `start_event_session` must refuse when no event sink was configured.
    #[tokio::test]
    async fn start_event_session_requires_event_sink() {
        let mut recorder = StreamingRecorder::new().expect("Failed to create recorder");
        let err = recorder
            .start_event_session(Some("en".to_string()))
            .await
            .expect_err("start_event_session should fail when event sink is missing");
        assert!(
            err.to_string().contains("requires event_sink"),
            "unexpected error: {err:?}"
        );
    }

    /// Live mic: SpeechSession emits chunks for lt/eq/gt VAD block sizes (opt-in).
    #[test]
    #[ignore] // Manual: requires microphone + Silero model (set CODESCRIBE_E2E_MIC=1)
    fn test_vad_gate_live_chunk_sizes() {
        if !env_bool("CODESCRIBE_E2E_MIC") {
            eprintln!("Skipping mic gate test (set CODESCRIBE_E2E_MIC=1 to enable)");
            return;
        }

        let model_path = vad::default_model_path();
        if !model_path.exists() {
            eprintln!(
                "Skipping: Silero VAD model not found at {}",
                model_path.display()
            );
            return;
        }

        let record_sec = env_f32("CODESCRIBE_E2E_MIC_SEC", 6.0).max(2.0);
        println!("Speak now for ~{:.1}s...", record_sec);

        let mut recorder = Recorder::new().expect("Failed to create recorder");
        let rt = tokio::runtime::Runtime::new().expect("Failed to create tokio runtime");
        let wav_path = rt
            .block_on(async {
                recorder.start().await.expect("Failed to start recorder");
                tokio::time::sleep(Duration::from_secs_f32(record_sec)).await;
                recorder.stop().await.expect("Failed to stop recorder")
            })
            .expect("No WAV produced");

        let (samples, sample_rate) =
            load_audio_file(&wav_path).expect("Failed to load recorded audio");

        let mut resampler = vad::Resampler::new(sample_rate);
        let samples_16k = resampler.resample(&samples);
        let chunk_sec = 4.0f32;
        let chunk_limit = (vad::VAD_SAMPLE_RATE as f32 * chunk_sec) as usize;

        let cases = [
            ("lt", chunk_limit / 2),
            ("eq", chunk_limit),
            ("gt", chunk_limit * 2),
        ];

        for (label, block_len) in cases {
            let mut session = SpeechSession::new_stream(vad::VAD_SAMPLE_RATE, chunk_sec, 0.0);
            let mut chunk_events = 0usize;
            let mut idx = 0usize;
            while idx < samples_16k.len() {
                let end = (idx + block_len).min(samples_16k.len());
                let slice = &samples_16k[idx..end];
                for event in session.feed(slice, vad::VAD_SAMPLE_RATE) {
                    if matches!(event, SpeechEvent::Chunk(_)) {
                        chunk_events += 1;
                    }
                }
                idx = end;
            }
            if let Some(SpeechEvent::Chunk(_)) = session.flush() {
                chunk_events += 1;
            }

            assert!(
                chunk_events > 0,
                "Expected at least one chunk for case {} (block_len={})",
                label,
                block_len
            );
        }

        let _ = fs::remove_file(&wav_path);
    }

    /// Opt-in flag: true only for `1` or case-insensitive `true`.
    fn env_bool(key: &str) -> bool {
        std::env::var(key)
            .ok()
            .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
            .unwrap_or(false)
    }

    /// Parse env `f32`; unset or unparsable yields `default`.
    fn env_f32(key: &str, default: f32) -> f32 {
        std::env::var(key)
            .ok()
            .and_then(|v| v.parse::<f32>().ok())
            .unwrap_or(default)
    }

    /// Terminal no-speech / `*_failed.wav` names that must not score as VAD misses.
    fn is_terminal_no_speech_artifact(file_name: &str) -> bool {
        file_name.contains("no-speech") || file_name.ends_with("_failed.wav")
    }

    /// Name-bounded filter: only explicit no-speech / `*_failed.wav` names match.
    #[test]
    fn terminal_no_speech_artifact_filter_is_name_bounded() {
        assert!(is_terminal_no_speech_artifact(
            "20260709_120000_no-speech_raw.wav"
        ));
        assert!(is_terminal_no_speech_artifact(
            "20260709_120001_dictation_failed.wav"
        ));
        assert!(!is_terminal_no_speech_artifact(
            "20260709_120002_failed-but-recovered_raw.wav"
        ));
        assert!(!is_terminal_no_speech_artifact(
            "03_algorytm-ma-zlozonosc.wav"
        ));
        assert!(!is_terminal_no_speech_artifact("dictation_failed.m4a"));
    }

    /// One VAD chunk in input sample-rate space — max allowed index drift.
    fn vad_index_drift_tolerance(input_sr: u32) -> usize {
        ((vad::CHUNK_SIZE as f32 * input_sr as f32) / vad::VAD_SAMPLE_RATE as f32) as usize
    }

    /// Synthetic tone: VAD→raw index mapping stays within one-chunk tolerance.
    #[test]
    #[serial]
    fn test_vad_index_sync_no_drift() {
        let input_sr = 48000u32;
        let callback_size = 1024usize;
        let num_callbacks = 100usize;

        let mut session = SpeechSession::new_stream(input_sr, 15.0, 0.0);
        assert_eq!(
            session.gate_mode(),
            crate::audio::chunker::VadGateMode::Supervisor,
            "drift guard must explicitly validate Supervisor mode"
        );

        let freq = 440.0f32;
        let mut phase = 0.0f32;
        let phase_inc = 2.0 * std::f32::consts::PI * freq / input_sr as f32;

        for _ in 0..num_callbacks {
            let mut buf = Vec::with_capacity(callback_size);
            for _ in 0..callback_size {
                buf.push(phase.sin() * 0.5);
                phase += phase_inc;
            }
            let _ = session.feed(&buf, input_sr);
        }

        let total_raw = num_callbacks * callback_size;
        assert_eq!(
            session.raw_cursor(),
            total_raw,
            "raw_cursor should equal total input samples"
        );

        let vad_sample = session
            .vad_current_sample()
            .expect("Supervisor mode should expose VAD sample index");
        let mapped = session.vad_to_raw_index_pub(vad_sample);
        let raw_cur = session.raw_cursor();
        let drift = mapped.abs_diff(raw_cur);
        let tolerance = vad_index_drift_tolerance(input_sr);
        assert!(
            drift <= tolerance,
            "VAD index drift too large: mapped={} raw_cursor={} drift={} tolerance={}",
            mapped,
            raw_cur,
            drift,
            tolerance
        );

        assert!(
            session.vad_resample_buf_len() < vad::CHUNK_SIZE,
            "Residual buffer should be < CHUNK_SIZE, got {}",
            session.vad_resample_buf_len()
        );
    }

    /// Busy Supervisor path: interim/final keep boundary and speech accounting.
    #[test]
    #[serial]
    fn test_supervisor_busy_flush_keeps_boundary_and_speech_accounting() {
        let input_sr = 48000u32;
        let callback_size = 1024usize;
        let num_callbacks = 210usize;

        let mut session = SpeechSession::new_utterance_with_silence(input_sr, 10.0);
        assert_eq!(
            session.gate_mode(),
            VadGateMode::Supervisor,
            "busy flush guard must explicitly validate Supervisor mode"
        );

        // Deterministic open segment even when VAD model is unavailable.
        session.set_vad_threshold_for_test(-1.0);

        let mut interim_events = 0usize;
        let mut accounted_speech_vad_samples = 0u64;

        for _ in 0..num_callbacks {
            let buf = vec![0.0f32; callback_size];
            for event in session.feed(&buf, input_sr) {
                let event_speech = session.take_event_speech_vad_samples();
                accounted_speech_vad_samples =
                    accounted_speech_vad_samples.saturating_add(event_speech);
                match event {
                    SpeechEvent::Utterance => {
                        interim_events = interim_events.saturating_add(1);
                        assert!(
                            event_speech > 0,
                            "busy interim event should carry positive speech sample accounting"
                        );
                    }
                    SpeechEvent::UtteranceFinal => {
                        panic!("unexpected UtteranceFinal before flush in long-silence test")
                    }
                    SpeechEvent::Chunk(_) => {
                        panic!("unexpected Chunk event in utterance mode")
                    }
                }
            }
        }

        assert!(
            interim_events > 0,
            "busy callback run should emit at least one interim utterance before flush"
        );

        let flush = session.flush();
        let flush_speech = session.take_event_speech_vad_samples();
        accounted_speech_vad_samples = accounted_speech_vad_samples.saturating_add(flush_speech);

        match flush {
            Some(SpeechEvent::UtteranceFinal) => (),
            Some(SpeechEvent::Utterance) => {
                panic!("flush should emit final utterance event")
            }
            Some(SpeechEvent::Chunk(_)) => {
                panic!("flush should not emit stream chunk in utterance mode")
            }
            None => panic!("flush should preserve active Supervisor boundary under busy load"),
        };
        assert!(
            flush_speech > 0,
            "flush final event should carry pending speech sample accounting"
        );
        assert_eq!(
            session.take_event_speech_vad_samples(),
            0,
            "speech accounting queue should be empty after consuming flush event"
        );

        let total_raw = num_callbacks * callback_size;
        assert_eq!(
            session.raw_cursor(),
            total_raw,
            "raw cursor should stay aligned with callback sample count under busy load"
        );

        let vad_sample = session
            .vad_current_sample()
            .expect("Supervisor mode should expose VAD sample index");
        let mapped = session.vad_to_raw_index_pub(vad_sample);
        let raw_cur = session.raw_cursor();
        let drift = mapped.abs_diff(raw_cur);
        let tolerance = vad_index_drift_tolerance(input_sr);
        assert!(
            drift <= tolerance,
            "busy path drift too large: mapped={} raw_cursor={} drift={} tolerance={}",
            mapped,
            raw_cur,
            drift,
            tolerance
        );
        assert_eq!(
            accounted_speech_vad_samples as usize, vad_sample,
            "sum of emitted speech sample accounting should equal processed VAD samples"
        );
    }

    /// Run VAD on real WAV files and report segmentation quality.
    #[test]
    fn test_vad_supervisor_segments_real_audio() {
        let corpus_dir =
            std::path::PathBuf::from(shellexpand::tilde("~/.codescribe/transcriptions").as_ref());
        if !corpus_dir.exists() {
            eprintln!("Skipping: no transcriptions dir");
            return;
        }
        let model_path = vad::default_model_path();
        if !model_path.exists() {
            eprintln!("Skipping: no Silero model");
            return;
        }

        let edge_cases = [
            "192322_nie-zmienia-to_raw.wav",
            "133135_no-dobra-teraz_raw.wav",
            "182340_klaudiusz-zacznijmy-od_raw.wav",
            "001615_dziekuje---dziekuje_raw.wav",
            "184818_dzien-dobry-chcialem_raw.wav",
        ];

        let mut wavs: Vec<std::path::PathBuf> = Vec::new();
        if let Ok(dirs) = fs::read_dir(&corpus_dir) {
            for dir_entry in dirs.flatten() {
                if !dir_entry.path().is_dir() {
                    continue;
                }
                for case in &edge_cases {
                    let candidate = dir_entry.path().join(case);
                    if candidate.exists() {
                        wavs.push(candidate);
                    }
                }
            }
        }
        if wavs.is_empty() {
            let mut dirs: Vec<_> = fs::read_dir(&corpus_dir)
                .unwrap()
                .flatten()
                .filter(|e| e.path().is_dir())
                .collect();
            dirs.sort_by_key(|e| e.file_name());
            dirs.reverse();
            for dir in dirs.iter().take(2) {
                if let Ok(entries) = fs::read_dir(dir.path()) {
                    for entry in entries.flatten() {
                        let p = entry.path();
                        if p.extension().and_then(|s| s.to_str()) == Some("wav") {
                            let fname = p.file_name().unwrap_or_default().to_string_lossy();
                            // Terminal failed/no-speech artifacts should not be scored as VAD segmentation misses.
                            if is_terminal_no_speech_artifact(&fname) {
                                continue;
                            }
                            wavs.push(p);
                            if wavs.len() >= 5 {
                                break;
                            }
                        }
                    }
                }
            }
        }

        println!("\n╭─── VAD v5 Segmentation Test ───────────────────────╮");
        let mut all_pass = true;

        for wav_path in &wavs {
            let fname = wav_path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();
            let (samples, sample_rate) = match load_audio_file(wav_path) {
                Ok(v) => v,
                Err(e) => {
                    println!("│ SKIP {} — {}", fname, e);
                    continue;
                }
            };
            let audio_sec = samples.len() as f32 / sample_rate as f32;

            let vad_config = vad::VadConfig {
                threshold: 0.50,
                min_speech_duration_sec: 0.05,
                max_silence_duration_sec: 0.20,
                max_utterance_sec: 300.0,
                pre_roll_sec: 0.064,
                ..vad::VadConfig::default()
            };
            let mut silero = vad::SileroVad::new(&model_path, vad_config).expect("load Silero");
            let mut resampler = vad::Resampler::new(sample_rate);
            let samples_16k = resampler.resample(&samples);

            let mut above = 0usize;
            let mut total = 0usize;
            for chunk in samples_16k.chunks(vad::CHUNK_SIZE) {
                if chunk.len() < vad::CHUNK_SIZE {
                    break;
                }
                total += 1;
                if silero.predict(chunk).unwrap_or(0.0) >= 0.5 {
                    above += 1;
                }
            }

            let callback_size = 1024usize;
            let mut session = SpeechSession::new_utterance(sample_rate);
            let mut events = Vec::new();
            let mut offset = 0usize;
            while offset < samples.len() {
                let end = (offset + callback_size).min(samples.len());
                for event in session.feed(&samples[offset..end], sample_rate) {
                    events.push(event);
                }
                offset = end;
            }
            if let Some(event) = session.flush() {
                events.push(event);
            }

            let n_segments = events.len();
            let speech_samples: usize = events
                .iter()
                .map(|e| match e {
                    SpeechEvent::Utterance | SpeechEvent::UtteranceFinal => 0,
                    SpeechEvent::Chunk(s) => s.len(),
                })
                .sum();
            let speech_sec = speech_samples as f32 / sample_rate as f32;
            let silence_cut = audio_sec - speech_sec;
            let cut_pct = if audio_sec > 0.0 {
                silence_cut / audio_sec * 100.0
            } else {
                0.0
            };

            let raw_txt = wav_path.to_string_lossy().replace("_raw.wav", "_raw.txt");
            let old_len = fs::read_to_string(&raw_txt).map(|s| s.len()).unwrap_or(0);

            println!("│");
            println!("│ 📁 {}", fname);
            println!(
                "│    Audio: {:.1}s | VAD speech: {:.0}% ({}/{} frames)",
                audio_sec,
                if total > 0 {
                    above as f32 / total as f32 * 100.0
                } else {
                    0.0
                },
                above,
                total,
            );
            println!(
                "│    Segments: {} | Speech: {:.1}s | Silence cut: {:.1}s ({:.0}%)",
                n_segments, speech_sec, silence_cut, cut_pct,
            );
            println!("│    Old transcript: {} chars", old_len,);

            let old_text = fs::read_to_string(&raw_txt).unwrap_or_default();
            let halluc_count = old_text.matches("Thank you").count()
                + old_text.matches("Dziękuję.").count()
                + old_text.matches(".com/").count();
            if halluc_count > 2 {
                println!(
                    "│    ⚠ Old transcript had {} hallucination markers (Thank you/Dziękuję./.com/)",
                    halluc_count,
                );
                println!(
                    "│    ✅ VAD v5 would cut {:.1}s silence → these tails eliminated",
                    silence_cut,
                );
            }

            if above == 0 && audio_sec > 1.0 {
                println!("│    ❌ VAD detected NO speech — possible model issue");
                all_pass = false;
            }
        }

        println!("│");
        println!("╰────────────────────────────────────────────────────╯\n");

        assert!(all_pass, "Some files had zero speech detection");
    }
}

#[cfg(test)]
mod terminal_seal_refusal_tests {
    use super::TerminalSealRefused;
    use crate::pipeline::acoustic_ledger::{SealCoverageReceipt, SealCoverageStatus};

    fn refusal(audio_path: Option<std::path::PathBuf>) -> TerminalSealRefused {
        let receipt = SealCoverageReceipt {
            session_id: "e4060d87-fe0f-49fd-bbd5-eaea7e89ca17".to_string(),
            capture_epoch: 0,
            speech_samples: 2_696_704,
            covered_samples: 585_216,
            uncovered_speech_ranges: Vec::new(),
            max_uncovered_samples: 2_111_488,
            incomplete_threshold_samples: 12_000,
            status: SealCoverageStatus::Incomplete,
            speech_producer: "capture_energy".to_string(),
            availability: "observed".to_string(),
            observed_samples: Some(2_696_704),
        };
        let mut ledger = crate::pipeline::acoustic_ledger::AcousticLedger::new();
        assert!(ledger.record_seal_coverage(receipt.clone()));
        TerminalSealRefused {
            finality: ledger
                .terminal_finality(&receipt.session_id, receipt.capture_epoch)
                .into_refusal()
                .unwrap(),
            audio_path,
            committed_text: String::new(),
        }
    }

    /// The controller tells a refused seal apart from a failed mic by type,
    /// and the take WAV path survives the trip through `anyhow`.
    #[test]
    fn refusal_downcasts_through_anyhow_with_its_audio_path() {
        let path = std::path::PathBuf::from("/tmp/codescribe_recording_1788315408813.wav");
        let err = anyhow::Error::new(refusal(Some(path.clone())));
        let refused = err
            .downcast::<TerminalSealRefused>()
            .expect("typed refusal survives anyhow");
        assert_eq!(refused.audio_path.as_deref(), Some(path.as_path()));
        assert_eq!(
            refused.finality.coverage().unwrap().status,
            SealCoverageStatus::Incomplete
        );
    }

    /// The message names the refused seal, never the recorder.
    #[test]
    fn refusal_message_names_the_seal_not_the_mic() {
        let text = refusal(None).to_string();
        assert!(text.starts_with("terminal transcript refused"), "{text}");
        assert!(text.contains("585216/2696704"), "{text}");
        assert!(!text.to_lowercase().contains("recorder"), "{text}");
    }
}

/// W2 source contracts: UNRUN. Only capture/archive ingress is injected;
/// complete_stop is the same notification/join/drain/error path used by stop.
#[cfg(test)]
mod capture_stop_failure_tests {
    use super::*;
    use crate::pipeline::acoustic_ledger::SealCoverageStatus;

    fn recorder() -> StreamingRecorder {
        let mut recorder = StreamingRecorder::new().unwrap();
        recorder.authority_session_id = Some("capture-owner".into());
        recorder.capture_epoch = 7;
        recorder.captured_samples.store(4, Ordering::Relaxed);
        recorder.lifecycle_handle = Some(recorder_lifecycle_channel().0);
        recorder
    }

    #[tokio::test]
    async fn short_capture_without_ledger_speech_ends_cleanly() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("take.wav");
        write_wav(&path);
        let mut recorder = recorder();
        recorder.captured_samples.store(2_560, Ordering::Relaxed);
        let (text, audio) = recorder
            .complete_stop(Ok(Some(path.clone())))
            .await
            .unwrap();
        assert!(text.is_empty());
        assert_eq!(audio.as_deref(), Some(path.as_path()));
    }

    #[tokio::test]
    async fn last_window_ack_does_not_wait_for_slow_refinement_or_recovery() {
        let mut recorder = recorder();
        let (closed_tx, closed_rx) = oneshot::channel();
        recorder.last_window_closed = Some(closed_rx);
        let l1 =
            tokio::spawn(async { tokio::time::sleep(std::time::Duration::from_millis(650)).await });
        let recovery =
            tokio::spawn(async { tokio::time::sleep(std::time::Duration::from_millis(700)).await });
        let close_start = std::time::Instant::now();
        let (elapsed_tx, elapsed_rx) = oneshot::channel();
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(55)).await;
            elapsed_tx.send(close_start.elapsed().as_millis()).unwrap();
            closed_tx.send(()).unwrap();
        });
        assert!(
            recorder
                .wait_last_window_closed(std::time::Duration::from_secs(4))
                .await
        );
        let last_window_close_ms = elapsed_rx.await.unwrap();
        let stop_to_ack_ms = close_start.elapsed().as_millis();
        eprintln!("last_window_close_ms={last_window_close_ms} stop_to_ack_ms={stop_to_ack_ms}");
        assert!(stop_to_ack_ms < last_window_close_ms + 300);
        assert!(!l1.is_finished());
        assert!(!recovery.is_finished());
        l1.await.unwrap();
        recovery.await.unwrap();
    }

    #[tokio::test]
    async fn last_window_timeout_keeps_terminal_delivery_available() {
        let mut recorder = recorder();
        let (_closed_tx, closed_rx) = oneshot::channel();
        recorder.last_window_closed = Some(closed_rx);
        assert!(
            !recorder
                .wait_last_window_closed(std::time::Duration::from_millis(10))
                .await
        );
        assert!(recorder.last_window_closed.is_none());
    }

    fn write_wav(path: &std::path::Path) -> Vec<u8> {
        let mut writer = hound::WavWriter::create(
            path,
            hound::WavSpec {
                channels: 1,
                sample_rate: 16_000,
                bits_per_sample: 16,
                sample_format: hound::SampleFormat::Int,
            },
        )
        .unwrap();
        for sample in [123_i16, -456, 789, -321] {
            writer.write_sample(sample).unwrap();
        }
        writer.finalize().unwrap();
        std::fs::read(path).unwrap()
    }

    fn assert_released(recorder: &StreamingRecorder) {
        assert!(recorder.transcription_handle.is_none());
        assert!(recorder.terminal_audio_sender.is_none());
        assert!(recorder.lifecycle_handle.is_none());
        assert!(recorder.event_sink.is_none());
        assert!(!recorder.is_recording());
    }

    #[tokio::test(start_paused = true)]
    async fn saved_wav_survives_task_panic_with_exact_identity_and_cause() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("take.wav");
        let bytes = write_wav(&path);
        let mut recorder = recorder();
        recorder.sample_rate = 16_000;
        let sink = Arc::new(crate::pipeline::sinks::CollectorEventSink::new());
        let weak_sink = Arc::downgrade(&sink);
        recorder.set_event_sink(Some(sink));
        // Neither preview nor a raw buffer can become recovery transcript.
        *recorder.transcript_buffer.lock().await = "UNAUTHENTICATED PREVIEW".into();
        let (sender, receiver) = std::sync::mpsc::channel();
        recorder.terminal_audio_sender = Some(sender);
        recorder.transcription_handle = Some(tokio::spawn(async {
            panic!("controlled transcription failure");
        }));
        let error = recorder
            .complete_stop(Ok(Some(path.clone())))
            .await
            .unwrap_err();
        let failure = error.downcast_ref::<CaptureStopFailure>().unwrap();
        assert_eq!(failure.session_id.as_deref(), Some("capture-owner"));
        assert_eq!(failure.capture_epoch, 7);
        assert_eq!(failure.audio_path.as_deref(), Some(path.as_path()));
        assert!(
            failure
                .cause
                .downcast_ref::<tokio::task::JoinError>()
                .unwrap()
                .is_panic()
        );
        assert!(
            error
                .to_string()
                .contains("controlled transcription failure")
        );
        assert!(!format!("{error:?}").contains("UNAUTHENTICATED PREVIEW"));
        assert!(error.downcast_ref::<TerminalSealRefused>().is_none());
        let archive = receiver.try_recv().unwrap().unwrap();
        assert_eq!(archive.session_id, "capture-owner");
        assert_eq!(archive.capture_epoch, 7);
        assert_eq!(archive.path, path);
        assert_eq!(archive.sample_count, 4);
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
        assert_released(&recorder);
        assert!(weak_sink.upgrade().is_none());
    }

    #[tokio::test(start_paused = true)]
    async fn archive_failure_reaps_worker_and_preserves_both_errors_without_a_path() {
        let mut recorder = recorder();
        recorder.set_event_sink(Some(Arc::new(
            crate::pipeline::sinks::CollectorEventSink::new(),
        )));
        let (sender, receiver) = std::sync::mpsc::channel();
        recorder.terminal_audio_sender = Some(sender);
        recorder.transcription_handle = Some(tokio::spawn(async {
            panic!("secondary worker failure");
        }));
        let error = recorder
            .complete_stop(Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "archive denied",
            )
            .into()))
            .await
            .unwrap_err();
        let failure = error.downcast_ref::<CaptureStopFailure>().unwrap();
        assert_eq!(
            failure
                .cause
                .downcast_ref::<std::io::Error>()
                .unwrap()
                .kind(),
            std::io::ErrorKind::PermissionDenied
        );
        assert!(
            failure
                .task_failure
                .as_ref()
                .unwrap()
                .downcast_ref::<tokio::task::JoinError>()
                .unwrap()
                .is_panic()
        );
        assert!(failure.audio_path.is_none());
        assert!(
            receiver
                .try_recv()
                .unwrap()
                .unwrap_err()
                .contains("archive denied")
        );
        assert_released(&recorder);
    }

    #[tokio::test]
    async fn public_stop_routes_worker_failure_through_the_cleanup_tail() {
        let mut recorder = recorder();
        recorder.transcription_handle = Some(tokio::spawn(async {
            panic!("public stop worker failure");
        }));
        // A never-opened device returns no archive. This exercises public stop
        // without a microphone; the saved-WAV case injects archive ingress above.
        let error = recorder.stop().await.unwrap_err();
        let failure = error.downcast_ref::<CaptureStopFailure>().unwrap();
        assert!(failure.audio_path.is_none());
        assert_eq!(failure.session_id.as_deref(), Some("capture-owner"));
        assert_eq!(failure.capture_epoch, 7);
        assert!(
            failure
                .cause
                .downcast_ref::<tokio::task::JoinError>()
                .unwrap()
                .is_panic()
        );
        assert_released(&recorder);
    }

    #[tokio::test]
    async fn archive_failure_still_joins_a_successful_worker() {
        let mut recorder = recorder();
        let joined = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let completed = Arc::clone(&joined);
        recorder.transcription_handle = Some(tokio::spawn(async move {
            completed.store(true, Ordering::SeqCst);
        }));
        let error = recorder
            .complete_stop(Err(anyhow!("archive failed")))
            .await
            .unwrap_err();
        assert!(joined.load(Ordering::SeqCst));
        assert!(
            error
                .downcast_ref::<CaptureStopFailure>()
                .unwrap()
                .task_failure
                .is_none()
        );
        assert_released(&recorder);
    }

    #[tokio::test]
    async fn local_execution_join_survives_stop_retry_and_preserves_refused_wav() {
        use crate::pipeline::streaming::session::LocalExecutionOwner;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("owned-local.wav");
        let bytes = write_wav(&path);
        let mut recorder = recorder();
        let mut ledger = AcousticLedger::new();
        let receipt = SealCoverageReceipt {
            session_id: "capture-owner".into(),
            capture_epoch: 7,
            speech_samples: 4,
            covered_samples: 0,
            uncovered_speech_ranges: vec![crate::stt::tail_provider::TailSampleRange {
                session: "capture-owner".into(),
                capture_epoch: 7,
                sample_start: 0,
                sample_end: 4,
            }],
            max_uncovered_samples: 4,
            incomplete_threshold_samples: 1,
            status: SealCoverageStatus::Incomplete,
            speech_producer: "capture_energy".to_string(),
            availability: "observed".to_string(),
            observed_samples: Some(4),
        };
        assert!(ledger.record_seal_coverage(receipt.clone()));
        recorder.acoustic_ledger = Some(Arc::new(StdMutex::new(ledger)));
        let execution = Arc::new(LocalExecutionOwner::default());
        let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let result = execution
            .spawn(move |_| {
                entered_tx.send(()).unwrap();
                release_rx.recv().unwrap();
                Ok("late label")
            })
            .unwrap();
        entered_rx.await.unwrap();
        drop(result); // closed ledger no longer consumes local labels
        recorder.transcription_handle = Some(tokio::spawn(async move {
            execution.close_and_join().await;
        }));
        let pending = tokio::time::timeout(
            std::time::Duration::from_millis(20),
            recorder.complete_stop(Ok(Some(path.clone()))),
        )
        .await;
        let retained = recorder.transcription_handle.is_some();
        release_tx.send(()).unwrap();
        let error = recorder
            .complete_stop(Ok(Some(path.clone())))
            .await
            .unwrap_err();
        assert!(
            pending.is_err(),
            "Stop must await actual retained execution"
        );
        assert!(
            retained,
            "caller expiry must leave the session handle available for retry"
        );
        let refused = error.downcast_ref::<TerminalSealRefused>().unwrap();
        assert_eq!(refused.finality.coverage(), Some(&receipt));
        assert_eq!(refused.audio_path.as_ref(), Some(&path));
        assert_eq!(std::fs::read(path).unwrap(), bytes);
        assert_released(&recorder);
    }

    #[tokio::test]
    async fn clean_stop_and_seal_refusal_keep_their_existing_outcomes() {
        let mut recorder = recorder();
        recorder.captured_samples.store(0, Ordering::Relaxed);
        let stopped = recorder.complete_stop(Ok(None)).await.unwrap();
        assert_eq!(stopped, (String::new(), None));
        let mut ledger = AcousticLedger::new();
        assert!(ledger.record_seal_coverage(SealCoverageReceipt {
            session_id: "capture-owner".into(),
            capture_epoch: 7,
            speech_samples: 100,
            covered_samples: 0,
            uncovered_speech_ranges: Vec::new(),
            max_uncovered_samples: 100,
            incomplete_threshold_samples: 10,
            status: SealCoverageStatus::Incomplete,
            speech_producer: "capture_energy".to_string(),
            availability: "observed".to_string(),
            observed_samples: Some(100),
        }));
        recorder.acoustic_ledger = Some(Arc::new(StdMutex::new(ledger)));
        let error = recorder.complete_stop(Ok(None)).await.unwrap_err();
        assert!(error.downcast_ref::<TerminalSealRefused>().is_some());
        assert!(error.downcast_ref::<CaptureStopFailure>().is_none());
        assert_released(&recorder);
    }

    fn stop_finality_ledger(issued: bool, silence: bool) -> AcousticLedger {
        stop_finality_ledger_with_label(issued, silence, true)
    }

    fn stop_finality_ledger_with_label(issued: bool, silence: bool, whole: bool) -> AcousticLedger {
        use crate::audio::capture_receipt::{
            AcousticAvailability, AcousticSpeechEvidence, CaptureEvidenceIdentity,
        };
        use crate::pipeline::acoustic_ledger::{
            AcousticEvidence, EnergyCalibration, ObservationIdentity, ObservationProducer,
            OccurrenceIdentity,
        };
        let mut ledger = AcousticLedger::new();
        let occurrence = OccurrenceIdentity::new("capture-owner", 7, 0, 4);
        if !silence {
            let calibration = EnergyCalibration::new("stop-synthetic", 1.0, 1);
            assert!(
                ledger
                    .qualify(
                        &AcousticEvidence {
                            occurrence: occurrence.clone(),
                            duration_ms: 1.0,
                            energy_integral: 100.0,
                            mean_rms_dbfs: -20.0,
                            peak_dbfs: -10.0,
                            vad_open_sample: Some(0),
                            vad_close_sample: Some(4),
                            evidence_calibration_version: calibration.version.clone(),
                        },
                        &calibration
                    )
                    .is_qualified()
            );
            ledger.schedule_frontier(occurrence.clone(), [ObservationProducer::Whisper]);
            ledger.admit(
                &ObservationIdentity::new(
                    ObservationProducer::Whisper,
                    1,
                    0,
                    if whole {
                        occurrence.clone()
                    } else {
                        OccurrenceIdentity::new("capture-owner", 7, 0, 2)
                    },
                ),
                "committed words",
            );
            ledger.note_frontier_return(&occurrence, ObservationProducer::Whisper);
            if whole {
                ledger.seal(&occurrence).unwrap();
            }
        }
        let speech = AcousticSpeechEvidence::measured(
            CaptureEvidenceIdentity::new("capture-owner", 7),
            "synthetic_stop_observer",
            AcousticAvailability::Observed {
                observed_samples: 4,
            },
            if silence {
                vec![]
            } else {
                vec![crate::stt::tail_provider::TailSampleRange {
                    session: "capture-owner".into(),
                    capture_epoch: 7,
                    sample_start: 0,
                    sample_end: if whole { 4 } else { 2 },
                }]
            },
        );
        ledger.record_seal_coverage(ledger.assess_seal_coverage("capture-owner", 7, &speech, 0));
        if issued {
            ledger.seal_terminal("capture-owner", 7).unwrap();
        }
        ledger
    }

    #[tokio::test]
    async fn complete_stop_refuses_label_missing_despite_complete_coverage() {
        use crate::pipeline::acoustic_ledger::SealRefusal;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("label-missing.wav");
        let bytes = write_wav(&path);
        let mut ledger = stop_finality_ledger_with_label(false, false, false);
        assert!(ledger.latest_seal_coverage().unwrap().status.is_complete());
        assert_eq!(
            ledger.seal_terminal("capture-owner", 7),
            Err(SealRefusal::LabelMissing)
        );
        let mut recorder = recorder();
        recorder.acoustic_ledger = Some(Arc::new(StdMutex::new(ledger)));
        *recorder.transcript_buffer.lock().await = "committed words".into();
        let error = recorder
            .complete_stop(Ok(Some(path.clone())))
            .await
            .unwrap_err();
        let refusal = error.downcast_ref::<TerminalSealRefused>().unwrap();
        assert!(refusal.finality.coverage().unwrap().status.is_complete());
        assert_eq!(refusal.audio_path.as_ref(), Some(&path));
        assert_eq!(std::fs::read(path).unwrap(), bytes);
    }

    #[tokio::test]
    async fn complete_stop_requires_issued_terminal_receipt_and_preserves_wav() {
        use crate::pipeline::acoustic_ledger::TerminalFinalityRefusalReason;
        for issued in [false, true] {
            let mut recorder = recorder();
            let ledger = Arc::new(StdMutex::new(stop_finality_ledger(issued, false)));
            recorder.acoustic_ledger = Some(ledger.clone());
            *recorder.transcript_buffer.lock().await = "committed words".into();
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("real-stop-tail.wav");
            let bytes = write_wav(&path);
            let result = recorder.complete_stop(Ok(Some(path.clone()))).await;
            if issued {
                assert_eq!(
                    result.unwrap(),
                    ("committed words".into(), Some(path.clone()))
                );
            } else {
                let error = result.unwrap_err();
                let refusal = error.downcast_ref::<TerminalSealRefused>().unwrap();
                assert_eq!(
                    refusal.finality.reason(),
                    TerminalFinalityRefusalReason::TerminalReceiptMissing
                );
                assert!(refusal.finality.coverage().unwrap().status.is_complete());
                assert_eq!(refusal.audio_path.as_ref(), Some(&path));
                assert_eq!(refusal.committed_text, "committed words");
                assert!(
                    ledger
                        .lock()
                        .unwrap()
                        .terminal_finality("capture-owner", 7)
                        .into_refusal()
                        .is_some(),
                    "Stop must not mint the missing terminal receipt"
                );
            }
            assert_eq!(std::fs::read(&path).unwrap(), bytes);
            assert_released(&recorder);
        }
    }

    #[tokio::test]
    async fn complete_stop_empty_text_is_not_proof_of_silence_or_authority() {
        for has_measurement in [false, true] {
            for text in ["", "unattributed words"] {
                let mut recorder = recorder();
                *recorder.transcript_buffer.lock().await = text.into();
                if has_measurement {
                    recorder.acoustic_ledger =
                        Some(Arc::new(StdMutex::new(stop_finality_ledger(false, true))));
                }
                let result = recorder.complete_stop(Ok(None)).await;
                if has_measurement && text.is_empty() {
                    assert_eq!(result.unwrap(), (String::new(), None));
                } else {
                    assert!(result.unwrap_err().is::<CaptureStopFailure>());
                }
                assert_released(&recorder);
            }
        }
        let mut recorder = recorder();
        recorder.acoustic_ledger =
            Some(Arc::new(StdMutex::new(stop_finality_ledger(false, false))));
        assert!(
            recorder
                .complete_stop(Ok(None))
                .await
                .unwrap_err()
                .is::<TerminalSealRefused>(),
            "empty rendering cannot erase measured speech"
        );
    }

    #[tokio::test]
    async fn complete_stop_foreign_epoch_seal_cannot_authorize_current_capture() {
        let mut recorder = recorder();
        recorder.capture_epoch = 8;
        recorder.acoustic_ledger = Some(Arc::new(StdMutex::new(stop_finality_ledger(true, false))));
        *recorder.transcript_buffer.lock().await = "committed words".into();
        let error = recorder.complete_stop(Ok(None)).await.unwrap_err();
        let refusal = error.downcast_ref::<TerminalSealRefused>().unwrap();
        assert_eq!(refusal.finality.capture_epoch(), 8);
        assert!(refusal.finality.coverage().is_none());
        assert_released(&recorder);
    }

    /// Missing acoustic measurement refuses the terminal transcript on the same
    /// typed path as measured uncovered speech — and the committed words and
    /// the take WAV survive it. A guard pinned to `Incomplete` alone would let
    /// this outcome through as a success and lose the words.
    #[tokio::test]
    async fn unavailable_measurement_refuses_through_the_same_typed_path() {
        use crate::pipeline::acoustic_ledger::AcousticEvidenceGap;

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("unavailable.wav");
        let bytes = write_wav(&path);
        let mut recorder = recorder();
        *recorder.transcript_buffer.lock().await = "słowa które przetrwały".to_string();
        let mut ledger = AcousticLedger::new();
        let receipt = SealCoverageReceipt {
            session_id: "capture-owner".into(),
            capture_epoch: 7,
            speech_samples: 0,
            covered_samples: 0,
            uncovered_speech_ranges: Vec::new(),
            max_uncovered_samples: 0,
            incomplete_threshold_samples: 4_000,
            status: SealCoverageStatus::Unavailable(AcousticEvidenceGap::NotObserved),
            speech_producer: "capture_energy".to_string(),
            availability: "not_observed".to_string(),
            observed_samples: None,
        };
        assert!(ledger.record_seal_coverage(receipt.clone()));
        recorder.acoustic_ledger = Some(Arc::new(StdMutex::new(ledger)));

        let error = recorder
            .complete_stop(Ok(Some(path.clone())))
            .await
            .unwrap_err();
        let refused = error
            .downcast_ref::<TerminalSealRefused>()
            .expect("unavailable measurement is a typed seal refusal");
        assert_eq!(refused.finality.coverage(), Some(&receipt));
        assert!(!refused.finality.coverage().unwrap().status.is_complete());
        assert_eq!(refused.committed_text, "słowa które przetrwały");
        assert_eq!(refused.audio_path.as_ref(), Some(&path));
        assert_eq!(std::fs::read(path).unwrap(), bytes);
        assert_released(&recorder);
    }
}
