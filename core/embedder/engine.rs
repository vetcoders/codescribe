//! Embedder Engine - offline MiniLM embeddings via Candle BERT.
//!
//! Provides text embeddings using a local/embedded paraphrase-multilingual-MiniLM-L12-v2 model (fp16).
//! No runtime downloads; model must be embedded or present on disk.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::{Context, Result, anyhow};
use candle_core::{DType, Device, Tensor};
use candle_nn::VarBuilder;
use candle_transformers::models::bert::{BertModel, Config as BertConfig};
use tokenizers::{PaddingParams, PaddingStrategy, Tokenizer, TruncationParams};
use tracing::{debug, info};

use super::embedded;
use crate::{hf_cache, safe_path};

const DEFAULT_MAX_LENGTH: usize = 512;
const DEFAULT_REPO: &str = "sentence-transformers/paraphrase-multilingual-MiniLM-L12-v2";
const ENV_EMBEDDER_REPO: &str = "CODESCRIBE_EMBEDDER_REPO";

/// Process-lifetime Candle device for the embedder (same Metal-leak rationale as
/// Whisper: idle-unload must not call `Device::new_metal` again).
static PROCESS_DEVICE: OnceLock<Device> = OnceLock::new();

/// Set once the accelerated backend has been caught returning non-finite output.
/// Checked ahead of `PROCESS_DEVICE` so the demotion survives idle-unload without
/// having to re-derive the (already-created) Metal device.
static FORCE_CPU: AtomicBool = AtomicBool::new(false);

/// Permanently demote this process's embedder to the CPU. Called by the loader's
/// self-test; there is no way back, because a backend that produced NaN once has
/// no claim on the next inference.
pub(super) fn demote_to_cpu() {
    FORCE_CPU.store(true, Ordering::Relaxed);
}

/// Whether [`demote_to_cpu`] has fired in this process.
pub(super) fn is_demoted_to_cpu() -> bool {
    FORCE_CPU.load(Ordering::Relaxed)
}

/// Force the embedder onto a specific Candle device. Exists because a silent
/// backend fault is otherwise indistinguishable from bad weights: the embedder
/// was found returning 384 dimensions of NaN for every input, and the only way
/// to separate "Metal produced NaN" from "the model loads wrong" is to run the
/// same weights on the CPU. `cpu` forces CPU; anything else keeps the default
/// Metal-with-CPU-fallback.
const ENV_EMBEDDER_DEVICE: &str = "CODESCRIBE_EMBEDDER_DEVICE";

/// The Candle device for this process, created at most once.
///
/// The demotion flag is checked *before* `PROCESS_DEVICE` so a CPU fallback
/// survives idle-unload without re-deriving the Metal device. Metal acquisition
/// falls back to CPU on failure, and `CODESCRIBE_EMBEDDER_DEVICE=cpu` forces CPU
/// outright. Availability is not correctness: the loader still runs a
/// finite-output self-test on top of whatever this returns.
fn process_device() -> Device {
    if is_demoted_to_cpu() {
        return Device::Cpu;
    }
    PROCESS_DEVICE
        .get_or_init(|| {
            let forced_cpu = std::env::var(ENV_EMBEDDER_DEVICE)
                .map(|value| value.trim().eq_ignore_ascii_case("cpu"))
                .unwrap_or(false);
            let device = if forced_cpu {
                Device::Cpu
            } else {
                Device::new_metal(0).unwrap_or(Device::Cpu)
            };
            info!("Embedder process device acquired once: {device:?}");
            device
            // NOTE: correctness of this device is not assumed — `init` runs a
            // finite-output self-test and falls back to CPU when the backend
            // returns NaN. See `assert_backend_produces_finite_vectors`.
        })
        .clone()
}

/// The cached process device, if one was ever created — without creating it.
/// Used by the idle reaper to prune the Metal free-buffer pool after unload.
pub(super) fn cached_process_device() -> Option<Device> {
    PROCESS_DEVICE.get().cloned()
}

/// Weight dtype for the embedder. **Always f32** — this is a correctness fix,
/// not a preference.
///
/// `Device::bf16_default_to_f32()` reports bf16 as available on Metal, and
/// candle's bf16 BERT path on this stack returns NaN for every dimension of
/// every input (measured 2026-08-09, macOS 27.0: 384/384 NaN in bf16, unit-norm
/// vectors and sensible similarities in f32 on the same weights and the same
/// Metal device). Nothing ever surfaced because the only consumer compares the
/// result against a threshold, and every comparison with NaN is false — the
/// semantic gate read as "nothing to drop" while it was in fact blind, across
/// 378 real deliveries.
///
/// MiniLM is 384-dimensional and already small; f32 costs little and is the
/// only dtype proven to produce numbers here. Do not "optimise" this back to
/// bf16 without re-running the finite-output check on the target OS.
fn embedder_dtype(_device: &Device) -> DType {
    DType::F32
}

