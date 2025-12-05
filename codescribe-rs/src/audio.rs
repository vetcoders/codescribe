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
//       // Record for 3 seconds (or until silence detected)
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
// configuration via environment variables:
//   - SILENCE_DB: silence threshold in dB (default: -45.0)
//   - SILENCE_HANG_SEC: silence duration before auto-stop (default: 0.8)
//   - AUTO_SILENCE: enable/disable silence detection (default: true)

use anyhow::{Context, Result};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{Device, Stream, StreamConfig};
use hound::{WavSpec, WavWriter};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc;
use tracing::{debug, error, info, warn};

// --- constants ---

/// Sample rate (samples per second)
/// 16kHz is standard for Whisper
const SAMPLE_RATE: u32 = 16000;

/// Number of channels (1 for mono)
const CHANNELS: u16 = 1;

/// Silence threshold in dB (runtime override: SILENCE_DB env var)
/// RMS values below this are considered silence.
/// Adjust this based on microphone sensitivity and background noise.
const DEFAULT_SILENCE_DB: f32 = -45.0;

/// Silence duration threshold (seconds) (runtime override: SILENCE_HANG_SEC env var)
/// Recording stops automatically after this duration of continuous silence.
const DEFAULT_HANG_SEC: f32 = 0.8;

/// Size of audio chunks to read from stream (samples)
const BLOCK_SIZE: usize = 1024;

// --- configuration ---

#[derive(Debug, Clone)]
pub struct RecorderConfig {
    /// Sample rate (Hz)
    pub sample_rate: u32,
    /// Number of audio channels
    pub channels: u16,
    /// Silence threshold in dB
    pub silence_db: f32,
    /// Hang time - silence duration before auto-stop (seconds)
    pub hang_sec: f32,
    /// Enable automatic silence detection
    pub auto_silence: bool,
    /// Block size for audio chunks
    pub block_size: usize,
}

impl Default for RecorderConfig {
    fn default() -> Self {
        Self {
            sample_rate: SAMPLE_RATE,
            channels: CHANNELS,
            silence_db: std::env::var("SILENCE_DB")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(DEFAULT_SILENCE_DB),
            hang_sec: std::env::var("SILENCE_HANG_SEC")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(DEFAULT_HANG_SEC),
            auto_silence: std::env::var("AUTO_SILENCE")
                .map(|v| !matches!(v.to_lowercase().as_str(), "0" | "false" | "no" | "off"))
                .unwrap_or(true),
            block_size: BLOCK_SIZE,
        }
    }
}

// --- diagnostics ---

#[derive(Debug, Default, Clone)]
pub struct RecorderDiagnostics {
    pub frames: usize,
    pub bytes: usize,
    pub chunks: usize,
    pub duration_sec: f32,
    pub snapshot_frames: usize,
    pub snapshot_bytes: usize,
}

impl RecorderDiagnostics {
    pub fn as_json(&self) -> serde_json::Value {
        serde_json::json!({
            "frames": self.frames,
            "bytes": self.bytes,
            "chunks": self.chunks,
            "duration_sec": (self.duration_sec * 1000.0).round() / 1000.0,
            "snapshot_frames": self.snapshot_frames,
            "snapshot_bytes": self.snapshot_bytes,
        })
    }
}

// --- audio buffer ---

type AudioBuffer = Arc<Mutex<Vec<i16>>>;

// --- recorder ---

pub struct Recorder {
    config: RecorderConfig,
    buffer: AudioBuffer,
    stream: Option<Stream>,
    device: Option<Device>,
    is_recording: Arc<AtomicBool>,
    stop_tx: Option<mpsc::Sender<()>>,
    last_duration: f32,
    diagnostics: RecorderDiagnostics,
}

