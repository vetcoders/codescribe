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

use crate::asr_session::recorder::{
    Layer1Decision, RecorderLifecycleHandle, recorder_lifecycle_channel,
};
use crate::audio::recorder::{Recorder, RecorderConfig};
use crate::config::{RuntimeSettingsSnapshot, UserSettings};
use crate::pipeline::acoustic_ledger::{AcousticLedger, SealCoverageReceipt, SealCoverageStatus};
use crate::pipeline::contracts::{EngineEvent, EventSink};
use crate::pipeline::streaming::{
    SessionConfig, TailPatchSessionReceipt, collect_buffered_engine_events_with_config,
    stream_log_path, transcription_session,
};
use anyhow::{Context, Result, anyhow};
use std::sync::Arc;
use std::sync::Mutex as StdMutex;
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::sync::{Mutex, mpsc};
use tokio::task::JoinHandle;
use tracing::{debug, info, warn};

/// Ledger refusal of the terminal transcript after a successful capture stop.
///
/// Raised by [`StreamingRecorder::stop`] when the acoustic ledger reports
/// [`SealCoverageStatus::Incomplete`]: speech physically existed that no sealed
/// occurrence covers, so the take text may not be promoted to a terminal
/// transcript. The capture itself succeeded — `audio_path` is the take WAV
/// already written to disk — which is why this is a typed error rather than a
/// string: the stop path must retain that audio and close the take instead of
/// reporting a recorder failure.
#[derive(Debug, Clone)]
pub struct TerminalSealRefused {
    pub receipt: SealCoverageReceipt,
    pub audio_path: Option<std::path::PathBuf>,
}

impl std::fmt::Display for TerminalSealRefused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "terminal transcript refused: seal coverage incomplete ({}/{} samples covered; max gap {} > threshold {})",
            self.receipt.covered_samples,
            self.receipt.speech_samples,
            self.receipt.max_uncovered_samples,
            self.receipt.incomplete_threshold_samples,
        )
    }
}

impl std::error::Error for TerminalSealRefused {}

