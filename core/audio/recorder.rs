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
//                   uses Silero VAD neural network via centralized vad::VadConfig.
//                   tokio is used for async collection to avoid blocking.
//                   saving to a temp file simplifies passing audio data
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
// Configuration via environment variables (VAD segmentation lives in streaming recorder):
//   - CODESCRIBE_VAD_THRESHOLD: speech probability 0.0-1.0 (default: 0.35)
//   - CODESCRIBE_VAD_SILENCE_SEC: silence before utterance flush (default: 2.5s)
//   - (no VAD enable flag; segmentation is always handled by the streaming recorder)

use crate::vad;
use anyhow::{Context, Result};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{Device, Stream, StreamConfig};
use hound::{WavSpec, WavWriter};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tracing::{debug, error, info, warn};

// --- constants ---

/// Sample rate (samples per second)
/// 16kHz is standard for Whisper
const SAMPLE_RATE: u32 = 16000;

/// Number of channels (1 for mono)
const CHANNELS: u16 = 1;

// VAD defaults are sourced from vad::VadConfig - no local constants needed.
// See core/vad/config.rs for threshold (0.35) and max_silence (2.5s) defaults.

/// Size of audio chunks to read from stream (samples)
const BLOCK_SIZE: usize = 1024;

// --- configuration ---

#[derive(Debug, Clone)]
pub struct RecorderConfig {
    /// Sample rate (Hz)
    pub sample_rate: u32,
    /// Number of audio channels
    pub channels: u16,
    /// Block size for audio chunks
    pub block_size: usize,
}

impl Default for RecorderConfig {
    fn default() -> Self {
        Self {
            sample_rate: SAMPLE_RATE,
            channels: CHANNELS,
            block_size: BLOCK_SIZE,
        }
    }
}

// --- diagnostics ---

#[derive(Debug, Default, Clone)]
pub struct RecorderDiagnostics {
    pub frames: usize,
    pub bytes: usize,
    pub duration_sec: f32,
}

// --- audio buffer ---

type AudioBuffer = Arc<Mutex<Vec<i16>>>;

pub type AudioCallback = Box<dyn Fn(&[f32]) + Send + Sync + 'static>;

// --- recorder ---

