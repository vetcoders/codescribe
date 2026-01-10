use anyhow::{anyhow, ensure, Context, Result};
use std::collections::HashMap;
use std::env;
use std::fs::File;
use std::io::{Cursor, Read};
use std::path::Path;

use candle_core::safetensors::Load;
use candle_core::{Device, DType, IndexOp, Tensor};
use candle_transformers::models::whisper::{self as whisper, Config};
use ndarray::Array2;
use ndarray_npy::ReadNpyExt;
use tokenizers::Tokenizer;

use crate::audio_loader;
use crate::whisper_model::Whisper as Model;

pub struct LocalWhisperEngine {
    model: Model,
    tokenizer: Tokenizer,
    device: Device,
    config: Config,
    mel_filters: Vec<f32>,
}

impl LocalWhisperEngine {
    pub fn new(model_path: &Path) -> Result<Self> {
        let device = Device::new_metal(0).unwrap_or(Device::Cpu);
        tracing::debug!("LocalWhisperEngine using device: {:?}", device);

        let config_path = model_path.join("config.json");
        let weights_path = model_path.join("weights.safetensors");
        let tokenizer_path = model_path.join("tokenizer.json");
        let mel_filters_path = model_path.join("mel_filters.npz");

        let config_str = std::fs::read_to_string(&config_path)
            .context(format!("Failed to read config from {}", config_path.display()))?;

        // Parse MLX config and map to Candle Config
        let mlx_config: serde_json::Value = serde_json::from_str(&config_str)
            .context("Failed to parse MLX config json")?;

        let n_mels = mlx_config["n_mels"].as_u64().unwrap_or(80);
        let new_config_json = serde_json::json!({
            "num_mel_bins": n_mels,
            "max_source_positions": mlx_config["n_audio_ctx"].as_u64().unwrap_or(1500),
            "d_model": mlx_config["n_audio_state"].as_u64().unwrap_or(512),
            "encoder_attention_heads": mlx_config["n_audio_head"].as_u64().unwrap_or(8),
            "encoder_layers": mlx_config["n_audio_layer"].as_u64().unwrap_or(6),
            "vocab_size": mlx_config["n_vocab"].as_u64().unwrap_or(51865),
            "decoder_attention_heads": mlx_config["n_text_head"].as_u64().unwrap_or(8),
            "decoder_layers": mlx_config["n_text_layer"].as_u64().unwrap_or(6),
            "max_target_positions": mlx_config["n_text_ctx"].as_u64().unwrap_or(448),
            "activation_function": "gelu",
            // defaults
            "dropout": 0.0,
            "attention_dropout": 0.0,
            "activation_dropout": 0.0,
            "init_std": 0.02,
            "encoder_layerdrop": 0.0,
            "decoder_layerdrop": 0.0,
            "use_cache": true,
            "scale_embedding": false
        });

        let config: Config = serde_json::from_value(new_config_json)
            .context("Failed to build Config from MLX values")?;

        let vb = unsafe {
            let tensors = candle_core::safetensors::MmapedSafetensors::new(&weights_path)?;
            let mut raw_tensors: HashMap<String, Tensor> = HashMap::new();

            // Load everything on CPU first so we can dequantize packed weights.
            for (name, view) in tensors.tensors() {
                let loaded = view.load(&Device::Cpu)?;
                raw_tensors.insert(name.to_string(), loaded);
            }

            let mut tensor_map = HashMap::new();
            let mut quantized_weights: Vec<String> = Vec::new();

            // First pass: handle non-quantized tensors and collect quantized weight names.
            for (name, tensor) in raw_tensors.iter() {
                if name.ends_with(".weight") && tensor.dtype() == DType::U32 {
                    quantized_weights.push(name.clone());
                    continue;
                }

                if name.ends_with(".scales") || name.ends_with(".biases") {
                    continue;
                }

                let mapped_name = map_tensor_name(name);
                let mut t = tensor.clone();
                if t.dtype() != DType::F32 {
                    t = t.to_dtype(DType::F32)?;
                }

                // Fix shape for conv weights (MLX [out, kernel, in] -> Candle [out, in, kernel])
                if mapped_name.ends_with("conv1.weight") || mapped_name.ends_with("conv2.weight") {
                    let dims = t.dims();
                    if dims.len() == 3 && dims[1] == 3 {
                        t = t.permute((0, 2, 1))?.contiguous()?;
                    }
                }

                let t = t.to_device(&device)?;
                tensor_map.insert(mapped_name, t);
            }

            // Second pass: dequantize packed q8 weights.
            for weight_name in quantized_weights {
                let base = weight_name.trim_end_matches(".weight");
                let packed = raw_tensors
                    .get(&weight_name)
                    .context(format!("Missing packed tensor for {}", weight_name))?;
                let scales_key = format!("{}.scales", base);
                let biases_key = format!("{}.biases", base);
                let scales = raw_tensors
                    .get(&scales_key)
                    .context(format!("Missing scales tensor for {}", weight_name))?;
                let biases = raw_tensors
                    .get(&biases_key)
                    .context(format!("Missing biases tensor for {}", weight_name))?;

                let mut dequant = dequantize_q8(packed, scales, biases, &device)?;
                let mapped_name = map_tensor_name(&weight_name);

                if mapped_name.ends_with("conv1.weight") || mapped_name.ends_with("conv2.weight") {
                    let dims = dequant.dims();
                    if dims.len() == 3 && dims[1] == 3 {
                        dequant = dequant.permute((0, 2, 1))?.contiguous()?;
                    }
                }

                tensor_map.insert(mapped_name, dequant);
            }

            candle_nn::VarBuilder::from_tensors(tensor_map, DType::F32, &device)
        };

        let model = Model::load(&vb, config.clone())
            .context("Failed to create Whisper Model")?;

        let tokenizer = Tokenizer::from_file(&tokenizer_path)
            .map_err(|e| anyhow!("Failed to load tokenizer from {}: {}", tokenizer_path.display(), e))?;

        // Load mel filters
        if !mel_filters_path.exists() {
             return Err(anyhow!("mel_filters.npz not found at {}. Please download it from OpenAI assets.", mel_filters_path.display()));
        }

        let n_mels = config.num_mel_bins;
        let mel_filters = load_mel_filters(&mel_filters_path, n_mels)
            .context("Failed to load mel filters")?;

        Ok(Self {
            model,
            tokenizer,
            device,
            config,
            mel_filters,
        })
    }