// Keep enough raw audio queued to survive a cold Whisper load without dropping
// the user's first words. The STT session drains this backlog once the model is ready.
/// Channel depth for cold Whisper load: first words queue instead of drop.
const AUDIO_BACKLOG_CHUNKS: usize = 2048;

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
    let layer1 = Layer1Decision::Disarmed;
    let layer1_armed = layer1.is_armed();
    // `transcription_session` has one live canvas route: Apple progressive.
    // Report the route we actually enter; never reconstruct it through the
    // deleted global engine selector.
    let streaming_engine_label = "live_apple".to_string();
    let utterance_silence_sec = settings.toggle_silence_sec.filter(|&sec| sec >= 0.5);
    let config = SessionConfig {
        session_id: uuid::Uuid::new_v4().to_string(),
        capture_epoch: 1,
        runtime_settings,
        acoustic_ledger: acoustic_ledger.clone(),
        sample_rate,
        capture_device_name: None,
        language,
        stream_log_path: None,
        utterance_silence_sec,
        layer1,
        lifecycle_events: None,
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
    /// Counter for audio chunks dropped due to channel backpressure.
    dropped_chunks: Arc<AtomicU64>,
    /// Sink used by `start_event_session`. Caller must configure it explicitly.
    event_sink: Option<Arc<dyn EventSink>>,
    /// Per-block input level tap: receives the RMS of every captured audio
    /// block (linear, 0..~1). Runs on the CoreAudio callback thread — keep it
    /// cheap and non-blocking (a broadcast send, an atomic store).
    level_callback: Option<Arc<dyn Fn(f32) + Send + Sync>>,
    /// Single-use Layer 1 decision consumed when the next session starts.
    layer1_decision: StdMutex<Layer1Decision>,
    /// O(1) host lifecycle signal for the currently active session.
    lifecycle_handle: Option<RecorderLifecycleHandle>,
    /// Session-frozen runtime truth. Set once by the controller before start.
    runtime_settings: Option<Arc<RuntimeSettingsSnapshot>>,
    /// The one ledger instance shared by PCM capture, engines, and reducer.
    acoustic_ledger: Option<Arc<StdMutex<AcousticLedger>>>,
    /// Controller-owned session identity bound with the ledger.
    authority_session_id: Option<String>,
    /// Last capture-open epoch issued for the currently bound session.
    /// Zero means this bind has not successfully opened capture yet.
    capture_epoch: u64,
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
            dropped_chunks: Arc::new(AtomicU64::new(0)),
            event_sink: None,
            level_callback: None,
            layer1_decision: StdMutex::new(Layer1Decision::Disarmed),
            lifecycle_handle: None,
            runtime_settings: None,
            acoustic_ledger: None,
            authority_session_id: None,
            capture_epoch: 0,
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
            dropped_chunks: Arc::new(AtomicU64::new(0)),
            event_sink: None,
            level_callback: None,
            layer1_decision: StdMutex::new(Layer1Decision::Disarmed),
            lifecycle_handle: None,
            runtime_settings: None,
            acoustic_ledger: None,
            authority_session_id: None,
            capture_epoch: 0,
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
        self.acoustic_ledger = Some(Arc::clone(&acoustic_ledger));
        acoustic_ledger
    }

    /// Borrow the ledger handle already bound for the next/active session.
    pub fn acoustic_ledger_handle(&self) -> Option<Arc<StdMutex<AcousticLedger>>> {
        self.acoustic_ledger.as_ref().map(Arc::clone)
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

        // Create channel for audio chunks. This is intentionally larger than a
        // normal live queue: cold STT initialization happens behind this buffer.
        let (tx, rx) = mpsc::channel::<Vec<f32>>(AUDIO_BACKLOG_CHUNKS);

        // Setup callback to send audio data
        let dropped = Arc::clone(&self.dropped_chunks);
        let level_callback = self.level_callback.clone();
        self.recorder.set_callback(Box::new(move |data| {
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

        let layer1 = std::mem::take(
            self.layer1_decision
                .get_mut()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        );
        let (lifecycle_handle, lifecycle_events) = recorder_lifecycle_channel();
        self.lifecycle_handle = Some(lifecycle_handle);
        self.transcription_handle = Some(tokio::spawn(async move {
            transcription_session(
                rx,
                event_sink,
                SessionConfig {
                    session_id,
                    capture_epoch: next_capture_epoch,
                    runtime_settings,
                    acoustic_ledger,
                    sample_rate: actual_sample_rate,
                    capture_device_name,
                    language,
                    stream_log_path: log_path,
                    utterance_silence_sec,
                    layer1,
                    lifecycle_events: Some(lifecycle_events),
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

        // 1. Stop recording (drops callback and sender)
        let audio_path = self.recorder.stop().await?;
        self.lifecycle_handle = None;

        // 2. Wait for worker to finish processing remaining chunks
        if let Some(handle) = self.transcription_handle.take() {
            debug!("Waiting for transcription session task to finish...");
            handle.await.context("Transcription session task failed")?;
        }

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

        let incomplete_coverage = self.acoustic_ledger.as_ref().and_then(|ledger| {
            ledger
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .latest_seal_coverage()
                .filter(|receipt| receipt.status == SealCoverageStatus::Incomplete)
                .cloned()
        });
        if let Some(receipt) = incomplete_coverage {
            // The ledger refused the terminal transcript, not the capture: the
            // mic is stopped and the take WAV is already on disk. Carry that
            // path in the typed refusal so the controller can retain the audio
            // and close the take without reading this as a recorder failure.
            return Err(anyhow::Error::new(TerminalSealRefused {
                receipt,
                audio_path,
            }));
        }

        // 4. Return collected transcript
        let transcript = self.transcript_buffer.lock().await.clone();
        Ok((transcript, audio_path))
    }

    /// Legacy alias for [`Self::stop_and_discard_path`].
    #[deprecated(note = "use stop_and_discard_path instead")]
    pub async fn stop_without_saving(&mut self) -> Result<String> {
        self.stop_and_discard_path().await
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
        TerminalSealRefused {
            receipt: SealCoverageReceipt {
                session_id: "e4060d87-fe0f-49fd-bbd5-eaea7e89ca17".to_string(),
                capture_epoch: 0,
                speech_samples: 2_696_704,
                covered_samples: 585_216,
                uncovered_speech_ranges: Vec::new(),
                max_uncovered_samples: 2_111_488,
                incomplete_threshold_samples: 12_000,
                status: SealCoverageStatus::Incomplete,
            },
            audio_path,
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
        assert_eq!(refused.receipt.status, SealCoverageStatus::Incomplete);
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
