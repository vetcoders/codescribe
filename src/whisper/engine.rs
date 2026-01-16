//! Local Whisper STT engine implementation.
//!
//! This module contains the LocalWhisperEngine struct that handles
//! local speech-to-text transcription using Candle and Whisper models.
//!
//! Supports two loading modes:
//! - `new(path)` - load from filesystem (development, external models)
//! - `from_embedded()` - load from binary-embedded bytes (production, zero I/O)
//!
//! Created by M&K (c)2026 VetCoders

use anyhow::{Context, Result, anyhow, ensure};
use std::collections::HashMap;
use std::env;
use std::io::{Cursor, Read, Write};
use std::path::Path;

use flate2::Compression;
use flate2::write::GzEncoder;
use rand::Rng;

use candle_core::safetensors::Load;
use candle_core::{DType, Device, IndexOp, Tensor};
use candle_transformers::models::whisper::{self as whisper, Config};
use ndarray::Array2;
use ndarray_npy::ReadNpyExt;
use tokenizers::Tokenizer;

use super::model::Whisper as Model;
use crate::audio::loader as audio_loader;
use crate::safe_path;

use super::embedded::EmbeddedModel;
use super::params::DecodingParams;

/// Callback for streaming chunk results (called after each chunk is transcribed)
pub type ChunkCallback<'a> = &'a dyn Fn(&str);

pub struct LocalWhisperEngine {
    model: Model,
    tokenizer: Tokenizer,
    device: Device,
    config: Config,
    mel_filters: Vec<f32>,
    pub decoding_params: DecodingParams,
}

impl LocalWhisperEngine {
    pub fn new(model_path: &Path) -> Result<Self> {
        let device = Device::new_metal(0).unwrap_or(Device::Cpu);
        tracing::debug!("LocalWhisperEngine using device: {:?}", device);

        let config_path = model_path.join("config.json");
        let weights_path = model_path.join("weights.safetensors");
        let tokenizer_path = model_path.join("tokenizer.json");
        let mel_filters_path = model_path.join("mel_filters.npz");

        let config_str = safe_path::safe_read_to_string(&config_path)?;

        // Parse MLX config and map to Candle Config
        let mlx_config: serde_json::Value =
            serde_json::from_str(&config_str).context("Failed to parse MLX config json")?;

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

        let model = Model::load(&vb, config.clone()).context("Failed to create Whisper Model")?;

        let tokenizer = Tokenizer::from_file(&tokenizer_path).map_err(|e| {
            anyhow!(
                "Failed to load tokenizer from {}: {}",
                tokenizer_path.display(),
                e
            )
        })?;

        // Load mel filters
        if !mel_filters_path.exists() {
            return Err(anyhow!(
                "mel_filters.npz not found at {}. Please download it from OpenAI assets.",
                mel_filters_path.display()
            ));
        }

        let n_mels = config.num_mel_bins;
        let mel_filters =
            load_mel_filters(&mel_filters_path, n_mels).context("Failed to load mel filters")?;

        Ok(Self {
            model,
            tokenizer,
            device,
            config,
            mel_filters,
            decoding_params: DecodingParams::default(),
        })
    }