/// Configuration for the embedder
#[derive(Debug, Clone)]
pub struct EmbedderConfig {
    /// Optional explicit model path
    pub model_path: Option<PathBuf>,
    /// Override max token length (default from model config)
    pub max_length: Option<usize>,
    /// Prefer embedded model if available
    pub use_embedded: bool,
}

impl Default for EmbedderConfig {
    fn default() -> Self {
        Self {
            model_path: None,
            max_length: None,
            use_embedded: true,
        }
    }
}

impl EmbedderConfig {
    /// Create config with explicit model path
    pub fn with_model_path(path: PathBuf) -> Self {
        Self {
            model_path: Some(path),
            ..Default::default()
        }
    }

    /// Override max token length
    pub fn with_max_length(mut self, max_length: usize) -> Self {
        self.max_length = Some(max_length);
        self
    }

    /// Disable embedded model usage
    pub fn disable_embedded(mut self) -> Self {
        self.use_embedded = false;
        self
    }
}

/// Text embedding engine using Candle BERT (MiniLM)
pub struct EmbedderEngine {
    model: BertModel,
    tokenizer: Tokenizer,
    config: BertConfig,
    device: Device,
}

impl EmbedderEngine {
    /// Create a new embedder with default config
    pub fn new() -> Result<Self> {
        Self::with_config(EmbedderConfig::default())
    }

