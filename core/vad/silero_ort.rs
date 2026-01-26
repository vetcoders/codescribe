//! Silero VAD wrapper using ort directly.
//!
//! Custom implementation that shares ort runtime with fastembed.
//! Model: silero_vad.onnx v5 from https://github.com/snakers4/silero-vad
//!
//! Created by M&K (c)2026 VetCoders

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, OnceLock};
use std::thread;

use anyhow::{Context, Result};
use ndarray::{Array1, Array2, Array3};
use ort::session::Session;
use ort::value::Value;
use tracing::{debug, info, warn};

use super::config::VadConfig;

/// Silero VAD sample rate (always 16kHz)
pub const VAD_SAMPLE_RATE: u32 = 16000;

/// Chunk size for Silero (512 samples = 32ms at 16kHz)
const CHUNK_SIZE: usize = 512;

/// State dimensions for Silero v5 model
const STATE_DIM: usize = 64;
const STATE_LAYERS: usize = 2;

/// Global VAD worker
static VAD_WORKER: OnceLock<VadWorker> = OnceLock::new();

/// Resampler for converting audio to 16kHz
pub struct Resampler {
    buffer: Vec<f32>,
    ratio: f32,
}

impl Resampler {
    /// Create resampler for given input sample rate
    pub fn new(input_rate: u32) -> Self {
        let ratio = VAD_SAMPLE_RATE as f32 / input_rate as f32;
        Self {
            buffer: Vec::with_capacity(CHUNK_SIZE * 2),
            ratio,
        }
    }

    /// Resample audio to 16kHz (linear interpolation)
    /// Returns owned Vec (avoids lifetime complexity)
    pub fn resample(&mut self, samples: &[f32]) -> Vec<f32> {
        if (self.ratio - 1.0).abs() < 0.001 {
            // No resampling needed - return copy
            return samples.to_vec();
        }

        let output_len = (samples.len() as f32 * self.ratio) as usize;
        self.buffer.clear();
        self.buffer.reserve(output_len);

        for i in 0..output_len {
            let src_idx = i as f32 / self.ratio;
            let idx0 = src_idx.floor() as usize;
            let idx1 = (idx0 + 1).min(samples.len().saturating_sub(1));
            let frac = src_idx - idx0 as f32;

            let sample = if idx0 < samples.len() {
                samples[idx0] * (1.0 - frac) + samples.get(idx1).copied().unwrap_or(0.0) * frac
            } else {
                0.0
            };
            self.buffer.push(sample);
        }

        self.buffer.clone()
    }
}

/// Silero VAD model wrapper
pub struct SileroVad {
    session: Session,
    state_h: Array3<f32>,
    state_c: Array3<f32>,
    config: VadConfig,
    resampler: Option<Resampler>,
}

impl SileroVad {
    /// Load Silero VAD model from path
    pub fn new(model_path: &Path, config: VadConfig) -> Result<Self> {
        info!("Loading Silero VAD model from: {}", model_path.display());

        let session = Session::builder()?
            .with_intra_threads(1)?
            .commit_from_file(model_path)
            .context("Failed to load Silero VAD ONNX model")?;

        debug!("Silero VAD model loaded successfully");

        Ok(Self {
            session,
            state_h: Array3::zeros((STATE_LAYERS, 1, STATE_DIM)),
            state_c: Array3::zeros((STATE_LAYERS, 1, STATE_DIM)),
            config,
            resampler: None,
        })
    }

    /// Set input sample rate (enables automatic resampling)
    pub fn set_input_sample_rate(&mut self, rate: u32) {
        if rate != VAD_SAMPLE_RATE {
            self.resampler = Some(Resampler::new(rate));
        } else {
            self.resampler = None;
        }
    }

    /// Get speech probability for audio chunk (0.0 - 1.0)
    ///
    /// Automatically resamples if input rate was set.
    pub fn predict(&mut self, samples: &[f32]) -> Result<f32> {
        if samples.is_empty() {
            return Ok(0.0);
        }

        // Resample if needed (get owned Vec to avoid borrow issues)
        let samples_16k: Vec<f32> = if let Some(ref mut resampler) = self.resampler {
            resampler.resample(samples)
        } else {
            samples.to_vec()
        };

        // Process in chunks and return max probability
        let mut max_prob = 0.0f32;

        for chunk in samples_16k.chunks(CHUNK_SIZE) {
            // Pad chunk if needed
            let padded: Vec<f32> = if chunk.len() < CHUNK_SIZE {
                let mut p = chunk.to_vec();
                p.resize(CHUNK_SIZE, 0.0);
                p
            } else {
                chunk.to_vec()
            };

            let prob = self.predict_chunk(&padded)?;
            max_prob = max_prob.max(prob);
        }

        Ok(max_prob)
    }