    pub fn transcribe_file_with_language(
        &mut self,
        path: &Path,
        language: Option<&str>,
    ) -> Result<String> {
        let (samples, sample_rate) = audio_loader::load_audio_file(path)
            .context("Failed to load audio file")?;

        tracing::debug!(
            "Loaded audio file {:?}: {} samples @ {} Hz",
            path,
            samples.len(),
            sample_rate
        );

        self.transcribe_with_language(&samples, sample_rate, language)
    }

    #[allow(dead_code)]
    pub fn transcribe_file(&mut self, path: &Path) -> Result<String> {
        self.transcribe_file_with_language(path, None)
    }

    pub fn transcribe_with_language(
        &mut self,
        audio: &[f32],
        sample_rate: u32,
        language: Option<&str>,
    ) -> Result<String> {
        let samples = audio_loader::resample_to_16k(audio, sample_rate);
        let debug_tokens = env::var("CODESCRIBE_DEBUG_TOKENS")
            .map(|v| v != "0" && v.to_lowercase() != "false")
            .unwrap_or(false);

        tracing::debug!(
            "Resampled audio: {} samples -> {} samples ({} Hz -> 16000 Hz)",
            audio.len(),
            samples.len(),
            sample_rate
        );

        self.model.reset_kv_cache();

        // Convert to mel
        let mel = whisper::audio::pcm_to_mel(&self.config, &samples, &self.mel_filters);
        let mel_len = mel.len();
        let mel = Tensor::from_vec(mel, (1, self.config.num_mel_bins, mel_len / self.config.num_mel_bins), &self.device)?;

        // Decode
        // Tokenizer setup
        let start_token = self.tokenizer.token_to_id("<|startoftranscript|>").unwrap();
        let eot_token = self.tokenizer.token_to_id("<|endoftext|>").unwrap();
        let nospeech_token = self.tokenizer.token_to_id("<|nospeech|>");

        // Initial tokens: <|startoftranscript|> <|lang|>? <|transcribe|> <|notimestamps|> (optional)
        let mut tokens = vec![start_token];
        if let Some(lang) = language {
            let lang_tok = format!("<|{}|>", lang.to_lowercase());
            if let Some(t) = self.tokenizer.token_to_id(&lang_tok) {
                tokens.push(t);
            }
        }
        if let Some(t) = self.tokenizer.token_to_id("<|transcribe|>") {
            tokens.push(t);
        }
        if let Some(t) = self.tokenizer.token_to_id("<|notimestamps|>") {
            tokens.push(t);
        }

        let mut all_tokens = Vec::new();

        // Run encoder once
        let encoder_output = self.model.encoder.forward(&mel, true)?;

        // Decoder loop – allow up to the configured maximum target positions
        for step in 0..self.config.max_target_positions {
            let token_tensor = Tensor::new(tokens.as_slice(), &self.device)?.unsqueeze(0)?;
            let hidden = self.model.decoder.forward(&token_tensor, &encoder_output, true)?;
            let logits = self.model.decoder.final_linear(&hidden)?;

            // Greedy: pick max logit
            let (_b, seq_len, _vocab) = logits.dims3()?;
            let last_logits = logits.i((.., seq_len-1, ..))?.squeeze(0)?;
            let mut logits_vec = last_logits.to_vec1::<f32>()?;

            // Avoid terminating immediately when nothing has been emitted yet
            let suppress_tokens = all_tokens.len() < 16;
            if suppress_tokens {
                if (eot_token as usize) < logits_vec.len() {
                    logits_vec[eot_token as usize] = f32::NEG_INFINITY;
                }
                if let Some(nos) = nospeech_token {
                    if (nos as usize) < logits_vec.len() {
                        logits_vec[nos as usize] = f32::NEG_INFINITY;
                    }
                }
            }

            let mut best_token = eot_token;
            let mut best_val = f32::NEG_INFINITY;
            for (idx, &val) in logits_vec.iter().enumerate() {
                if val > best_val {
                    best_val = val;
                    best_token = idx as u32;
                }
            }

            if debug_tokens && step < 16 {
                if let Some(tok) = self.tokenizer.id_to_token(best_token) {
                    tracing::debug!(step, best_token, best_val, token = %tok, "decoder step");
                } else {
                    tracing::debug!(step, best_token, best_val, "decoder step (token decode failed)");
                }
            }

            let next_token = best_token;

            if next_token == eot_token {
                if all_tokens.is_empty() {
                    // If we got EOT immediately, try to continue (maybe silence?)
                    // But for now let's just break
                    break;
                } else {
                    break;
                }
            }

            tokens.push(next_token);
            all_tokens.push(next_token);
        }

        let text = self
            .tokenizer
            .decode(&all_tokens, true)
            .map_err(|e| anyhow!("Tokenizer error: {}", e))?;

        Ok(text)
    }
}

