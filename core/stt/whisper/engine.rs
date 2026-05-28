//! Local Whisper STT engine implementation.
//!
//! This module contains the LocalWhisperEngine struct that handles
//! local speech-to-text transcription using Candle and Whisper models.
//!
//! Supports two loading modes:
//! - `new(path)` - load from filesystem (development, external models)
//! - `from_embedded()` - load from binary-embedded bytes (production, zero I/O)

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
use super::timestamps::{self, TimestampRange};
use crate::audio::loader as audio_loader;
use crate::pipeline::contracts::{
    FileTranscriptionOptions, FinalPassDisposition, FinalPassMode, FinalPassVerdict, RawTranscript,
    TranscriptionEngineMode, TranscriptionEngineVerdict, TranscriptionSource, TranscriptionVerdict,
    VadVerdict,
};
use crate::pipeline::stream_postprocess::{
    StreamPostProcessStats, StreamPostProcessor, final_pass_guardrail_reason,
};
use crate::safe_path;

use super::embedded::EmbeddedModel;
use super::params::DecodingParams;

/// Callback for streaming chunk results (called after each chunk is transcribed)
pub type ChunkCallback<'a> = &'a dyn Fn(&str);

fn skipped_final_pass(options: FileTranscriptionOptions, reason: &str) -> Option<FinalPassVerdict> {
    match options.final_pass {
        FinalPassMode::None => None,
        mode => Some(FinalPassVerdict {
            mode,
            disposition: FinalPassDisposition::Skipped,
            reason: Some(reason.to_string()),
            lexicon_rewrites: 0,
            repetition_cleanups: 0,
        }),
    }
}

fn finalize_requested_final_pass(
    raw_text: &str,
    candidate_text: String,
    mode: FinalPassMode,
    stats: StreamPostProcessStats,
) -> (String, FinalPassVerdict) {
    let lexicon_rewrites = stats.lexicon_rewrites;
    let repetition_cleanups = stats.repetition_cleanups;

    if candidate_text == raw_text {
        return (
            candidate_text,
            FinalPassVerdict {
                mode,
                disposition: FinalPassDisposition::Unchanged,
                reason: None,
                lexicon_rewrites,
                repetition_cleanups,
            },
        );
    }

    if let Some(reason) = final_pass_guardrail_reason(raw_text, &candidate_text) {
        return (
            raw_text.to_string(),
            FinalPassVerdict {
                mode,
                disposition: FinalPassDisposition::Rejected,
                reason: Some(reason),
                lexicon_rewrites,
                repetition_cleanups,
            },
        );
    }

    (
        candidate_text,
        FinalPassVerdict {
            mode,
            disposition: FinalPassDisposition::Changed,
            reason: None,
            lexicon_rewrites,
            repetition_cleanups,
        },
    )
}

fn apply_requested_final_pass(
    raw: &RawTranscript,
    options: FileTranscriptionOptions,
) -> (String, Option<FinalPassVerdict>) {
    match options.final_pass {
        FinalPassMode::None => (raw.text.clone(), None),
        FinalPassMode::EmbeddedLexiconCleanup => {
            let mut processor = StreamPostProcessor::new();
            match processor.process_utterance(&raw.text) {
                Some(text) => {
                    let stats = processor.stats();
                    let (text, verdict) = finalize_requested_final_pass(
                        &raw.text,
                        text,
                        FinalPassMode::EmbeddedLexiconCleanup,
                        stats,
                    );
                    (text, Some(verdict))
                }
                None => {
                    let stats = processor.stats();
                    (
                        String::new(),
                        Some(FinalPassVerdict {
                            mode: FinalPassMode::EmbeddedLexiconCleanup,
                            disposition: FinalPassDisposition::Dropped,
                            reason: Some("empty_after_cleanup".to_string()),
                            lexicon_rewrites: stats.lexicon_rewrites,
                            repetition_cleanups: stats.repetition_cleanups,
                        }),
                    )
                }
            }
        }
    }
}