    /// Create with custom configuration
    pub fn with_config(mut config: EmbedderConfig) -> Result<Self> {
        let device = process_device();
        debug!("Embedder using device: {:?}", device);

        // Explicit overrides disable embedded usage.
        let repo_override = std::env::var(ENV_EMBEDDER_REPO)
            .ok()
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty());
        let path_override = std::env::var("CODESCRIBE_EMBEDDER_PATH")
            .ok()
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty());
        if config.model_path.is_some() || repo_override.is_some() || path_override.is_some() {
            config.use_embedded = false;
        }

        if config.use_embedded
            && let Some(embedded) = embedded::get_embedded_data()
        {
            return Self::from_embedded(&embedded, device, config.max_length);
        }

        let model_path = resolve_model_path(config.model_path.as_ref(), repo_override.as_deref())?;
        Self::from_path(&model_path, device, config.max_length)
    }

    /// Build the engine from weights compiled into the binary.
    ///
    /// Tensors are deserialized on the CPU and then moved, because the embedded
    /// weights are a plain byte slice with no device affinity.
    fn from_embedded(
        embedded: &embedded::EmbeddedModel,
        device: Device,
        max_length: Option<usize>,
    ) -> Result<Self> {
        let config: BertConfig = serde_json::from_slice(embedded.config)
            .context("Failed to parse embedded model config")?;
        let tokenizer = Tokenizer::from_bytes(embedded.tokenizer)
            .map_err(|e| anyhow!("Failed to load embedded tokenizer: {}", e))?;

        let tokenizer = prepare_tokenizer(tokenizer, &config, max_length)?;

        let dtype = embedder_dtype(&device);
        let tensors = candle_core::safetensors::load_buffer(embedded.weights, &Device::Cpu)
            .context("Failed to deserialize embedded model weights")?;
        let tensors = move_tensors_to_device(tensors, &device, dtype)?;
        let vb = VarBuilder::from_tensors(tensors, dtype, &device);
        let model = BertModel::load(vb, &config).context("Failed to load embedder model")?;

        info!(
            "Embedder initialized from embedded model (device: {:?}, dim={})",
            device, config.hidden_size
        );

        Ok(Self {
            model,
            tokenizer,
            config,
            device,
        })
    }

    /// Build the engine from a model directory on disk.
    ///
    /// Expects `config.json`, `tokenizer.json` and `model.safetensors`. A
    /// directory holding only the ONNX export is refused with a message naming
    /// both paths, since this loader is safetensors-only.
    fn from_path(model_path: &Path, device: Device, max_length: Option<usize>) -> Result<Self> {
        let config_path = model_path.join("config.json");
        let tokenizer_path = model_path.join("tokenizer.json");
        let weights_path = model_path.join("model.safetensors");
        if !weights_path.exists() {
            let onnx_path = model_path.join("model_optimized.onnx");
            return Err(anyhow!(
                "Unsupported embedder format. Expected model.safetensors at {} or ONNX at {}",
                weights_path.display(),
                onnx_path.display()
            ));
        }

        let config_str = safe_path::safe_read_to_string(&config_path)
            .with_context(|| format!("Failed to read {}", config_path.display()))?;
        let config: BertConfig =
            serde_json::from_str(&config_str).context("Failed to parse embedder config.json")?;

        let tokenizer_str = safe_path::safe_read_to_string(&tokenizer_path)
            .with_context(|| format!("Failed to read {}", tokenizer_path.display()))?;
        let tokenizer: Tokenizer = tokenizer_str
            .parse()
            .map_err(|e| anyhow!("Failed to load tokenizer: {}", e))?;
        let tokenizer = prepare_tokenizer(tokenizer, &config, max_length)?;

        let dtype = embedder_dtype(&device);
        let vb = unsafe {
            VarBuilder::from_mmaped_safetensors(&[&weights_path], dtype, &device)
                .context("Failed to load embedder weights")?
        };
        let model = BertModel::load(vb, &config).context("Failed to load embedder model")?;

        info!(
            "Embedder initialized from path: {} (device: {:?}, dim={})",
            model_path.display(),
            device,
            config.hidden_size
        );

        Ok(Self {
            model,
            tokenizer,
            config,
            device,
        })
    }

    /// Embed a single text
    pub fn embed(&mut self, text: &str) -> Result<Vec<f32>> {
        let vecs = self.embed_batch(&[text])?;
        vecs.into_iter()
            .next()
            .ok_or_else(|| anyhow!("No embedding generated"))
    }

    /// Embed a passage (document) for indexing
    pub fn embed_passage(&mut self, text: &str) -> Result<Vec<f32>> {
        let vecs = self.embed_passages(&[text])?;
        vecs.into_iter()
            .next()
            .ok_or_else(|| anyhow!("No embedding generated"))
    }

    /// Embed multiple texts at once
    pub fn embed_batch(&mut self, texts: &[&str]) -> Result<Vec<Vec<f32>>> {
        let inputs: Vec<String> = texts.iter().map(|t| t.to_string()).collect();
        self.embed_internal(&inputs)
    }

    /// Embed multiple passages at once
    pub fn embed_passages(&mut self, texts: &[&str]) -> Result<Vec<Vec<f32>>> {
        let inputs: Vec<String> = texts.iter().map(|t| t.to_string()).collect();
        self.embed_internal(&inputs)
    }

    /// The single inference path behind every `embed*` entry point.
    ///
    /// Runs BERT, mean-pools over the attention mask, L2-normalizes, then forces
    /// f32 on the CPU before extraction — an accelerated backend may hold the
    /// result in a narrower dtype, and `to_vec2::<f32>()` would fail on it.
    fn embed_internal(&mut self, inputs: &[String]) -> Result<Vec<Vec<f32>>> {
        let (input_ids, token_type_ids, attention_mask) = encode_batch(
            &self.tokenizer,
            inputs,
            self.config.pad_token_id as u32,
            self.device.clone(),
        )?;

        let outputs = self
            .model
            .forward(&input_ids, &token_type_ids, Some(&attention_mask))?;

        let pooled = mean_pool(&outputs, &attention_mask)?;
        let normalized = l2_normalize(&pooled)?;
        // Ensure f32 on CPU before extraction — Metal may keep tensors in bf16/f16
        let normalized = normalized.to_dtype(DType::F32)?;
        let normalized = normalized.to_device(&Device::Cpu)?;
        normalized
            .to_vec2::<f32>()
            .context("Failed to convert embeddings to Vec")
    }

    /// Calculate cosine similarity between two embeddings
    pub fn similarity(a: &[f32], b: &[f32]) -> f32 {
        if a.len() != b.len() || a.is_empty() {
            return 0.0;
        }

        let dot: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
        let norm_a: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
        let norm_b: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();

        if norm_a == 0.0 || norm_b == 0.0 {
            return 0.0;
        }

        dot / (norm_a * norm_b)
    }

    /// Get embedding dimension
    pub fn dimension(&self) -> usize {
        self.config.hidden_size
    }

    /// Get the device being used
    pub fn device(&self) -> &Device {
        &self.device
    }
}

