// audio.rs
//
// purpose: handles audio recording from the default microphone. includes
//          functionality for starting/stopping recording, detecting periods
//          of silence to automatically stop recording, and saving the captured
//          audio to a temporary wav file suitable for transcription.
//
// dependencies: cpal (audio i/o)
//               hound (saving audio to .wav format)
//               tokio (async runtime for collection loop)
//
// key components: Recorder struct
//                 RecorderConfig (configurable parameters)
//                 start method (initializes and starts the audio stream)
//                 collect method (async task to read audio chunks, detect silence)
//                 stop method (stops stream, saves buffer to temp wav file)
//                 snapshot_wav method (save current buffer without stopping)
//
// design rationale: uses cpal for cross-platform audio input. silence detection
//                   is based on root mean square (rms) of audio chunks compared
//                   to a db threshold. tokio is used for async collection to avoid
//                   blocking. saving to a temp file simplifies passing audio data
//                   to the transcription api.
//
// usage example:
//   ```rust
//   use codescribe::Recorder;
//   use std::time::Duration;
//
//   #[tokio::main]
//   async fn main() -> anyhow::Result<()> {
//       // Create recorder with default config (16kHz mono, -45dB silence threshold)
//       let mut recorder = Recorder::new()?;
//
//       // Start recording
//       recorder.start().await?;
//       println!("Recording... speak now!");
//
//       // Record for 3 seconds (or until silence detected when enabled)
//       tokio::time::sleep(Duration::from_secs(3)).await;
//
//       // Stop and save to WAV file
//       if let Some(path) = recorder.stop().await? {
//           println!("Recorded to: {:?}", path);
//           println!("Duration: {:.2}s", recorder.last_duration());
//       } else {
//           println!("No audio captured");
//       }
//
//       Ok(())
//   }
//   ```
//
// configuration via constants in `core/vad/config.rs`:
//   - speech probability threshold (Silero default: 0.5)
//   - silence duration before auto-stop (Silero default profile)
//   - AUTO_SILENCE: enable/disable silence detection (default: false)

use crate::vad;
use anyhow::{Context, Result};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{Device, Stream, StreamConfig};
use hound::{WavSpec, WavWriter};
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc;
use tracing::{debug, error, info, warn};

// ═══════════════════════════════════════════════════════════
// RecorderVad — local VAD for auto-silence detection
// ═══════════════════════════════════════════════════════════

/// Recorder-local VAD: dedicated thread with AccumulatingVad.
///
/// Replaces the broken global VadWorker. Fixes:
/// - Initial probability 0.0 (not 1.0) → seen_speech gate works
/// - Proper sample accumulation → Silero actually processes audio
/// - Locally owned → dies with the Recorder
struct RecorderVad {
    tx: Option<std::sync::mpsc::SyncSender<Vec<f32>>>,
    last_prob: Arc<AtomicU32>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl RecorderVad {
    /// Spawn the VAD thread and return a handle, or `None` if it cannot start.
    ///
    /// `None` is a soft failure: auto-silence is lost but recording continues.
    /// The probability starts at 0.0 (no speech seen), which is what makes the
    /// `seen_speech` gate in the capture callback work — a 1.0 start would let
    /// initial silence trip the auto-stop before the user says anything.
    fn new(sample_rate: u32) -> Option<Self> {
        let (tx, rx) = std::sync::mpsc::sync_channel::<Vec<f32>>(128);
        // 0.0 = no speech seen yet (the critical fix)
        let last_prob = Arc::new(AtomicU32::new(0.0_f32.to_bits()));
        let writer = Arc::clone(&last_prob);

        let handle = std::thread::Builder::new()
            .name("recorder-vad".into())
            .spawn(move || {
                // Embedded Silero VAD — zero I/O, never fails on fresh machines.
                let mut acc_vad = match vad::AccumulatingVad::new(sample_rate) {
                    Ok(v) => v,
                    Err(e) => {
                        warn!("RecorderVad: Silero embedded load failed: {e}");
                        return;
                    }
                };
                info!("RecorderVad ready (sample_rate={sample_rate}Hz, embedded)");
                for samples in rx {
                    let prob = acc_vad.feed(&samples);
                    writer.store(prob.to_bits(), Ordering::Release);
                }
                debug!("RecorderVad thread exiting (channel closed)");
            })
            .ok()?;

        Some(Self {
            tx: Some(tx),
            last_prob,
            thread: Some(handle),
        })
    }
}

impl Drop for RecorderVad {
    /// Close the sample channel *before* joining the worker.
    ///
    /// The worker loops over the receiver, so joining first would deadlock: the
    /// loop only ends once the sender is gone.
    fn drop(&mut self) {
        // Close channel first so the worker loop can exit before join().
        drop(self.tx.take());
        if let Some(handle) = self.thread.take()
            && handle.join().is_err()
        {
            warn!("RecorderVad thread panicked during shutdown");
        }
    }
}

// --- constants ---

/// Sample rate (samples per second)
/// 16kHz is standard for Whisper
const SAMPLE_RATE: u32 = 16000;

/// Number of channels (1 for mono)
const CHANNELS: u16 = 1;

/// Speech probability threshold (0.0-1.0) for VAD
/// Below this is considered silence. Default: 0.5
/// Silence duration threshold (seconds)
/// Recording stops automatically after this duration of continuous silence.
/// Synced with `vad::config::SILERO_DEFAULT_MAX_SILENCE_SEC`.
/// Size of audio chunks to read from stream (samples)
const BLOCK_SIZE: usize = 1024;

/// Retain at most this much mono i16 audio when a streaming callback consumes
/// the live samples. Non-streaming recorder usage still keeps the full take.
const STREAMING_BUFFER_CAP_SECONDS: usize = 300;

// --- configuration ---

/// Tunables for a recording session.
///
/// `sample_rate` is a request, not a guarantee: the stream is opened at the
/// device's native rate, and [`Recorder::actual_sample_rate`] reports what was
/// actually used.
#[derive(Debug, Clone)]
pub struct RecorderConfig {
    /// Sample rate (Hz)
    pub sample_rate: u32,
    /// Number of audio channels
    pub channels: u16,
    /// Speech probability threshold (0.0-1.0) for VAD
    /// Below this is considered silence
    pub speech_threshold: f32,
    /// Hang time - silence duration before auto-stop (seconds)
    pub hang_sec: f32,
    /// Enable automatic silence detection
    pub auto_silence: bool,
    /// Block size for audio chunks
    pub block_size: usize,
}

impl Default for RecorderConfig {
    /// Defaults derived from [`vad::VadConfig`], with auto-silence read from the
    /// `AUTO_SILENCE` environment variable.
    ///
    /// Deriving rather than duplicating keeps one source of truth for the VAD
    /// thresholds. Note this `Default` is environment-sensitive: two calls in
    /// the same process can differ if `AUTO_SILENCE` changes between them.
    fn default() -> Self {
        // Keep a single source of truth for VAD thresholds/silence durations.
        // VAD internals are hardcoded in `vad::VadConfig`.
        let vad_cfg = vad::VadConfig::default();
        Self {
            sample_rate: SAMPLE_RATE,
            channels: CHANNELS,
            speech_threshold: vad_cfg.threshold.clamp(0.1, 0.9),
            hang_sec: vad_cfg.max_silence_duration_sec.clamp(0.1, 10.0),
            auto_silence: auto_silence_from_env_value(
                std::env::var("AUTO_SILENCE").ok().as_deref(),
            ),
            block_size: BLOCK_SIZE,
        }
    }
}

// --- diagnostics ---

/// Counters for the most recently completed take, filled in by
/// [`Recorder::stop`].
#[derive(Debug, Default, Clone)]
pub struct RecorderDiagnostics {
    pub frames: usize,
    pub bytes: usize,
    pub duration_sec: f32,
}

/// Outcome of a non-stopping buffer snapshot. Recording stream remains active.
///
/// Caller stores `end_offset` and passes it as `from_offset` in the next
/// `snapshot_wav` call to get the *next* segment without overlap.
#[derive(Debug, Clone)]
pub struct SnapshotResult {
    /// Temp WAV file path containing the segment's samples.
    pub wav_path: PathBuf,
    /// Sample-index marking the end of the snapshot (exclusive). Use as
    /// `from_offset` for the next call.
    pub end_offset: usize,
    /// Number of samples copied into the snapshot.
    pub sample_count: usize,
    /// Audio duration of the snapshot in seconds.
    pub duration_sec: f32,
}

// --- audio buffer ---

/// Shared capture buffer: mono i16 samples, written from the CoreAudio callback.
///
/// A `VecDeque` because streaming sessions evict from the front once the
/// retention cap is reached.
type AudioBuffer = Arc<Mutex<VecDeque<i16>>>;

/// Per-block tap on the live capture stream, receiving mono f32 samples.
///
/// Invoked on the CoreAudio callback thread — anything slow or blocking here
/// shows up as dropped audio.
pub type AudioCallback = Box<dyn Fn(&[f32]) + Send + Sync + 'static>;

// --- recorder ---

/// Microphone capture session over cpal.
///
/// Owns the input stream, the shared sample buffer and — when auto-silence is
/// enabled — a [`RecorderVad`] that dies with the recorder. Samples accumulate
/// until [`Recorder::stop`] writes them to a temp WAV;
/// [`Recorder::snapshot_wav`] can slice the buffer without interrupting
/// capture.
/// Resolve the input device exactly as a recording start would: an
/// `AUDIO_INPUT_DEVICE` exact/substring match wins, otherwise the system
/// default. Shared by [`Recorder::start`] and [`probe_input_capture_path`].
fn select_input_device(host: &cpal::Host) -> Result<(Device, String)> {
    let preferred = std::env::var("AUDIO_INPUT_DEVICE")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());

