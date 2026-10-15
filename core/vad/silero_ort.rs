//! Silero VAD wrapper using ort directly.
//!
//! Custom implementation using ort runtime directly.
//! Model: silero_vad.onnx v6 from https://github.com/snakers4/silero-vad

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use anyhow::{Context, Result};
use ndarray::ArrayD;
use ort::session::Session;
use ort::value::Value;
use tracing::{debug, info, trace};

use super::config::VadConfig;
use crate::hf_cache;

/// The Silero ONNX graph baked into the binary at build time.
///
/// `build.rs` emits `embedded_vad_data.rs` into `OUT_DIR` exposing the model as
/// a `MODEL` byte slice, which is why VAD needs no disk file and no download on
/// the production path.
mod embedded {
    include!(concat!(env!("OUT_DIR"), "/embedded_vad_data.rs"));
}

/// Process-wide shared Silero ONNX session for the embedded model.
///
/// The Silero graph is **stateless** between calls — the recurrent state
/// (`state`) and the 64-sample `context` live in each [`SileroVad`] instance
/// and are passed in/out as tensors per inference. Only the compiled graph is
/// heavy (ORT arena + graph optimizer scratch), and rebuilding it per recording
/// leaked native memory (the session is created hundreds of times over a long
/// run and ORT arenas do not return to the OS). We therefore build it **once**
/// and share it behind a `Mutex` (ORT's `Session::run` takes `&mut self`).
static EMBEDDED_SESSION: OnceLock<Arc<Mutex<Session>>> = OnceLock::new();