/// Locate a model directory: explicit path, then `CODESCRIBE_EMBEDDER_PATH`,
/// then an HF cache snapshot for the configured or default repo.
///
/// Never downloads. Failure returns an error naming the exact commands and env
/// vars that would fix it.
fn resolve_model_path(explicit: Option<&PathBuf>, repo_override: Option<&str>) -> Result<PathBuf> {
    if let Some(path) = explicit {
        return Ok(path.clone());
    }

    if let Ok(path) = std::env::var("CODESCRIBE_EMBEDDER_PATH") {
        let p = PathBuf::from(path);
        if model_files_present(&p) {
            return Ok(p);
        }
    }

    if let Some(repo) = repo_override {
        if let Some(snapshot) = hf_cache::find_snapshot_with_any(
            repo,
            &["config.json", "tokenizer.json"],
            &["model.safetensors", "model_optimized.onnx"],
        ) {
            return Ok(snapshot);
        }
    } else if let Some(snapshot) = hf_cache::find_snapshot_with_any(
        DEFAULT_REPO,
        &["config.json", "tokenizer.json"],
        &["model.safetensors", "model_optimized.onnx"],
    ) {
        return Ok(snapshot);
    }

    Err(anyhow!(
        "Embedder model not found. Run: hf download {} (uses cache) or set CODESCRIBE_EMBEDDER_PATH / {}",
        repo_override.unwrap_or(DEFAULT_REPO),
        ENV_EMBEDDER_REPO
    ))
}

/// Whether `path` holds a usable model: both config files plus safetensors or
/// ONNX weights.
///
/// Broader than what [`EmbedderEngine::from_path`] accepts — this only gates
/// *candidate* directories during resolution.
fn model_files_present(path: &Path) -> bool {
    let has_config = path.join("config.json").exists() && path.join("tokenizer.json").exists();
    let has_weights = path.join("model.safetensors").exists();
    let has_onnx = path.join("model_optimized.onnx").exists();
    has_config && (has_weights || has_onnx)
}

/// Pin padding and truncation on a freshly loaded tokenizer.
///
/// Effective length is `min(override_or_model_max, DEFAULT_MAX_LENGTH)`, so a
/// caller cannot request a window wider than the model or than the 512-token
/// ceiling. Padding is batch-longest, which keeps single-text calls cheap.
fn prepare_tokenizer(
    tokenizer: Tokenizer,
    config: &BertConfig,
    max_length_override: Option<usize>,
) -> Result<Tokenizer> {
    let max_len = max_length_override
        .unwrap_or(config.max_position_embeddings)
        .min(DEFAULT_MAX_LENGTH);

    let pad_id = config.pad_token_id as u32;
    let pad_token = tokenizer
        .id_to_token(pad_id)
        .unwrap_or_else(|| "[PAD]".to_string());

    let mut tokenizer = tokenizer;
    tokenizer.with_padding(Some(PaddingParams {
        strategy: PaddingStrategy::BatchLongest,
        pad_id,
        pad_token,
        ..Default::default()
    }));
    tokenizer
        .with_truncation(Some(TruncationParams {
            max_length: max_len,
            ..Default::default()
        }))
        .map_err(anyhow::Error::msg)?;

    Ok(tokenizer)
}

/// Tokenize a batch into the `(input_ids, token_type_ids, attention_mask)`
/// triple BERT expects, each shaped `[batch, max_len]`.
///
/// Rows are re-padded here even though the tokenizer pads: the widths are
/// re-derived from the actual encodings so a tokenizer configured differently
/// cannot produce a ragged tensor. Missing type ids default to zeros
/// (single-segment input); the mask is f32 so it can be broadcast during pooling.
fn encode_batch(
    tokenizer: &Tokenizer,
    inputs: &[String],
    pad_id: u32,
    device: Device,
) -> Result<(Tensor, Tensor, Tensor)> {
    let encodings = tokenizer
        .encode_batch(inputs.to_vec(), true)
        .map_err(|e| anyhow!("Tokenization failed: {}", e))?;

    let max_len = encodings.iter().map(|e| e.len()).max().unwrap_or(0);

    let mut input_ids = Vec::with_capacity(encodings.len() * max_len);
    let mut token_type_ids = Vec::with_capacity(encodings.len() * max_len);
    let mut attention_mask = Vec::with_capacity(encodings.len() * max_len);

    for enc in encodings {
        let ids = enc.get_ids();
        let types = enc.get_type_ids();
        let mask = enc.get_attention_mask();

        let mut ids_vec = ids.to_vec();
        let mut type_vec = if types.is_empty() {
            vec![0u32; ids.len()]
        } else {
            types.to_vec()
        };
        let mut mask_vec = mask.to_vec();

        pad_to(&mut ids_vec, max_len, pad_id);
        pad_to(&mut type_vec, max_len, 0);
        pad_to(&mut mask_vec, max_len, 0);

        input_ids.extend_from_slice(&ids_vec);
        token_type_ids.extend_from_slice(&type_vec);
        attention_mask.extend_from_slice(&mask_vec);
    }

    let batch = inputs.len();
    let input_ids = Tensor::from_vec(input_ids, (batch, max_len), &device)?.to_dtype(DType::I64)?;
    let token_type_ids =
        Tensor::from_vec(token_type_ids, (batch, max_len), &device)?.to_dtype(DType::I64)?;
    let attention_mask =
        Tensor::from_vec(attention_mask, (batch, max_len), &device)?.to_dtype(DType::F32)?;

    Ok((input_ids, token_type_ids, attention_mask))
}