    let device = if let Some(preferred) = preferred {
        let devices = host
            .input_devices()
            .context("Failed to enumerate input devices")?;

        let mut selected: Option<Device> = None;
        for d in devices {
            if let Ok(desc) = d.description() {
                let name = desc.to_string();
                if name == preferred || name.to_lowercase().contains(&preferred.to_lowercase()) {
                    selected = Some(d);
                    break;
                }
            }
        }

        selected
            .or_else(|| host.default_input_device())
            .context("No input device available")?
    } else {
        host.default_input_device()
            .context("No input device available")?
    };

    let device_name = device
        .description()
        .map(|d| d.to_string())
        .unwrap_or_else(|_| "Unknown".to_string());
    Ok((device, device_name))
}

/// Identity of the capture path a recording would open right now — device
/// name, native sample rate, native channels — without opening a stream or
/// prompting for permission. The admission gate resolves the calibration
/// profile from this before any microphone is touched.
pub fn probe_input_capture_path() -> Result<crate::audio::capture_receipt::CapturePathMeta> {
    let host = cpal::default_host();
    let (device, device_name) = select_input_device(&host)?;
    let supported = device
        .default_input_config()
        .context("Failed to get default input config")?;
    Ok(crate::audio::capture_receipt::CapturePathMeta {
        device_name,
        sample_rate: supported.sample_rate(),
        channels: supported.channels().max(1),
    })
}

pub struct Recorder {
    pub config: RecorderConfig,
    buffer: AudioBuffer,
    buffer_start_offset: Arc<AtomicUsize>,
    stream: Option<Stream>,
    device: Option<Device>,
    is_recording: Arc<AtomicBool>,
    stop_tx: Option<mpsc::Sender<()>>,
    last_duration: f32,
    diagnostics: RecorderDiagnostics,
    /// Actual sample rate used for recording (may differ from config)
    actual_sample_rate: u32,
    /// Last resolved input device name (empty until `start`).
    last_input_device: String,
    /// Native channel count of the last opened stream (1 after downmix).
    last_native_channels: u16,
    on_data: Option<AudioCallback>,
    /// Disk spill of the full streaming take (operator decision B): survives
    /// the RAM ring cap; `None` when disabled or not a streaming session.
    spill: Option<SpillSink>,
    /// Callback invoked when VAD (silence detection) stops recording
    on_vad_stop: Option<Arc<dyn Fn() + Send + Sync>>,
    /// Local VAD for auto-silence (replaces global VadWorker)
    recorder_vad: Option<RecorderVad>,
}

