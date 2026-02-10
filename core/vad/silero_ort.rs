//! Silero VAD wrapper using ort directly.
//!
//! Custom implementation using ort runtime directly.
//! Model: silero_vad.onnx v6 from https://github.com/snakers4/silero-vad
//!
//! Created by M&K (c)2026 VetCoders

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{OnceLock, mpsc};
use std::thread;

use anyhow::{Context, Result};
use ndarray::ArrayD;
use ort::session::Session;
use ort::value::Value;
use tracing::{debug, info, trace};

use super::config::VadConfig;
use crate::hf_cache;

/// Silero VAD sample rate (always 16kHz)
pub const VAD_SAMPLE_RATE: u32 = 16000;

/// Chunk size for Silero (512 samples = 32ms at 16kHz)
const CHUNK_SIZE: usize = 512;

/// Context size for Silero v6 (64 samples at 16kHz).
/// Each inference call requires prepending the last 64 samples from the
/// previous chunk.  Without this the model receives incomplete input and
/// returns unreliable speech probabilities.
const CONTEXT_SIZE: usize = 64;

/// Unified state shape for Silero v6: [2, 1, 128].
/// v4 used separate h/c tensors with dim 64; v5+ merged them.
const STATE_SHAPE: [usize; 3] = [2, 1, 128];

/// Global VAD worker
static VAD_WORKER: OnceLock<VadWorker> = OnceLock::new();

/// Resampler for converting audio to 16kHz
pub struct Resampler {
    buffer: Vec<f32>,
    ratio: f32,
}

impl Resampler {
    pub fn new(input_rate: u32) -> Self {
        let ratio = VAD_SAMPLE_RATE as f32 / input_rate as f32;
        Self {
            buffer: Vec::with_capacity(CHUNK_SIZE * 2),
            ratio,
        }
    }

    /// Resample audio to 16kHz (linear interpolation).
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

/// Silero VAD v6 model wrapper (backwards-compatible with v5 ONNX API).
///
/// v5+ API differences from v4:
///  - Unified state tensor `[2, 1, 128]` (v4 had separate h/c `[2, 1, 64]`)
///  - Input order: `input, state, sr` (v4: `input, sr, h, c`)
///  - Output names: `output`, `stateN` (v4: positional)
///  - Context window: 64 samples prepended to each 512-sample chunk
pub struct SileroVad {
    session: Session,
    state: ArrayD<f32>,
    context: Vec<f32>,
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
            state: ArrayD::zeros(STATE_SHAPE.as_slice()),
            context: vec![0.0; CONTEXT_SIZE],
            config,
            resampler: None,
        })
    }

    pub fn set_input_sample_rate(&mut self, rate: u32) {
        if rate != VAD_SAMPLE_RATE {
            self.resampler = Some(Resampler::new(rate));
        } else {
            self.resampler = None;
        }
    }

    /// Get speech probability for a single CHUNK_SIZE (512) frame at 16kHz.
    ///
    /// The caller is responsible for providing exactly CHUNK_SIZE samples
    /// already at 16kHz.  The internal resampler path is kept for
    /// backwards-compat but callers in streaming_recorder pre-resample.
    pub fn predict(&mut self, samples: &[f32]) -> Result<f32> {
        if samples.is_empty() {
            return Ok(0.0);
        }

        // Resample if needed
        let samples_16k: Vec<f32> = if let Some(ref mut resampler) = self.resampler {
            resampler.resample(samples)
        } else {
            samples.to_vec()
        };

        let mut max_prob = 0.0f32;
        for chunk in samples_16k.chunks(CHUNK_SIZE) {
            if chunk.len() < CHUNK_SIZE {
                break;
            }
            let prob = self.predict_chunk(chunk)?;
            max_prob = max_prob.max(prob);
        }
        Ok(max_prob)
    }

    /// Predict on a single 512-sample chunk using Silero v6 API.
    ///
    /// Prepends 64-sample context, sends `[input, state, sr]`,
    /// reads `output` (prob) and `stateN` (updated state).
    fn predict_chunk(&mut self, chunk: &[f32]) -> Result<f32> {
        // Build context + chunk → [1, 576]
        let mut input_data = Vec::with_capacity(CONTEXT_SIZE + chunk.len());
        input_data.extend_from_slice(&self.context);
        input_data.extend_from_slice(chunk);

        // Update context for next call
        let ctx_start = chunk.len().saturating_sub(CONTEXT_SIZE);
        self.context[..].copy_from_slice(&chunk[ctx_start..]);

        // Input tensors — v5+ order: input, state, sr
        let input = ndarray::Array2::from_shape_vec([1, input_data.len()], input_data)
            .map_err(|e| anyhow::anyhow!("input shape: {}", e))?;
        let sr = ndarray::Array1::from_vec(vec![VAD_SAMPLE_RATE as i64]);
        let state = std::mem::replace(&mut self.state, ArrayD::zeros(STATE_SHAPE.as_slice()));

        let input_value = Value::from_array(input)?;
        let state_value = Value::from_array(state)?;
        let sr_value = Value::from_array(sr)?;

        let outputs = self.session.run([
            (&input_value).into(),
            (&state_value).into(),
            (&sr_value).into(),
        ])?;

        // Read probability from "output" (safe access — no panic on missing key)
        let prob = {
            let output = outputs
                .get("output")
                .context("Silero model missing 'output' tensor")?;
            let (_shape, data) = output.try_extract_tensor::<f32>()?;
            data.first().copied().unwrap_or(0.0)
        };

        // Read updated state from "stateN" (safe access — no panic on missing key)
        {
            let state_output = outputs
                .get("stateN")
                .context("Silero model missing 'stateN' tensor")?;
            let (shape, data) = state_output.try_extract_tensor::<f32>()?;
            let shape_usize: Vec<usize> = shape.as_ref().iter().map(|&d| d as usize).collect();
            if let Ok(arr) = ArrayD::from_shape_vec(shape_usize.as_slice(), data.to_vec()) {
                self.state = arr;
            }
        }

        Ok(prob)
    }

    /// Reset internal state
    pub fn reset(&mut self) {
        self.state = ArrayD::zeros(STATE_SHAPE.as_slice());
        self.context.fill(0.0);
    }

    pub fn threshold(&self) -> f32 {
        self.config.threshold
    }
}