    /// Create engine from embedded model bytes - zero disk I/O!
    ///
    /// Model data is `include_bytes!` from binary at compile time.
    /// At runtime: bytes → tensors → GPU. No temp files, no extraction.
    pub fn from_embedded(embedded: &EmbeddedModel) -> Result<Self> {
        let device = Device::new_metal(0).unwrap_or(Device::Cpu);
        tracing::info!(
            "Loading embedded Whisper model ({:.1} MB) to {:?}",
            embedded.total_size() as f64 / 1_000_000.0,
            device
        );

        // Parse config from bytes
        let config_str = std::str::from_utf8(embedded.config)
            .context("Invalid UTF-8 in embedded config.json")?;
        let mlx_config: serde_json::Value =
            serde_json::from_str(config_str).context("Failed to parse embedded config json")?;

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
            .context("Failed to build Config from embedded MLX values")?;

        // Load weights directly from bytes - NO DISK I/O!
        let raw_tensors = candle_core::safetensors::load_buffer(embedded.weights, &Device::Cpu)
            .context("Failed to deserialize embedded weights")?;

        let vb = build_varbuilder_from_tensors(raw_tensors, &device)?;
        let model = Model::load(&vb, config.clone()).context("Failed to create Whisper Model")?;

        // Load tokenizer from bytes
        let tokenizer = Tokenizer::from_bytes(embedded.tokenizer)
            .map_err(|e| anyhow!("Failed to load embedded tokenizer: {}", e))?;

        // Load mel filters from bytes
        let mel_filters = load_mel_filters_from_bytes(embedded.mel_filters, n_mels as usize)
            .context("Failed to load embedded mel filters")?;

        tracing::info!("Embedded Whisper model loaded successfully");

        Ok(Self {
            model,
            tokenizer,
            device,
            config,
            mel_filters,
            decoding_params: DecodingParams::default(),
        })
    }

    /// Create a new LocalWhisperEngine with custom decoding parameters.
    pub fn new_with_params(model_path: &Path, params: DecodingParams) -> Result<Self> {
        let mut engine = Self::new(model_path)?;
        engine.decoding_params = params;
        Ok(engine)
    }

    /// Get current decoding parameters.
    #[allow(dead_code)] // Public API for external consumers
    pub fn decoding_params(&self) -> &DecodingParams {
        &self.decoding_params
    }

    pub fn transcribe_file_with_language(
        &mut self,
        path: &Path,
        language: Option<&str>,
    ) -> Result<String> {
        let (samples, sample_rate) =
            audio_loader::load_audio_file(path).context("Failed to load audio file")?;

        let duration_secs = samples.len() as f32 / sample_rate as f32;
        tracing::debug!(
            "Loaded audio file {:?}: {} samples @ {} Hz ({:.1}s)",
            path,
            samples.len(),
            sample_rate,
            duration_secs
        );

        // Use chunking for all files - handles both short and long audio
        self.transcribe_long_with_language(&samples, sample_rate, language)
    }

    #[allow(dead_code)] // Used by tauri-app
    pub fn detect_language_file(&mut self, path: &Path) -> Result<String> {
        let (samples, sample_rate) =
            audio_loader::load_audio_file(path).context("Failed to load audio file")?;
        self.detect_language(&samples, sample_rate)
    }

    #[allow(dead_code)] // Used by tauri-app
    pub fn transcribe_file(&mut self, path: &Path) -> Result<String> {
        self.transcribe_file_with_language(path, None)
    }

    #[allow(dead_code)] // Used by tauri-app
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

        let detected_lang;
        let language = match language {
            Some(l) => Some(l),
            None => {
                detected_lang = self.detect_language_16k(&samples)?;
                Some(detected_lang.as_str())
            }
        };