fn load_mel_filters(path: &Path, n_mels: usize) -> Result<Vec<f32>> {
    let file = File::open(path)?;
    let mut zip = zip::ZipArchive::new(file)?;

    let key = format!("mel_{}", n_mels);
    let candidates = [format!("{}.npy", key), key.clone()];

    let mut buf = Vec::new();
    let mut found = false;
    for name in candidates {
        if let Ok(mut f) = zip.by_name(&name) {
            f.read_to_end(&mut buf)?;
            found = true;
            break;
        }
    }

    if !found {
        anyhow::bail!("mel filter {} not found in npz", key);
    }

    let cursor = Cursor::new(buf);
    let array: Array2<f32> = <Array2<f32> as ReadNpyExt>::read_npy(cursor)
        .context("Failed to parse mel filters npy")?;
    let (data, _) = array.into_raw_vec_and_offset();
    Ok(data)
}

fn map_tensor_name(name: &str) -> String {
    let mut new_name = name.to_string();

    new_name = new_name.replace("blocks", "layers");
    new_name = new_name.replace("mlp1", "fc1");
    new_name = new_name.replace("mlp2", "fc2");
    new_name = new_name.replace("decoder.ln", "decoder.layer_norm");
    // Replace cross-attn layer norms before generic attn replacement to avoid mangling
    new_name = new_name.replace("cross_attn_ln", "encoder_attn_layer_norm");
    new_name = new_name.replace("attn_ln", "self_attn_layer_norm");
    new_name = new_name.replace("mlp_ln", "final_layer_norm");
    new_name = new_name.replace("ln_post", "layer_norm");

    // Important: handle cross_attn BEFORE attn
    new_name = new_name.replace("cross_attn", "encoder_attn");

    // Replace ".attn." segment with ".self_attn."
    new_name = new_name.replace(".attn.", ".self_attn.");

    // Projections
    new_name = new_name.replace("query", "q_proj");
    new_name = new_name.replace("key", "k_proj");
    new_name = new_name.replace("value", "v_proj");
    new_name = new_name.replace(".out.", ".out_proj.");

    // Embedding aliases
    new_name = new_name.replace("decoder.token_embedding", "decoder.embed_tokens");

    // Prefix
    if !new_name.starts_with("model.") {
        new_name = format!("model.{}", new_name);
    }

    // Positional embedding key from MLX
    if new_name == "model.decoder.positional_embedding" {
        new_name = "model.decoder.embed_positions.weight".to_string();
    }
    new_name = new_name.replace(".biases", ".bias");
    new_name
}

