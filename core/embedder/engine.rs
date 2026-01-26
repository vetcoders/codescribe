//! Embedder Engine - offline E5 embeddings via Candle BERT.
//!
//! Provides text embeddings using a local/embedded multilingual-e5-large model.
//! No runtime downloads; model must be embedded or present on disk.
//!
//! Created by M&K (c)2026 VetCoders

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow};
use candle_core::{DType, Device, Tensor};
use candle_nn::VarBuilder;
use candle_transformers::models::bert::{BertModel, Config as BertConfig};
use tokenizers::{PaddingParams, PaddingStrategy, Tokenizer, TruncationParams};
use tracing::{debug, info};

use super::DEFAULT_MODEL;
use super::embedded;
use crate::config::Config;
use crate::safe_path;

const DEFAULT_MAX_LENGTH: usize = 512;

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

/// Text embedding engine using Candle BERT (E5)
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
    pub fn with_config(config: EmbedderConfig) -> Result<Self> {
        let device = Device::new_metal(0).unwrap_or(Device::Cpu);
        debug!("Embedder using device: {:?}", device);

        if config.use_embedded
            && let Some(embedded) = embedded::get_embedded_data()
        {
            return Self::from_embedded(&embedded, device, config.max_length);
        }

        let model_path = resolve_model_path(config.model_path.as_ref())?;
        Self::from_path(&model_path, device, config.max_length)
    }

    fn from_embedded(
        embedded: &embedded::EmbeddedE5,
        device: Device,
        max_length: Option<usize>,
    ) -> Result<Self> {
        let config: BertConfig = serde_json::from_slice(embedded.config)
            .context("Failed to parse embedded E5 config")?;
        let tokenizer = Tokenizer::from_bytes(embedded.tokenizer)
            .map_err(|e| anyhow!("Failed to load embedded tokenizer: {}", e))?;

        let tokenizer = prepare_tokenizer(tokenizer, &config, max_length)?;

        let dtype = device.bf16_default_to_f32();
        let tensors = candle_core::safetensors::load_buffer(embedded.weights, &Device::Cpu)
            .context("Failed to deserialize embedded E5 weights")?;
        let tensors = move_tensors_to_device(tensors, &device, dtype)?;
        let vb = VarBuilder::from_tensors(tensors, dtype, &device);
        let model = BertModel::load(vb, &config).context("Failed to load E5 model")?;

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

    fn from_path(model_path: &Path, device: Device, max_length: Option<usize>) -> Result<Self> {
        let config_path = model_path.join("config.json");
        let tokenizer_path = model_path.join("tokenizer.json");
        let weights_path = model_path.join("model.safetensors");

        let config_str = safe_path::safe_read_to_string(&config_path)
            .with_context(|| format!("Failed to read {}", config_path.display()))?;
        let config: BertConfig =
            serde_json::from_str(&config_str).context("Failed to parse E5 config.json")?;

        let tokenizer_str = safe_path::safe_read_to_string(&tokenizer_path)
            .with_context(|| format!("Failed to read {}", tokenizer_path.display()))?;
        let tokenizer: Tokenizer = tokenizer_str
            .parse()
            .map_err(|e| anyhow!("Failed to load tokenizer: {}", e))?;
        let tokenizer = prepare_tokenizer(tokenizer, &config, max_length)?;

        let dtype = device.bf16_default_to_f32();
        let vb = unsafe {
            VarBuilder::from_mmaped_safetensors(&[&weights_path], dtype, &device)
                .context("Failed to load E5 weights")?
        };
        let model = BertModel::load(vb, &config).context("Failed to load E5 model")?;

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

    /// Embed a single text (query)
    ///
    /// For queries (search), the text is prefixed with "query: "
    pub fn embed(&mut self, text: &str) -> Result<Vec<f32>> {
        let vecs = self.embed_batch(&[text])?;
        vecs.into_iter()
            .next()
            .ok_or_else(|| anyhow!("No embedding generated"))
    }

    /// Embed a passage (document) for indexing
    ///
    /// Passages are prefixed with "passage: " for optimal retrieval
    pub fn embed_passage(&mut self, text: &str) -> Result<Vec<f32>> {
        let vecs = self.embed_passages(&[text])?;
        vecs.into_iter()
            .next()
            .ok_or_else(|| anyhow!("No embedding generated"))
    }

    /// Embed multiple texts at once (queries)
    pub fn embed_batch(&mut self, texts: &[&str]) -> Result<Vec<Vec<f32>>> {
        let inputs: Vec<String> = texts.iter().map(|t| format!("query: {}", t)).collect();
        self.embed_internal(&inputs)
    }

    /// Embed multiple passages at once (documents)
    pub fn embed_passages(&mut self, texts: &[&str]) -> Result<Vec<Vec<f32>>> {
        let inputs: Vec<String> = texts.iter().map(|t| format!("passage: {}", t)).collect();
        self.embed_internal(&inputs)
    }

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

fn resolve_model_path(explicit: Option<&PathBuf>) -> Result<PathBuf> {
    if let Some(path) = explicit {
        return Ok(path.clone());
    }

    if let Ok(path) = std::env::var("CODESCRIBE_EMBEDDER_PATH") {
        let p = PathBuf::from(path);
        if model_files_present(&p) {
            return Ok(p);
        }
    }

    let config_dir = Config::config_dir();
    let candidates = [
        config_dir.join("models").join(DEFAULT_MODEL),
        PathBuf::from("models").join(DEFAULT_MODEL),
    ];

    for candidate in candidates {
        if model_files_present(&candidate) {
            return Ok(candidate);
        }
    }

    Err(anyhow!(
        "E5 model not found. Set CODESCRIBE_EMBEDDER_PATH or download with: ./scripts/download-e5.sh"
    ))
}

fn model_files_present(path: &Path) -> bool {
    path.join("config.json").exists()
        && path.join("tokenizer.json").exists()
        && path.join("model.safetensors").exists()
}

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

fn pad_to(vec: &mut Vec<u32>, target_len: usize, pad: u32) {
    if vec.len() < target_len {
        vec.extend(std::iter::repeat_n(pad, target_len - vec.len()));
    }
}

fn mean_pool(hidden: &Tensor, mask: &Tensor) -> Result<Tensor> {
    // hidden: [batch, seq, hidden], mask: [batch, seq]
    let mask = mask.unsqueeze(2)?; // [batch, seq, 1]
    let masked = hidden.broadcast_mul(&mask)?;
    let sum = masked.sum(1)?; // [batch, hidden]
    let counts = mask.sum(1)?; // [batch, 1]
    let eps = Tensor::from_vec(vec![1e-9f32], (1,), hidden.device())?;
    let counts = counts.broadcast_add(&eps)?;
    Ok(sum.broadcast_div(&counts)?)
}

fn l2_normalize(t: &Tensor) -> Result<Tensor> {
    let squared = t.sqr()?;
    let sum = squared.sum(1)?.unsqueeze(1)?;
    let norm = sum.sqrt()?;
    let eps = Tensor::from_vec(vec![1e-9f32], (1,), t.device())?;
    let norm = norm.broadcast_add(&eps)?;
    Ok(t.broadcast_div(&norm)?)
}

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