        self.transcribe_samples_16k(&samples, language, debug_tokens)
    }

    #[allow(dead_code)] // Used by tauri-app
    pub fn transcribe_long(&mut self, audio: &[f32], sample_rate: u32) -> Result<String> {
        self.transcribe_long_with_language(audio, sample_rate, None)
    }

    pub fn transcribe_long_with_language(
        &mut self,
        audio: &[f32],
        sample_rate: u32,
        language: Option<&str>,
    ) -> Result<String> {
        self.transcribe_long_streaming(audio, sample_rate, language, None)
    }

    /// Transcribe long audio with streaming callback
    /// Callback is called after each chunk with cumulative transcription so far
    pub fn transcribe_long_streaming(
        &mut self,
        audio: &[f32],
        sample_rate: u32,
        language: Option<&str>,
        on_chunk: Option<ChunkCallback>,
    ) -> Result<String> {
        let samples = audio_loader::resample_to_16k(audio, sample_rate);
        let debug_tokens = env::var("CODESCRIBE_DEBUG_TOKENS")
            .map(|v| v != "0" && v.to_lowercase() != "false")
            .unwrap_or(false);

        let detected_lang;
        let language = match language {
            Some(l) => Some(l),
            None => {
                detected_lang = self.detect_language_16k(&samples)?;
                tracing::info!("Detected language: {}", detected_lang);
                Some(detected_lang.as_str())
            }
        };

        let chunk_samples = 16_000usize * 25; // 25 seconds
        let overlap = 16_000usize * 5; // 5 seconds overlap
        ensure!(chunk_samples > overlap, "chunk_samples must be > overlap");
        let step = chunk_samples - overlap;

        let total_chunks = (samples.len().saturating_sub(1) / step) + 1;
        let mut out = String::new();
        let mut offset = 0usize;
        let mut chunk_num = 0usize;

        while offset < samples.len() {
            chunk_num += 1;
            let end = (offset + chunk_samples).min(samples.len());
            let chunk = &samples[offset..end];

            tracing::debug!(
                "Processing chunk {}/{} ({} samples)",
                chunk_num,
                total_chunks,
                chunk.len()
            );

            let text = self.transcribe_samples_16k(chunk, language, debug_tokens)?;
            append_with_overlap_dedup(&mut out, &text);

            // Call streaming callback with cumulative result
            if let Some(ref callback) = on_chunk {
                callback(out.trim());
            }

            offset = offset.saturating_add(step);
        }

        Ok(out.trim().to_string())
    }

    pub fn detect_language(&mut self, audio: &[f32], sample_rate: u32) -> Result<String> {
        let samples = audio_loader::resample_to_16k(audio, sample_rate);
        self.detect_language_16k(&samples)
    }

    fn detect_language_16k(&mut self, samples_16k: &[f32]) -> Result<String> {
        let max_samples = 16_000usize * 30;
        let samples = &samples_16k[..samples_16k.len().min(max_samples)];
        ensure!(!samples.is_empty(), "audio is empty");

        self.model.reset_kv_cache();

        let mel = whisper::audio::pcm_to_mel(&self.config, samples, &self.mel_filters);
        let mel_len = mel.len();
        let mel = Tensor::from_vec(
            mel,
            (
                1,
                self.config.num_mel_bins,
                mel_len / self.config.num_mel_bins,
            ),
            &self.device,
        )?;

        let encoder_output = self.model.encoder.forward(&mel, true)?;

        let start_token = self
            .tokenizer
            .token_to_id("<|startoftranscript|>")
            .ok_or_else(|| anyhow!("Tokenizer missing <|startoftranscript|>"))?;

        let token_tensor = Tensor::new(&[start_token], &self.device)?.unsqueeze(0)?;
        let hidden = self
            .model
            .decoder
            .forward(&token_tensor, &encoder_output, true)?;
        let logits = self.model.decoder.final_linear(&hidden)?;
        let (_b, seq_len, _vocab) = logits.dims3()?;
        let last_logits = logits.i((.., seq_len - 1, ..))?.squeeze(0)?;
        let logits_vec = last_logits.to_vec1::<f32>()?;

        let candidates = self.language_token_candidates(logits_vec.len());
        ensure!(
            !candidates.is_empty(),
            "No language token candidates available in tokenizer"
        );

        let mut best_lang = "en".to_string();
        let mut best_score = f32::NEG_INFINITY;
        for (token_id, lang) in candidates {
            let idx = token_id as usize;
            if idx >= logits_vec.len() {
                continue;
            }
            let score = logits_vec[idx];
            if score > best_score {
                best_score = score;
                best_lang = lang;
            }
        }

        Ok(best_lang)
    }

    fn language_token_candidates(&self, vocab_size: usize) -> Vec<(u32, String)> {
        // Whisper language tokens are typically in this range.
        const LANG_TOKEN_START: u32 = 50_259;
        const LANG_TOKEN_END: u32 = 50_358;

        let mut out = Vec::new();
        for id in LANG_TOKEN_START..=LANG_TOKEN_END {
            if (id as usize) >= vocab_size {
                break;
            }
            if let Some(tok) = self.tokenizer.id_to_token(id) {
                if let Some(lang) = parse_language_token(&tok) {
                    out.push((id, lang.to_string()));
                }
            }
        }

        if !out.is_empty() {
            return out;
        }

        // Fallback: common languages only.
        let fallback = [
            "en", "pl", "de", "fr", "es", "it", "pt", "nl", "ru", "uk", "cs", "sk",
        ];
        for lang in fallback {
            let tok = format!("<|{}|>", lang);
            if let Some(id) = self.tokenizer.token_to_id(&tok) {
                if (id as usize) < vocab_size {
                    out.push((id, lang.to_string()));
                }
            }
        }
        out
    }

    fn transcribe_samples_16k(
        &mut self,
        samples_16k: &[f32],
        language: Option<&str>,
        debug_tokens: bool,
    ) -> Result<String> {
        ensure!(!samples_16k.is_empty(), "audio is empty");

        self.model.reset_kv_cache();

        // Convert to mel
        let mel = whisper::audio::pcm_to_mel(&self.config, samples_16k, &self.mel_filters);
        let mel_len = mel.len();
        let mel = Tensor::from_vec(
            mel,
            (
                1,
                self.config.num_mel_bins,
                mel_len / self.config.num_mel_bins,
            ),
            &self.device,
        )?;

        // Decode
        let start_token = self
            .tokenizer
            .token_to_id("<|startoftranscript|>")
            .ok_or_else(|| anyhow!("Tokenizer missing <|startoftranscript|>"))?;
        let eot_token = self
            .tokenizer
            .token_to_id("<|endoftext|>")
            .ok_or_else(|| anyhow!("Tokenizer missing <|endoftext|>"))?;
        let nospeech_token = self.tokenizer.token_to_id("<|nospeech|>");

        // Initial tokens: <|startoftranscript|> <|lang|>? <|transcribe|> <|notimestamps|>
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

        // Decoder loop – allow up to the configured maximum target positions minus initial tokens
        let max_new_tokens = self
            .config
            .max_target_positions
            .saturating_sub(tokens.len());
        let ngram_size = self.decoding_params.no_repeat_ngram_size;

        let mut sum_logprob = 0.0f32;
        let mut token_count = 0usize;

        for step in 0..max_new_tokens {
            let token_tensor = Tensor::new(tokens.as_slice(), &self.device)?.unsqueeze(0)?;
            let hidden = self
                .model
                .decoder
                .forward(&token_tensor, &encoder_output, true)?;
            let logits = self.model.decoder.final_linear(&hidden)?;

            // Get logits for last position
            let (_b, seq_len, _vocab) = logits.dims3()?;
            let last_logits = logits.i((.., seq_len - 1, ..))?.squeeze(0)?;
            let mut logits_vec = last_logits.to_vec1::<f32>()?;

            // 3. No-Speech Threshold (no_speech_threshold)
            if step == 0 {
                if let Some(nos) = nospeech_token {
                    let nos_idx = nos as usize;
                    if nos_idx < logits_vec.len() {
                        // Compute softmax probability for nospeech only
                        let max_val = logits_vec.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
                        let exp_sum: f32 = logits_vec.iter().map(|&x| (x - max_val).exp()).sum();
                        let nos_prob = (logits_vec[nos_idx] - max_val).exp() / exp_sum;

                        if nos_prob > self.decoding_params.no_speech_threshold {
                            tracing::debug!("No speech detected (prob={:.3})", nos_prob);
                            return Ok(String::new()); // Return empty for silence
                        }
                    }
                }
            }

            // 2. Suppress Blank (suppress_blank)
            if self.decoding_params.suppress_blank && all_tokens.len() < 4 {
                // Block common blank tokens (space, empty, etc.)
                // Token IDs depend on tokenizer - check whisper tokenizer
                let blank_tokens = [220, 50256];
                for &tok in &blank_tokens {
                    if tok < logits_vec.len() {
                        logits_vec[tok] = f32::NEG_INFINITY;
                    }
                }
            }

            // Apply no_repeat_ngram blocking (faster-whisper style)
            // Block tokens that would create a repeated n-gram
            // Need at least ngram_size tokens to have a potential repeat
            if ngram_size > 0 && all_tokens.len() >= ngram_size {
                // Look at last (ngram_size - 1) tokens as prefix
                let prefix_start = all_tokens.len() + 1 - ngram_size;
                let prefix = &all_tokens[prefix_start..];

                // Find all earlier positions where this (n-1)-gram occurred
                let search_end = all_tokens.len() - ngram_size + 1;
                for i in 0..search_end {
                    if all_tokens[i..i + ngram_size - 1] == *prefix {
                        // Block the token that followed this n-gram
                        let blocked_token = all_tokens[i + ngram_size - 1] as usize;
                        if blocked_token < logits_vec.len() {
                            logits_vec[blocked_token] = f32::NEG_INFINITY;
                        }
                    }
                }
            }

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

            // Select token (greedy or sampling)
            let (best_token, best_val) = if self.decoding_params.temperature > 0.0 {
                // Apply temperature scaling
                let temp = self.decoding_params.temperature;
                let scaled: Vec<f32> = logits_vec.iter().map(|&x| x / temp).collect();

                // Softmax
                let max_val = scaled.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
                let exp_sum: f32 = scaled.iter().map(|&x| (x - max_val).exp()).sum();
                let probs: Vec<f32> = scaled
                    .iter()
                    .map(|&x| (x - max_val).exp() / exp_sum)
                    .collect();

                // Sample from distribution
                let mut rng = rand::thread_rng();
                let r: f32 = rng.r#gen();
                let mut cumsum = 0.0;
                let mut selected = 0u32;
                for (idx, &p) in probs.iter().enumerate() {
                    cumsum += p;
                    if r < cumsum {
                        selected = idx as u32;
                        break;
                    }
                }
                let val = logits_vec[selected as usize];
                (selected, val)
            } else {
                // Greedy (default)
                let mut best_token = eot_token;
                let mut best_val = f32::NEG_INFINITY;
                for (idx, &val) in logits_vec.iter().enumerate() {
                    if val > best_val {
                        best_val = val;
                        best_token = idx as u32;
                    }
                }
                (best_token, best_val)
            };

            // Track logprobs (5. Logprob Threshold)
            {
                let max_val = logits_vec.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
                let exp_sum: f32 = logits_vec.iter().map(|&x| (x - max_val).exp()).sum();
                let token_prob = (logits_vec[best_token as usize] - max_val).exp() / exp_sum;
                sum_logprob += token_prob.ln();
                token_count += 1;
            }

            if debug_tokens && step < 16 {
                if let Some(tok) = self.tokenizer.id_to_token(best_token) {
                    tracing::debug!(step, best_token, best_val, token = %tok, "decoder step");
                } else {
                    tracing::debug!(
                        step,
                        best_token,
                        best_val,
                        "decoder step (token decode failed)"
                    );
                }
            }

            if best_token == eot_token {
                break;
            }

            tokens.push(best_token);
            all_tokens.push(best_token);
        }

        let text = self
            .tokenizer
            .decode(&all_tokens, true)
            .map_err(|e| anyhow!("Tokenizer error: {}", e))?;

        // 5. Logprob Threshold
        if token_count > 0 {
            let avg_logprob = sum_logprob / token_count as f32;
            if avg_logprob < self.decoding_params.logprob_threshold {
                tracing::warn!(
                    "Low avg logprob ({:.2}) - possible hallucination",
                    avg_logprob
                );
            }
        }

        // 4. Compression Ratio Threshold
        let ratio = compression_ratio(&text);
        if ratio > self.decoding_params.compression_ratio_threshold {
            tracing::warn!(
                "High compression ratio ({:.2}) - possible hallucination",
                ratio
            );
        }

        Ok(text)
    }
}