pub struct LocalWhisperEngine {
    model: Model,
    tokenizer: Tokenizer,
    device: Device,
    config: Config,
    mel_filters: Vec<f32>,
    ts_range: Option<TimestampRange>,
    engine_provenance: TranscriptionEngineVerdict,
    pub decoding_params: DecodingParams,
}

impl LocalWhisperEngine {
    pub fn new(model_path: &Path) -> Result<Self> {
        let device = Device::new_metal(0).unwrap_or(Device::Cpu);
        tracing::debug!("LocalWhisperEngine using device: {:?}", device);

        let config_path = model_path.join("config.json");
        let weights_path = if model_path.join("weights.safetensors").exists() {
            model_path.join("weights.safetensors")
        } else {
            model_path.join("model.safetensors")
        };
        if !weights_path.exists() {
            anyhow::bail!(
                "Whisper weights not found (expected weights.safetensors or model.safetensors) in {}",
                model_path.display()
            );
        }
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

        let ts_range = TimestampRange::from_tokenizer(&tokenizer);

        Ok(Self {
            model,
            tokenizer,
            device,
            config,
            mel_filters,
            ts_range,
            engine_provenance: TranscriptionEngineVerdict::whisper(
                TranscriptionEngineMode::RuntimeFallback,
            ),
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

        let ts_range = TimestampRange::from_tokenizer(&tokenizer);

        Ok(Self {
            model,
            tokenizer,
            device,
            config,
            mel_filters,
            ts_range,
            engine_provenance: TranscriptionEngineVerdict::whisper(
                TranscriptionEngineMode::EmbeddedDefault,
            ),
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
    pub fn decoding_params(&self) -> &DecodingParams {
        &self.decoding_params
    }

    pub fn transcribe_file_with_language(
        &mut self,
        path: &Path,
        language: Option<&str>,
        options: FileTranscriptionOptions,
    ) -> Result<TranscriptionVerdict> {
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

        let (speech_samples, stats) = crate::vad::extract_speech(&samples, sample_rate);
        let speech_sec = speech_samples.len() as f32 / sample_rate as f32;
        tracing::info!(
            "transcribe_file VAD: {:.1}s speech / {:.1}s total ({:.0}% speech)",
            speech_sec,
            duration_secs,
            stats.speech_pct
        );

        let no_speech = speech_samples.is_empty();
        let vad = VadVerdict {
            speech_pct: stats.speech_pct,
            speech_windows: stats.speech_windows,
            total_windows: stats.total_windows,
            no_speech,
            no_speech_reason: stats.no_speech_reason.clone(),
            sparkline: stats.sparkline.clone(),
        };

        if no_speech {
            tracing::info!(
                "transcribe_file: no speech detected after VAD; returning empty verdict"
            );
            return Ok(TranscriptionVerdict::from_parts(
                String::new(),
                RawTranscript::default(),
                Some(vad),
                TranscriptionSource::LocalFinalPass,
                self.engine_provenance,
                skipped_final_pass(
                    options,
                    stats
                        .no_speech_reason
                        .as_deref()
                        .unwrap_or("vad_no_speech_detected"),
                ),
            ));
        }

        tracing::debug!(
            "transcribe_file: speech detected; preserving full-audio decode path and using VAD as telemetry/no-speech gate only"
        );

        // Keep file transcription semantically honest: VAD contributes verdict
        // metadata and an explicit no-speech short-circuit, but the raw STT
        // result still comes from the full recording. Trimming down to
        // `speech_samples` changed the behavior of the historical "raw file
        // transcription" path and regressed canonical transcripts.
        let raw = self.transcribe_long_with_language_segments(&samples, sample_rate, language)?;
        let vad_config = crate::vad::VadConfig::default();
        let timeline = crate::vad::classify_windows(&stats.probabilities, &vad_config);

        let (raw_for_final_pass, tail_drop_count) = if raw.segments.is_empty() {
            (raw.clone(), 0u32)
        } else {
            let outcome = crate::stt::whisper::map_whisper_segments_to_silero(
                &raw.segments,
                &timeline,
                &vad_config,
            );
            if outcome.dropped_count > 0 {
                tracing::info!(
                    target: "tail_silence_filter",
                    dropped_count = outcome.dropped_count,
                    dropped_samples = ?outcome.dropped_text_samples,
                    "Silero dropped Whisper tail segment(s)"
                );
            }

            let mut filtered = raw.clone();
            filtered.text = outcome.text;
            filtered.segments = outcome.segments;
            (filtered, outcome.dropped_count)
        };

        let (text, final_pass) = apply_requested_final_pass(&raw_for_final_pass, options);

        Ok(TranscriptionVerdict::from_parts_with_silero_drops(
            text,
            raw_for_final_pass,
            Some(vad),
            TranscriptionSource::LocalFinalPass,
            self.engine_provenance,
            final_pass,
            tail_drop_count,
        ))
    }

    pub fn detect_language_file(&mut self, path: &Path) -> Result<String> {
        let (samples, sample_rate) =
            audio_loader::load_audio_file(path).context("Failed to load audio file")?;
        self.detect_language(&samples, sample_rate)
    }

    pub fn transcribe_file(
        &mut self,
        path: &Path,
        options: FileTranscriptionOptions,
    ) -> Result<TranscriptionVerdict> {
        self.transcribe_file_with_language(path, None, options)
    }

    pub fn transcribe_with_language(
        &mut self,
        audio: &[f32],
        sample_rate: u32,
        language: Option<&str>,
    ) -> Result<String> {
        Ok(self
            .transcribe_with_language_segments(audio, sample_rate, language)?
            .text)
    }

    pub fn transcribe_with_language_segments(
        &mut self,
        audio: &[f32],
        sample_rate: u32,
        language: Option<&str>,
    ) -> Result<RawTranscript> {
        let samples = audio_loader::resample_to_16k(audio, sample_rate);
        if samples.is_empty() {
            tracing::debug!("Skipping transcription: empty audio after resampling");
            return Ok(RawTranscript::default());
        }
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

        self.transcribe_samples_16k_raw(&samples, language, debug_tokens)
    }

    pub fn transcribe_long_with_language_segments(
        &mut self,
        audio: &[f32],
        sample_rate: u32,
        language: Option<&str>,
    ) -> Result<RawTranscript> {
        let samples = audio_loader::resample_to_16k(audio, sample_rate);
        if samples.is_empty() {
            tracing::debug!("Skipping long transcription: empty audio after resampling");
            return Ok(RawTranscript::default());
        }
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

        let mut out = String::new();
        let mut all_segments = Vec::new();
        let mut offset = 0usize;
        let mut logprob_sum = 0.0_f32;
        let mut logprob_count = 0_u32;
        let mut worst_compression = 0.0_f32;
        let mut any_quality_gate_dropped = false;

        while offset < samples.len() {
            let end = (offset + chunk_samples).min(samples.len());
            let chunk = &samples[offset..end];
            let transcript = self.transcribe_samples_16k_raw(chunk, language, debug_tokens)?;
            append_with_overlap_dedup(&mut out, &transcript.text);

            if let Some(lp) = transcript.avg_logprob {
                logprob_sum += lp;
                logprob_count += 1;
            }
            if let Some(cr) = transcript.compression_ratio
                && cr > worst_compression
            {
                worst_compression = cr;
            }
            if transcript.quality_gate_dropped {
                any_quality_gate_dropped = true;
            }

            if !transcript.segments.is_empty() {
                let offset_sec = offset as f32 / 16_000.0;
                all_segments.extend(transcript.segments.into_iter().map(|mut s| {
                    s.start_ts += offset_sec;
                    s.end_ts += offset_sec;
                    s
                }));
            }

            offset = offset.saturating_add(step);
        }

        Ok(RawTranscript {
            text: dedup_repetitions(out.trim()),
            segments: all_segments,
            avg_logprob: if logprob_count > 0 {
                Some(logprob_sum / logprob_count as f32)
            } else {
                None
            },
            compression_ratio: if worst_compression > 0.0 {
                Some(worst_compression)
            } else {
                None
            },
            quality_gate_dropped: any_quality_gate_dropped,
        })
    }

    /// Legacy convenience wrapper kept for direct engine callers and tests.
    pub fn transcribe_long_with_language(
        &mut self,
        audio: &[f32],
        sample_rate: u32,
        language: Option<&str>,
    ) -> Result<String> {
        Ok(self
            .transcribe_long_with_language_segments(audio, sample_rate, language)?
            .text)
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

        // Apply word/phrase-level repetition deduplication before returning
        Ok(dedup_repetitions(out.trim()))
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
            if let Some(tok) = self.tokenizer.id_to_token(id)
                && let Some(lang) = parse_language_token(&tok)
            {
                out.push((id, lang.to_string()));
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
            if let Some(id) = self.tokenizer.token_to_id(&tok)
                && (id as usize) < vocab_size
            {
                out.push((id, lang.to_string()));
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
        Ok(self
            .transcribe_samples_16k_raw(samples_16k, language, debug_tokens)?
            .text)
    }

    fn transcribe_samples_16k_raw(
        &mut self,
        samples_16k: &[f32],
        language: Option<&str>,
        debug_tokens: bool,
    ) -> Result<RawTranscript> {
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
        let timestamps_enabled = self.decoding_params.emit_timestamps && self.ts_range.is_some();
        if !timestamps_enabled && let Some(t) = self.tokenizer.token_to_id("<|notimestamps|>") {
            tokens.push(t);
        }

        // Initial prompt: tokenize and prepend to decoder context (helps with vocabulary/formatting)
        if let Some(ref prompt) = self.decoding_params.initial_prompt
            && let Ok(encoding) = self.tokenizer.encode(prompt.as_str(), false)
        {
            let prompt_tokens: Vec<u32> = encoding.get_ids().to_vec();
            if !prompt_tokens.is_empty() {
                tracing::debug!(
                    "Initial prompt: {} ({} tokens)",
                    prompt,
                    prompt_tokens.len()
                );
                tokens.extend(prompt_tokens);
            }
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
            if step == 0
                && let Some(nos) = nospeech_token
            {
                let nos_idx = nos as usize;
                if nos_idx < logits_vec.len() {
                    // Compute softmax probability for nospeech only
                    let max_val = logits_vec.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
                    let exp_sum: f32 = logits_vec.iter().map(|&x| (x - max_val).exp()).sum();
                    let nos_prob = (logits_vec[nos_idx] - max_val).exp() / exp_sum;

                    if nos_prob > self.decoding_params.no_speech_threshold {
                        tracing::debug!("No speech detected (prob={:.3})", nos_prob);
                        return Ok(RawTranscript::default()); // Return empty for silence
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
                if let Some(nos) = nospeech_token
                    && (nos as usize) < logits_vec.len()
                {
                    logits_vec[nos as usize] = f32::NEG_INFINITY;
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

        let (text, segments) = if timestamps_enabled {
            let range = self
                .ts_range
                .as_ref()
                .ok_or_else(|| anyhow!("Timestamp range missing despite emit_timestamps=true"))?;
            timestamps::extract_segments(&all_tokens, &self.tokenizer, range)
        } else {
            (
                self.tokenizer
                    .decode(&all_tokens, true)
                    .map_err(|e| anyhow!("Tokenizer error: {}", e))?,
                Vec::new(),
            )
        };
        let text = text.trim().to_string();

        // 5. Logprob Threshold
        let avg_logprob = if token_count > 0 {
            let value = sum_logprob / token_count as f32;
            if value < self.decoding_params.logprob_threshold {
                tracing::warn!("Low avg logprob ({:.2}) - possible hallucination", value);
            }
            Some(value)
        } else {
            None
        };

        // 4. Compression Ratio Threshold - apply dedup if ratio too high
        let mut final_text = text;
        let mut final_segments = segments;
        let mut final_ratio = compression_ratio(&final_text);
        if final_ratio > self.decoding_params.compression_ratio_threshold {
            tracing::warn!(
                "High compression ratio ({:.2}) - applying dedup cleanup",
                final_ratio
            );

            // Apply word/phrase deduplication to reduce repetitions
            let cleaned = dedup_repetitions(&final_text).trim().to_string();
            let new_ratio = compression_ratio(&cleaned);

            if new_ratio > self.decoding_params.compression_ratio_threshold {
                tracing::warn!("Still high after dedup ({:.2})", new_ratio);
            } else {
                tracing::debug!(
                    "Compression ratio improved: {:.2} -> {:.2}",
                    final_ratio,
                    new_ratio
                );
            }
            final_text = cleaned;
            final_segments = Vec::new();
            final_ratio = new_ratio;
        }

        if should_drop_for_quality_gate(avg_logprob, final_ratio, &self.decoding_params) {
            tracing::warn!(
                "Quality gate dropped transcript (avg_logprob={:?}, compression_ratio={:.2})",
                avg_logprob,
                final_ratio
            );
            return Ok(RawTranscript {
                avg_logprob,
                compression_ratio: Some(final_ratio),
                quality_gate_dropped: true,
                ..Default::default()
            });
        }

        if final_text.is_empty() {
            return Ok(RawTranscript {
                avg_logprob,
                compression_ratio: Some(final_ratio),
                ..Default::default()
            });
        }

        Ok(RawTranscript {
            text: final_text,
            segments: final_segments,
            avg_logprob,
            compression_ratio: Some(final_ratio),
            quality_gate_dropped: false,
        })
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

fn normalize_token_for_overlap(token: &str) -> String {
    let mut out = String::new();
    for ch in token.chars() {
        if ch.is_alphanumeric() {
            out.extend(ch.to_lowercase());
        }
    }
    if out.is_empty() {
        token.to_lowercase()
    } else {
        out
    }
}

/// Word-level edit distance for short sequences (used by fuzzy overlap)
fn word_edit_distance(a: &[String], b: &[String]) -> usize {
    let m = a.len();
    let n = b.len();
    let mut prev: Vec<usize> = (0..=n).collect();
    let mut cur = vec![0usize; n + 1];

    for i in 1..=m {
        cur[0] = i;
        for j in 1..=n {
            let cost = if a[i - 1] == b[j - 1] { 0 } else { 1 };
            cur[j] = (prev[j] + 1).min(cur[j - 1] + 1).min(prev[j - 1] + cost);
        }
        prev.clone_from(&cur);
    }
    prev[n]
}

/// Helper for deduplication at chunk boundaries.
///
/// Two-pass approach:
/// 1. Exact match (fast path) — suffix of `out` == prefix of `segment`
/// 2. Fuzzy match (fallback) — allows up to k/3 word-level edits in overlap region
///    Catches cases where Whisper produces slightly different text for the same audio
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

    let out_norm: Vec<String> = out_words
        .iter()
        .map(|word| normalize_token_for_overlap(word))
        .collect();
    let seg_norm: Vec<String> = seg_words
        .iter()
        .map(|word| normalize_token_for_overlap(word))
        .collect();

    let max_overlap = out_words.len().min(seg_words.len()).min(30);
    let mut overlap = 0usize;

    // Pass 1: exact match (fast path)
    for k in (1..=max_overlap).rev() {
        if out_norm[out_norm.len() - k..] == seg_norm[..k] {
            overlap = k;
            break;
        }
    }

    // Pass 2: fuzzy match — allow up to k/3 word edits (min 1)
    if overlap == 0 {
        for k in (3..=max_overlap).rev() {
            let tail = &out_norm[out_norm.len() - k..];
            let head = &seg_norm[..k];
            let max_errors = (k / 3).max(1);
            let dist = word_edit_distance(tail, head);
            if dist <= max_errors {
                overlap = k;
                tracing::debug!(
                    "[FUZZY_DEDUP] matched k={} dist={} max_err={} tail={:?} head={:?}",
                    k,
                    dist,
                    max_errors,
                    &tail[..tail.len().min(5)],
                    &head[..head.len().min(5)]
                );
                break;
            }
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

fn should_drop_for_quality_gate(
    avg_logprob: Option<f32>,
    compression_ratio: f32,
    params: &DecodingParams,
) -> bool {
    let low_logprob = avg_logprob.is_some_and(|avg| avg < params.logprob_threshold);
    let high_compression = compression_ratio > params.compression_ratio_threshold;
    low_logprob && high_compression
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

// ═══════════════════════════════════════════════════════════════════════════════
// Repetition Deduplication (Word and Phrase Level)
// ═══════════════════════════════════════════════════════════════════════════════

/// Normalize word for comparison: lowercase + strip trailing punctuation
fn normalize_for_compare(word: &str) -> String {
    word.trim_end_matches(|c: char| c.is_ascii_punctuation())
        .to_lowercase()
}

/// Check if two words are equivalent (ignoring case and trailing punctuation)
fn words_equivalent(a: &str, b: &str) -> bool {
    normalize_for_compare(a) == normalize_for_compare(b)
}

/// Remove consecutive repeated words: "test test test value" -> "test value"
/// Case-insensitive comparison, ignores trailing punctuation.
/// Preserves original form of first occurrence.
pub fn dedup_repeated_words(text: &str) -> String {
    let words: Vec<&str> = text.split_whitespace().collect();
    if words.len() < 2 {
        return text.to_string();
    }

    let mut result: Vec<&str> = Vec::with_capacity(words.len());
    let mut i = 0;

    while i < words.len() {
        result.push(words[i]);
        // Skip consecutive duplicates (case-insensitive, punctuation-tolerant)
        while i + 1 < words.len() && words_equivalent(words[i], words[i + 1]) {
            i += 1;
        }
        i += 1;
    }

    result.join(" ")
}

/// Remove repeated 2-4 word phrases: "w tej chwili w tej chwili zajmuje" -> "w tej chwili zajmuje"
pub fn dedup_repeated_phrases(text: &str) -> String {
    let words: Vec<&str> = text.split_whitespace().collect();
    if words.len() < 4 {
        return text.to_string();
    }

    let mut result: Vec<&str> = Vec::with_capacity(words.len());
    let mut i = 0;

    while i < words.len() {
        // Try phrase lengths 4, 3, 2 (longest first)
        let mut skipped = false;
        for phrase_len in (2..=4).rev() {
            if i + phrase_len * 2 <= words.len() {
                let phrase1 = &words[i..i + phrase_len];
                let phrase2 = &words[i + phrase_len..i + phrase_len * 2];

                // Case-insensitive, punctuation-tolerant phrase comparison
                let matches = phrase1
                    .iter()
                    .zip(phrase2.iter())
                    .all(|(a, b)| words_equivalent(a, b));

                if matches {
                    // Add phrase once, skip the duplicate
                    result.extend_from_slice(phrase1);
                    i += phrase_len * 2;
                    // Continue checking for more repetitions of same phrase
                    while i + phrase_len <= words.len() {
                        let next = &words[i..i + phrase_len];
                        let still_matches = phrase1
                            .iter()
                            .zip(next.iter())
                            .all(|(a, b)| words_equivalent(a, b));
                        if still_matches {
                            i += phrase_len;
                        } else {
                            break;
                        }
                    }
                    skipped = true;
                    break;
                }
            }
        }

        if !skipped {
            result.push(words[i]);
            i += 1;
        }
    }

    result.join(" ")
}

/// Apply both word and phrase deduplication
pub fn dedup_repetitions(text: &str) -> String {
    let pass1 = dedup_repeated_phrases(text);
    dedup_repeated_words(&pass1)
}

#[cfg(test)]
mod dedup_tests {
    use super::*;

    #[test]
    fn test_dedup_repeated_words() {
        assert_eq!(
            dedup_repeated_words("zaimplementowane. zaimplementowane i w idei"),
            "zaimplementowane. i w idei"
        );
        assert_eq!(dedup_repeated_words("test test test value"), "test value");
        assert_eq!(
            dedup_repeated_words("no repetition here"),
            "no repetition here"
        );
    }

    #[test]
    fn test_dedup_repeated_phrases() {
        assert_eq!(
            dedup_repeated_phrases("56 GB. 56 GB. który zajmuje"),
            "56 GB. który zajmuje"
        );
        assert_eq!(
            dedup_repeated_phrases("w tej chwili w tej chwili zajmuje"),
            "w tej chwili zajmuje"
        );
    }

    #[test]
    fn test_dedup_repetitions_combined() {
        let input = "który zajmuje który zajmuje 56 GB. 56 GB. test test";
        let expected = "który zajmuje 56 GB. test";
        assert_eq!(dedup_repetitions(input), expected);
    }

    #[test]
    fn quality_gate_requires_both_logprob_and_compression_signals() {
        let params = DecodingParams::default();
        assert!(!should_drop_for_quality_gate(Some(-0.2), 3.0, &params));
        assert!(!should_drop_for_quality_gate(Some(-3.0), 1.4, &params));
        assert!(should_drop_for_quality_gate(Some(-3.0), 3.0, &params));
    }

    #[test]
    fn requested_final_pass_reports_embedded_lexicon_changes() {
        let raw = RawTranscript {
            text: "doker".to_string(),
            ..Default::default()
        };

        let (text, final_pass) = apply_requested_final_pass(
            &raw,
            FileTranscriptionOptions {
                final_pass: FinalPassMode::EmbeddedLexiconCleanup,
            },
        );

        assert_eq!(text, "Docker");
        let final_pass = final_pass.expect("expected final-pass provenance");
        assert_eq!(final_pass.mode, FinalPassMode::EmbeddedLexiconCleanup);
        assert_eq!(final_pass.disposition, FinalPassDisposition::Changed);
        assert_eq!(final_pass.lexicon_rewrites, 1);
    }

    #[test]
    fn requested_final_pass_skips_when_no_speech_already_known() {
        let final_pass = skipped_final_pass(
            FileTranscriptionOptions {
                final_pass: FinalPassMode::EmbeddedLexiconCleanup,
            },
            "vad_no_speech_detected",
        )
        .expect("expected skipped final-pass provenance");

        assert_eq!(final_pass.disposition, FinalPassDisposition::Skipped);
        assert_eq!(final_pass.reason.as_deref(), Some("vad_no_speech_detected"));
    }

    #[test]
    fn requested_final_pass_rejects_artifact_token_drift_and_keeps_raw() {
        let raw = "zastanawiam się co ośreda, że ta funkcja już teoretycznie obsolesi legacy";
        let candidate =
            "zastanawiam going co ośreda, use ta funkcja już teoretycznie obsolesi legacy"
                .to_string();
        let stats = StreamPostProcessStats::default();

        let (text, final_pass) = finalize_requested_final_pass(
            raw,
            candidate,
            FinalPassMode::EmbeddedLexiconCleanup,
            stats,
        );

        assert_eq!(text, raw);
        assert_eq!(final_pass.disposition, FinalPassDisposition::Rejected);
        assert_eq!(
            final_pass.reason.as_deref(),
            Some("artifact_token_drift:going,use")
        );
    }
}