// Safety: Recorder can be sent between threads because:
// - AudioBuffer (Arc<Mutex<Vec<i16>>>) is Send
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
            if let Ok(name) = device.name() {
                info!("Default input device: {}", name);
            }
        } else {
            warn!("No default input device found");
        }

        Ok(Self {
            config,
            buffer: Arc::new(Mutex::new(Vec::new())),
            stream: None,
            device: None,
            is_recording: Arc::new(AtomicBool::new(false)),
            stop_tx: None,
            last_duration: 0.0,
            diagnostics: RecorderDiagnostics::default(),
        })
    }

    /// Starts the audio recording process.
    ///
    /// Clears the buffer, creates and starts a new input stream,
    /// and launches the asynchronous collection task to read audio data.
    pub async fn start(&mut self) -> Result<()> {
        if self.is_recording.load(Ordering::SeqCst) {
            anyhow::bail!("Recording is already in progress");
        }

        info!("Starting recording...");

        // Clear buffer and reset diagnostics
        self.buffer.lock().unwrap().clear();
        self.diagnostics = RecorderDiagnostics::default();

        // Get default input device
        let host = cpal::default_host();
        let device = host
            .default_input_device()
            .context("No input device available")?;

        let device_name = device.name().unwrap_or_else(|_| "Unknown".to_string());
        info!("Using input device: {}", device_name);

        // Get supported config
        let supported_config = device
            .default_input_config()
            .context("Failed to get default input config")?;

        // Build stream config
        let stream_config = StreamConfig {
            channels: self.config.channels,
            sample_rate: cpal::SampleRate(self.config.sample_rate),
            buffer_size: cpal::BufferSize::Fixed(self.config.block_size as u32),
        };

        info!(
            "Audio stream config: {:?} (supported: {:?})",
            stream_config, supported_config
        );

        // Create channel for stopping
        let (stop_tx, mut stop_rx) = mpsc::channel::<()>(1);
        self.stop_tx = Some(stop_tx);

        // Setup stream callback
        let buffer = Arc::clone(&self.buffer);
        let is_recording_data = Arc::clone(&self.is_recording);
        let is_recording_error = Arc::clone(&self.is_recording);
        let silence_db = self.config.silence_db;
        let hang_sec = self.config.hang_sec;
        let auto_silence = self.config.auto_silence;
        let sample_rate = self.config.sample_rate;

        let silent_frames = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let silent_frames_clone = Arc::clone(&silent_frames);

        let stream = device
            .build_input_stream(
                &stream_config,
                move |data: &[i16], _: &cpal::InputCallbackInfo| {
                    // Append data to buffer
                    if let Ok(mut buf) = buffer.lock() {
                        buf.extend_from_slice(data);
                    }

                    // Calculate RMS in dBFS
                    let rms_amplitude = calculate_rms(data);
                    let rms_db = 20.0 * (rms_amplitude + 1e-9).log10();

                    if auto_silence {
                        // Check for silence
                        if rms_db < silence_db {
                            silent_frames_clone.fetch_add(data.len(), Ordering::SeqCst);
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

        self.device = None;
        self.is_recording.store(false, Ordering::SeqCst);

        // Get buffer data
        let wav_data = {
            let buf = self.buffer.lock().unwrap();
            if buf.is_empty() {
                warn!("No audio data captured");
                self.last_duration = 0.0;
                return Ok(None);
            }
            buf.clone()
        };

        let num_frames = wav_data.len();
        self.last_duration = num_frames as f32 / self.config.sample_rate as f32;
        self.diagnostics.frames = num_frames;
        self.diagnostics.bytes = num_frames * std::mem::size_of::<i16>();
        self.diagnostics.duration_sec = self.last_duration;

        info!(
            "Captured audio: {} frames ({:.2}s)",
            num_frames, self.last_duration
        );

        // Create temp file
        let temp_path = std::env::temp_dir().join(format!(
            "codescribe_recording_{}.wav",
            chrono::Utc::now().timestamp_millis()
        ));

        info!("Saving audio to: {:?}", temp_path);

        // Write WAV file
        write_wav_file(&temp_path, &wav_data, &self.config)?;

        info!("Audio successfully saved to WAV file");

        // Clear buffer
        self.buffer.lock().unwrap().clear();

        Ok(Some(temp_path))
    }

    /// Write a point-in-time WAV snapshot of the buffered audio.
    ///
    /// Does not stop the stream. Returns path to a temp WAV if enough audio is
    /// buffered (min_seconds), otherwise returns None. Intended for live
    /// chunking while recording (e.g., HOLD streaming).
    pub fn snapshot_wav(&mut self, min_seconds: f32) -> Result<Option<PathBuf>> {
        let buf = self.buffer.lock().unwrap();

        if buf.is_empty() {
            return Ok(None);
        }

        let total_frames = buf.len();
        let min_frames = (self.config.sample_rate as f32 * min_seconds) as usize;

        if total_frames < min_frames {
            return Ok(None);
        }

        let wav_data = buf.clone();
        drop(buf); // Release lock

        // Create temp file
        let temp_path = std::env::temp_dir().join(format!(
            "codescribe_snapshot_{}.wav",
            chrono::Utc::now().timestamp_millis()
        ));

        // Write WAV file
        write_wav_file(&temp_path, &wav_data, &self.config)?;

        self.diagnostics.snapshot_frames = total_frames;
        self.diagnostics.snapshot_bytes = total_frames * std::mem::size_of::<i16>();

        debug!(
            "Snapshot saved: {} frames ({:.2}s) to {:?}",
            total_frames,
            total_frames as f32 / self.config.sample_rate as f32,
            temp_path
        );

        Ok(Some(temp_path))
    }

    /// Returns the duration in seconds of the most recent recording.
    pub fn last_duration(&self) -> f32 {
        self.last_duration
    }

    /// Returns diagnostics for the most recent recording.
    pub fn diagnostics(&self) -> &RecorderDiagnostics {
        &self.diagnostics
    }

    /// Returns true if currently recording.
    pub fn is_recording(&self) -> bool {
        self.is_recording.load(Ordering::SeqCst)
    }
}

impl Default for Recorder {
    fn default() -> Self {
        Self::new().expect("Failed to create default recorder")
    }
}

impl Drop for Recorder {
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

/// Calculate RMS (Root Mean Square) amplitude of audio samples.
fn calculate_rms(samples: &[i16]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }

    let sum_squares: f64 = samples
        .iter()
        .map(|&s| {
            let normalized = s as f64 / i16::MAX as f64;
            normalized * normalized
        })
        .sum();

    (sum_squares / samples.len() as f64).sqrt() as f32
}

/// Write audio samples to a WAV file.
fn write_wav_file(path: &PathBuf, samples: &[i16], config: &RecorderConfig) -> Result<()> {
    let spec = WavSpec {
        channels: config.channels,
        sample_rate: config.sample_rate,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_calculate_rms() {
        let samples = vec![0i16, 1000, -1000, 500, -500];
        let rms = calculate_rms(&samples);
        assert!(rms > 0.0);
        assert!(rms < 1.0);
    }

    #[test]
    fn test_calculate_rms_empty() {
        let samples: Vec<i16> = vec![];
        let rms = calculate_rms(&samples);
        assert_eq!(rms, 0.0);
    }

    #[test]
    fn test_recorder_config_default() {
        let config = RecorderConfig::default();
        assert_eq!(config.sample_rate, SAMPLE_RATE);
        assert_eq!(config.channels, CHANNELS);
        assert!(config.auto_silence);
    }

    #[test]
    fn test_recorder_config_from_env() {
        std::env::set_var("SILENCE_DB", "-50.0");
        std::env::set_var("SILENCE_HANG_SEC", "1.5");
        std::env::set_var("AUTO_SILENCE", "0");

        let config = RecorderConfig::default();
        assert_eq!(config.silence_db, -50.0);
        assert_eq!(config.hang_sec, 1.5);
        assert!(!config.auto_silence);

        // Cleanup
        std::env::remove_var("SILENCE_DB");
        std::env::remove_var("SILENCE_HANG_SEC");
        std::env::remove_var("AUTO_SILENCE");
    }

    #[tokio::test]
    async fn test_recorder_new() {
        let recorder = Recorder::new();
        assert!(recorder.is_ok());

        let recorder = recorder.unwrap();
        assert!(!recorder.is_recording());
        assert_eq!(recorder.last_duration(), 0.0);
    }
}