#[allow(clippy::needless_range_loop)]
fn dequantize_q8(packed: &Tensor, scales: &Tensor, biases: &Tensor, device: &Device) -> Result<Tensor> {
    ensure!(packed.dtype() == DType::U32, "Packed tensor must be u32");

    let packed_dims = packed.dims();
    ensure!(packed_dims.len() == 2, "Packed weight must be 2D");
    let out_dim = packed_dims[0];
    let packed_in = packed_dims[1];
    let in_dim = packed_in * 4;

    let scales_dims = scales.dims();
    let biases_dims = biases.dims();
    ensure!(
        scales_dims.len() == 2 && biases_dims.len() == 2,
        "Scales and biases must be 2D"
    );
    ensure!(
        scales_dims[0] == out_dim && biases_dims[0] == out_dim,
        "Scales/biases out dimension mismatch"
    );

    let group_size = 32usize;
    let expected_groups = in_dim / group_size;
    ensure!(
        scales_dims[1] == expected_groups && biases_dims[1] == expected_groups,
        "Scales/biases group dimension mismatch"
    );

    let packed_data = packed.to_vec2::<u32>()?;
    let scales_data = scales.to_dtype(DType::F32)?.to_vec2::<f32>()?;
    let biases_data = biases.to_dtype(DType::F32)?.to_vec2::<f32>()?;

    let mut output: Vec<f32> = Vec::with_capacity(out_dim * in_dim);

    for o in 0..out_dim {
        for p in 0..packed_in {
            let val = packed_data[o][p];
            for b in 0..4 {
                let idx = p * 4 + b;
                let group = idx / group_size;
                // Treat as uint8
                let w = ((val >> (8 * b)) & 0xff) as u8;
                let scale = scales_data[o][group];
                let bias = biases_data[o][group];
                output.push((w as f32) * scale + bias);
            }
        }
    }

    Ok(Tensor::from_vec(output, (out_dim, in_dim), device)?)
}