// Safety: Recorder can be sent between threads because:
// - AudioBuffer (Arc<Mutex<VecDeque<i16>>>) is Send
// - Stream operations are thread-safe (internally uses Arc)
// - All other fields are Send
unsafe impl Send for Recorder {}

impl Recorder {
    /// Initializes the recorder with default configuration and no active stream.
    pub fn new() -> Result<Self> {
        Self::with_config(RecorderConfig::default())
    }

    /// Initializes the recorder with custom configuration.
    pub fn with_config(config: RecorderConfig) -> Result<Self> {
        info!("Recorder initialized with config: {:?}", config);

        // Query default input device at initialization
        let host = cpal::default_host();
        if let Some(device) = host.default_input_device() {
            if let Ok(desc) = device.description() {
                info!("Default input device: {}", desc);
            }
        } else {
            warn!("No default input device found");
        }

        Ok(Self {
            config: config.clone(),
            buffer: Arc::new(Mutex::new(VecDeque::new())),
            buffer_start_offset: Arc::new(AtomicUsize::new(0)),
            stream: None,
            device: None,
            is_recording: Arc::new(AtomicBool::new(false)),
            stop_tx: None,
            last_duration: 0.0,
            diagnostics: RecorderDiagnostics::default(),
            actual_sample_rate: config.sample_rate, // Will be updated in start()
            last_input_device: String::new(),
            last_native_channels: 1,
            on_data: None,
            on_vad_stop: None,
            recorder_vad: None,
            spill: None,
        })
    }

    /// Set a callback to receive raw audio data (f32 samples)
    pub fn set_callback(&mut self, callback: AudioCallback) {
        self.on_data = Some(callback);
    }

    /// Warn when a VAD-stop callback is registered while auto-silence is off.
    ///
    /// The registration is kept, not rejected — but it can never fire, and a
    /// silent no-op callback is the kind of thing that reads as a broken
    /// recorder rather than a disabled feature.
    fn warn_inert_vad_stop_callback(&self, context: &str) {
        if self.on_vad_stop.is_some() && !self.config.auto_silence {
            warn!(
                "{context}: VAD stop callback is registered but auto_silence is disabled; callback will remain inert until auto_silence is enabled"
            );
        }
    }

    /// Set a callback invoked when VAD (silence detection) stops recording
    pub fn set_on_vad_stop<F>(&mut self, callback: F)
    where
        F: Fn() + Send + Sync + 'static,
    {
        self.on_vad_stop = Some(Arc::new(callback));
    }

    /// Actual sample rate used by the underlying input stream.
    ///
    /// Note: This may differ from `config.sample_rate` because we always open
    /// the device stream at its native rate for compatibility.
    pub fn actual_sample_rate(&self) -> u32 {
        self.actual_sample_rate
    }

    /// Device name resolved for the last `start()`, if any.
    pub fn last_input_device(&self) -> Option<&str> {
        let name = self.last_input_device.trim();
        if name.is_empty() { None } else { Some(name) }
    }

    /// Native channel count of the last opened input stream.
    pub fn last_native_channels(&self) -> u16 {
        self.last_native_channels.max(1)
    }

    /// Returns true when the recorder still has an active stream/session.
    ///
    /// This is used by higher-level state recovery to detect desyncs where the
    /// controller thinks it is idle but CoreAudio is still running.
    pub fn is_active(&self) -> bool {
        self.is_recording.load(Ordering::SeqCst) || self.stream.is_some()
    }