fn parse_language_token(token: &str) -> Option<&str> {
    if !token.starts_with("<|") || !token.ends_with("|>") {
        return None;
    }
    let inner = &token[2..token.len() - 2];
    if inner.len() < 2 || inner.len() > 3 {
        return None;
    }
    if inner.chars().all(|c| c.is_ascii_alphabetic()) {
        Some(inner)
    } else {
        None
    }
}

/// Helper for deduplication at chunk boundaries
pub fn append_with_overlap_dedup(out: &mut String, segment: &str) {
    let seg = segment.trim();
    if seg.is_empty() {
        return;
    }

    if out.trim().is_empty() {
        out.push_str(seg);
        return;
    }

    let out_trim = out.trim_end();
    let out_words: Vec<&str> = out_trim.split_whitespace().collect();
    let seg_words: Vec<&str> = seg.split_whitespace().collect();
    if out_words.is_empty() || seg_words.is_empty() {
        if !out.ends_with(' ') {
            out.push(' ');
        }
        out.push_str(seg);
        return;
    }

    let max_overlap = out_words.len().min(seg_words.len()).min(20);
    let mut overlap = 0usize;
    for k in (1..=max_overlap).rev() {
        if out_words[out_words.len() - k..] == seg_words[..k] {
            overlap = k;
            break;
        }
    }

    if !out.ends_with(' ') {
        out.push(' ');
    }

    if overlap >= seg_words.len() {
        return;
    }
    if overlap > 0 {
        out.push_str(&seg_words[overlap..].join(" "));
    } else {
        out.push_str(seg);
    }
}

