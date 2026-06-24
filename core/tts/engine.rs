//! TTS Engine implementation using CSM-1B from Sesame.
//!
//! This module contains the TtsEngine struct that handles local text-to-speech
//! synthesis using Candle and the CSM model with Mimi codec for audio decoding.
//!
//! Supports two loading modes:
//! - `new(path)` - load from filesystem (development, external models)
//! - `from_embedded()` - load from binary-embedded bytes (production, zero I/O)

use std::collections::HashMap;
use std::path::Path;

use anyhow::{Context, Result, anyhow};
use candle_core::{DType, Device, Tensor};
use candle_nn::VarBuilder;
use candle_transformers::generation::LogitsProcessor;
use candle_transformers::models::csm::{Config as CsmConfig, Model as CsmModel};
use candle_transformers::models::mimi::{Config as MimiConfig, Model as MimiModel};
use tokenizers::Tokenizer;
use tracing::{debug, info};

use super::embedded::EmbeddedTts;
use crate::{hf_cache, safe_path};

/// Default CSM model output sample rate
const SAMPLE_RATE: u32 = 24000;

/// End-of-text token for Llama tokenizer
const EOT_TOKEN: &str = "<|end_of_text|>";

/// Maximum frames to generate (prevents infinite loops)
const MAX_FRAMES: usize = 1000;

/// TTS Engine using CSM-1B + Mimi codec
pub struct TtsEngine {
    /// CSM model for text-to-audio-codes generation
    csm: CsmModel,
    /// Mimi codec for audio codes to PCM decoding
    mimi: MimiModel,
    /// Llama tokenizer for text processing
    tokenizer: Tokenizer,
    /// Compute device (Metal/CPU)
    device: Device,
    /// CSM configuration
    _config: CsmConfig,
    /// Sample rate (24kHz)
    pub sample_rate: u32,
}

impl TtsEngine {
    /// Create engine from filesystem path (development mode)
    ///
    /// Expected directory structure:
    /// ```text
    /// model_path/
    /// ├── config.json          # CSM config
    /// ├── tokenizer.json       # Llama tokenizer
    /// ├── model.safetensors    # CSM weights
    /// └── mimi.safetensors     # Mimi codec weights (optional, uses default config)
    /// ```
    pub fn new(model_path: &Path) -> Result<Self> {
        let device = Device::new_metal(0).unwrap_or(Device::Cpu);
        debug!("TtsEngine using device: {:?}", device);

        // Load CSM config
        let config_path = model_path.join("config.json");
        let config_str = safe_path::safe_read_to_string(&config_path)
            .context("Failed to read CSM config.json")?;
        let config: CsmConfig =
            serde_json::from_str(&config_str).context("Failed to parse CSM config")?;

        // Load tokenizer
        let tokenizer_path = model_path.join("tokenizer.json");
        let tokenizer = Tokenizer::from_file(&tokenizer_path)
            .map_err(|e| anyhow!("Failed to load tokenizer: {}", e))?;

        // Load CSM model weights
        let weights_path = model_path.join("model.safetensors");
        let dtype = device.bf16_default_to_f32();
        let vb = unsafe {
            VarBuilder::from_mmaped_safetensors(&[&weights_path], dtype, &device)
                .context("Failed to load CSM weights")?
        };
        let csm = CsmModel::new(&config, vb).context("Failed to create CSM model")?;

        // Load Mimi codec with default v0.1 config
        let mimi_weights_path = model_path.join("mimi.safetensors");
        let mimi_config = MimiConfig::v0_1(None);

        let mimi = if mimi_weights_path.exists() {
            let mimi_vb = unsafe {
                VarBuilder::from_mmaped_safetensors(&[&mimi_weights_path], dtype, &device)
                    .context("Failed to load Mimi weights")?
            };
            MimiModel::new(mimi_config, mimi_vb).context("Failed to create Mimi model")?
        } else if let Some(snapshot) =
            hf_cache::find_snapshot("kyutai/mimi", &["model.safetensors"])
        {
            let cached_path = snapshot.join("model.safetensors");
            let mimi_vb = unsafe {
                VarBuilder::from_mmaped_safetensors(&[&cached_path], dtype, &device)
                    .context("Failed to load Mimi weights")?
            };
            MimiModel::new(mimi_config, mimi_vb).context("Failed to create Mimi model")?
        } else {
            return Err(anyhow!(
                "Mimi codec weights not found. Run:\n\
                 - hf download kyutai/mimi\n\
                 - hf download sesame/csm-1b"
            ));
        };

        info!(
            "TTS engine initialized from path: {} (device: {:?})",
            model_path.display(),
            device
        );

        Ok(Self {
            csm,
            mimi,
            tokenizer,
            device,
            _config: config,
            sample_rate: SAMPLE_RATE,
        })
    }