    /// Starts the audio recording process.
    ///
    /// Clears the buffer, creates and starts a new input stream,
    /// and launches the asynchronous collection task to read audio data.
    pub async fn start(&mut self) -> Result<()> {
        let is_recording = self.is_recording.load(Ordering::SeqCst);
        let has_stream = self.stream.is_some();

        // Self-heal stale atomic flag observed after abrupt stop races:
        // if there is no live stream, we can safely clear the flag and continue.
        if is_recording && !has_stream {
            warn!(
                "Recorder start: stale is_recording flag detected without active stream; clearing"
            );
            self.is_recording.store(false, Ordering::SeqCst);
        }

        if self.is_recording.load(Ordering::SeqCst) || self.stream.is_some() {
            anyhow::bail!("Recording is already in progress");
        }

        info!("Starting recording...");

        // Clear buffer and reset diagnostics
        self.buffer
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clear();
        self.buffer_start_offset.store(0, Ordering::SeqCst);
        self.diagnostics = RecorderDiagnostics::default();
        self.warn_inert_vad_stop_callback("Recorder start");

        // Select input device — the same policy the admission probe uses, so
        // readiness can never disagree with the device a take would open.
        let host = cpal::default_host();
        let (device, device_name) = select_input_device(&host)?;
        info!("Using input device: {}", device_name);
        self.last_input_device = device_name;

        // Get supported config
        let supported_config = device
            .default_input_config()
            .context("Failed to get default input config")?;

        // Use the device's native sample rate for compatibility
        // (backend will handle resampling if needed)
        let native_sample_rate = supported_config.sample_rate();
        let native_channels = supported_config.channels().max(1);
        self.last_native_channels = native_channels;

        // Build stream config using native sample rate/channel count. The
        // callback downmixes interleaved native channels to mono for downstream.
        let stream_config = StreamConfig {
            channels: native_channels,
            sample_rate: native_sample_rate,
            buffer_size: cpal::BufferSize::Default, // Let system choose buffer size
        };

        info!(
            "Audio stream config: {:?} (native rate: {}Hz, native channels: {})",
            stream_config, native_sample_rate, native_channels
        );

        // Store actual sample rate for WAV file and duration calculations
        self.actual_sample_rate = native_sample_rate;

        // Use actual sample rate for silence detection calculations
        let sample_rate = native_sample_rate;

        // Create local VAD for auto-silence (replaces global VadWorker)
        self.recorder_vad = if self.config.auto_silence {
            RecorderVad::new(native_sample_rate)
        } else {
            None
        };

        // Create channel for stopping
        let (stop_tx, mut stop_rx) = mpsc::channel::<()>(1);
        self.stop_tx = Some(stop_tx);

        // Setup stream callback
        let buffer = Arc::clone(&self.buffer);
        let buffer_start_offset = Arc::clone(&self.buffer_start_offset);
        let is_recording_data = Arc::clone(&self.is_recording);
        let is_recording_error = Arc::clone(&self.is_recording);
        let speech_threshold = self.config.speech_threshold;
        let hang_sec = self.config.hang_sec;
        let auto_silence = self.config.auto_silence;
        let native_channels = usize::from(native_channels);

        let silent_frames = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let silent_frames_clone = Arc::clone(&silent_frames);
        let seen_speech = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let seen_speech_clone = Arc::clone(&seen_speech);
        let has_streaming_callback = self.on_data.is_some();
        let on_data = self.on_data.take();
        let buffer_cap_samples =
            has_streaming_callback.then(|| streaming_buffer_cap_samples(native_sample_rate));

        // Disk spill (operator decision B): streaming sessions keep the FULL
        // take on disk while the RAM ring stays capped for live consumers.
        self.spill = None;
        let spill_tx = if has_streaming_callback
            && audio_spill_from_env_value(std::env::var("CODESCRIBE_AUDIO_SPILL").ok().as_deref())
        {
            match SpillSink::spawn(native_sample_rate, &std::env::temp_dir()) {
                Ok(sink) => {
                    let tx = sink.sender();
                    self.spill = Some(sink);
                    info!(
                        "Audio spill armed: full take survives the {}s ring cap",
                        STREAMING_BUFFER_CAP_SECONDS
                    );
                    tx
                }
                Err(e) => {
                    warn!("Audio spill unavailable, ring-only session: {e}");
                    None
                }
            }
        } else {
            None
        };

        // Share local VAD with audio callback (RecorderVad is Send via channel+atomic)
        let vad_prob = self
            .recorder_vad
            .as_ref()
            .map(|rv| Arc::clone(&rv.last_prob));
        let vad_tx: Option<std::sync::mpsc::SyncSender<Vec<f32>>> = self
            .recorder_vad
            .as_ref()
            .and_then(|rv| rv.tx.as_ref().cloned());

        let stream = device
            .build_input_stream(
                &stream_config,
                move |data: &[f32], _: &cpal::InputCallbackInfo| {
                    let mono_samples = downmix_to_mono(data, native_channels);

                    // Send data to callback if present
                    if let Some(ref cb) = on_data {
                        cb(&mono_samples);
                    }

                    // Convert once; the ring and the disk spill share the samples.
                    let mono_i16 = convert_mono_f32_to_i16(&mono_samples);
                    if let Some(ref tx) = spill_tx {
                        // Send-only on the audio thread; the writer thread does the I/O.
                        let _ = tx.send(mono_i16.clone());
                    }
                    if let Ok(mut buf) = buffer.lock() {
                        append_mono_i16_samples(
                            &mut buf,
                            &buffer_start_offset,
                            &mono_i16,
                            buffer_cap_samples,
                        );
                    }

                    // Feed audio to local VAD (non-blocking)
                    if let Some(ref tx) = vad_tx {
                        let _ = tx.try_send(mono_samples.clone());
                    }

                    // Read latest speech probability from local VAD
                    let speech_prob = vad_prob
                        .as_ref()
                        .map(|p| f32::from_bits(p.load(Ordering::Acquire)))
                        .unwrap_or(0.0);
                    let is_silence = speech_prob < speech_threshold;

                    // Only check silence if still recording (avoid spam after stop)
                    if auto_silence && is_recording_data.load(Ordering::SeqCst) {
                        // Gate: don't auto-stop until we've seen *any* speech since start.
                        //
                        // Without this, hands-off "toggle" recording can stop before the user
                        // starts speaking (initial silence > hang_sec).
                        if !is_silence {
                            seen_speech_clone.store(true, Ordering::SeqCst);
                        }

                        // Check for silence using VAD (only after speech has been observed)
                        if seen_speech_clone.load(Ordering::SeqCst) {
                            if is_silence {
                                silent_frames_clone.fetch_add(mono_samples.len(), Ordering::SeqCst);
                            } else {
                                silent_frames_clone.store(0, Ordering::SeqCst);
                            }
                        } else {
                            silent_frames_clone.store(0, Ordering::SeqCst);
                        }

                        // Check if silence duration exceeds hang time
                        let current_silent = silent_frames_clone.load(Ordering::SeqCst);
                        let silent_duration = current_silent as f32 / sample_rate as f32;
                        if silent_duration > hang_sec {
                            info!(
                                "Silence detected for > {:.2}s. Stopping collection.",
                                hang_sec
                            );
                            is_recording_data.store(false, Ordering::SeqCst);
                        }
                    }
                },
                move |err| {
                    error!("Audio stream error: {}", err);
                    is_recording_error.store(false, Ordering::SeqCst);
                },
                None, // timeout
            )
            .context("Failed to build input stream")?;

        // Start the stream
        stream.play().context("Failed to start audio stream")?;
        self.is_recording.store(true, Ordering::SeqCst);
        info!("Audio stream started");

        // Store stream and device
        self.stream = Some(stream);
        self.device = Some(device);

        // Spawn monitoring task
        let is_recording_clone = Arc::clone(&self.is_recording);
        let stop_tx_clone = self.stop_tx.clone();
        let on_vad_stop_clone = self.on_vad_stop.clone();
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = stop_rx.recv() => {
                        debug!("Stop signal received");
                        break;
                    }
                    _ = tokio::time::sleep(tokio::time::Duration::from_millis(100)) => {
                        if !is_recording_clone.load(Ordering::SeqCst) {
                            debug!("Recording stopped by silence detection");
                            let callback_registered = on_vad_stop_clone.is_some();
                            if should_invoke_vad_stop_callback(auto_silence, callback_registered) {
                                if let Some(ref callback) = on_vad_stop_clone {
                                    info!("VAD triggered - invoking callback");
                                    callback();
                                }
                            } else if callback_registered && !auto_silence {
                                debug!(
                                    "Recording stopped while auto_silence is disabled; VAD callback remains inert"
                                );
                            }
                            if let Some(tx) = stop_tx_clone.as_ref() {
                                let _ = tx.send(()).await;
                            }
                            break;
                        }
                    }
                }
            }
        });

        Ok(())
    }

    /// Stops the audio recording and saves the buffer to a temp WAV file.
    ///
    /// Stops and closes the audio stream, concatenates the buffered audio chunks,
    /// and writes them to a temporary .wav file.
    ///
    /// Returns the absolute path to the saved .wav file, or None if no audio
    /// was recorded or an error occurred.
    pub async fn stop(&mut self) -> Result<Option<PathBuf>> {
        if !self.is_recording.load(Ordering::SeqCst) && self.stream.is_none() {
            warn!("Stop called but no active stream");
            self.last_duration = 0.0;
            return Ok(None);
        }

        info!("Stopping recording...");

        // Signal stop
        if let Some(tx) = self.stop_tx.take() {
            let _ = tx.send(()).await;
        }

        // Stop stream
        if let Some(stream) = self.stream.take() {
            drop(stream); // Dropping the stream stops it
            info!("Audio stream stopped");
        }

        // Ensure VAD worker is shut down deterministically on every stop().
        self.recorder_vad = None;
        self.device = None;
        self.is_recording.store(false, Ordering::SeqCst);

        // The stream is gone, so every callback-held spill sender is dropped —
        // finalize returns the COMPLETE take (immune to the ring cap).
        let spill_take = self.spill.take().and_then(SpillSink::finalize);
        if let Some((path, samples)) = spill_take
            && samples > 0
        {
            self.last_duration = samples as f32 / self.actual_sample_rate as f32;
            self.diagnostics.frames = samples;
            self.diagnostics.bytes = samples * std::mem::size_of::<i16>();
            self.diagnostics.duration_sec = self.last_duration;
            info!(
                "Captured audio (spill, full take): {} frames ({:.2}s) at {}Hz",
                samples, self.last_duration, self.actual_sample_rate
            );
            info!("Audio successfully saved to WAV file");
            self.buffer
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .clear();
            self.buffer_start_offset.store(0, Ordering::SeqCst);
            return Ok(Some(path));
        }

        // Get buffer data
        let wav_data = {
            let buf = self.buffer.lock().unwrap_or_else(|e| e.into_inner());
            if buf.is_empty() {
                warn!("No audio data captured");
                self.last_duration = 0.0;
                self.buffer_start_offset.store(0, Ordering::SeqCst);
                return Ok(None);
            }
            buf.iter().copied().collect::<Vec<_>>()
        };

        let num_frames = wav_data.len();
        self.last_duration = num_frames as f32 / self.actual_sample_rate as f32;
        self.diagnostics.frames = num_frames;
        self.diagnostics.bytes = num_frames * std::mem::size_of::<i16>();
        self.diagnostics.duration_sec = self.last_duration;

        info!(
            "Captured audio: {} frames ({:.2}s) at {}Hz",
            num_frames, self.last_duration, self.actual_sample_rate
        );

        // Create temp file
        let temp_path = std::env::temp_dir().join(format!(
            "codescribe_recording_{}.wav",
            chrono::Utc::now().timestamp_millis()
        ));

        info!("Saving audio to: {:?}", temp_path);

        // Write WAV file using actual sample rate
        write_wav_file(&temp_path, &wav_data, self.actual_sample_rate, CHANNELS)?;

        info!("Audio successfully saved to WAV file");

        // Clear buffer
        self.buffer
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clear();
        self.buffer_start_offset.store(0, Ordering::SeqCst);

        Ok(Some(temp_path))
    }

    /// Snapshot a slice of the recording buffer to a WAV file WITHOUT stopping
    /// the recorder.
    ///
    /// The recording stream remains active, the VAD worker keeps running, and
    /// the underlying buffer is **not** cleared. `from_offset` is a sample index
    /// (not byte offset). Returns `Ok(None)` if there are no new samples since
    /// `from_offset` (caller should treat this as a graceful no-op).
    ///
    /// Callers persist the returned `end_offset` and pass it as `from_offset`
    /// in the next snapshot call to obtain the *next* non-overlapping segment.
    pub async fn snapshot_wav(&self, from_offset: usize) -> Result<Option<SnapshotResult>> {
        // Read slice under lock (cheap memcpy), drop lock before WAV write.
        let (slice, end_offset) = {
            let buf = self.buffer.lock().unwrap_or_else(|e| e.into_inner());
            let buffer_start = self.buffer_start_offset.load(Ordering::SeqCst);
            let total = buffer_start.saturating_add(buf.len());
            if from_offset >= total {
                return Ok(None);
            }
            let start_offset = from_offset.max(buffer_start);
            if from_offset < buffer_start {
                warn!(
                    "snapshot_wav requested from_offset={} older than retained buffer start {}; using retained start",
                    from_offset, buffer_start
                );
            }
            let start = start_offset - buffer_start;
            let slice: Vec<i16> = buf.iter().skip(start).copied().collect();
            (slice, total)
        };

        let sample_count = slice.len();
        let duration_sec = sample_count as f32 / self.actual_sample_rate as f32;

        let temp_path = std::env::temp_dir().join(format!(
            "codescribe_segment_{}.wav",
            chrono::Utc::now().timestamp_millis()
        ));

        write_wav_file(&temp_path, &slice, self.actual_sample_rate, CHANNELS)?;

        info!(
            "Segment snapshot: {} samples ({:.2}s) saved to {:?}",
            sample_count, duration_sec, temp_path
        );

        Ok(Some(SnapshotResult {
            wav_path: temp_path,
            end_offset,
            sample_count,
            duration_sec,
        }))
    }

    /// Current sample count in the buffer.
    ///
    /// Use as `from_offset` for the next `snapshot_wav` call when you want to
    /// start a fresh segment without saving anything yet. Returns 0 on poisoned
    /// lock (recoverable).
    pub fn current_sample_offset(&self) -> usize {
        let buf = self.buffer.lock().unwrap_or_else(|e| e.into_inner());
        self.buffer_start_offset
            .load(Ordering::SeqCst)
            .saturating_add(buf.len())
    }
}