pub struct Recorder {
    pub config: RecorderConfig,
    buffer: AudioBuffer,
    stream: Option<Stream>,
    device: Option<Device>,
    is_recording: Arc<AtomicBool>,
    last_duration: f32,
    diagnostics: RecorderDiagnostics,
    /// Actual sample rate used for recording (may differ from config)
    actual_sample_rate: u32,
    on_data: Option<AudioCallback>,
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
            if let Ok(desc) = device.description() {
                info!("Default input device: {}", desc);
            }
        } else {
            warn!("No default input device found");
        }

        Ok(Self {
            config: config.clone(),
            buffer: Arc::new(Mutex::new(Vec::new())),
            stream: None,
            device: None,
            is_recording: Arc::new(AtomicBool::new(false)),
            last_duration: 0.0,
            diagnostics: RecorderDiagnostics::default(),
            actual_sample_rate: config.sample_rate, // Will be updated in start()
            on_data: None,
        })
    }

    /// Set a callback to receive raw audio data (f32 samples)
    pub fn set_callback(&mut self, callback: AudioCallback) {
        self.on_data = Some(callback);
    }

    /// Actual sample rate used by the underlying input stream.
    ///
    /// Note: This may differ from `config.sample_rate` because we always open
    /// the device stream at its native rate for compatibility.
    pub fn actual_sample_rate(&self) -> u32 {
        self.actual_sample_rate
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

        // Initialize VAD (lazy init - only loads model on first use)
        if !vad::is_initialized() {
            let model_path = vad::default_model_path();
            if let Err(e) = vad::init(&model_path) {
                warn!(
                    "VAD init failed ({}): {} - segmentation disabled, speech_probability will return 1.0",
                    model_path.display(),
                    e
                );
            }
        }

        // Clear buffer and reset diagnostics
        self.buffer
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clear();
        self.diagnostics = RecorderDiagnostics::default();

        // Select input device
        let host = cpal::default_host();

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
                    if name == preferred || name.to_lowercase().contains(&preferred.to_lowercase())
                    {
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
        info!("Using input device: {}", device_name);

        // Get supported config
        let supported_config = device
            .default_input_config()
            .context("Failed to get default input config")?;

        // Use the device's native sample rate for compatibility
        // (backend will handle resampling if needed)
        let native_sample_rate = supported_config.sample_rate();

        // Build stream config using native sample rate
        let stream_config = StreamConfig {
            channels: self.config.channels,
            sample_rate: native_sample_rate,
            buffer_size: cpal::BufferSize::Default, // Let system choose buffer size
        };

        info!(
            "Audio stream config: {:?} (native rate: {}Hz)",
            stream_config, native_sample_rate
        );

        // Store actual sample rate for WAV file and duration calculations
        self.actual_sample_rate = native_sample_rate;

        // Setup stream callback
        let buffer = Arc::clone(&self.buffer);
        let is_recording_data = Arc::clone(&self.is_recording);
        let is_recording_error = Arc::clone(&self.is_recording);
        let on_data = self.on_data.take();

        let stream = device
            .build_input_stream(
                &stream_config,
                move |data: &[f32], _: &cpal::InputCallbackInfo| {
                    // Send data to callback if present
                    if let Some(ref cb) = on_data {
                        cb(data);
                    }

                    // Convert f32 samples to i16 and append to buffer
                    if let Ok(mut buf) = buffer.lock() {
                        for &sample in data {
                            // Clamp and convert f32 [-1.0, 1.0] to i16
                            let clamped = sample.clamp(-1.0, 1.0);
                            let i16_sample = (clamped * i16::MAX as f32) as i16;
                            buf.push(i16_sample);
                        }
                    }

                    let _ = is_recording_data.load(Ordering::SeqCst);
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
        self.stop_internal(true).await
    }

    /// Stops the audio recording without saving a WAV file.
    ///
    /// Returns None if no audio was recorded or an error occurred.
    pub async fn stop_without_saving(&mut self) -> Result<Option<PathBuf>> {
        self.stop_internal(false).await
    }

    async fn stop_internal(&mut self, save_wav: bool) -> Result<Option<PathBuf>> {
        if !self.is_recording.load(Ordering::SeqCst) && self.stream.is_none() {
            warn!("Stop called but no active stream");
            self.last_duration = 0.0;
            return Ok(None);
        }

        info!("Stopping recording...");

        // Stop stream
        if let Some(stream) = self.stream.take() {
            drop(stream); // Dropping the stream stops it
            info!("Audio stream stopped");
        }

        self.device = None;
        self.is_recording.store(false, Ordering::SeqCst);

        // Get buffer data
        let mut buf = self.buffer.lock().unwrap_or_else(|e| e.into_inner());
        if buf.is_empty() {
            warn!("No audio data captured");
            self.last_duration = 0.0;
            return Ok(None);
        }

        let num_frames = buf.len();
        self.last_duration = num_frames as f32 / self.actual_sample_rate as f32;
        self.diagnostics.frames = num_frames;
        self.diagnostics.bytes = num_frames * std::mem::size_of::<i16>();
        self.diagnostics.duration_sec = self.last_duration;

        info!(
            "Captured audio: {} frames ({:.2}s) at {}Hz",
            num_frames, self.last_duration, self.actual_sample_rate
        );

        if !save_wav {
            buf.clear();
            return Ok(None);
        }

        let wav_data = buf.clone();

        // Create temp file
        let temp_path = std::env::temp_dir().join(format!(
            "codescribe_recording_{}.wav",
            chrono::Utc::now().timestamp_millis()
        ));

        info!("Saving audio to: {:?}", temp_path);

        // Write WAV file using actual sample rate
        write_wav_file(
            &temp_path,
            &wav_data,
            self.actual_sample_rate,
            self.config.channels,
        )?;

        info!("Audio successfully saved to WAV file");

        // Clear buffer
        buf.clear();

        Ok(Some(temp_path))
    }
}

impl Default for Recorder {
    fn default() -> Self {
        Self::new().unwrap_or_else(|e| {
            error!("Recorder::default() failed: {e}");
            panic!("Cannot create default Recorder: {e}");
        })
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

// Note: RMS-based silence detection replaced with Silero VAD neural network (see vad module)

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

#[cfg(test)]
mod tests {
    use super::*;

    // Note: RMS tests removed - now using Silero VAD neural network (see vad module tests)

    #[test]
    fn test_recorder_config_default() {
        // Note: This test checks hardcoded defaults, not env-dependent behavior
        // to avoid race conditions with parallel tests
        assert_eq!(SAMPLE_RATE, 16000);
        assert_eq!(CHANNELS, 1);
    }

    #[tokio::test]
    async fn test_recorder_new() {
        let recorder = Recorder::new();
        assert!(recorder.is_ok());
    }
}