/// Right-pad `vec` to `target_len` with `pad`. Longer input is left untouched.
fn pad_to(vec: &mut Vec<u32>, target_len: usize, pad: u32) {
    if vec.len() < target_len {
        vec.extend(std::iter::repeat_n(pad, target_len - vec.len()));
    }
}

/// Mask-aware mean pooling: `[batch, seq, hidden]` → `[batch, hidden]`.
///
/// This is the pooling sentence-transformers trained the MiniLM checkpoint with,
/// so it is a correctness requirement, not a choice. Padding positions are
/// zeroed before summing and excluded from the divisor; an epsilon guards the
/// all-padding row against division by zero.
fn mean_pool(hidden: &Tensor, mask: &Tensor) -> Result<Tensor> {
    // hidden: [batch, seq, hidden], mask: [batch, seq]
    let dtype = hidden.dtype();
    let mask = mask.to_dtype(dtype)?;
    let mask = mask.unsqueeze(2)?; // [batch, seq, 1]
    let masked = hidden.broadcast_mul(&mask)?;
    let sum = masked.sum(1)?; // [batch, hidden]
    let counts = mask.sum(1)?; // [batch, 1]
    let eps = Tensor::from_vec(vec![1e-9f32], (1,), hidden.device())?.to_dtype(dtype)?;
    let counts = counts.broadcast_add(&eps)?;
    Ok(sum.broadcast_div(&counts)?)
}

/// Scale each row to unit length, with an epsilon so a zero row stays finite.
///
/// Unit-norm output is what lets [`EmbedderEngine::similarity`] read as cosine
/// similarity.
fn l2_normalize(t: &Tensor) -> Result<Tensor> {
    let dtype = t.dtype();
    let squared = t.sqr()?;
    let sum = squared.sum(1)?.unsqueeze(1)?;
    let norm = sum.sqrt()?;
    let eps = Tensor::from_vec(vec![1e-9f32], (1,), t.device())?.to_dtype(dtype)?;
    let norm = norm.broadcast_add(&eps)?;
    Ok(t.broadcast_div(&norm)?)
}

/// Cast every tensor to `dtype` and move it onto `device`, preserving names.
///
/// The cast runs before the move so the conversion happens on the CPU rather
/// than on the accelerator.
fn move_tensors_to_device(
    tensors: std::collections::HashMap<String, Tensor>,
    device: &Device,
    dtype: DType,
) -> Result<std::collections::HashMap<String, Tensor>> {
    let mut result = std::collections::HashMap::with_capacity(tensors.len());

    for (name, tensor) in tensors {
        let mut t = tensor;
        if t.dtype() != dtype {
            t = t.to_dtype(dtype)?;
        }
        t = t.to_device(device)?;
        result.insert(name, t);
    }

    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_similarity_identical() {
        let a = vec![1.0, 0.0, 0.0];
        let b = vec![1.0, 0.0, 0.0];
        let sim = EmbedderEngine::similarity(&a, &b);
        assert!((sim - 1.0).abs() < 0.001);
    }

    #[test]
    fn test_similarity_orthogonal() {
        let a = vec![1.0, 0.0, 0.0];
        let b = vec![0.0, 1.0, 0.0];
        let sim = EmbedderEngine::similarity(&a, &b);
        assert!(sim.abs() < 0.001);
    }
}