impl Default for Recorder {
    /// # Panics
    ///
    /// Panics if the recorder cannot be constructed. Prefer [`Recorder::new`],
    /// which surfaces the same failure as a `Result`.
    fn default() -> Self {
        Self::new().expect("Failed to create default recorder")
    }
}

impl Drop for Recorder {
    /// Tear down the input stream if the caller never reached
    /// [`Recorder::stop`].
    ///
    /// Defensive only: it releases the device, but the buffered audio is
    /// discarded rather than written to a WAV.
    fn drop(&mut self) {
        // Defensive cleanup: ensure stream is stopped when Recorder is dropped
        if self.stream.is_some() {
            self.is_recording.store(false, Ordering::SeqCst);
            self.stream = None;
            debug!("Recorder::drop - cleaned up audio stream");
        }
    }
}

// --- helper functions ---

// Note: RMS-based silence detection replaced with Silero VAD (see vad module)

/// Retention cap in samples for a streaming session
/// (`STREAMING_BUFFER_CAP_SECONDS` at `sample_rate`).
///
/// Only applied when a streaming callback is consuming the audio live; a plain
/// recorder keeps the whole take so `stop()` can write all of it.
fn streaming_buffer_cap_samples(sample_rate: u32) -> usize {
    (sample_rate as usize).saturating_mul(STREAMING_BUFFER_CAP_SECONDS)
}