    /// Create engine from embedded model bytes - zero disk I/O!
    ///
    /// Model data is `include_bytes!` from binary at compile time.
    /// At runtime: bytes → tensors → GPU. No temp files, no extraction.
    pub fn from_embedded(embedded: &EmbeddedTts) -> Result<Self> {
        let device = Device::new_metal(0).unwrap_or(Device::Cpu);
        debug!("TtsEngine (embedded) using device: {:?}", device);

        // Parse CSM config
        let config: CsmConfig = serde_json::from_slice(embedded.config)
            .context("Failed to parse embedded CSM config")?;

        // Load tokenizer from bytes
        let tokenizer = Tokenizer::from_bytes(embedded.tokenizer)
            .map_err(|e| anyhow!("Failed to load embedded tokenizer: {}", e))?;

        // Load CSM model from bytes using load_buffer (like Whisper does)
        let dtype = device.bf16_default_to_f32();
        let csm_tensors: HashMap<String, Tensor> =
            candle_core::safetensors::load_buffer(embedded.weights, &Device::Cpu)
                .context("Failed to deserialize CSM weights")?;

        // Move tensors to target device and build VarBuilder
        let csm_tensors = move_tensors_to_device(csm_tensors, &device, dtype)?;
        let vb = VarBuilder::from_tensors(csm_tensors, dtype, &device);
        let csm = CsmModel::new(&config, vb).context("Failed to create CSM model from embedded")?;

        // Load Mimi codec from bytes with default v0.1 config
        let mimi_config = MimiConfig::v0_1(None);
        let mimi_tensors: HashMap<String, Tensor> =
            candle_core::safetensors::load_buffer(embedded.mimi_weights, &Device::Cpu)
                .context("Failed to deserialize Mimi weights")?;

        let mimi_tensors = move_tensors_to_device(mimi_tensors, &device, dtype)?;
        let mimi_vb = VarBuilder::from_tensors(mimi_tensors, dtype, &device);
        let mimi =
            MimiModel::new(mimi_config, mimi_vb).context("Failed to create Mimi from embedded")?;

        info!(
            "TTS engine initialized from embedded model (device: {:?}, size: {:.1} MB)",
            device,
            embedded.total_size() as f64 / 1_000_000.0
        );

        Ok(Self {
            csm,
            mimi,
            tokenizer,
            device,
            _config: config,
            sample_rate: SAMPLE_RATE,
        })
    }

    /// Synthesize text to audio samples
    ///
    /// Returns f32 PCM samples at 24kHz sample rate.
    pub fn synthesize(&mut self, text: &str) -> Result<Vec<f32>> {
        self.synthesize_with_speaker(text, 0)
    }

