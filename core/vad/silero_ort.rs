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
use ort::session::{builder::GraphOptimizationLevel, Session};
use ort::value::Value;
use tracing::{debug, info, warn};

use super::config::VadConfig;

/// Silero VAD sample rate (always 16kHz)
pub const VAD_SAMPLE_RATE: u32 = 16000;

/// Chunk size for Silero (512 samples = 32ms at 16kHz)
const CHUNK_SIZE: usize = 512;

/// State size for Silero v5 model (2, 1, 64)
const STATE_SIZE: usize = 2 * 1 * 64;

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
    /// Returns slice of resampled data (reuses internal buffer)
    pub fn resample(&mut self, samples: &[f32]) -> &[f32] {
        if (self.ratio - 1.0).abs() < 0.001 {
            // No resampling needed
            return samples;
        }

        let output_len = (samples.len() as f32 * self.ratio) as usize;
        self.buffer.clear();
        self.buffer.reserve(output_len);

        for i in 0..output_len {
            let src_idx = i as f32 / self.ratio;
            let idx0 = src_idx.floor() as usize;
            let idx1 = (idx0 + 1).min(samples.len() - 1);
            let frac = src_idx - idx0 as f32;

            let sample = samples[idx0] * (1.0 - frac) + samples[idx1] * frac;
            self.buffer.push(sample);
        }

        &self.buffer
    }
}

/// Silero VAD model wrapper
pub struct SileroVad {
    session: Session,
    state_h: Vec<f32>,
    state_c: Vec<f32>,
    config: VadConfig,
    resampler: Option<Resampler>,
}

impl SileroVad {
    /// Load Silero VAD model from path
    pub fn new(model_path: &Path, config: VadConfig) -> Result<Self> {
        info!("Loading Silero VAD model from: {}", model_path.display());

        let session = Session::builder()?
            .with_optimization_level(GraphOptimizationLevel::Level3)?
            .commit_from_file(model_path)
            .context("Failed to load Silero VAD ONNX model")?;

        debug!("Silero VAD model loaded successfully");

        Ok(Self {
            session,
            state_h: vec![0.0; STATE_SIZE],
            state_c: vec![0.0; STATE_SIZE],
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

        // Resample if needed
        let samples_16k = if let Some(ref mut resampler) = self.resampler {
            resampler.resample(samples)
        } else {
            samples
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
        // Prepare inputs
        // Input shape: (batch=1, samples=512)
        let input = Value::from_array(([1, chunk.len()], chunk.to_vec()))?;

        // State shapes: (2, 1, 64)
        let h = Value::from_array(([2usize, 1, 64], self.state_h.clone()))?;
        let c = Value::from_array(([2usize, 1, 64], self.state_c.clone()))?;

        // Sample rate
        let sr = Value::from_array(([], vec![VAD_SAMPLE_RATE as i64]))?;

        // Run inference
        let outputs = self.session.run(ort::inputs![input, sr, h, c]?)?;

        // Extract probability (first output)
        let prob_tensor = outputs[0].try_extract_tensor::<f32>()?;
        let prob = prob_tensor.view().iter().next().copied().unwrap_or(0.0);

        // Update states (outputs 1 and 2)
        if outputs.len() > 2 {
            let h_out = outputs[1].try_extract_tensor::<f32>()?;
            let c_out = outputs[2].try_extract_tensor::<f32>()?;

            self.state_h.clear();
            self.state_h.extend(h_out.view().iter().copied());

            self.state_c.clear();
            self.state_c.extend(c_out.view().iter().copied());
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

/// Message to VAD worker
enum VadMessage {
    Predict {
        samples: Vec<f32>,
        sample_rate: u32,
        response: mpsc::Sender<f32>,
    },
    Reset,
    Shutdown,
}

/// VAD worker that processes requests off the audio thread
struct VadWorker {
    sender: mpsc::Sender<VadMessage>,
    initialized: AtomicBool,
}

impl VadWorker {
    fn new(model_path: &Path, config: VadConfig) -> Result<Self> {
        let (tx, rx) = mpsc::channel::<VadMessage>();
        let path = model_path.to_path_buf();

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
                        response,
                    } => {
                        vad.set_input_sample_rate(sample_rate);
                        let prob = vad.predict(&samples).unwrap_or(0.0);
                        let _ = response.send(prob);
                    }
                    VadMessage::Reset => {
                        vad.reset();
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
        })
    }

    fn predict(&self, samples: &[f32], sample_rate: u32) -> f32 {
        let (tx, rx) = mpsc::channel();
        if self
            .sender
            .send(VadMessage::Predict {
                samples: samples.to_vec(),
                sample_rate,
                response: tx,
            })
            .is_ok()
        {
            rx.recv().unwrap_or(0.0)
        } else {
            0.0
        }
    }

    fn reset(&self) {
        let _ = self.sender.send(VadMessage::Reset);
    }
}

// ═══════════════════════════════════════════════════════════
// Public singleton API
// ═══════════════════════════════════════════════════════════

/// Model path storage
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
    VAD_WORKER.get().map(|w| w.initialized.load(Ordering::SeqCst)).unwrap_or(false)
}

/// Get speech probability for audio chunk
///
/// This is safe to call from audio callbacks - it sends to worker thread.
pub fn speech_probability(samples: &[f32], sample_rate: u32) -> f32 {
    if let Some(worker) = VAD_WORKER.get() {
        worker.predict(samples, sample_rate)
    } else {
        warn!("VAD not initialized, returning 0.0");
        0.0
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

        // Should be same pointer (no copy)
        assert_eq!(output.len(), input.len());
    }
}