/// Parse `CODESCRIBE_AUDIO_SPILL`: ON by default; only `0`/`false`/`no`/`off`
/// (case-insensitive) opt out.
///
/// Default ON because the operator decision (2026-08-10, option B) is that a
/// streaming session must never lose audio to the RAM ring cap — a >5 min
/// phone-call take archived without its head is worse than a temp file.
fn audio_spill_from_env_value(value: Option<&str>) -> bool {
    value
        .map(|v| !matches!(v.to_lowercase().as_str(), "0" | "false" | "no" | "off"))
        .unwrap_or(true)
}

/// Disk spill for a streaming session: every captured chunk is forwarded to a
/// dedicated writer thread over a channel (the CoreAudio callback only clones
/// and sends — no disk I/O on the audio thread) and lands in a WAV via
/// incremental `hound` writes. `finalize()` closes the writer and returns the
/// COMPLETE take, immune to [`STREAMING_BUFFER_CAP_SECONDS`] eviction.
struct SpillSink {
    tx: Option<std::sync::mpsc::Sender<Vec<i16>>>,
    handle: Option<std::thread::JoinHandle<Result<(PathBuf, usize)>>>,
}

impl SpillSink {
    /// Open the spill WAV in `dir` and start the writer thread.
    fn spawn(sample_rate: u32, dir: &std::path::Path) -> Result<Self> {
        let path = dir.join(format!(
            "codescribe_recording_{}.wav",
            chrono::Utc::now().timestamp_millis()
        ));
        let spec = hound::WavSpec {
            channels: CHANNELS,
            sample_rate,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut writer = hound::WavWriter::create(&path, spec)
            .map_err(|e| anyhow::anyhow!("spill wav create {}: {e}", path.display()))?;
        let (tx, rx) = std::sync::mpsc::channel::<Vec<i16>>();
        let handle = std::thread::Builder::new()
            .name("audio-spill".into())
            .spawn(move || -> Result<(PathBuf, usize)> {
                let mut written = 0usize;
                while let Ok(chunk) = rx.recv() {
                    for sample in &chunk {
                        writer
                            .write_sample(*sample)
                            .map_err(|e| anyhow::anyhow!("spill wav write: {e}"))?;
                    }
                    written += chunk.len();
                }
                writer
                    .finalize()
                    .map_err(|e| anyhow::anyhow!("spill wav finalize: {e}"))?;
                Ok((path, written))
            })
            .map_err(|e| anyhow::anyhow!("spill thread spawn: {e}"))?;
        Ok(Self {
            tx: Some(tx),
            handle: Some(handle),
        })
    }

    /// Clone of the sender for the capture callback (send-only, non-blocking).
    fn sender(&self) -> Option<std::sync::mpsc::Sender<Vec<i16>>> {
        self.tx.clone()
    }

    /// Feed one mono i16 chunk. Test-only surface: the live path sends
    /// through [`Self::sender`] clones from the capture callback.
    #[cfg(test)]
    fn feed(&self, chunk: Vec<i16>) {
        if let Some(tx) = &self.tx {
            let _ = tx.send(chunk);
        }
    }

    /// Close the channel, join the writer, return the finalized take.
    /// `None` means the spill failed — callers fall back to the RAM ring.
    fn finalize(mut self) -> Option<(PathBuf, usize)> {
        self.tx.take(); // drop our sender; callback senders die with the stream
        let handle = self.handle.take()?;
        match handle.join() {
            Ok(Ok(result)) => Some(result),
            Ok(Err(e)) => {
                warn!("Audio spill failed during write/finalize: {e}");
                None
            }
            Err(_) => {
                warn!("Audio spill writer thread panicked");
                None
            }
        }
    }
}

/// Parse `AUTO_SILENCE`: absent is off, and only `0`/`false`/`no`/`off`
/// (case-insensitive) disable it once set.
///
/// Off by default because auto-stop cutting someone off mid-sentence is worse
/// than making them stop the recording themselves.
fn auto_silence_from_env_value(value: Option<&str>) -> bool {
    value
        .map(|v| !matches!(v.to_lowercase().as_str(), "0" | "false" | "no" | "off"))
        .unwrap_or(false)
}

/// The VAD-stop emission gate, extracted so the policy is testable without a
/// live audio device: both the feature and a registered callback are required.
fn should_invoke_vad_stop_callback(auto_silence: bool, callback_registered: bool) -> bool {
    auto_silence && callback_registered
}

/// Average interleaved frames down to mono.
///
/// The device is opened at its native channel count, but VAD, Whisper and the
/// WAV writer are all mono, so this is the single conversion point. A trailing
/// partial frame is averaged over the samples it actually has, and mono input
/// short-circuits without copying per frame.
fn downmix_to_mono(samples: &[f32], channels: usize) -> Vec<f32> {
    let channels = channels.max(1);
    if channels == 1 {
        return samples.to_vec();
    }

    samples
        .chunks(channels)
        .map(|frame| frame.iter().copied().sum::<f32>() / frame.len() as f32)
        .collect()
}

/// Convert mono f32 samples to i16 and append them, evicting from the front
/// once `cap_samples` is exceeded.
///
/// Samples are clamped to `[-1.0, 1.0]` before scaling, so an out-of-range block
/// saturates instead of wrapping to the opposite polarity. `buffer_start_offset`
/// advances by exactly the number of evicted samples, which is what keeps
/// [`Recorder::snapshot_wav`] offsets absolute across a trimmed buffer.
fn convert_mono_f32_to_i16(samples: &[f32]) -> Vec<i16> {
    samples
        .iter()
        .map(|sample| {
            let clamped = sample.clamp(-1.0, 1.0);
            (clamped * i16::MAX as f32) as i16
        })
        .collect()
}

fn append_mono_i16_samples(
    buffer: &mut VecDeque<i16>,
    buffer_start_offset: &AtomicUsize,
    samples: &[i16],
    cap_samples: Option<usize>,
) {
    buffer.extend(samples.iter().copied());

    if let Some(cap_samples) = cap_samples
        && buffer.len() > cap_samples
    {
        let drop_count = buffer.len() - cap_samples;
        for _ in 0..drop_count {
            buffer.pop_front();
        }
        buffer_start_offset.fetch_add(drop_count, Ordering::SeqCst);
    }
}

/// Write audio samples to a WAV file.
fn write_wav_file(path: &PathBuf, samples: &[i16], sample_rate: u32, channels: u16) -> Result<()> {
    let spec = WavSpec {
        channels,
        sample_rate,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };

    let mut writer = WavWriter::create(path, spec)
        .with_context(|| format!("Failed to create WAV file at {:?}", path))?;

    for &sample in samples {
        writer
            .write_sample(sample)
            .context("Failed to write sample to WAV file")?;
    }

    writer.finalize().context("Failed to finalize WAV file")?;

    Ok(())
}

/// Recorded length of a WAV file in seconds, read from its header.
///
/// Header-only: `hound` parses the `fmt `/`data` chunk sizes and reports the
/// frame count without decoding a single sample, so this is cheap enough to sit
/// on a latency-sensitive stop path. `duration()` counts frames per channel,
/// which is what "seconds of audio" means for both the mono capture path here
/// and any multi-channel file that reaches it.
///
/// Returns `None` on an unreadable or truncated header and on a zero sample
/// rate. Failure is never reported as `0.0`: a caller measuring speech density
/// must be able to tell "I could not measure this audio" apart from "this audio
/// is empty", because the two demand opposite decisions.
pub fn wav_duration_secs(path: &Path) -> Option<f32> {
    let reader = hound::WavReader::open(path).ok()?;
    let sample_rate = reader.spec().sample_rate;
    if sample_rate == 0 {
        return None;
    }
    Some(reader.duration() as f32 / sample_rate as f32)
}

/// Recorder defaults, auto-silence gating, streaming buffer cap, and downmix.
#[cfg(test)]
mod tests {
    use super::*;