fn load_mel_filters(path: &Path, n_mels: usize) -> Result<Vec<f32>> {
    let file = safe_path::safe_open(path)?;
    load_mel_filters_from_reader(file, n_mels)
}

/// Load mel filters from bytes (for embedded model)
fn load_mel_filters_from_bytes(data: &[u8], n_mels: usize) -> Result<Vec<f32>> {
    let cursor = Cursor::new(data);
    load_mel_filters_from_reader(cursor, n_mels)
}

/// Common mel filter loading logic
fn load_mel_filters_from_reader<R: Read + std::io::Seek>(
    reader: R,
    n_mels: usize,
) -> Result<Vec<f32>> {
    let mut zip = zip::ZipArchive::new(reader)?;

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
    let array: Array2<f32> =
        <Array2<f32> as ReadNpyExt>::read_npy(cursor).context("Failed to parse mel filters npy")?;
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

fn compression_ratio(text: &str) -> f32 {
    let original_len = text.len();
    if original_len == 0 {
        return 0.0;
    }

    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(text.as_bytes()).ok();
    let compressed = encoder.finish().unwrap_or_default();

    original_len as f32 / compressed.len() as f32
}

fn dequantize_q8(
    packed: &Tensor,
    scales: &Tensor,
    biases: &Tensor,
    device: &Device,
) -> Result<Tensor> {
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

    for (o, packed_row) in packed_data.iter().enumerate() {
        for (p, &val) in packed_row.iter().enumerate() {
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

/// Build VarBuilder from raw tensors with Q8 dequantization
///
/// Handles MLX quantized weights (packed U32 + scales + biases)
/// and converts tensor names to Candle format.
fn build_varbuilder_from_tensors(
    raw_tensors: HashMap<String, Tensor>,
    device: &Device,
) -> Result<candle_nn::VarBuilder<'static>> {
    let mut tensor_map = HashMap::new();
    let mut quantized_weights: Vec<String> = Vec::new();

    // First pass: handle non-quantized tensors and collect quantized weight names
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

        let t = t.to_device(device)?;
        tensor_map.insert(mapped_name, t);
    }

    // Second pass: dequantize packed Q8 weights
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

        let mut dequant = dequantize_q8(packed, scales, biases, device)?;
        let mapped_name = map_tensor_name(&weight_name);

        if mapped_name.ends_with("conv1.weight") || mapped_name.ends_with("conv2.weight") {
            let dims = dequant.dims();
            if dims.len() == 3 && dims[1] == 3 {
                dequant = dequant.permute((0, 2, 1))?.contiguous()?;
            }
        }

        tensor_map.insert(mapped_name, dequant);
    }

    Ok(candle_nn::VarBuilder::from_tensors(
        tensor_map,
        DType::F32,
        device,
    ))
}