    /// Predict on a single 512-sample chunk
    fn predict_chunk(&mut self, chunk: &[f32]) -> Result<f32> {
        // Input: (batch=1, samples)
        let input_array = Array2::from_shape_vec((1, chunk.len()), chunk.to_vec())?;

        // Sample rate as i64
        let sr_array = Array1::from_vec(vec![VAD_SAMPLE_RATE as i64]);

        // Create input values
        let input = Value::from_array(input_array)?;
        let sr = Value::from_array(sr_array)?;
        let h = Value::from_array(self.state_h.clone())?;
        let c = Value::from_array(self.state_c.clone())?;

        // Run inference with named inputs
        // Silero VAD v5 expects: input, sr, h, c
        let outputs = self.session.run(ort::inputs![
            "input" => input,
            "sr" => sr,
            "h" => h,
            "c" => c
        ])?;

        // Extract probability from first output
        let prob = {
            let output_value = &outputs[0];
            let (_shape, data) = output_value.try_extract_tensor::<f32>()?;
            data.first().copied().unwrap_or(0.0)
        };

        // Update states if model returns them (outputs 1 and 2)
        if outputs.len() > 2 {
            // Extract new h state (let chains - Rust 2024)
            if let Ok((_shape, h_data)) = outputs[1].try_extract_tensor::<f32>()
                && let Ok(arr) = Array3::from_shape_vec((STATE_LAYERS, 1, STATE_DIM), h_data.to_vec())
            {
                self.state_h = arr;
            }

            // Extract new c state
            if let Ok((_shape, c_data)) = outputs[2].try_extract_tensor::<f32>()
                && let Ok(arr) = Array3::from_shape_vec((STATE_LAYERS, 1, STATE_DIM), c_data.to_vec())
            {
                self.state_c = arr;
            }
        }

        Ok(prob)
    }

    /// Reset internal state
    pub fn reset(&mut self) {
        self.state_h.fill(0.0);
        self.state_c.fill(0.0);
    }

    /// Get current threshold
    pub fn threshold(&self) -> f32 {
        self.config.threshold
    }
}

// ═══════════════════════════════════════════════════════════
// Worker-based singleton (no mutex in hot path)
// ═══════════════════════════════════════════════════════════

use std::sync::{atomic::AtomicU32, Arc};

/// Message to VAD worker (fire-and-forget, no response channel)
enum VadMessage {
    Predict { samples: Vec<f32>, sample_rate: u32 },
    Reset,
    #[allow(dead_code)]
    Shutdown,
}

/// VAD worker that processes requests off the audio thread.
///
/// Non-blocking design: callback submits audio via try_send,
/// worker updates atomic last_prob. No waiting in hot path.
struct VadWorker {
    sender: mpsc::SyncSender<VadMessage>,
    initialized: AtomicBool,
    /// Last computed probability (f32 as bits for atomic access)
    last_prob: Arc<AtomicU32>,
}

impl VadWorker {
    fn new(model_path: &Path, config: VadConfig) -> Result<Self> {
        // Bounded channel - if full, oldest messages dropped (backpressure)
        let (tx, rx) = mpsc::sync_channel::<VadMessage>(4);
        let path = model_path.to_path_buf();

        // Shared atomic for worker to update
        let last_prob = Arc::new(AtomicU32::new(0.0_f32.to_bits()));
        let last_prob_writer = Arc::clone(&last_prob);

        thread::spawn(move || {
            let mut vad = match SileroVad::new(&path, config) {
                Ok(v) => v,
                Err(e) => {
                    tracing::error!("Failed to initialize Silero VAD: {}", e);
                    return;
                }
            };

            for msg in rx {
                match msg {
                    VadMessage::Predict {
                        samples,
                        sample_rate,
                    } => {
                        vad.set_input_sample_rate(sample_rate);
                        let prob = vad.predict(&samples).unwrap_or(0.0);
                        // Update atomic (Relaxed - just caching value, no sync needed)
                        last_prob_writer.store(prob.to_bits(), Ordering::Relaxed);
                    }
                    VadMessage::Reset => {
                        vad.reset();
                        last_prob_writer.store(0.0_f32.to_bits(), Ordering::Relaxed);
                    }
                    VadMessage::Shutdown => {
                        break;
                    }
                }
            }
        });

        Ok(Self {
            sender: tx,
            initialized: AtomicBool::new(true),
            last_prob,
        })
    }