/// Build (once) and return a handle to the shared embedded Silero session.
///
/// First call compiles the graph from the embedded bytes and caches it; every
/// subsequent call clones the `Arc` — no reload, no extra native allocation.
fn embedded_session() -> Result<Arc<Mutex<Session>>> {
    // Fast path: already initialized.
    if let Some(existing) = EMBEDDED_SESSION.get() {
        return Ok(Arc::clone(existing));
    }
    // Build outside the OnceLock so a failure isn't cached as a poisoned slot.
    info!(
        "Loading Silero VAD model from embedded bytes ({} bytes) — shared once, reused",
        embedded::MODEL.len()
    );
    let session = Session::builder()?
        .with_intra_threads(1)
        .map_err(ort::Error::<()>::from)?
        .commit_from_memory(embedded::MODEL)
        .context("Failed to load embedded Silero VAD ONNX model")?;
    debug!("Silero VAD model loaded successfully (embedded, shared)");
    // Race-safe: if another thread won the init, drop ours and use theirs.
    let handle = Arc::new(Mutex::new(session));
    let stored = EMBEDDED_SESSION.get_or_init(|| handle);
    Ok(Arc::clone(stored))
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

/// Continuous linear resampling onto Silero's 16 kHz sample clock.
/// Packet boundaries never reset phase or invent a right-hand sample.
pub struct Resampler {
    input_rate: u32,
    input_cursor: u128,
    next_source_numerator: u128,
    previous_sample: Option<f32>,
}

impl Resampler {
    /// Build a resampler from `input_rate` to the fixed 16 kHz Silero expects.
    pub fn new(input_rate: u32) -> Self {
        assert!(input_rate > 0, "resampling requires a nonzero input rate");
        Self {
            input_rate,
            input_cursor: 0,
            next_source_numerator: 0,
            previous_sample: None,
        }
    }

    /// Resample audio to 16kHz (linear interpolation).
    pub fn resample(&mut self, samples: &[f32]) -> Vec<f32> {
        if samples.is_empty() {
            return Vec::new();
        }
        let start = self.input_cursor;
        let end = start + samples.len() as u128;
        let clock = u128::from(VAD_SAMPLE_RATE);
        let capacity = (samples.len() as u128 * clock).div_ceil(u128::from(self.input_rate));
        let mut output = Vec::with_capacity(capacity as usize);
        loop {
            let left = self.next_source_numerator / clock;
            let remainder = self.next_source_numerator % clock;
            let right = left + u128::from(remainder != 0);
            if right >= end {
                break;
            }
            let a = if left < start {
                self.previous_sample
                    .expect("continuous resampler predecessor")
            } else {
                samples[(left - start) as usize]
            };
            let b = if right == left {
                a
            } else {
                samples[(right - start) as usize]
            };
            let fraction = remainder as f32 / VAD_SAMPLE_RATE as f32;
            output.push(a * (1.0 - fraction) + b * fraction);
            self.next_source_numerator += u128::from(self.input_rate);
        }
        self.input_cursor = end;
        self.previous_sample = samples.last().copied();
        output
    }

    /// Close the stream's exact duration with its final real sample.
    /// Never finish at an intermediate capture-packet boundary.
    pub fn finish(&mut self) -> Vec<f32> {
        let mut output = Vec::new();
        if let Some(last) = self.previous_sample {
            let end = self.input_cursor * u128::from(VAD_SAMPLE_RATE);
            while self.next_source_numerator < end {
                output.push(last);
                self.next_source_numerator += u128::from(self.input_rate);
            }
        }
        output
    }

    /// Start a new capture/file on a fresh clock.
    pub fn reset(&mut self) {
        self.input_cursor = 0;
        self.next_source_numerator = 0;
        self.previous_sample = None;
    }
}

/// Silero v5/v6 ONNX frame contract: 512 new samples at 16 kHz.
///
/// v5+ API differences from v4:
///  - Unified state tensor `[2, 1, 128]` (v4 had separate h/c `[2, 1, 64]`)
///  - Input order: `input, state, sr` (v4: `input, sr, h, c`)
///  - Output names: `output`, `stateN` (v4: positional)
///  - Context window: 64 samples prepended to each 512-sample chunk
pub struct SileroVad {
    /// Shared, compiled Silero graph. The embedded production path hands out a
    /// clone of one process-wide session (see [`embedded_session`]); the legacy
    /// path-based loader builds its own. The graph is stateless across calls, so
    /// sharing it behind a `Mutex` is safe — per-stream state lives below.
    session: Arc<Mutex<Session>>,
    state: ArrayD<f32>,
    context: Vec<f32>,
    config: VadConfig,
}

impl SileroVad {
    /// Legacy path-based loader. Embedded path is canonical via [`Self::new_embedded`].
    /// Kept for dev/test overrides where a custom model file is required.
    #[doc(hidden)]
    pub fn new(model_path: &Path, config: VadConfig) -> Result<Self> {
        info!("Loading Silero VAD model from: {}", model_path.display());
        let session = Session::builder()?
            .with_intra_threads(1)
            .map_err(ort::Error::<()>::from)?
            .commit_from_file(model_path)
            .context("Failed to load Silero VAD ONNX model")?;

        debug!("Silero VAD model loaded successfully");

        Ok(Self {
            // Path-based loads are dev/test overrides with a caller-chosen file,
            // so they are not cached — each gets its own session.
            session: Arc::new(Mutex::new(session)),
            state: ArrayD::zeros(STATE_SHAPE.as_slice()),
            context: vec![0.0; CONTEXT_SIZE],
            config,
        })
    }

    /// Load Silero VAD model from embedded bytes (production path, zero I/O).
    ///
    /// The compiled ONNX graph is shared process-wide (built once, cloned
    /// thereafter) — only the lightweight per-stream state is allocated here.
    /// This is the hot path: it runs once per recording, so rebuilding the graph
    /// every time previously leaked native ORT memory over long sessions.
    pub(crate) fn new_embedded(config: VadConfig) -> Result<Self> {
        let session = embedded_session()?;

        Ok(Self {
            session,
            state: ArrayD::zeros(STATE_SHAPE.as_slice()),
            context: vec![0.0; CONTEXT_SIZE],
            config,
        })
    }

    /// Get speech probability for a single CHUNK_SIZE (512) frame at 16kHz.
    ///
    /// The caller is responsible for providing exactly CHUNK_SIZE samples
    /// already at 16kHz. Capture resampling and accumulation belong to the
    /// caller; an incomplete frame is not a successful silence observation.
    pub fn predict(&mut self, samples: &[f32]) -> Result<f32> {
        anyhow::ensure!(
            samples.len() == CHUNK_SIZE,
            "Silero requires exactly {CHUNK_SIZE} samples at {VAD_SAMPLE_RATE} Hz"
        );
        anyhow::ensure!(
            samples.iter().all(|sample| sample.is_finite()),
            "Silero input contains nonfinite PCM"
        );
        self.predict_chunk(samples)
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

        // Input tensors — v5+ order: input, state, sr
        let input = ndarray::Array2::from_shape_vec([1, input_data.len()], input_data)
            .map_err(|e| anyhow::anyhow!("input shape: {}", e))?;
        let sr = ndarray::Array1::from_vec(vec![VAD_SAMPLE_RATE as i64]);
        // Commit state and context together only after a valid result.
        let state = self.state.clone();

        let input_value = Value::from_array(input)?;
        let state_value = Value::from_array(state)?;
        let sr_value = Value::from_array(sr)?;

        // Lock the shared session only for the inference call. The graph is
        // stateless across runs, so serializing concurrent streams here is
        // correct — each keeps its own `state`/`context`. Extract everything we
        // need from `outputs` (which borrows the guard) before the guard drops.
        let (prob, next_state) = {
            let mut session = self
                .session
                .lock()
                .map_err(|e| anyhow::anyhow!("Silero session lock poisoned: {e}"))?;
            let outputs = session.run([
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
                anyhow::ensure!(
                    data.len() == 1,
                    "Silero probability tensor must contain one value"
                );
                let probability = data[0];
                anyhow::ensure!(
                    probability.is_finite() && (0.0..=1.0).contains(&probability),
                    "Silero probability is invalid"
                );
                probability
            };

            // Read updated state from "stateN" (safe access — no panic on missing key)
            let next_state = {
                let state_output = outputs
                    .get("stateN")
                    .context("Silero model missing 'stateN' tensor")?;
                let (shape, data) = state_output.try_extract_tensor::<f32>()?;
                let shape_usize: Vec<usize> = shape.as_ref().iter().map(|&d| d as usize).collect();
                anyhow::ensure!(
                    shape_usize == STATE_SHAPE,
                    "Silero recurrent state has an unexpected shape"
                );
                anyhow::ensure!(
                    data.iter().all(|value| value.is_finite()),
                    "Silero recurrent state is nonfinite"
                );
                ArrayD::from_shape_vec(shape_usize.as_slice(), data.to_vec())
                    .context("Silero recurrent state tensor is malformed")?
            };

            (prob, next_state)
        };

        self.state = next_state;
        self.context
            .copy_from_slice(&chunk[CHUNK_SIZE - CONTEXT_SIZE..]);

        Ok(prob)
    }

    /// Reset internal state
    pub fn reset(&mut self) {
        self.state = ArrayD::zeros(STATE_SHAPE.as_slice());
        self.context.fill(0.0);
    }

    /// Configured probability above which a chunk counts as speech.
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
            // The accumulator hands the model exactly one 16 kHz frame.
            if let Ok(prob) = self.vad.predict(&chunk) {
                self.last_prob = prob;
            }
        }
        self.last_prob
    }

    /// Like [`feed`](Self::feed), but returns the **maximum** speech probability
    /// across all 512-sample chunks processed in THIS call instead of only the
    /// last chunk's probability.
    ///
    /// Window-based extraction (`vad::extract_speech`) feeds ~500ms slabs and
    /// keeps a slab when its probability clears the threshold. With `feed`'s
    /// last-chunk semantics, a slab containing a fully-spoken word would be
    /// dropped if it merely *ends* in the micro-pause after that word. Using the
    /// per-call max makes the keep/drop decision reflect any speech inside the
    /// window, not just its final 32ms.
    ///
    /// `last_prob` / [`probability`](Self::probability) are still updated to the
    /// final chunk's value, so callers relying on the latest-probability
    /// semantics (live indicators, turn detection) are unaffected.
    pub fn feed_max(&mut self, samples: &[f32]) -> f32 {
        let resampled = if let Some(ref mut r) = self.resampler {
            r.resample(samples)
        } else {
            samples.to_vec()
        };
        self.accumulator.extend_from_slice(&resampled);

        let mut max_prob = 0.0f32;
        let mut processed_any = false;
        while self.accumulator.len() >= CHUNK_SIZE {
            let chunk: Vec<f32> = self.accumulator.drain(..CHUNK_SIZE).collect();
            if let Ok(prob) = self.vad.predict(&chunk) {
                self.last_prob = prob;
                max_prob = max_prob.max(prob);
                processed_any = true;
            }
        }
        // No full chunk available this call (or all predicts errored): fall back
        // to the latest known probability, matching `feed`'s sub-chunk behavior.
        if processed_any {
            max_prob
        } else {
            self.last_prob
        }
    }

    /// Sibling of [`feed_max`](Self::feed_max): same resampling, accumulator
    /// drain, predict loop, `last_prob` updates and return value, plus a
    /// hop-order push of every 512-sample chunk probability into `sink`.
    ///
    /// One call replaces `feed_max` when a caller needs both the 500 ms max
    /// and Silero's native 32 ms timeline. Feeding the same window twice
    /// would advance Silero v6 state and change the second result.
    pub fn feed_trace(&mut self, samples: &[f32], sink: &mut Vec<f32>) -> f32 {
        let resampled = if let Some(ref mut r) = self.resampler {
            r.resample(samples)
        } else {
            samples.to_vec()
        };
        self.accumulator.extend_from_slice(&resampled);

        let mut max_prob = 0.0f32;
        let mut processed_any = false;
        while self.accumulator.len() >= CHUNK_SIZE {
            let chunk: Vec<f32> = self.accumulator.drain(..CHUNK_SIZE).collect();
            if let Ok(prob) = self.vad.predict(&chunk) {
                self.last_prob = prob;
                max_prob = max_prob.max(prob);
                processed_any = true;
                sink.push(prob);
            }
        }
        // No full chunk available this call (or all predicts errored): fall back
        // to the latest known probability, matching `feed`'s sub-chunk behavior.
        if processed_any {
            max_prob
        } else {
            self.last_prob
        }
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
        if let Some(resampler) = self.resampler.as_mut() {
            resampler.reset();
        }
        self.accumulator.clear();
        self.last_prob = 0.0;
    }
}