    // Note: RMS tests removed - now using Silero VAD (see vad module tests)

    /// The stop-path density guard divides by this number, so it must come from
    /// the header exactly — and a file the probe cannot parse must report `None`
    /// rather than a plausible `0.0`, which would read as total starvation.
    #[test]
    fn wav_duration_secs_reads_header_and_refuses_unreadable_files() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("take.wav");
        let rate = 16_000u32;
        // 2.5 s of mono silence: duration must be frames/rate, not file size.
        let samples = vec![0i16; rate as usize * 5 / 2];
        write_wav_file(&path, &samples, rate, 1).expect("write wav");

        let secs = wav_duration_secs(&path).expect("written header must be readable");
        assert!((secs - 2.5).abs() < 1e-3, "expected 2.5 s, got {secs}");

        let garbage = dir.path().join("not-a-wav.bin");
        std::fs::write(&garbage, b"definitely not RIFF").expect("write garbage");
        assert_eq!(
            wav_duration_secs(&garbage),
            None,
            "an unparseable header must not report a duration"
        );
        assert_eq!(wav_duration_secs(&dir.path().join("missing.wav")), None);
    }

    /// Operator decision B (2026-08-10): the RAM ring caps at
    /// STREAMING_BUFFER_CAP_SECONDS, so a phone-call take longer than 5 min
    /// lost its head in the archived WAV. The disk spill must retain EVERY
    /// sample of the session regardless of the ring cap.
    #[test]
    fn spill_sink_retains_full_take_beyond_streaming_cap() {
        let dir = tempfile::tempdir().expect("tempdir");
        let rate = 1_000u32; // 1 kHz keeps the test fast; cap math is per-second
        let sink = SpillSink::spawn(rate, dir.path()).expect("spawn spill sink");
        let chunk = vec![7i16; rate as usize]; // 1 s per chunk
        let seconds = STREAMING_BUFFER_CAP_SECONDS + 5;
        for _ in 0..seconds {
            sink.feed(chunk.clone());
        }
        let (path, samples) = sink.finalize().expect("spill finalize");
        assert_eq!(samples, seconds * rate as usize, "spill must evict nothing");
        let reader = hound::WavReader::open(&path).expect("open spilled wav");
        assert_eq!(reader.len() as usize, seconds * rate as usize);
        assert_eq!(reader.spec().sample_rate, rate);
        assert_eq!(reader.spec().channels, 1);
    }

    /// Sample values survive the spill byte-exact (no resample, no transcode).
    #[test]
    fn spill_sink_roundtrips_sample_values() {
        let dir = tempfile::tempdir().expect("tempdir");
        let sink = SpillSink::spawn(16_000, dir.path()).expect("spawn spill sink");
        let pattern: Vec<i16> = vec![i16::MIN, -1, 0, 1, i16::MAX, 1234, -4321];
        sink.feed(pattern.clone());
        let (path, samples) = sink.finalize().expect("spill finalize");
        assert_eq!(samples, pattern.len());
        let read: Vec<i16> = hound::WavReader::open(&path)
            .expect("open spilled wav")
            .into_samples::<i16>()
            .map(|s| s.expect("sample"))
            .collect();
        assert_eq!(read, pattern);
    }