    /// Synthesize text with specific speaker index
    pub fn synthesize_with_speaker(&mut self, text: &str, speaker_idx: usize) -> Result<Vec<f32>> {
        // Clear KV cache for fresh generation
        self.csm.clear_kv_cache();

        // Format text with speaker index
        let prompt = format!("[{}]{}{}", speaker_idx, text, EOT_TOKEN);
        debug!("TTS prompt: {}", prompt);

        // Tokenize text
        let encoding = self
            .tokenizer
            .encode(prompt.as_str(), false)
            .map_err(|e| anyhow!("Tokenization failed: {}", e))?;
        let text_tokens: Vec<u32> = encoding.get_ids().to_vec();
        debug!("Text tokens: {} tokens", text_tokens.len());

        // Convert to tensors - shape [batch, seq_len]
        let tokens = Tensor::new(text_tokens.as_slice(), &self.device)?
            .unsqueeze(0)?
            .to_dtype(DType::I64)?;
        let mask = Tensor::ones(tokens.dims(), DType::F32, &self.device)?;

        // Generate audio codes
        let mut logits_processor = LogitsProcessor::new(42, None, None);
        let mut all_codes: Vec<Vec<u32>> = Vec::new();

        for frame_idx in 0..MAX_FRAMES {
            // Generate one frame of audio codes
            // API: generate_frame(&tokens, &mask, pos, &mut logits_processor) -> Result<Vec<u32>>
            let frame: Vec<u32> =
                self.csm
                    .generate_frame(&tokens, &mask, frame_idx, &mut logits_processor)?;

            // Check for end of generation (all zeros)
            if frame.iter().all(|&x| x == 0) {
                debug!("Generation complete at frame {}", frame_idx);
                break;
            }

            all_codes.push(frame);
        }

        if all_codes.is_empty() {
            return Err(anyhow!("No audio frames generated"));
        }

        debug!("Generated {} audio frames", all_codes.len());

        // Convert to tensor and decode with Mimi
        let audio_samples = self.decode_audio_codes_vec(&all_codes)?;

        Ok(audio_samples)
    }

    /// Decode RVQ audio codes (Vec format) to PCM samples using Mimi codec
    fn decode_audio_codes_vec(&mut self, codes: &[Vec<u32>]) -> Result<Vec<f32>> {
        if codes.is_empty() {
            return Ok(Vec::new());
        }

        let num_frames = codes.len();
        let num_codebooks = codes[0].len();

        // Create tensor [num_codebooks, num_frames] by transposing the data
        // codes is [frames, codebooks], we need [codebooks, frames]
        let mut flat_codes: Vec<u32> = Vec::with_capacity(num_codebooks * num_frames);
        for cb_idx in 0..num_codebooks {
            for frame in codes {
                flat_codes.push(frame.get(cb_idx).copied().unwrap_or(0));
            }
        }

        let codes_tensor = Tensor::new(flat_codes.as_slice(), &self.device)?
            .reshape((num_codebooks, num_frames))?
            .to_dtype(DType::I64)?;

        self.decode_audio_codes(&codes_tensor)
    }

    /// Decode RVQ audio codes (Tensor format) to PCM samples using Mimi codec
    fn decode_audio_codes(&mut self, codes: &Tensor) -> Result<Vec<f32>> {
        // Add batch dimension if needed: [codebooks, frames] -> [batch, codebooks, frames]
        let codes = if codes.dims().len() == 2 {
            codes.unsqueeze(0)?
        } else {
            codes.clone()
        };

        // Decode with Mimi
        let audio = self.mimi.decode(&codes)?;

        // Extract samples: [batch, channels, samples] -> Vec<f32>
        let samples: Vec<f32> = audio.squeeze(0)?.squeeze(0)?.to_vec1()?;

        Ok(samples)
    }

    /// Get the compute device
    pub fn device(&self) -> &Device {
        &self.device
    }
}

/// Move tensors from CPU to target device with optional dtype conversion
fn move_tensors_to_device(
    tensors: HashMap<String, Tensor>,
    device: &Device,
    dtype: DType,
) -> Result<HashMap<String, Tensor>> {
    let mut result = HashMap::with_capacity(tensors.len());

    for (name, tensor) in tensors {
        let mut t = tensor;

        // Convert dtype if needed
        if t.dtype() != dtype {
            t = t.to_dtype(dtype)?;
        }

        // Move to device
        t = t.to_device(device)?;

        result.insert(name, t);
    }

    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sample_rate() {
        assert_eq!(SAMPLE_RATE, 24000);
    }
}
