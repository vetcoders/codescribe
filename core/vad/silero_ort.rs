//! Silero VAD wrapper using ort directly.
//!
//! Custom implementation using ort runtime directly.
//! Model: silero_vad.onnx v6 from https://github.com/snakers4/silero-vad
//!
//! Created by M&K (c)2026 VetCoders

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use ndarray::ArrayD;
use ort::session::Session;
use ort::value::Value;
use tracing::{debug, info, trace};

use super::config::VadConfig;
use crate::hf_cache;

mod embedded {
    include!(concat!(env!("OUT_DIR"), "/embedded_vad_data.rs"));
}

/// Silero VAD sample rate (always 16kHz)
pub const VAD_SAMPLE_RATE: u32 = 16000;

/// Chunk size for Silero (512 samples = 32ms at 16kHz)
pub(crate) const CHUNK_SIZE: usize = 512;

/// Context size for Silero v6 (64 samples at 16kHz).
/// Each inference call requires prepending the last 64 samples from the
/// previous chunk.  Without this the model receives incomplete input and
/// returns unreliable speech probabilities.
const CONTEXT_SIZE: usize = 64;

/// Unified state shape for Silero v6: [2, 1, 128].
/// v4 used separate h/c tensors with dim 64; v5+ merged them.
const STATE_SHAPE: [usize; 3] = [2, 1, 128];

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
    /// Legacy path-based loader. Embedded path is canonical via [`Self::new_embedded`].
    /// Kept for dev/test overrides where a custom model file is required.
    #[doc(hidden)]
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

    /// Load Silero VAD model from embedded bytes (production path, zero I/O).
    pub(crate) fn new_embedded(config: VadConfig) -> Result<Self> {
        info!(
            "Loading Silero VAD model from embedded bytes ({} bytes)",
            embedded::MODEL.len()
        );
        let session = Session::builder()?
            .with_intra_threads(1)?
            .commit_from_memory(embedded::MODEL)
            .context("Failed to load embedded Silero VAD ONNX model")?;

        debug!("Silero VAD model loaded successfully (embedded)");

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
// AccumulatingVad — local SileroVad with proper chunk accumulation
// ═══════════════════════════════════════════════════════════

/// SileroVad with sample accumulation for correct chunk-boundary handling.
///
/// Unlike the old global VadWorker, this:
/// - Properly accumulates resampled samples across calls (no lost sub-chunks)
/// - Starts with last_prob = 0.0 (no phantom speech on first frame)
/// - Is locally owned, not a global singleton
///
/// Created by M&K (c)2026 VetCoders
pub struct AccumulatingVad {
    vad: SileroVad,
    resampler: Option<Resampler>,
    accumulator: Vec<f32>,
    last_prob: f32,
}

impl AccumulatingVad {
    /// Legacy path-based loader. Embedded path is canonical via [`Self::new`].
    /// Kept for dev/test overrides where a custom model file is required.
    #[doc(hidden)]
    pub fn with_config(model_path: &Path, config: VadConfig, sample_rate: u32) -> Result<Self> {
        let vad = SileroVad::new(model_path, config)?;
        let resampler = if sample_rate != VAD_SAMPLE_RATE {
            Some(Resampler::new(sample_rate))
        } else {
            None
        };
        Ok(Self {
            vad,
            resampler,
            accumulator: Vec::with_capacity(CHUNK_SIZE * 4),
            last_prob: 0.0,
        })
    }

    /// Create using embedded model bytes and given config (production path).
    pub(crate) fn with_config_embedded(config: VadConfig, sample_rate: u32) -> Result<Self> {
        let vad = SileroVad::new_embedded(config)?;
        let resampler = if sample_rate != VAD_SAMPLE_RATE {
            Some(Resampler::new(sample_rate))
        } else {
            None
        };
        Ok(Self {
            vad,
            resampler,
            accumulator: Vec::with_capacity(CHUNK_SIZE * 4),
            last_prob: 0.0,
        })
    }

    /// Create using embedded model and default config (production path, zero I/O).
    pub fn new(sample_rate: u32) -> Result<Self> {
        Self::with_config_embedded(VadConfig::default(), sample_rate)
    }

    /// Feed audio samples (at the sample_rate given at construction).
    /// Returns latest speech probability (0.0–1.0).
    ///
    /// Internally resamples to 16kHz, accumulates until a full 512-sample
    /// chunk is available, then runs Silero inference.
    pub fn feed(&mut self, samples: &[f32]) -> f32 {
        let resampled = if let Some(ref mut r) = self.resampler {
            r.resample(samples)
        } else {
            samples.to_vec()
        };
        self.accumulator.extend_from_slice(&resampled);

        // Process every complete 512-sample chunk
        while self.accumulator.len() >= CHUNK_SIZE {
            let chunk: Vec<f32> = self.accumulator.drain(..CHUNK_SIZE).collect();
            // predict() on a 16kHz chunk with no resampler set → single predict_chunk()
            if let Ok(prob) = self.vad.predict(&chunk) {
                self.last_prob = prob;
            }
        }
        self.last_prob
    }

    /// Current speech probability without feeding new audio.
    pub fn probability(&self) -> f32 {
        self.last_prob
    }

    /// Speech detection threshold from config.
    pub fn threshold(&self) -> f32 {
        self.vad.threshold()
    }

    /// Reset internal Silero state and accumulator.
    pub fn reset(&mut self) {
        self.vad.reset();
        self.accumulator.clear();
        self.last_prob = 0.0;
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

    /// P0-02 regression guard: embedded VAD must load without any disk file present.
    /// Production hot path is `AccumulatingVad::new(sample_rate)` — it MUST succeed
    /// even when `~/.codescribe/models/silero_vad.onnx` does not exist on the system.
    #[test]
    fn embedded_vad_loads_without_disk_file() {
        // Verify embedded blob is non-trivial (Silero VAD ONNX is ~2.3MB).
        assert!(
            embedded::MODEL.len() > 1_000_000,
            "Silero VAD embedded blob must be >1MB, got {} bytes",
            embedded::MODEL.len()
        );

        // Confirm AccumulatingVad::new succeeds via the embedded path,
        // independent of any disk file at default_model_path().
        let vad = AccumulatingVad::new(16000);
        assert!(vad.is_ok(), "embedded VAD must load: {:?}", vad.err());
    }
}