// ═══════════════════════════════════════════════════════════
// Worker-based singleton (no mutex in hot path)
// ═══════════════════════════════════════════════════════════

use std::sync::{Arc, atomic::AtomicU32};

/// Message to VAD worker (fire-and-forget, no response channel)
enum VadMessage {
    Predict { samples: Vec<f32>, sample_rate: u32 },
    Reset,
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
        // Validate model exists before spawning worker thread
        if !model_path.exists() {
            anyhow::bail!(
                "Silero VAD model not found at: {} - download with scripts/download-silero.sh",
                model_path.display()
            );
        }

        // Bounded channel - if full, oldest messages dropped (backpressure)
        let (tx, rx) = mpsc::sync_channel::<VadMessage>(4);
        let path = model_path.to_path_buf();

        // Shared atomic for worker to update
        // Initialize to 1.0 (assume speech) - safe default if worker fails
        let last_prob = Arc::new(AtomicU32::new(1.0_f32.to_bits()));
        let last_prob_writer = Arc::clone(&last_prob);

        // Oneshot channel to confirm model loaded successfully
        let (init_tx, init_rx) = mpsc::sync_channel::<Result<()>>(1);

        thread::spawn(move || {
            let mut vad = match SileroVad::new(&path, config) {
                Ok(v) => {
                    // Signal success to main thread
                    let _ = init_tx.send(Ok(()));
                    v
                }
                Err(e) => {
                    // Signal failure to main thread
                    let _ = init_tx.send(Err(anyhow::anyhow!(
                        "Failed to load Silero VAD model: {}",
                        e
                    )));
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
                        let prob = match vad.predict(&samples) {
                            Ok(p) => p,
                            Err(e) => {
                                tracing::warn!("VAD predict error (assuming speech): {e}");
                                1.0
                            }
                        };
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

        // Wait for worker to confirm model loaded (with timeout)
        match init_rx.recv_timeout(std::time::Duration::from_secs(30)) {
            Ok(Ok(())) => {
                debug!("VAD worker initialized successfully");
                Ok(Self {
                    sender: tx,
                    initialized: AtomicBool::new(true),
                    last_prob,
                })
            }
            Ok(Err(e)) => {
                // Model failed to load - propagate error
                Err(e)
            }
            Err(_) => {
                // Timeout waiting for worker
                anyhow::bail!("Timeout waiting for VAD worker to initialize (30s)")
            }
        }
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

    fn shutdown(&self) {
        let _ = self.sender.try_send(VadMessage::Shutdown);
        self.initialized.store(false, Ordering::SeqCst);
    }
}

// ═══════════════════════════════════════════════════════════
// Public singleton API
// ═══════════════════════════════════════════════════════════

use std::sync::Mutex;

/// Initialization lock to prevent concurrent init attempts
static INIT_LOCK: Mutex<()> = Mutex::new(());

/// Model path storage (set only after successful init)
static MODEL_PATH: OnceLock<std::path::PathBuf> = OnceLock::new();
static VAD_CONFIG: OnceLock<VadConfig> = OnceLock::new();

pub fn init(model_path: &Path) -> Result<()> {
    init_with_config(model_path, VadConfig::default())
}

/// Initialize VAD with model path and config
///
/// Returns Ok if initialized successfully, Err if model not found.
/// When VAD is not initialized, `speech_probability()` returns 1.0 (assume speech).
///
/// **Important:** This is a no-op if VAD is already initialized.
/// Repeated calls are safe and cheap (early-exit with mutex).
///
/// **Warning:** First call may block up to 30s waiting for model load.
/// Call early in app startup, not on UI thread.
pub fn init_with_config(model_path: &Path, config: VadConfig) -> Result<()> {
    // Fast path: already initialized (no lock needed)
    if is_initialized() {
        return Ok(());
    }

    // Slow path: acquire lock to prevent concurrent init
    let _guard = INIT_LOCK.lock().unwrap_or_else(|e| e.into_inner());

    // Double-check after acquiring lock (another thread may have initialized)
    if is_initialized() {
        debug!("VAD initialized by another thread, skipping");
        return Ok(());
    }

    // Try to create worker - if it fails, VAD stays uninitialized
    // (speech_probability will return 1.0, effectively disabling segmentation)
    match VadWorker::new(model_path, config.clone()) {
        Ok(worker) => {
            // Only set config/path AFTER successful init
            let _ = MODEL_PATH.set(model_path.to_path_buf());
            let _ = VAD_CONFIG.set(config);
            let _ = VAD_WORKER.set(worker);
            info!("VAD initialized successfully");
            Ok(())
        }
        Err(e) => {
            tracing::error!("VAD init failed: {} - segmentation disabled", e);
            Err(e)
        }
    }
}

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
/// **Important:** Returns 1.0 when VAD not initialized (assume speech),
/// which effectively disables silence-based segmentation.
pub fn speech_probability(samples: &[f32], sample_rate: u32) -> f32 {
    if let Some(worker) = VAD_WORKER.get() {
        // Submit new audio (non-blocking)
        worker.submit(samples, sample_rate);
        // Return last computed probability (instant, atomic read)
        worker.last_probability()
    } else {
        // VAD not initialized - assume speech to prevent premature segmentation
        1.0
    }
}

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

/// Gracefully stop the VAD worker thread.
pub fn shutdown() {
    if let Some(worker) = VAD_WORKER.get() {
        worker.shutdown();
    }
}

/// HuggingFace repo for Silero VAD model
const SILERO_VAD_REPO: &str = "snakers4/silero-vad";
const SILERO_VAD_FILE: &str = "silero_vad.onnx";

/// Get default model path (bundled/models dir -> HF cache -> ~/.codescribe/models/)
pub fn default_model_path() -> PathBuf {
    // 1) Bundled / models dir (app Resources/models or ./models)
    if let Ok(manager) = crate::config::models::ModelManager::new() {
        let candidate = manager.models_dir().join(SILERO_VAD_FILE);
        if candidate.exists() {
            trace!("Using Silero VAD from models dir: {}", candidate.display());
            return candidate;
        }
    }

    // Try HF cache first (from `hf download snakers4/silero-vad`)
    if let Some(snapshot) = hf_cache::find_snapshot(SILERO_VAD_REPO, &[SILERO_VAD_FILE]) {
        let model_path = snapshot.join(SILERO_VAD_FILE);
        if model_path.exists() {
            trace!("Using Silero VAD from HF cache: {}", model_path.display());
            return model_path;
        }
    }

    // Fallback to legacy path
    directories::BaseDirs::new()
        .map(|d| d.home_dir().to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".codescribe")
        .join("models")
        .join(SILERO_VAD_FILE)
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