    /// Spill defaults ON for streaming sessions; only explicit falsey opts out.
    #[test]
    fn audio_spill_env_parser_defaults_on() {
        assert!(audio_spill_from_env_value(None));
        for disabled in ["0", "false", "no", "off", "OFF"] {
            assert!(!audio_spill_from_env_value(Some(disabled)));
        }
        assert!(audio_spill_from_env_value(Some("1")));
    }

    /// Default config tracks Silero hang_sec; SAMPLE_RATE/CHANNELS stay 16k mono.
    #[test]
    fn test_recorder_config_default() {
        let config = RecorderConfig::default();

        assert_eq!(SAMPLE_RATE, 16000);
        assert_eq!(CHANNELS, 1);
        assert!(
            (config.hang_sec - vad::config::SILERO_DEFAULT_MAX_SILENCE_SEC).abs() < f32::EPSILON,
            "RecorderConfig hang_sec should follow vad::config::SILERO_DEFAULT_MAX_SILENCE_SEC"
        );
    }

    /// Absent/falsey env → off; any other non-empty value opts auto-silence in.
    #[test]
    fn auto_silence_env_parser_defaults_off_and_honors_opt_in() {
        assert!(
            !auto_silence_from_env_value(None),
            "AUTO_SILENCE absent means manual stop unless explicitly enabled"
        );
        for disabled in ["0", "false", "no", "off", "OFF"] {
            assert!(
                !auto_silence_from_env_value(Some(disabled)),
                "{disabled} should disable auto-silence"
            );
        }
        for enabled in ["1", "true", "yes", "on", "anything-else"] {
            assert!(
                auto_silence_from_env_value(Some(enabled)),
                "{enabled} should enable auto-silence"
            );
        }
    }

    /// Callback registration survives when auto_silence is false (emission gated).
    #[test]
    fn vad_stop_callback_is_retained_but_inert_when_auto_silence_disabled() {
        let config = RecorderConfig {
            auto_silence: false,
            ..Default::default()
        };
        let mut recorder = Recorder::with_config(config).expect("recorder should initialize");

        recorder.set_on_vad_stop(|| {});

        assert!(
            recorder.on_vad_stop.is_some(),
            "disabled auto_silence should retain registration while emission remains gated"
        );
    }

    /// With auto_silence on, set_on_vad_stop arms the retained callback slot.
    #[test]
    fn vad_stop_callback_is_armed_when_auto_silence_enabled() {
        let config = RecorderConfig {
            auto_silence: true,
            ..Default::default()
        };
        let mut recorder = Recorder::with_config(config).expect("recorder should initialize");

        recorder.set_on_vad_stop(|| {});

        assert!(
            recorder.on_vad_stop.is_some(),
            "enabled auto_silence should arm the VAD-stop callback"
        );
    }

    /// Emission needs both auto_silence and a registered callback (AND gate).
    #[test]
    fn vad_stop_callback_emission_is_gated_by_auto_silence() {
        assert!(
            !should_invoke_vad_stop_callback(false, true),
            "registered callbacks must stay inert while auto_silence is disabled"
        );
        assert!(
            !should_invoke_vad_stop_callback(true, false),
            "auto_silence alone is not enough without a registered callback"
        );
        assert!(
            should_invoke_vad_stop_callback(true, true),
            "auto_silence can invoke a registered VAD-stop callback"
        );
    }

    /// Cap drops oldest i16 samples and advances absolute start offset.
    #[test]
    fn streaming_i16_buffer_is_capped_and_tracks_absolute_offset() {
        let mut buf = VecDeque::new();
        let start_offset = std::sync::atomic::AtomicUsize::new(0);

        let first = convert_mono_f32_to_i16(&[0.10, 0.20, 0.30, 0.40]);
        let second = convert_mono_f32_to_i16(&[0.50, 0.60, 0.70, 0.80]);
        append_mono_i16_samples(&mut buf, &start_offset, &first, Some(6));
        append_mono_i16_samples(&mut buf, &start_offset, &second, Some(6));

        assert_eq!(buf.len(), 6, "streaming buffer must not grow past cap");
        let retained: Vec<i16> = buf.iter().copied().collect();
        assert_eq!(
            retained,
            vec![
                (0.30 * i16::MAX as f32) as i16,
                (0.40 * i16::MAX as f32) as i16,
                (0.50 * i16::MAX as f32) as i16,
                (0.60 * i16::MAX as f32) as i16,
                (0.70 * i16::MAX as f32) as i16,
                (0.80 * i16::MAX as f32) as i16,
            ],
            "streaming buffer must retain the newest samples after cap eviction"
        );
        assert_eq!(
            start_offset.load(Ordering::SeqCst),
            2,
            "absolute sample offset must advance by trimmed samples"
        );
    }

    /// Interleaved N-channel PCM averages to mono; mono input is identity.
    #[test]
    fn downmix_to_mono_averages_interleaved_native_channels() {
        let stereo = [1.0, -1.0, 0.25, 0.75, -0.50, -0.25];

        let mono = downmix_to_mono(&stereo, 2);

        assert_eq!(mono, vec![0.0, 0.5, -0.375]);

        let three_channel_with_tail = [1.0, 0.0, -1.0, 0.75, 0.0, 0.75, 0.25];
        let mono = downmix_to_mono(&three_channel_with_tail, 3);
        assert_eq!(mono, vec![0.0, 0.5, 0.25]);

        let already_mono = [-0.25, 0.5, 0.75];
        assert_eq!(downmix_to_mono(&already_mono, 1), already_mono);
    }

    /// Default `Recorder::new` constructs without error (device open deferred).
    #[tokio::test]
    async fn test_recorder_new() {
        let recorder = Recorder::new();
        assert!(recorder.is_ok());
    }
}