/// HuggingFace repo for Silero VAD model
const SILERO_VAD_REPO: &str = "snakers4/silero-vad";
/// ONNX file name inside that repo, and the name used in the models directory.
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

/// Resampler parity and embedded Silero session sharing regression tests.
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invalid_frames_refuse_without_mutating_stream_state() {
        let mut vad = SileroVad::new_embedded(VadConfig::default()).expect("embedded graph");
        vad.state.fill(0.25);
        vad.context.fill(0.125);
        let state = vad.state.clone();
        let context = vad.context.clone();
        for invalid in [vec![], vec![0.0; 511], vec![0.0; 513], vec![f32::NAN; 512]] {
            assert!(vad.predict(&invalid).is_err());
            assert_eq!(vad.state, state);
            assert_eq!(vad.context, context);
        }
    }

    #[test]
    fn resampler_capture_partition_does_not_change_pcm() {
        let input: Vec<f32> = (0..48_013).map(|i| (i as f32 * 0.013).sin()).collect();
        for rate in [8_000, 16_000, 44_100, 48_000] {
            let whole = Resampler::new(rate).resample(&input);
            for partition in [1, 17, 480, 960, 1024, 1536] {
                let mut resampler = Resampler::new(rate);
                let actual: Vec<f32> = input
                    .chunks(partition)
                    .flat_map(|chunk| resampler.resample(chunk))
                    .collect();
                assert_eq!(actual, whole, "rate={rate}, partition={partition}");
            }
        }
    }

    #[test]
    fn resampler_keeps_five_physical_speech_bursts() {
        let mut input = vec![0.0; 48_000];
        for start in [2400, 11040, 19680, 28320, 36960] {
            input[start..start + 2400].fill(0.25);
        }
        let expected: Vec<f32> = input.iter().step_by(3).copied().collect();
        let mut resampler = Resampler::new(48_000);
        let actual: Vec<f32> = input
            .chunks(1024)
            .flat_map(|chunk| resampler.resample(chunk))
            .collect();
        assert_eq!(
            actual, expected,
            "physical PCM positions must survive framing"
        );
        let bursts = actual
            .iter()
            .enumerate()
            .filter(|(i, sample)| **sample > 0.0 && (*i == 0 || actual[*i - 1] == 0.0))
            .count();
        assert_eq!(bursts, 5);
    }

    #[test]
    fn resampler_finish_and_reset_preserve_exact_capture_duration() {
        let input: Vec<f32> = (0..4409).map(|i| i as f32 / 4410.0).collect();
        for rate in [8000, 16000, 44100, 48000] {
            let mut resampler = Resampler::new(rate);
            let mut output: Vec<f32> = input
                .chunks(17)
                .flat_map(|chunk| resampler.resample(chunk))
                .collect();
            output.extend(resampler.finish());
            assert_eq!(output.len(), (input.len() * 16000).div_ceil(rate as usize));
            assert!(resampler.finish().is_empty());
            resampler.reset();
            let mut repeated = resampler.resample(&input);
            repeated.extend(resampler.finish());
            assert_eq!(
                output, repeated,
                "new stream must not inherit interpolation phase"
            );
        }
    }

    /// 48 kHz frames downsample to ~16 kHz length for Silero VAD input.
    #[test]
    fn test_resampler_48k_to_16k() {
        let mut resampler = Resampler::new(48000);

        // 48kHz input: 480 samples = 10ms
        let input: Vec<f32> = (0..480).map(|i| (i as f32 * 0.01).sin()).collect();

        // Should become ~160 samples at 16kHz
        let output = resampler.resample(&input);
        assert!((output.len() as i32 - 160).abs() <= 1);
    }

    /// Integer-rate decimation retains the physical samples, without rounding
    /// the frame's fractional phase back to zero on each callback.
    #[test]
    fn resample_value_parity_and_repeatable() {
        let mut resampler = Resampler::new(48000);
        let input: Vec<f32> = (0..480).map(|i| (i as f32 * 0.013).sin()).collect();
        for _ in 0..3 {
            let out = resampler.resample(&input);
            assert_eq!(out, input.iter().step_by(3).copied().collect::<Vec<_>>());
        }
    }

    /// Native 16 kHz input passes through without length change.
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

    /// Two embedded VAD instances share one ONNX session Arc (memory fix).
    #[test]
    fn embedded_session_is_shared_across_instances() {
        // The core of the memory fix: two embedded VAD instances must point at
        // ONE underlying ONNX session (graph built once, cloned thereafter),
        // instead of each rebuilding it and leaking native ORT memory.
        let a = SileroVad::new_embedded(VadConfig::default()).expect("first embedded VAD");
        let b = SileroVad::new_embedded(VadConfig::default()).expect("second embedded VAD");
        assert!(
            Arc::ptr_eq(&a.session, &b.session),
            "embedded Silero session must be shared, not rebuilt per instance"
        );
    }

    /// Shared session still isolates recurrent state and reset per instance.
    #[test]
    fn shared_session_keeps_per_instance_state() {
        // Sharing the session must not couple per-stream recurrent state: each
        // instance owns its `state`/`context`, so both predict correctly and a
        // reset on one does not disturb the other.
        let mut a = SileroVad::new_embedded(VadConfig::default()).expect("VAD a");
        let mut b = SileroVad::new_embedded(VadConfig::default()).expect("VAD b");

        let chunk = vec![0.0f32; CHUNK_SIZE];
        let pa = a.predict(&chunk).expect("predict a");
        let pb = b.predict(&chunk).expect("predict b");
        assert!((0.0..=1.0).contains(&pa), "prob a in range: {pa}");
        assert!((0.0..=1.0).contains(&pb), "prob b in range: {pb}");

        // Reset one stream; the other must keep working on the shared session.
        a.reset();
        let pb2 = b.predict(&chunk).expect("predict b after a.reset");
        assert!((0.0..=1.0).contains(&pb2), "prob b2 in range: {pb2}");
    }
}