    /// Submit audio for VAD processing (non-blocking, fire-and-forget).
    /// Returns immediately, does NOT wait for result.
    fn submit(&self, samples: &[f32], sample_rate: u32) {
        // try_send: if channel full, drop oldest (backpressure)
        let _ = self.sender.try_send(VadMessage::Predict {
            samples: samples.to_vec(),
            sample_rate,
        });
    }

    /// Get the last computed probability (non-blocking atomic read).
    fn last_probability(&self) -> f32 {
        f32::from_bits(self.last_prob.load(Ordering::Relaxed))
    }

    fn reset(&self) {
        let _ = self.sender.try_send(VadMessage::Reset);
    }
}

// ═══════════════════════════════════════════════════════════
// Public singleton API
// ═══════════════════════════════════════════════════════════

/// Model path storage
#[allow(dead_code)]
static MODEL_PATH: OnceLock<std::path::PathBuf> = OnceLock::new();
static VAD_CONFIG: OnceLock<VadConfig> = OnceLock::new();

/// Initialize VAD with model path
pub fn init(model_path: &Path) -> Result<()> {
    init_with_config(model_path, VadConfig::default())
}

/// Initialize VAD with model path and config
pub fn init_with_config(model_path: &Path, config: VadConfig) -> Result<()> {
    let _ = MODEL_PATH.set(model_path.to_path_buf());
    let _ = VAD_CONFIG.set(config.clone());

    let _ = VAD_WORKER.get_or_init(|| {
        VadWorker::new(model_path, config).expect("Failed to initialize VAD worker")
    });

    Ok(())
}

/// Check if VAD is initialized
pub fn is_initialized() -> bool {
    VAD_WORKER
        .get()
        .map(|w| w.initialized.load(Ordering::SeqCst))
        .unwrap_or(false)
}

/// Get speech probability for audio chunk (NON-BLOCKING).
///
/// Safe to call from audio callbacks:
/// 1. Submits audio to worker thread (fire-and-forget)
/// 2. Returns the LAST computed probability (may be from previous chunk)
///
/// This "eventual consistency" approach avoids blocking the audio thread.
/// After a few calls, the returned value will reflect recent audio.
///
/// **Important:** Returns 1.0 when VAD not initialized (assume speech)
/// to prevent immediate auto-stop in recorders.
pub fn speech_probability(samples: &[f32], sample_rate: u32) -> f32 {
    if let Some(worker) = VAD_WORKER.get() {
        // Submit new audio (non-blocking)
        worker.submit(samples, sample_rate);
        // Return last computed probability (instant, atomic read)
        worker.last_probability()
    } else {
        // VAD not initialized - assume speech to prevent premature auto-stop
        1.0
    }
}

/// Check if audio contains speech
pub fn is_speech(samples: &[f32], sample_rate: u32) -> bool {
    let threshold = VAD_CONFIG.get().map(|c| c.threshold).unwrap_or(0.5);
    speech_probability(samples, sample_rate) > threshold
}

/// Reset VAD state
pub fn reset() {
    if let Some(worker) = VAD_WORKER.get() {
        worker.reset();
    }
}

/// Get default model path
pub fn default_model_path() -> std::path::PathBuf {
    directories::BaseDirs::new()
        .map(|d| d.data_local_dir().to_path_buf())
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join("codescribe")
        .join("models")
        .join("silero_vad.onnx")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resampler_48k_to_16k() {
        let mut resampler = Resampler::new(48000);

        // 48kHz input: 480 samples = 10ms
        let input: Vec<f32> = (0..480).map(|i| (i as f32 * 0.01).sin()).collect();

        // Should become ~160 samples at 16kHz
        let output = resampler.resample(&input);
        assert!((output.len() as i32 - 160).abs() <= 1);
    }

    #[test]
    fn test_resampler_16k_passthrough() {
        let mut resampler = Resampler::new(16000);

        let input: Vec<f32> = (0..512).map(|i| (i as f32 * 0.01).sin()).collect();
        let output = resampler.resample(&input);

        // Should be same length
        assert_eq!(output.len(), input.len());
    }
}
