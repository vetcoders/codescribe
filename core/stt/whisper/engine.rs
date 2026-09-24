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
use std::sync::OnceLock;

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
use crate::safe_path;

use super::embedded::EmbeddedModel;
use super::params::DecodingParams;

fn candle_config(architecture: crate::whisper_weights::WhisperArchitecture) -> Config {
    Config {
        num_mel_bins: architecture.n_mels,
        max_source_positions: architecture.n_audio_ctx,
        d_model: architecture.n_audio_state,
        encoder_attention_heads: architecture.n_audio_head,
        encoder_layers: architecture.n_audio_layer,
        vocab_size: architecture.n_vocab,
        max_target_positions: architecture.n_text_ctx,
        decoder_attention_heads: architecture.n_text_head,
        decoder_layers: architecture.n_text_layer,
        suppress_tokens: Vec::new(),
    }
}

/// Process-lifetime Candle device for Whisper.
///
/// Cached so idle-unload can drop model weights without calling
/// `Device::new_metal` again. Recreating Metal devices leaks IOAccelerator
/// Mach ports + dispatch threads and forces a multi-second cold reload.
///
/// `Device` is `Clone` (Metal backend shares Arc'd queues/buffer maps); the
/// cached value keeps the underlying MTL device alive for the process life.
static PROCESS_DEVICE: OnceLock<Device> = OnceLock::new();

/// The process-lifetime device, created on first use.
///
/// Falls back to CPU when Metal is unavailable. Never call `Device::new_metal`
/// outside this initializer — see [`PROCESS_DEVICE`] for why re-creating the
/// device is expensive and leaky.
fn process_device() -> Device {
    PROCESS_DEVICE
        .get_or_init(|| {
            let device = Device::new_metal(0).unwrap_or(Device::Cpu);
            tracing::info!("Whisper process device acquired once: {device:?}");
            device
        })
        .clone()
}

/// The cached process device, if one was ever created — without creating it.
///
/// The idle reaper uses this to prune the Metal free-buffer pool after a
/// weight unload; a `None` means no engine ever loaded, so nothing to prune.
pub(super) fn cached_process_device() -> Option<Device> {
    PROCESS_DEVICE.get().cloned()
}

/// Average decoder tokens per spoken word (BPE subwords + punctuation). Used to
/// convert the words-per-second cap into a token budget for the runaway
/// watchdog. Conservative (higher = looser budget).
const RUNAWAY_TOKENS_PER_WORD: f32 = 2.0;

/// Safety margin on the runaway token budget so legitimate fast/long speech is
/// never cut: the watchdog only fires well past any plausible real word rate.
const RUNAWAY_BUDGET_MARGIN: f32 = 2.0;

/// Minimum token budget for the runaway watchdog regardless of audio length, so
/// very short chunks still get enough headroom to emit normal short utterances.
const RUNAWAY_MIN_BUDGET: usize = 64;
/// Frozen decoder watchdog value retained as a decoder safety limit, not text
/// admission.
const RUNAWAY_MAX_WORDS_PER_SEC: f32 = 5.0;
/// Prompt capacity is a Whisper decoder constraint, independent of the retired
/// transcript postprocessor that used to provide prompt contents.
const WHISPER_INITIAL_PROMPT_TOKEN_BUDGET: usize = 224;
/// Whisper's marker introducing previous-context tokens. Everything between it
/// and the decode prefix is treated by the model as prior context, not as text
/// to transcribe.
const WHISPER_START_OF_PREVIOUS_TOKEN: &str = "<|startofprev|>";

/// Token budget for the in-loop runaway watchdog given the chunk audio length.
///
/// Derived from the decoder's words-per-second cap times tokens-per-word and a
/// generous safety margin. When generated tokens exceed this budget the decode
/// loop stops instead of paying the full O(n^2)/O(n^3) cost of a runaway decode.
fn runaway_token_budget(audio_sec: f32) -> usize {
    let raw = (RUNAWAY_MAX_WORDS_PER_SEC
        * audio_sec.max(0.0)
        * RUNAWAY_TOKENS_PER_WORD
        * RUNAWAY_BUDGET_MARGIN)
        .ceil();
    (raw as usize).max(RUNAWAY_MIN_BUDGET)
}

/// Splice an initial prompt in front of the decode prefix and report how many
/// prompt tokens were kept.
///
/// Order matters: `<|startofprev|>`, then the prompt, then the existing prefix
/// (`<|startoftranscript|>` and friends) — the prompt is *previous context*, so
/// putting it after the prefix would make the model transcribe it. Capped at
/// [`WHISPER_INITIAL_PROMPT_TOKEN_BUDGET`]; an empty prompt is a no-op.
fn prepend_initial_prompt_tokens(
    tokens: &mut Vec<u32>,
    start_of_previous_token: u32,
    prompt_tokens: &[u32],
    max_target_positions: usize,
) -> usize {
    let available = max_target_positions.saturating_sub(tokens.len() + 2);
    let keep = prompt_tokens
        .len()
        .min(WHISPER_INITIAL_PROMPT_TOKEN_BUDGET)
        .min(available);
    if keep == 0 {
        return 0;
    }

    let current_prefix = std::mem::take(tokens);
    tokens.reserve_exact(1 + keep + current_prefix.len());
    tokens.push(start_of_previous_token);
    tokens.extend_from_slice(&prompt_tokens[..keep]);
    tokens.extend_from_slice(&current_prefix);
    keep
}

fn prompt_token_ids_fit_vocab(tokens: &[u32], vocab_size: usize) -> bool {
    tokens.iter().all(|token| (*token as usize) < vocab_size)
}

/// Record that a requested final pass was skipped, with the reason.
///
/// `None` when no final pass was requested — the caller must not fabricate a
/// verdict for work nobody asked for.
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

/// Adjudicate a final-pass candidate against the raw transcript.
///
/// Three outcomes: identical text is `Unchanged`; a candidate that trips
/// `final_pass_guardrail_reason` is `Rejected` and the **raw text is kept**;
/// otherwise the candidate wins as `Changed`. The guardrail is what stops
/// cleanup from silently rewriting words the model actually heard.
#[cfg(any())]
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

/// Whether decoder control tokens should be suppressed at this decode step.
///
/// Only before the first generated token: suppressing them later would stop the
/// model from ever emitting `<|endoftext|>` and turn a normal utterance into a
/// runaway decode.
fn should_suppress_decoder_control_tokens(generated_tokens: usize) -> bool {
    generated_tokens == 0
}

/// How many opening sampled tokens still count as the blank window.
///
/// With native timestamps the first sampled token is a clock, so a mask that
/// stops at token 0 never reaches speech. The next three samples are the first
/// text tokens of the window — the same span Classic masked — and a bare space
/// there changes the rest of the autoregressive pass. Spaces after that stay
/// available. End-of-text stays masked only at token 0; blocking it later
/// prevents a finished span from stopping.
const INITIAL_BLANK_TOKEN_WINDOW: usize = 4;

/// Blank suppression for the opening sampled tokens, using this tokenizer's
/// space encoding. The end token is masked only before the first sample.
fn apply_initial_blank_suppression(
    logits: &mut [f32],
    generated_tokens: usize,
    space_tokens: &[u32],
    eot_token: u32,
) {
    if generated_tokens >= INITIAL_BLANK_TOKEN_WINDOW {
        return;
    }
    for &token in space_tokens {
        if let Some(logit) = logits.get_mut(token as usize) {
            *logit = f32::NEG_INFINITY;
        }
    }
    if generated_tokens == 0
        && let Some(logit) = logits.get_mut(eot_token as usize)
    {
        *logit = f32::NEG_INFINITY;
    }
}

/// Apply Whisper's timestamp-token constraints to one decoder step.
///
/// This mirrors OpenAI Whisper's `ApplyTimestampRules`: the first generated
/// token is a timestamp, timestamps are monotonic and paired around text, and
/// aggregate timestamp probability can outrank the best text token. Merely
/// omitting `<|notimestamps|>` from the prompt is insufficient — without these
/// masks the model can emit text-only output and the seam judge has no clock.
fn apply_timestamp_rules(
    logits: &mut [f32],
    sampled_tokens: &[u32],
    eot_token: u32,
    no_timestamps_token: Option<u32>,
    range: &timestamps::TimestampRange,
) {
    let timestamp_begin = range.begin as usize;
    let timestamp_end = (range.end_inclusive as usize).min(logits.len().saturating_sub(1));
    if timestamp_begin >= logits.len() || timestamp_begin > timestamp_end {
        return;
    }

    if let Some(token) = no_timestamps_token
        && let Some(logit) = logits.get_mut(token as usize)
    {
        *logit = f32::NEG_INFINITY;
    }

    let last_was_timestamp = sampled_tokens
        .last()
        .is_some_and(|token| range.is_timestamp(*token));
    let penultimate_was_timestamp =
        sampled_tokens.len() < 2 || range.is_timestamp(sampled_tokens[sampled_tokens.len() - 2]);

    if last_was_timestamp {
        if penultimate_was_timestamp {
            logits[timestamp_begin..=timestamp_end].fill(f32::NEG_INFINITY);
        } else {
            let eot_index = (eot_token as usize).min(logits.len());
            logits[..eot_index].fill(f32::NEG_INFINITY);
        }
    }

    if let Some(last_timestamp) = sampled_tokens
        .iter()
        .rev()
        .find(|token| range.is_timestamp(**token))
    {
        let first_allowed = if last_was_timestamp && !penultimate_was_timestamp {
            *last_timestamp
        } else {
            last_timestamp.saturating_add(1)
        } as usize;
        let forbid_end = first_allowed.min(timestamp_end.saturating_add(1));
        if timestamp_begin < forbid_end {
            logits[timestamp_begin..forbid_end].fill(f32::NEG_INFINITY);
        }
    }

    if sampled_tokens.is_empty() {
        logits[..timestamp_begin].fill(f32::NEG_INFINITY);
    }

    let timestamp_max = logits[timestamp_begin..=timestamp_end]
        .iter()
        .copied()
        .fold(f32::NEG_INFINITY, f32::max);
    let timestamp_logsumexp = if timestamp_max.is_finite() {
        timestamp_max
            + logits[timestamp_begin..=timestamp_end]
                .iter()
                .map(|logit| (*logit - timestamp_max).exp())
                .sum::<f32>()
                .ln()
    } else {
        f32::NEG_INFINITY
    };
    let max_text_logit = logits[..timestamp_begin]
        .iter()
        .copied()
        .fold(f32::NEG_INFINITY, f32::max);
    if timestamp_logsumexp > max_text_logit {
        logits[..timestamp_begin].fill(f32::NEG_INFINITY);
    }
}

/// A loaded Whisper model plus everything one transcription needs: tokenizer,
/// device, mel filters, timestamp range and decoding parameters.
///
/// Construct with [`LocalWhisperEngine::new`] (filesystem) or
/// [`LocalWhisperEngine::from_embedded`] (binary-embedded weights). Methods take
/// `&mut self` because decoding mutates model KV-cache state — an engine is not
/// safe to share across concurrent transcriptions.
pub struct LocalWhisperEngine {
    model: Model,
    tokenizer: Tokenizer,
    device: Device,
    config: Config,
    mel_filters: Vec<f32>,
    ts_range: Option<TimestampRange>,
    engine_provenance: TranscriptionEngineVerdict,
    pub decoding_params: DecodingParams,
    /// `(decoder_layer, head)` pairs from the checkpoint. Empty when the file
    /// has no `alignment_heads` tensor — word pins are then not measured.
    alignment_heads: Vec<(usize, usize)>,
    /// L1 tail decode asks the sample decoder to retain tokens and the encoder
    /// output. File transcription leaves this false, so that path does not
    /// clone either.
    capture_word_alignment: bool,
    captured_tokens: Vec<u32>,
    captured_encoder: Option<Tensor>,
    captured_sample_len: usize,
}

struct EngineRequest<'a> {
    engine: &'a mut LocalWhisperEngine,
    previous_prompt: Option<String>,
}

impl Drop for EngineRequest<'_> {
    fn drop(&mut self) {
        self.engine.decoding_params.initial_prompt = self.previous_prompt.take();
        self.engine.clear_execution_cache();
    }
}

impl LocalWhisperEngine {
    /// One cleanup corridor for public file calls and controlled local repair.
    /// Restores prompt and cache on success, cancellation, error, and unwind.
    pub(crate) fn with_request<R>(
        &mut self,
        initial_prompt: Option<String>,
        work: impl FnOnce(&mut Self) -> Result<R>,
    ) -> Result<R> {
        let previous_prompt =
            std::mem::replace(&mut self.decoding_params.initial_prompt, initial_prompt);
        let request = EngineRequest {
            engine: self,
            previous_prompt,
        };
        work(&mut *request.engine)
    }

    /// Load a model from a directory (development / external models).
    ///
    /// Expects `config.json` plus `weights.safetensors` or `model.safetensors`.
    /// Quantized MLX weights are refused. Runtime Whisper is fp16/fp32 only;
    /// this keeps q8 dequantization off every product path.
    ///
    /// # Errors
    /// Missing config or weights, an unreadable tokenizer, a quantized payload,
    /// or tensor shapes the fp16 loader cannot reconcile.
    pub fn new(model_path: &Path) -> Result<Self> {
        let config_path = model_path.join("config.json");
        if !config_path.is_file() {
            anyhow::bail!("Whisper config not found at {}", config_path.display());
        }
        if !model_path.join("weights.safetensors").is_file()
            && !model_path.join("model.safetensors").is_file()
        {
            anyhow::bail!(
                "Whisper weights not found (expected weights.safetensors or model.safetensors) in {}",
                model_path.display()
            );
        }
        let tokenizer_path = model_path.join("tokenizer.json");
        let mel_filters_path = model_path.join("mel_filters.npz");
        crate::whisper_weights::validate_whisper_model_bundle(model_path)
            .context("validate complete Whisper model bundle")?;
        let architecture = crate::whisper_weights::load_whisper_architecture(&config_path)?;
        let tokenizer = crate::whisper_weights::load_validated_whisper_tokenizer_for_architecture(
            &tokenizer_path,
            architecture,
        )?;
        let weights_path = crate::config::models::resolve_compatible_whisper_weights_path(
            model_path,
            architecture,
        )
        .context("resolve architecture-compatible Whisper weights")?;
        let device = process_device();
        tracing::debug!("LocalWhisperEngine using device: {:?}", device);

        let config = candle_config(architecture);

        // Phase timings for the cold load. "Preloaded, zero latency" is the
        // product's claim, and the operator's logs show 56 cold loads costing a
        // median of 9.2 s (p90 21.9 s, worst 34.9 s, 805 s in total) because the
        // idle reaper drops the weights every 45 minutes and this function then
        // rebuilds them from scratch. A single aggregate number cannot say
        // whether to attack the read, tensor conversion/mapping, or GPU upload, so
        // each phase reports its own cost.
        let load_started = std::time::Instant::now();
        let read_secs;
        let plain_secs;

        let mut alignment_heads = Vec::new();
        let vb = unsafe {
            let tensors = candle_core::safetensors::MmapedSafetensors::new(&weights_path)?;
            let mut raw_tensors: HashMap<String, Tensor> = HashMap::new();

            // Load the verified unquantized tensors on CPU before device transfer.
            let read_started = std::time::Instant::now();
            for (name, view) in tensors.tensors() {
                if name == "alignment_heads" {
                    let loaded = view.load(&Device::Cpu)?;
                    alignment_heads = parse_alignment_heads(
                        &loaded,
                        config.decoder_layers,
                        config.decoder_attention_heads,
                    )?;
                    continue;
                }
                let loaded = view.load(&Device::Cpu)?;
                raw_tensors.insert(name.to_string(), loaded);
            }
            read_secs = read_started.elapsed().as_secs_f64();

            let plain_started = std::time::Instant::now();
            let vb = build_varbuilder_from_tensors(raw_tensors, &device)?;
            plain_secs = plain_started.elapsed().as_secs_f64();
            vb
        };

        let build_started = std::time::Instant::now();
        let model = Model::load(&vb, config.clone()).context("Failed to create Whisper Model")?;
        tracing::info!(
            "whisper_cold_load_phases total={:.2}s read={:.2}s plain_tensors={:.2}s build_model={:.2}s",
            load_started.elapsed().as_secs_f64(),
            read_secs,
            plain_secs,
            build_started.elapsed().as_secs_f64()
        );

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

        let ts_range = TimestampRange::from_tokenizer(&tokenizer, config.vocab_size)?;

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
            alignment_heads,
            capture_word_alignment: false,
            captured_tokens: Vec::new(),
            captured_encoder: None,
            captured_sample_len: 0,
        })
    }

    /// Create engine from embedded model bytes - zero disk I/O!
    ///
    /// Model data is `include_bytes!` from binary at compile time.
    /// At runtime: bytes → tensors → GPU. No temp files, no extraction.
    pub fn from_embedded(embedded: &EmbeddedModel) -> Result<Self> {
        let device = process_device();
        tracing::info!(
            "Loading embedded Whisper model ({:.1} MB) to {:?}",
            embedded.total_size() as f64 / 1_000_000.0,
            device
        );

        // Parse config from bytes
        let config_str = std::str::from_utf8(embedded.config)
            .context("Invalid UTF-8 in embedded config.json")?;
        let architecture =
            crate::whisper_weights::parse_whisper_config(config_str, "embedded config.json")?;
        let config = candle_config(architecture);

        // Load weights directly from bytes - NO DISK I/O!
        let mut raw_tensors = candle_core::safetensors::load_buffer(embedded.weights, &Device::Cpu)
            .context("Failed to deserialize embedded weights")?;
        let alignment_heads = match raw_tensors.remove("alignment_heads") {
            Some(tensor) => parse_alignment_heads(
                &tensor,
                config.decoder_layers,
                config.decoder_attention_heads,
            )?,
            None => Vec::new(),
        };

        let vb = build_varbuilder_from_tensors(raw_tensors, &device)?;
        let model = Model::load(&vb, config.clone()).context("Failed to create Whisper Model")?;

        // Load tokenizer from bytes
        let tokenizer = Tokenizer::from_bytes(embedded.tokenizer)
            .map_err(|e| anyhow!("Failed to load embedded tokenizer: {}", e))?;

        // Load mel filters from bytes
        let mel_filters = load_mel_filters_from_bytes(embedded.mel_filters, config.num_mel_bins)
            .context("Failed to load embedded mel filters")?;

        tracing::info!("Embedded Whisper model loaded successfully");

        let ts_range = TimestampRange::from_tokenizer(&tokenizer, config.vocab_size)?;

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
            alignment_heads,
            capture_word_alignment: false,
            captured_tokens: Vec::new(),
            captured_encoder: None,
            captured_sample_len: 0,
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

    /// Transcribe a file end to end and return the full verdict.
    ///
    /// The complete path loads and resamples audio, records Silero VAD evidence
    /// and silence-window guidance, and decodes the original full recording
    /// (chunked for long input). The returned [`TranscriptionVerdict`] carries
    /// both the delivered text and the raw text, so a rejected final pass stays
    /// auditable.
    ///
    /// `language` of `None` triggers detection from the audio itself.
    pub fn transcribe_file_with_language(
        &mut self,
        path: &Path,
        language: Option<&str>,
        options: FileTranscriptionOptions,
    ) -> Result<TranscriptionVerdict> {
        self.transcribe_file_with_language_observed(path, language, options, &mut |_| Ok(()))
    }

    /// Observe admitted segments after each window, before decoding the next.
    /// Streaming and ordinary file calls use the same assembly and verdict.
    pub fn transcribe_file_with_language_observed(
        &mut self,
        path: &Path,
        language: Option<&str>,
        options: FileTranscriptionOptions,
        on_segments: &mut dyn FnMut(&[crate::pipeline::contracts::TranscriptSegment]) -> Result<()>,
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
            fine_sparkline: stats.fine_sparkline.clone(),
            fine_hop_ms: (stats.fine_hop_samples * 1000 / 16_000) as u16,
        };

        // Silero is the judge of whether there is anything to decode. Whisper on
        // audio with no speech (a 0.2 s click, a noise-floor take) hallucinates
        // a language-model prior ("Thank you.") instead of staying silent
        // (measured 2026-09-09 on session 05d37129), so an audible-speech
        // verdict of "none" ends the file pass here with an empty transcript.
        // Silero never cuts or glues the PCM the decoder sees; it only decides
        // whether the decoder runs at all.
        if no_speech {
            tracing::info!(
                reason = vad
                    .no_speech_reason
                    .as_deref()
                    .unwrap_or("no_speech_windows"),
                total_secs = duration_secs,
                total_windows = stats.total_windows,
                "file_pass_skipped_no_speech: Silero found no speech; decoder not run"
            );
            let final_pass = skipped_final_pass(options, "no_speech");
            return Ok(TranscriptionVerdict::from_parts(
                String::new(),
                RawTranscript::default(),
                Some(vad),
                TranscriptionSource::LocalFinalPass,
                self.engine_provenance,
                final_pass,
            ));
        }

        tracing::debug!(
            "transcribe_file: VAD evidence recorded; decoding full audio with silence-aligned windows"
        );

        // VAD contributes verdict evidence and silence-window guidance. It does
        // not author the raw STT result: decode uses the original full samples,
        // never `speech_samples`.
        let vad_config = crate::vad::VadConfig::default();
        let silence_spans = silence_spans_from_vad_probabilities(
            &stats.probabilities,
            vad_config.threshold,
            duration_secs,
        );
        let inference_started = std::time::Instant::now();
        let raw = self.transcribe_long_with_language_segments_using_silences(
            &samples,
            sample_rate,
            language,
            &silence_spans,
            on_segments,
            &crate::stt::LocalExecutionControl::default(),
        )?;
        super::timing::record_inference_ms(inference_started.elapsed().as_millis() as u64);
        let raw_for_final_pass = raw;
        let text = raw_for_final_pass.text.clone();
        let final_pass = skipped_final_pass(options, "whole_session_final_pass_retired");

        Ok(TranscriptionVerdict::from_parts(
            text,
            raw_for_final_pass,
            Some(vad),
            TranscriptionSource::LocalFinalPass,
            self.engine_provenance,
            final_pass,
        ))
    }

    /// Detect the spoken language of an audio file, returning its Whisper
    /// language code (e.g. `"pl"`).
    pub fn detect_language_file(&mut self, path: &Path) -> Result<String> {
        let (samples, sample_rate) =
            audio_loader::load_audio_file(path).context("Failed to load audio file")?;
        self.detect_language(&samples, sample_rate)
    }

    /// [`Self::transcribe_file_with_language`] with automatic language
    /// detection.
    pub fn transcribe_file(
        &mut self,
        path: &Path,
        options: FileTranscriptionOptions,
    ) -> Result<TranscriptionVerdict> {
        self.transcribe_file_with_language(path, None, options)
    }

    /// Transcribe in-memory audio, returning text only.
    ///
    /// Single-window path: audio longer than one Whisper window should go
    /// through [`Self::transcribe_long_with_language`] instead.
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

    /// Transcribe in-memory audio, keeping segments and quality signals.
    ///
    /// Resamples to 16 kHz first; audio that resamples to nothing yields an
    /// empty [`RawTranscript`] rather than an error. Set `CODESCRIBE_DEBUG_TOKENS`
    /// to log the raw token stream.
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

        self.transcribe_samples_16k_raw(
            &samples,
            language,
            debug_tokens,
            &crate::stt::LocalExecutionControl::default(),
        )
    }

    /// Transcribe arbitrarily long audio in VAD-aligned, overlapping windows.
    ///
    /// The overlap exists so a word split across a boundary is still heard
    /// whole; [`merge_chunk_transcripts`] then removes the duplicated region by
    /// segment time. Non-empty output without timestamped segments is refused
    /// because text is not replay identity.
    /// Segment timestamps are rebased onto the full recording, `avg_logprob` is
    /// averaged across chunks, and `compression_ratio` reports the **worst**
    /// chunk — one hallucinating window must not be hidden by good neighbours.
    pub fn transcribe_long_with_language_segments(
        &mut self,
        audio: &[f32],
        sample_rate: u32,
        language: Option<&str>,
    ) -> Result<RawTranscript> {
        self.transcribe_long_controlled(
            audio,
            sample_rate,
            language,
            &crate::stt::LocalExecutionControl::default(),
        )
    }

    pub(crate) fn clear_execution_cache(&mut self) {
        self.model.reset_kv_cache();
    }

    pub(crate) fn transcribe_long_controlled(
        &mut self,
        audio: &[f32],
        sample_rate: u32,
        language: Option<&str>,
        control: &crate::stt::LocalExecutionControl,
    ) -> Result<RawTranscript> {
        let result = self.transcribe_long_inner(audio, sample_rate, language, control);
        self.clear_execution_cache();
        control.check()?;
        result
    }

    fn transcribe_long_inner(
        &mut self,
        audio: &[f32],
        sample_rate: u32,
        language: Option<&str>,
        control: &crate::stt::LocalExecutionControl,
    ) -> Result<RawTranscript> {
        control.check()?;
        let (_, stats) = crate::vad::extract_speech(audio, sample_rate);
        control.check()?;
        let silence_spans = silence_spans_from_vad_probabilities(
            &stats.probabilities,
            crate::vad::VadConfig::default().threshold,
            if sample_rate == 0 {
                0.0
            } else {
                audio.len() as f32 / sample_rate as f32
            },
        );
        self.transcribe_long_with_language_segments_using_silences(
            audio,
            sample_rate,
            language,
            &silence_spans,
            &mut |_| Ok(()),
            control,
        )
    }

    /// Decode long audio using silence spans already measured by the caller.
    ///
    /// File transcription passes its existing Silero result here so window
    /// planning does not add a second VAD run to the stop-path budget.
    ///
    /// A window that decodes words but no closed timestamp span is retried as
    /// halves up to `SEGMENTLESS_WINDOW_MAX_SPLITS` times (quarters of the
    /// planned window) and then refused on its own; the file continues.
    fn transcribe_long_with_language_segments_using_silences(
        &mut self,
        audio: &[f32],
        sample_rate: u32,
        language: Option<&str>,
        silence_spans: &[(f32, f32)],
        on_segments: &mut dyn FnMut(&[crate::pipeline::contracts::TranscriptSegment]) -> Result<()>,
        control: &crate::stt::LocalExecutionControl,
    ) -> Result<RawTranscript> {
        control.check()?;
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
                detected_lang = self.detect_language_16k_controlled(&samples, control)?;
                tracing::info!("Detected language: {}", detected_lang);
                Some(detected_lang.as_str())
            }
        };

        let total_secs = samples.len() as f32 / 16_000.0;
        let windows = plan_vad_aligned_windows(silence_spans, total_secs);
        tracing::debug!(
            window_count = windows.len(),
            silence_span_count = silence_spans.len(),
            "planned VAD-aligned long-file decode windows"
        );

        // One file-level log-mel pass (not per window). candle 0.9.2 pads each
        // call; `file_level_log_mel` slices at 30 s and keeps real hops only.
        let energy = {
            let n_mels = self.config.num_mel_bins;
            let mel_all = file_level_log_mel(&self.config, &samples, &self.mel_filters);
            Some(super::energy::energy_timeline(&mel_all, n_mels, 10))
        };

        let mut merged = RawTranscript::default();
        let mut covered_until_secs = 0.0_f32;
        let mut logprob_sum = 0.0_f32;
        let mut logprob_count = 0_u32;
        let mut worst_compression = 0.0_f32;
        let mut refused_windows = 0_u32;

        // Time-ordered work stack: a window whose decode carries words but no
        // closed timestamp span (a runaway decode never emits the closing
        // clock token) is re-tried as two halves before it is refused.
        let mut pending: Vec<(f32, f32, u8)> = windows
            .into_iter()
            .rev()
            .map(|(start_sec, end_sec)| (start_sec, end_sec, 0_u8))
            .collect();

        while let Some((start_sec, end_sec, splits)) = pending.pop() {
            control.checkpoint(crate::stt::LocalExecutionBoundary::Window)?;
            let start = ((start_sec * 16_000.0).round() as usize).min(samples.len());
            let end = ((end_sec * 16_000.0).round() as usize).min(samples.len());
            if end <= start {
                continue;
            }
            let chunk = &samples[start..end];
            let mut transcript =
                self.transcribe_samples_16k_raw(chunk, language, debug_tokens, control)?;

            // `merge_chunk_transcripts` refuses words without timestamp
            // provenance by contract. That refusal is the WINDOW's verdict,
            // not the file's: retry shorter, then drop the window and keep
            // `covered_until_secs` where it was so the neighbour's overlap
            // re-describes as much of the hole as it can.
            if transcript.segments.is_empty() && !transcript.text.trim().is_empty() {
                let window_chars = transcript.text.chars().count();
                if splits < SEGMENTLESS_WINDOW_MAX_SPLITS
                    && end_sec - start_sec >= SEGMENTLESS_WINDOW_MIN_SPLIT_SECS
                {
                    let mid_sec = (start_sec + end_sec) / 2.0;
                    tracing::warn!(
                        window_start = start_sec,
                        window_end = end_sec,
                        chars = window_chars,
                        split = splits + 1,
                        "long-file window decoded words without a closed timestamp span; \
                         retrying as two halves"
                    );
                    pending.push((mid_sec, end_sec, splits + 1));
                    pending.push((start_sec, mid_sec, splits + 1));
                    continue;
                }
                refused_windows += 1;
                tracing::warn!(
                    window_start = start_sec,
                    window_end = end_sec,
                    chars = window_chars,
                    refused_windows,
                    "long-file window refused: words without timestamp provenance \
                     after retries; this span is missing from the transcript"
                );
                continue;
            }

            if let Some(lp) = transcript.avg_logprob {
                logprob_sum += lp;
                logprob_count += 1;
            }
            if let Some(cr) = transcript.compression_ratio
                && cr > worst_compression
            {
                worst_compression = cr;
            }
            if !transcript.segments.is_empty() {
                let offset_sec = start as f32 / 16_000.0;
                transcript.segments.iter_mut().for_each(|segment| {
                    segment.start_ts += offset_sec;
                    segment.end_ts += offset_sec;
                });
            }

            let overlap_end_secs = covered_until_secs.max(start_sec);
            let window_segments = transcript.segments.len();
            let window_chars = transcript.text.chars().count();
            let prior_segments = merged.segments.len();
            merge_chunk_transcripts(&mut merged, transcript, overlap_end_secs).with_context(
                || {
                    format!(
                        "long-file window {start_sec:.2}-{end_sec:.2}s \
                         (overlap_end {overlap_end_secs:.2}s, {window_segments} segments, \
                         {window_chars} chars)"
                    )
                },
            )?;
            on_segments(&merged.segments[prior_segments..])?;
            covered_until_secs = covered_until_secs.max(end_sec);
        }

        if refused_windows > 0 {
            tracing::warn!(
                refused_windows,
                total_secs,
                "long-file transcript assembled with refused windows; \
                 the missing spans are logged above"
            );
        }

        Ok(RawTranscript {
            text: merged.text.trim().to_string(),
            segments: merged.segments,
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
            energy,
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

    /// Detect the spoken language of in-memory audio, resampling to 16 kHz
    /// first.
    pub fn detect_language(&mut self, audio: &[f32], sample_rate: u32) -> Result<String> {
        let samples = audio_loader::resample_to_16k(audio, sample_rate);
        self.detect_language_16k(&samples)
    }

    /// Language detection on already-16 kHz samples.
    ///
    /// Runs a single decoder step over the mel window and picks the highest
    /// scoring language token, so detection costs one step rather than a full
    /// decode.
    fn detect_language_16k(&mut self, samples_16k: &[f32]) -> Result<String> {
        self.detect_language_16k_controlled(
            samples_16k,
            &crate::stt::LocalExecutionControl::default(),
        )
    }

    fn detect_language_16k_controlled(
        &mut self,
        samples_16k: &[f32],
        control: &crate::stt::LocalExecutionControl,
    ) -> Result<String> {
        control.check()?;
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

        control.check()?;
        let encoder_output = self.model.encoder.forward(&mel, true)?;
        control.check()?;

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
        control.check()?;

        let candidates =
            crate::whisper_weights::language_token_candidates(&self.tokenizer, logits_vec.len());
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

    /// The decode loop: mel spectrogram, encoder pass, then greedy decoding of
    /// one audio window into text, segments and quality signals.
    ///
    /// Decoder safeguards and diagnostics remain explicit inside the loop:
    /// - a runaway watchdog ([`runaway_token_budget`]) bails before a
    ///   hallucination costs the full quadratic decode,
    /// - avg-logprob and compression-ratio diagnostics remain attached to the
    ///   decoded observation for downstream inspection.
    ///
    /// `debug_tokens` logs the raw token stream for diagnosis.
    fn transcribe_samples_16k_raw(
        &mut self,
        samples_16k: &[f32],
        language: Option<&str>,
        debug_tokens: bool,
        control: &crate::stt::LocalExecutionControl,
    ) -> Result<RawTranscript> {
        control.check()?;
        ensure!(!samples_16k.is_empty(), "audio is empty");

        if self.capture_word_alignment {
            self.captured_tokens.clear();
            self.captured_encoder = None;
            self.captured_sample_len = 0;
        }

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
        let no_timestamps_token = self.tokenizer.token_to_id("<|notimestamps|>");
        let start_of_previous_token = self.tokenizer.token_to_id(WHISPER_START_OF_PREVIOUS_TOKEN);
        let space_tokens = if self.decoding_params.suppress_blank {
            self.tokenizer
                .encode(" ", false)
                .map_err(|error| anyhow!("Tokenizer space encoding failed: {error}"))?
                .get_ids()
                .to_vec()
        } else {
            Vec::new()
        };

        // Initial tokens: <|startoftranscript|> <|lang|>? <|transcribe|> <|notimestamps|>
        let mut tokens = vec![start_token];
        if let Some(lang) = language {
            let lang_tok = format!("<|{}|>", lang.to_lowercase());
            if let Some(t) = self.tokenizer.token_to_id(&lang_tok)
                && (t as usize) < self.config.vocab_size
            {
                tokens.push(t);
            }
        }
        if let Some(t) = self.tokenizer.token_to_id("<|transcribe|>")
            && (t as usize) < self.config.vocab_size
        {
            tokens.push(t);
        }
        let timestamps_enabled = self.decoding_params.emit_timestamps && self.ts_range.is_some();
        if !timestamps_enabled
            && let Some(t) = self.tokenizer.token_to_id("<|notimestamps|>")
            && (t as usize) < self.config.vocab_size
        {
            tokens.push(t);
        }

        // Initial prompt is previous-context text, not current transcript text:
        // <|startofprev|> prompt... <|startoftranscript|> <|lang|>? <|transcribe|> ...
        if let Some(ref prompt) = self.decoding_params.initial_prompt
            && let Ok(encoding) = self.tokenizer.encode(prompt.as_str(), false)
        {
            let prompt_tokens = encoding.get_ids();
            if !prompt_token_ids_fit_vocab(prompt_tokens, self.config.vocab_size) {
                tracing::warn!("Ignoring Whisper initial prompt containing out-of-vocabulary IDs");
            } else if !prompt_tokens.is_empty() {
                if let Some(start_of_previous_token) = start_of_previous_token
                    && (start_of_previous_token as usize) < self.config.vocab_size
                {
                    let used = prepend_initial_prompt_tokens(
                        &mut tokens,
                        start_of_previous_token,
                        prompt_tokens,
                        self.config.max_target_positions,
                    );
                    tracing::debug!("Initial prompt: {} ({} tokens)", prompt, used);
                } else {
                    tracing::warn!(
                        "Ignoring Whisper initial prompt: tokenizer missing {}",
                        WHISPER_START_OF_PREVIOUS_TOKEN
                    );
                }
            }
        }

        let mut all_tokens = Vec::new();

        // Run encoder once
        control.check()?;
        let encoder_output = self.model.encoder.forward(&mel, true)?;
        control.check()?;

        // Decoder loop – allow up to the configured maximum target positions minus initial tokens
        let max_new_tokens = self
            .config
            .max_target_positions
            .saturating_sub(tokens.len());

        // Runaway watchdog: cap generated tokens at a generous multiple of the
        // plausible word rate for this chunk's audio length, so a hallucinating
        // decode bails early instead of grinding to max_new_tokens at O(n^2) cost.
        let audio_sec = samples_16k.len() as f32 / whisper::SAMPLE_RATE as f32;
        let runaway_budget = runaway_token_budget(audio_sec);
        let mut sum_logprob = 0.0f32;
        let mut token_count = 0usize;

        for step in 0..max_new_tokens {
            control.checkpoint(crate::stt::LocalExecutionBoundary::Token)?;
            if all_tokens.len() >= runaway_budget {
                tracing::warn!(
                    "Runaway watchdog tripped: {} tokens for {:.2}s audio (budget {})",
                    all_tokens.len(),
                    audio_sec,
                    runaway_budget
                );
                break;
            }
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
            control.check()?;

            if self.decoding_params.suppress_blank {
                apply_initial_blank_suppression(
                    &mut logits_vec,
                    all_tokens.len(),
                    &space_tokens,
                    eot_token,
                );
            }

            if timestamps_enabled && let Some(range) = self.ts_range.as_ref() {
                apply_timestamp_rules(
                    &mut logits_vec,
                    &all_tokens,
                    eot_token,
                    no_timestamps_token,
                    range,
                );
            }

            // Avoid terminating immediately when nothing has been emitted yet
            let suppress_tokens = should_suppress_decoder_control_tokens(all_tokens.len());
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

        // 4. Compression Ratio Threshold - diagnostic evidence only
        let final_ratio = compression_ratio(&text);
        if final_ratio > self.decoding_params.compression_ratio_threshold {
            tracing::warn!(
                threshold = self.decoding_params.compression_ratio_threshold,
                "High compression ratio ({:.2}); preserving decoded transcript for occurrence-authority adjudication",
                final_ratio
            );
        }

        if text.is_empty() {
            return Ok(RawTranscript {
                avg_logprob,
                compression_ratio: Some(final_ratio),
                energy: None,
                ..Default::default()
            });
        }

        if self.capture_word_alignment {
            self.captured_tokens = all_tokens;
            self.captured_encoder = Some(encoder_output);
            self.captured_sample_len = samples_16k.len();
        }

        Ok(RawTranscript {
            text,
            segments,
            avg_logprob,
            compression_ratio: Some(final_ratio),
            energy: None,
        })
    }

    /// One L1 window. The returned transcript is the ordinary phrase-grain
    /// decode. `Some` word segments are measured cross-attention pins on that
    /// same token sequence; `None` means the checkpoint or the path could not
    /// measure them, and the caller keeps the phrase segments.
    pub(crate) fn transcribe_tail_window(
        &mut self,
        audio: &[f32],
        sample_rate: u32,
        language: Option<&str>,
        control: &crate::stt::LocalExecutionControl,
    ) -> Result<(
        RawTranscript,
        Option<Vec<crate::pipeline::contracts::TranscriptSegment>>,
    )> {
        control.check()?;
        let samples = audio_loader::resample_to_16k(audio, sample_rate);
        if samples.is_empty() {
            return Ok((RawTranscript::default(), None));
        }
        let detected_lang;
        let language = match language {
            Some(lang) => Some(lang),
            None => {
                detected_lang = self.detect_language_16k_controlled(&samples, control)?;
                Some(detected_lang.as_str())
            }
        };
        self.capture_word_alignment = true;
        let decode_started = std::time::Instant::now();
        let transcript = self.transcribe_samples_16k_raw(&samples, language, false, control);
        let decode_ms = decode_started.elapsed().as_millis() as u64;
        self.capture_word_alignment = false;
        let transcript = transcript?;
        let align_started = std::time::Instant::now();
        let words = self.align_captured_words(language)?;
        tracing::info!(
            decode_ms,
            align_ms = align_started.elapsed().as_millis() as u64,
            word_pins = words.as_ref().map_or(0, Vec::len),
            "tail_window_latency"
        );
        self.captured_tokens.clear();
        self.captured_encoder = None;
        self.captured_sample_len = 0;
        Ok((transcript, words))
    }

    /// DTW word pins for the decode just captured. Failure keeps phrase grain.
    fn align_captured_words(
        &mut self,
        language: Option<&str>,
    ) -> Result<Option<Vec<crate::pipeline::contracts::TranscriptSegment>>> {
        if self.alignment_heads.is_empty() || self.captured_tokens.is_empty() {
            if self.alignment_heads.is_empty() {
                tracing::info!("tail_word_pins_unavailable: checkpoint has no alignment_heads");
            }
            return Ok(None);
        }
        let Some(encoder) = self.captured_encoder.clone() else {
            return Ok(None);
        };
        let started = std::time::Instant::now();
        let eot = self
            .tokenizer
            .token_to_id("<|endoftext|>")
            .context("tokenizer missing <|endoftext|>")?;
        let text_tokens = self
            .captured_tokens
            .iter()
            .copied()
            .filter(|token| *token < eot)
            .collect::<Vec<_>>();
        if text_tokens.is_empty() {
            return Ok(None);
        }
        let pieces = text_tokens
            .iter()
            .map(|id| self.tokenizer.id_to_token(*id).unwrap_or_default())
            .collect::<Vec<_>>();
        let spans = super::word_pins::word_token_spans(&pieces);
        if spans.is_empty() {
            return Ok(None);
        }
        let mut groups = Vec::with_capacity(spans.len());
        for (start, len) in spans {
            let end = start + len;
            let text = self
                .tokenizer
                .decode(&text_tokens[start..end], true)
                .unwrap_or_default();
            let text = text.trim().to_string();
            if text.is_empty() {
                return Ok(None);
            }
            groups.push((text, len));
        }
        let mut prefix = Vec::new();
        let sot = self
            .tokenizer
            .token_to_id("<|startoftranscript|>")
            .context("tokenizer missing <|startoftranscript|>")?;
        prefix.push(sot);
        if let Some(lang) = language {
            let lang_tok = format!("<|{}|>", lang.to_lowercase());
            if let Some(id) = self.tokenizer.token_to_id(&lang_tok)
                && (id as usize) < self.config.vocab_size
            {
                prefix.push(id);
            }
        }
        if let Some(id) = self.tokenizer.token_to_id("<|transcribe|>")
            && (id as usize) < self.config.vocab_size
        {
            prefix.push(id);
        }
        let sot_len = prefix.len();
        let no_timestamps = self
            .tokenizer
            .token_to_id("<|notimestamps|>")
            .context("tokenizer missing <|notimestamps|>")?;
        prefix.push(no_timestamps);
        prefix.extend(text_tokens);
        prefix.push(eot);

        let token_tensor = Tensor::new(prefix.as_slice(), &self.device)?.unsqueeze(0)?;
        let heads = self
            .model
            .alignment_qk(&token_tensor, &encoder, &self.alignment_heads)
            .context("alignment cross-attention")?;
        let content_frames = self
            .captured_sample_len
            .div_euclid(whisper::HOP_LENGTH)
            .div_euclid(2);
        let counts = groups.iter().map(|(_, count)| *count).collect::<Vec<_>>();
        let texts = groups
            .iter()
            .map(|(text, _)| text.as_str())
            .collect::<Vec<_>>();
        let Some(mut words) = super::word_pins::align_measured_words(
            &heads,
            sot_len,
            &counts,
            &texts,
            content_frames,
        ) else {
            tracing::info!(
                elapsed_ms = started.elapsed().as_millis() as u64,
                "tail_word_pin_alignment_unmeasured"
            );
            return Ok(None);
        };
        super::word_pins::merge_word_punctuation(&mut words);
        let duration = self.captured_sample_len as f32 / whisper::SAMPLE_RATE as f32;
        let mut segments = Vec::with_capacity(words.len());
        let mut previous_end = 0.0_f32;
        for word in words {
            if word.start_secs + 1.0e-3 < previous_end {
                tracing::info!(
                    elapsed_ms = started.elapsed().as_millis() as u64,
                    "tail_word_pin_alignment_unmeasured"
                );
                return Ok(None);
            }
            let start = word.start_secs.clamp(0.0, duration);
            let end = word.end_secs.min(duration);
            if end <= start {
                tracing::info!(
                    elapsed_ms = started.elapsed().as_millis() as u64,
                    "tail_word_pin_alignment_unmeasured"
                );
                return Ok(None);
            }
            previous_end = end;
            segments.push(crate::pipeline::contracts::TranscriptSegment {
                text: word.text,
                start_ts: start,
                end_ts: end,
            });
        }
        tracing::info!(
            words = segments.len(),
            elapsed_ms = started.elapsed().as_millis() as u64,
            "tail_word_pin_alignment"
        );
        Ok(Some(segments))
    }
}

/// File-level log-mel in 30 s slices, concatenated on the frame axis.
///
/// candle-transformers 0.9.2 log-mel conversion does not truncate to 30 s. It pads
/// `n_len` up to a multiple of 1500 frames (15 s) and then adds another 1500
/// frames. A single call on a long file would therefore emit a longer timeline
/// than the audio. Slicing at `N_SAMPLES` and keeping `chunk.len() / HOP_LENGTH`
/// frames per slice keeps `frames.len() ≈ samples.len() / 160`.
fn file_level_log_mel(config: &Config, samples: &[f32], mel_filters: &[f32]) -> Vec<f32> {
    let n_mels = config.num_mel_bins;
    if n_mels == 0 || samples.is_empty() {
        return Vec::new();
    }
    let hop = whisper::HOP_LENGTH;
    let mut bins: Vec<Vec<f32>> = vec![Vec::new(); n_mels];
    for chunk in samples.chunks(whisper::N_SAMPLES) {
        let padded = whisper::audio::pcm_to_mel(config, chunk, mel_filters);
        let padded_frames = padded.len() / n_mels;
        if padded_frames == 0 {
            continue;
        }
        let take = (chunk.len() / hop).min(padded_frames);
        for (bin, dest) in bins.iter_mut().enumerate() {
            let start = bin * padded_frames;
            dest.extend_from_slice(&padded[start..start + take]);
        }
    }
    let n_frames = bins[0].len();
    let mut out = Vec::with_capacity(n_mels.saturating_mul(n_frames));
    for dest in bins {
        out.extend(dest);
    }
    out
}

/// Load mel filters from an `.npz` on disk, opened through `safe_path`.
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

/// Ratio of raw length to gzip-compressed length.
///
/// A hallucinated loop compresses far better than natural speech, so a high
/// ratio is useful diagnostic evidence for repetition. Empty text yields `0.0`.
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

/// Read `(layer, head)` pairs from the checkpoint's `alignment_heads` tensor.
fn parse_alignment_heads(
    tensor: &Tensor,
    n_layers: usize,
    n_heads: usize,
) -> Result<Vec<(usize, usize)>> {
    let dims = tensor.dims();
    ensure!(
        dims.len() == 2 && dims[1] == 2 && dims[0] > 0,
        "alignment_heads shape {dims:?}, expected [N, 2]"
    );
    let pairs = tensor
        .to_vec2::<i64>()
        .context("alignment_heads must be i64")?;
    let mut heads = Vec::with_capacity(pairs.len());
    for pair in pairs {
        let layer = usize::try_from(pair[0]).context("alignment head layer is negative")?;
        let head = usize::try_from(pair[1]).context("alignment head index is negative")?;
        ensure!(
            layer < n_layers && head < n_heads,
            "alignment head ({layer}, {head}) is outside the decoder ({n_layers} layers × {n_heads} heads)"
        );
        heads.push((layer, head));
    }
    Ok(heads)
}

/// Build a VarBuilder from verified unquantized tensors.
fn is_supported_runtime_tensor(name: &str, tensor: &Tensor) -> bool {
    if name == "alignment_heads" {
        return tensor.dtype() == DType::I64;
    }
    if name.ends_with(".scales") || name.ends_with(".biases") {
        return false;
    }
    matches!(tensor.dtype(), DType::F16 | DType::F32)
}

fn build_varbuilder_from_tensors(
    raw_tensors: HashMap<String, Tensor>,
    device: &Device,
) -> Result<candle_nn::VarBuilder<'static>> {
    if raw_tensors
        .iter()
        .any(|(name, tensor)| !is_supported_runtime_tensor(name, tensor))
    {
        anyhow::bail!("Unsupported Whisper tensor payload refused; fp16 weights are required");
    }
    crate::whisper_weights::validate_mapped_tensor_name_uniqueness(
        raw_tensors.keys().map(String::as_str),
    )?;
    let mut tensor_map = HashMap::new();

    // alignment_heads is integer metadata used by upstream timestamp tooling,
    // not a model weight consumed by Candle's Whisper loader.
    for (name, tensor) in raw_tensors.iter() {
        if name == "alignment_heads" {
            continue;
        }
        let mapped_name = crate::whisper_weights::map_whisper_tensor_name(name);
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

    Ok(candle_nn::VarBuilder::from_tensors(
        tensor_map,
        DType::F32,
        device,
    ))
}

#[cfg(test)]
mod model_payload_tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn decode_hex(raw: &str) -> Vec<u8> {
        let digits: String = raw.chars().filter(|ch| !ch.is_whitespace()).collect();
        assert!(digits.len().is_multiple_of(2));
        digits
            .as_bytes()
            .as_chunks::<2>()
            .0
            .iter()
            .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
            .collect()
    }

    fn write_valid_bundle_artifacts(path: &Path) {
        fs::create_dir_all(path).unwrap();
        fs::write(
            path.join("config.json"),
            include_str!("../../../tests/fixtures/whisper_test_config.json"),
        )
        .unwrap();
        fs::write(
            path.join("tokenizer.json"),
            include_str!("../../../tests/fixtures/whisper_tokenizer.json"),
        )
        .unwrap();
        fs::write(
            path.join("mel_filters.npz"),
            decode_hex(include_str!(
                "../../../tests/fixtures/whisper_mel_filters.npz.hex"
            )),
        )
        .unwrap();
    }

    fn write_tiny_model(path: &Path, name: &str, dtype: &str, payload_bytes: usize) {
        write_valid_bundle_artifacts(path);
        let header = serde_json::json!({
            name: {
                "dtype": dtype,
                "shape": [1],
                "data_offsets": [0, payload_bytes]
            }
        });
        let header = serde_json::to_vec(&header).unwrap();
        let mut file = (header.len() as u64).to_le_bytes().to_vec();
        file.extend_from_slice(&header);
        file.resize(file.len() + payload_bytes, 0);
        fs::write(path.join("weights.safetensors"), file).unwrap();
    }

    #[test]
    fn local_loader_refuses_tiny_u32_safetensors() {
        let temp = TempDir::new().unwrap();
        write_tiny_model(temp.path(), "encoder.weight", "U32", 4);

        let err = LocalWhisperEngine::new(temp.path())
            .err()
            .expect("U32 must be refused");
        assert!(format!("{err:#}").contains("unsupported Whisper tensor dtype U32"));
    }

    #[test]
    fn local_loader_refuses_non_allowlisted_integer_safetensors() {
        let temp = TempDir::new().unwrap();
        write_tiny_model(temp.path(), "encoder.weight", "I32", 4);

        let err = LocalWhisperEngine::new(temp.path())
            .err()
            .expect("I32 must be refused");
        assert!(format!("{err:#}").contains("unsupported Whisper tensor dtype I32"));
    }

    #[test]
    fn tensor_builder_refuses_u32_before_mapping() {
        let mut tensors = HashMap::new();
        tensors.insert(
            "encoder.weight".to_string(),
            Tensor::from_vec(vec![0_u32], 1, &Device::Cpu).unwrap(),
        );

        let err = build_varbuilder_from_tensors(tensors, &Device::Cpu)
            .err()
            .expect("U32 must be refused by the builder gate");
        assert!(format!("{err:#}").contains("refused"));
    }

    #[test]
    fn tensor_builder_excludes_i64_alignment_metadata() {
        let mut tensors = HashMap::new();
        tensors.insert(
            "encoder.weight".to_string(),
            Tensor::from_vec(vec![1.0_f32], 1, &Device::Cpu).unwrap(),
        );
        tensors.insert(
            "alignment_heads".to_string(),
            Tensor::from_vec(
                vec![2_i64, 4, 2, 11, 3, 3, 3, 6, 3, 11, 3, 14],
                (6, 2),
                &Device::Cpu,
            )
            .unwrap(),
        );

        let vb = build_varbuilder_from_tensors(tensors, &Device::Cpu).unwrap();

        assert!(vb.contains_tensor("model.encoder.weight"));
        assert_eq!(
            vb.get_unchecked("model.encoder.weight").unwrap().dtype(),
            DType::F32
        );
        assert!(!vb.contains_tensor("model.alignment_heads"));
    }

    #[test]
    fn tensor_builder_rejects_mapped_name_collisions() {
        let mut tensors = HashMap::new();
        tensors.insert(
            "decoder.ln.weight".to_string(),
            Tensor::from_vec(vec![1.0_f32], 1, &Device::Cpu).unwrap(),
        );
        tensors.insert(
            "decoder.layer_norm.weight".to_string(),
            Tensor::from_vec(vec![2.0_f32], 1, &Device::Cpu).unwrap(),
        );

        let err = build_varbuilder_from_tensors(tensors, &Device::Cpu)
            .err()
            .expect("mapped collision must be rejected");
        let message = format!("{err:#}");
        assert!(message.contains("decoder.ln.weight"), "{message}");
        assert!(message.contains("decoder.layer_norm.weight"), "{message}");
        assert!(
            message.contains("model.decoder.layer_norm.weight"),
            "{message}"
        );
    }

    #[test]
    fn tensor_builder_rejects_float_alignment_metadata() {
        let mut tensors = HashMap::new();
        tensors.insert(
            "alignment_heads".to_string(),
            Tensor::from_vec(vec![1.0_f32], 1, &Device::Cpu).unwrap(),
        );
        let err = build_varbuilder_from_tensors(tensors, &Device::Cpu)
            .err()
            .expect("float alignment metadata must be rejected");
        assert!(format!("{err:#}").contains("refused"));
    }

    #[test]
    fn local_loader_rejects_invalid_tokenizer_before_model_load() {
        let temp = TempDir::new().unwrap();
        write_valid_bundle_artifacts(temp.path());
        let architecture = crate::whisper_weights::parse_whisper_config(
            include_str!("../../../tests/fixtures/whisper_test_config.json"),
            "test fixture",
        )
        .unwrap();
        crate::whisper_weights::write_test_whisper_weights(
            &temp.path().join("weights.safetensors"),
            architecture,
        )
        .unwrap();
        fs::write(temp.path().join("tokenizer.json"), "{}").unwrap();

        let err = LocalWhisperEngine::new(temp.path())
            .err()
            .expect("invalid tokenizer must be rejected");
        let message = format!("{err:#}");
        assert!(message.contains("tokenizer"), "{message}");
        assert!(
            !message.contains("Failed to create Whisper Model"),
            "{message}"
        );
    }

    #[test]
    fn local_loader_rejects_oversized_tokenizer_before_model_load() {
        let temp = TempDir::new().unwrap();
        write_valid_bundle_artifacts(temp.path());
        let architecture = crate::whisper_weights::parse_whisper_config(
            include_str!("../../../tests/fixtures/whisper_test_config.json"),
            "test fixture",
        )
        .unwrap();
        crate::whisper_weights::write_test_whisper_weights(
            &temp.path().join("weights.safetensors"),
            architecture,
        )
        .unwrap();
        fs::File::create(temp.path().join("tokenizer.json"))
            .unwrap()
            .set_len(crate::whisper_weights::MAX_WHISPER_TOKENIZER_BYTES + 1)
            .unwrap();

        let err = LocalWhisperEngine::new(temp.path())
            .err()
            .expect("oversized tokenizer must be rejected");
        let message = format!("{err:#}");
        assert!(message.contains("16777216-byte limit"), "{message}");
        assert!(
            !message.contains("Failed to create Whisper Model"),
            "{message}"
        );
    }

    #[test]
    fn local_loader_rejects_unpinned_mel_before_model_load() {
        let temp = TempDir::new().unwrap();
        write_valid_bundle_artifacts(temp.path());
        let architecture = crate::whisper_weights::parse_whisper_config(
            include_str!("../../../tests/fixtures/whisper_test_config.json"),
            "test fixture",
        )
        .unwrap();
        crate::whisper_weights::write_test_whisper_weights(
            &temp.path().join("weights.safetensors"),
            architecture,
        )
        .unwrap();
        fs::write(
            temp.path().join("mel_filters.npz"),
            vec![0_u8; crate::whisper_weights::MEL_FILTERS_SIZE_BYTES as usize],
        )
        .unwrap();

        let err = LocalWhisperEngine::new(temp.path())
            .err()
            .expect("unpinned mel must be rejected");
        let message = format!("{err:#}");
        assert!(message.contains("SHA-256 mismatch"), "{message}");
        assert!(
            !message.contains("Failed to create Whisper Model"),
            "{message}"
        );
    }

    #[test]
    fn local_loader_uses_valid_alternative_after_invalid_primary() {
        let temp = TempDir::new().unwrap();
        fs::create_dir_all(temp.path()).unwrap();
        fs::write(
            temp.path().join("config.json"),
            include_str!("../../../tests/fixtures/whisper_test_config.json"),
        )
        .unwrap();
        write_valid_bundle_artifacts(temp.path());
        let architecture = crate::whisper_weights::parse_whisper_config(
            include_str!("../../../tests/fixtures/whisper_test_config.json"),
            "test fixture",
        )
        .unwrap();
        crate::whisper_weights::write_test_whisper_weights(
            &temp.path().join("model.safetensors"),
            architecture,
        )
        .unwrap();
        write_tiny_model(temp.path(), "encoder.weight", "U32", 4);

        assert!(
            LocalWhisperEngine::new(temp.path()).is_ok(),
            "compatible alternative should load after invalid primary"
        );
    }
}

/// Decoder diagnostics, Silero filter, and final-pass tests.
#[cfg(test)]
mod dedup_tests {
    use super::*;

    /// Token budget floors short audio and stops a runaway before max_new_tokens.
    #[test]
    fn runaway_watchdog_bails() {
        // 1s of audio with 5 words/s cap, 2 tokens/word, 2x margin => 20 tokens,
        // but RUNAWAY_MIN_BUDGET (64) floors it for short chunks.
        assert_eq!(runaway_token_budget(1.0), RUNAWAY_MIN_BUDGET);

        // 10s of audio: 5 * 10 * 2 * 2 = 200 tokens budget.
        assert_eq!(runaway_token_budget(10.0), 200);

        // The loop bails when generated tokens reach the budget, well before
        // max_new_tokens (448). Simulate the in-loop guard for a runaway decode.
        let audio_sec = 10.0;
        let budget = runaway_token_budget(audio_sec);
        let max_new_tokens = 448usize; // model max_target_positions ceiling
        let mut generated = 0usize;
        for _ in 0..max_new_tokens {
            if generated >= budget {
                break;
            }
            generated += 1; // pretend every step emits a non-EOT token
        }
        assert_eq!(generated, budget);
        assert!(
            generated < max_new_tokens,
            "watchdog must bail before max_new_tokens"
        );

        // Budget is conservative: a normal 10s utterance at a realistic ~2.5
        // words/s, 2 tokens/word = ~50 tokens, far below the 200 budget.
        let normal_tokens = (2.5f32 * 10.0 * 2.0) as usize;
        assert!(
            normal_tokens < budget,
            "normal speech ({normal_tokens}) must not trip budget ({budget})"
        );

        // Zero / negative audio_sec is clamped and floored, never panics.
        assert_eq!(runaway_token_budget(0.0), RUNAWAY_MIN_BUDGET);
        assert_eq!(runaway_token_budget(-5.0), RUNAWAY_MIN_BUDGET);
    }

    /// Initial prompt tokens sit after start-of-prev and before the decode prefix.
    #[test]
    fn initial_prompt_tokens_are_previous_context_before_current_decode_prefix() {
        let without_prompt = vec![1_u32, 2, 3];
        let mut with_prompt = without_prompt.clone();

        let used = prepend_initial_prompt_tokens(&mut with_prompt, 99, &[10, 11, 12], 448);

        assert_eq!(used, 3);
        assert_ne!(with_prompt, without_prompt);
        assert_eq!(&with_prompt[..4], &[99, 10, 11, 12]);
        assert_eq!(&with_prompt[4..], without_prompt.as_slice());
    }

    /// Oversized prompts are truncated to `WHISPER_INITIAL_PROMPT_TOKEN_BUDGET`.
    #[test]
    fn initial_prompt_tokens_are_capped_before_decode() {
        let mut tokens = vec![1_u32, 2, 3];
        let prompt_tokens: Vec<u32> =
            (0..(WHISPER_INITIAL_PROMPT_TOKEN_BUDGET as u32 + 10)).collect();

        let used = prepend_initial_prompt_tokens(&mut tokens, 99, &prompt_tokens, 448);

        assert_eq!(used, WHISPER_INITIAL_PROMPT_TOKEN_BUDGET);
        assert_eq!(tokens.len(), 4 + WHISPER_INITIAL_PROMPT_TOKEN_BUDGET);
        assert_eq!(tokens[0], 99);
        assert_eq!(
            tokens[WHISPER_INITIAL_PROMPT_TOKEN_BUDGET],
            WHISPER_INITIAL_PROMPT_TOKEN_BUDGET as u32 - 1
        );
        assert_eq!(
            &tokens[(WHISPER_INITIAL_PROMPT_TOKEN_BUDGET + 1)..],
            &[1, 2, 3]
        );
    }

    #[test]
    fn initial_prompt_reserves_one_decode_position() {
        let prompt = [10_u32, 11, 12, 13];

        let mut minimum_context = vec![1_u32, 2, 3, 4];
        let used = prepend_initial_prompt_tokens(&mut minimum_context, 99, &prompt, 5);
        assert_eq!(used, 0);
        assert_eq!(minimum_context, vec![1, 2, 3, 4]);
        assert!(minimum_context.len() < 5);

        let mut short_context = vec![1_u32, 2, 3, 4];
        let used = prepend_initial_prompt_tokens(&mut short_context, 99, &prompt, 8);
        assert_eq!(used, 2);
        assert_eq!(short_context, vec![99, 10, 11, 1, 2, 3, 4]);
        assert_eq!(8 - short_context.len(), 1);
    }

    #[test]
    fn initial_prompt_rejects_any_out_of_vocabulary_id() {
        assert!(prompt_token_ids_fit_vocab(&[0, 1, 3], 4));
        assert!(!prompt_token_ids_fit_vocab(&[0, 4], 4));
        assert!(!prompt_token_ids_fit_vocab(&[5], 4));
    }

    /// Control-token suppression applies only at decode step zero.
    #[test]
    fn decoder_control_tokens_are_only_suppressed_before_first_token() {
        assert!(should_suppress_decoder_control_tokens(0));
        assert!(!should_suppress_decoder_control_tokens(1));
        assert!(!should_suppress_decoder_control_tokens(15));
    }

    #[test]
    fn blank_suppression_covers_opening_timestamp_and_first_text_tokens() {
        let masked_space = vec![
            1.0,
            1.0,
            f32::NEG_INFINITY,
            1.0,
            1.0,
            f32::NEG_INFINITY,
            1.0,
            1.0,
            1.0,
        ];
        let mut opening = vec![1.0; 9];
        apply_initial_blank_suppression(&mut opening, 0, &[2, 5], 7);
        assert_eq!(
            opening,
            vec![
                1.0,
                1.0,
                f32::NEG_INFINITY,
                1.0,
                1.0,
                f32::NEG_INFINITY,
                1.0,
                f32::NEG_INFINITY,
                1.0
            ]
        );
        for generated in 1..INITIAL_BLANK_TOKEN_WINDOW {
            let mut text_step = vec![1.0; 9];
            apply_initial_blank_suppression(&mut text_step, generated, &[2, 5], 7);
            assert_eq!(
                text_step, masked_space,
                "step {generated} still masks the space token and leaves end-of-text"
            );
        }
        for generated in [INITIAL_BLANK_TOKEN_WINDOW, 15] {
            let mut later = vec![1.0; 9];
            apply_initial_blank_suppression(&mut later, generated, &[2, 5], 7);
            assert_eq!(later, vec![1.0; 9], "step {generated}");
        }
    }

    #[test]
    fn blank_suppression_bounds_tokenizer_ids_to_logits() {
        let mut logits = vec![1.0; 3];
        apply_initial_blank_suppression(&mut logits, 0, &[1, u32::MAX], 8);
        assert_eq!(logits, vec![1.0, f32::NEG_INFINITY, 1.0]);
        apply_initial_blank_suppression(&mut [], 0, &[1], 8);
    }

    /// Embedded lexicon cleanup reports Changed with rewrite counts.
    #[cfg(any())]
    #[test]
    fn requested_final_pass_reports_embedded_lexicon_changes() {
        let raw = RawTranscript {
            text: "doker".to_string(),
            energy: None,
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

    /// Known no-speech skip path records Skipped with the VAD reason.
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

    /// Artifact-token drift rejects the candidate and keeps the raw transcript.
    #[cfg(any())]
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

// ─── stt-live-first-v2 TDD stubs (dispatch 2026-08-10) ──────────────────────
//
// The seam function below remains a contract stub for cut w1-c. Ground truth
// for both cuts is the operator's three-way recording
// (tests/e2e_long_window_truth.rs).

/// Plan long-file decode windows aligned to VAD silence spans.
///
/// Contract (w1-a): boundaries between consecutive windows land INSIDE a
/// silence span whenever one exists near the target step; windows stay within
/// [`VAD_WINDOW_MIN_SECS`, `VAD_WINDOW_MAX_SECS`]; consecutive windows overlap
/// so no audio is skipped. A fixed-step grid is only the fallback for audio
/// with no usable silences (constant speech).
///
pub const VAD_WINDOW_MIN_SECS: f32 = 6.0;
pub const VAD_WINDOW_MAX_SECS: f32 = 28.0;

const TARGET_WINDOW_SECS: f32 = 25.0;
const VAD_WINDOW_OVERLAP_SECS: f32 = 5.0;
const VAD_BOUNDARY_TOLERANCE_SECS: f32 = 5.0;

/// Calibration for the shared VAD-aligned window planner.
///
/// File decode and the live rolling lane use the same boundary algorithm with
/// different competence horizons. Keeping the policy explicit prevents the
/// live bridge from forking a second silence picker.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct VadWindowPlanConfig {
    pub min_secs: f32,
    pub target_secs: f32,
    pub max_secs: f32,
    pub overlap_secs: f32,
    pub boundary_tolerance_secs: f32,
}

impl VadWindowPlanConfig {
    const FILE: Self = Self {
        min_secs: VAD_WINDOW_MIN_SECS,
        target_secs: TARGET_WINDOW_SECS,
        max_secs: VAD_WINDOW_MAX_SECS,
        overlap_secs: VAD_WINDOW_OVERLAP_SECS,
        boundary_tolerance_secs: VAD_BOUNDARY_TOLERANCE_SECS,
    };
}

pub fn plan_vad_aligned_windows(silences: &[(f32, f32)], total_secs: f32) -> Vec<(f32, f32)> {
    plan_vad_aligned_windows_with_config(silences, total_secs, VadWindowPlanConfig::FILE)
}

/// Shared planner with an explicit window calibration.
pub(crate) fn plan_vad_aligned_windows_with_config(
    silences: &[(f32, f32)],
    total_secs: f32,
    config: VadWindowPlanConfig,
) -> Vec<(f32, f32)> {
    if !total_secs.is_finite() || total_secs <= 0.0 {
        return Vec::new();
    }

    let usable_silences: Vec<(f32, f32)> = silences
        .iter()
        .filter_map(|&(start, end)| {
            if !start.is_finite() || !end.is_finite() {
                return None;
            }
            let start = start.clamp(0.0, total_secs);
            let end = end.clamp(0.0, total_secs);
            (end > start).then_some((start, end))
        })
        .collect();

    let mut windows = Vec::new();
    let mut start = 0.0_f32;
    while start < total_secs {
        if total_secs - start <= config.max_secs {
            windows.push((start, total_secs));
            break;
        }

        let target = start + config.target_secs;
        let candidate_min = (start + config.min_secs).max(target - config.boundary_tolerance_secs);
        let candidate_max = (start + config.max_secs)
            .min(target + config.boundary_tolerance_secs)
            .min(total_secs);

        let boundary = usable_silences
            .iter()
            .filter_map(|&(silence_start, silence_end)| {
                let lo = silence_start.max(candidate_min);
                let hi = silence_end.min(candidate_max);
                if hi < lo {
                    return None;
                }
                let point = target.clamp(lo, hi);
                Some((point, (point - target).abs()))
            })
            .min_by(|(_, a_distance), (_, b_distance)| a_distance.total_cmp(b_distance))
            .map(|(point, _)| point)
            .unwrap_or_else(|| (start + config.target_secs).min(total_secs));

        let boundary = boundary.min(start + config.max_secs).min(total_secs);
        windows.push((start, boundary));

        let next_start = (boundary - config.overlap_secs).max(0.0);
        if next_start <= start {
            break;
        }
        start = next_start;
    }
    windows
}

/// Convert the existing 500 ms Silero probability stream into contiguous
/// silence spans for the window planner.
pub(crate) fn silence_spans_from_vad_probabilities(
    probabilities: &[f32],
    threshold: f32,
    total_secs: f32,
) -> Vec<(f32, f32)> {
    if probabilities.is_empty() || !threshold.is_finite() || total_secs <= 0.0 {
        return Vec::new();
    }

    let window_sec = crate::vad::DISCRIMINATOR_WINDOW_MS as f32 / 1000.0;
    let mut spans = Vec::new();
    let mut index = 0usize;
    while index < probabilities.len() {
        if probabilities[index] >= threshold {
            index += 1;
            continue;
        }
        let run_start = index;
        while index < probabilities.len() && probabilities[index] < threshold {
            index += 1;
        }
        let start = run_start as f32 * window_sec;
        let end = (index as f32 * window_sec).min(total_secs);
        if end > start {
            spans.push((start, end));
        }
    }
    spans
}

/// How many times a long-file window may be halved after decoding words
/// without a closed timestamp span (2 = down to quarters of the planned
/// window) before that span is refused and the file continues without it.
const SEGMENTLESS_WINDOW_MAX_SPLITS: u8 = 2;

/// Windows shorter than this are not split further; a runaway decode on
/// two seconds of audio is refused outright.
const SEGMENTLESS_WINDOW_MIN_SPLIT_SECS: f32 = 2.0;

/// Merge the next window's transcript onto the accumulated one, deduplicating
/// the overlap REGION by segment time instead of by text.
///
/// Contract (w1-c): segments of `next` that end before `overlap_end_secs`
/// re-describe audio the previous window already decoded (usually with
/// divergent text — that is WHY text-based dedup misses them) and must be
/// dropped; segments past the overlap are appended verbatim.
///
pub fn merge_chunk_transcripts(
    out: &mut crate::pipeline::contracts::RawTranscript,
    next: crate::pipeline::contracts::RawTranscript,
    overlap_end_secs: f32,
) -> Result<()> {
    ensure!(
        out.text.trim().is_empty() || !out.segments.is_empty(),
        "overlap assembly requires segment provenance for accumulated text"
    );

    if next.text.trim().is_empty() && next.segments.is_empty() {
        return Ok(());
    }

    ensure!(
        next.text.trim().is_empty() || !next.segments.is_empty(),
        "overlap assembly refused non-empty decode without timestamped segments"
    );

    out.segments.extend(
        next.segments
            .into_iter()
            .filter(|segment| segment.end_ts > overlap_end_secs),
    );
    out.text = out
        .segments
        .iter()
        .map(|segment| segment.text.trim())
        .filter(|text| !text.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    Ok(())
}

#[cfg(test)]
mod stt_live_first_v2_red {
    use super::*;
    use crate::pipeline::contracts::{RawTranscript, TranscriptSegment};

    /// Timestamp mode must actively force a clock token; omitting the
    /// `<|notimestamps|>` prompt token alone produced `segments=0` in the real
    /// file route.
    #[test]
    fn timestamp_rules_force_an_initial_timestamp_then_text() {
        let range = timestamps::TimestampRange {
            begin: 5,
            end_inclusive: 7,
        };
        let mut initial_logits = vec![1.0; 8];
        apply_timestamp_rules(&mut initial_logits, &[], 3, Some(4), &range);
        assert!(initial_logits[..5].iter().all(|value| value.is_infinite()));
        assert!(initial_logits[5..].iter().all(|value| value.is_finite()));

        let mut after_timestamp = vec![1.0; 8];
        apply_timestamp_rules(&mut after_timestamp, &[5], 3, Some(4), &range);
        assert!(after_timestamp[..3].iter().all(|value| value.is_finite()));
        assert!(
            after_timestamp[5..].iter().all(|value| value.is_infinite()),
            "a single opening timestamp must be followed by text"
        );
    }

    /// Silences at 22–23 s and 41–42.5 s: every internal boundary must land in
    /// one of them, not on the bare 20 s/40 s grid. RED until w1-a.
    #[test]
    fn window_boundaries_land_inside_silence_spans() {
        let silences = [(22.0_f32, 23.0_f32), (41.0, 42.5)];
        let windows = plan_vad_aligned_windows(&silences, 60.0);
        assert!(windows.len() >= 2, "60 s must yield multiple windows");
        for pair in windows.windows(2) {
            let boundary = pair[0].1; // end of the earlier window
            if boundary >= 60.0 {
                continue;
            }
            assert!(
                silences
                    .iter()
                    .any(|(s, e)| boundary >= *s && boundary <= *e),
                "boundary {boundary}s falls outside every silence span — mid-speech window \
                 starts derail the decoder (measured: window c6 @120s, 2026-08-10)"
            );
        }
    }

    /// Windows must respect the decode-competence floor and ceiling.
    #[test]
    fn windows_stay_within_competence_bounds() {
        let silences = [(7.0_f32, 7.4), (14.0, 14.5), (21.0, 21.5), (28.0, 28.5)];
        let windows = plan_vad_aligned_windows(&silences, 30.0);
        for (start, end) in &windows {
            let len = end - start;
            let is_tail = (*end - 30.0).abs() < f32::EPSILON;
            assert!(
                len <= VAD_WINDOW_MAX_SECS,
                "window {start}-{end} exceeds {VAD_WINDOW_MAX_SECS}s"
            );
            assert!(
                is_tail || len >= VAD_WINDOW_MIN_SECS,
                "non-tail window {start}-{end} under {VAD_WINDOW_MIN_SECS}s (clips <6s decode as \
                 No speech — measured on operator fixtures)"
            );
        }
    }

    /// Constant speech keeps the historical 25 s window / 20 s step exactly.
    #[test]
    fn constant_speech_falls_back_to_legacy_grid() {
        let windows = plan_vad_aligned_windows(&[], 60.0);
        assert_eq!(windows, vec![(0.0, 25.0), (20.0, 45.0), (40.0, 60.0)]);
        assert!(
            windows
                .windows(2)
                .all(|pair| pair[1].0 < pair[0].1 && pair[1].0 > pair[0].0),
            "fallback windows must overlap while continuing to advance"
        );
    }

    /// The planner consumes the existing Silero 500 ms probability timeline.
    #[test]
    fn vad_probabilities_coalesce_into_silence_spans() {
        let spans = silence_spans_from_vad_probabilities(&[0.9, 0.2, 0.1, 0.8, 0.3, 0.7], 0.5, 3.0);
        assert_eq!(spans, vec![(0.5, 1.5), (2.0, 2.5)]);
    }

    /// The seam judge drops next-window segments that re-describe the overlap
    /// with divergent text. RED until w1-c.
    #[test]
    fn seam_merge_drops_overlap_redecode_by_time() {
        let mut out = RawTranscript {
            text: "mówię teraz spokojnie prostymi słowami bez żadnych pułapek".into(),
            segments: vec![TranscriptSegment {
                text: "mówię teraz spokojnie prostymi słowami bez żadnych pułapek".into(),
                start_ts: 15.0,
                end_ts: 24.0,
            }],
            energy: None,
            ..Default::default()
        };
        // Next window starts at 20 s; its decode of the 20–25 s overlap came out
        // DIFFERENT ("Zdanie pierwsze.") — text dedup can never match it.
        let next = RawTranscript {
            text: "Zdanie pierwsze. Zdanie drugie. Whisper, Codescribe i Loctree".into(),
            segments: vec![
                TranscriptSegment {
                    text: "Zdanie pierwsze.".into(),
                    start_ts: 20.5,
                    end_ts: 24.0,
                },
                TranscriptSegment {
                    text: "Zdanie drugie. Whisper, Codescribe i Loctree".into(),
                    start_ts: 25.5,
                    end_ts: 33.0,
                },
            ],
            energy: None,
            ..Default::default()
        };
        merge_chunk_transcripts(&mut out, next, 25.0)
            .expect("timestamped overlap merge must succeed");
        assert!(
            !out.text.contains("Zdanie pierwsze"),
            "overlap re-decode leaked into the merged text: {}",
            out.text
        );
        assert!(
            out.text.contains("Zdanie drugie"),
            "post-overlap content must be appended: {}",
            out.text
        );
        assert_eq!(
            out.segments.len(),
            2,
            "one segment per real utterance — overlap segment dropped"
        );
    }

    /// A segment crossing the time seam contains new audio and must survive as
    /// one whole segment; only spans ending inside the overlap are discarded.
    #[test]
    fn seam_merge_keeps_a_boundary_straddler_once() {
        let mut out = RawTranscript {
            text: "trusted earlier middle".into(),
            segments: vec![TranscriptSegment {
                text: "trusted earlier middle".into(),
                start_ts: 2.0,
                end_ts: 10.0,
            }],
            energy: None,
            ..Default::default()
        };
        let next = RawTranscript {
            text: "divergent head boundary bridge clean tail".into(),
            segments: vec![
                TranscriptSegment {
                    text: "divergent head".into(),
                    start_ts: 8.0,
                    end_ts: 9.8,
                },
                TranscriptSegment {
                    text: "boundary bridge".into(),
                    start_ts: 9.8,
                    end_ts: 11.0,
                },
                TranscriptSegment {
                    text: "clean tail".into(),
                    start_ts: 11.0,
                    end_ts: 13.0,
                },
            ],
            energy: None,
            ..Default::default()
        };

        merge_chunk_transcripts(&mut out, next, 10.0)
            .expect("timestamped boundary merge must succeed");

        assert_eq!(out.segments.len(), 3);
        assert_eq!(
            out.text,
            "trusted earlier middle boundary bridge clean tail"
        );
        assert_eq!(out.text.matches("boundary bridge").count(), 1);
        assert!(!out.text.contains("  "));
    }

    /// Non-empty decoder output without timestamp provenance fails closed.
    #[test]
    fn seam_merge_rejects_nonempty_segmentless_decode() {
        let mut out = RawTranscript {
            text: "one two".into(),
            segments: vec![TranscriptSegment {
                text: "one two".into(),
                start_ts: 0.0,
                end_ts: 2.0,
            }],
            energy: None,
            ..Default::default()
        };
        let next = RawTranscript {
            text: "two middle three".into(),
            energy: None,
            ..Default::default()
        };

        let error = merge_chunk_transcripts(&mut out, next, 2.0)
            .expect_err("segmentless non-empty decode must fail closed");

        assert!(
            error
                .to_string()
                .contains("overlap assembly refused non-empty decode without timestamped segments")
        );
        assert_eq!(out.text, "one two");
        assert_eq!(out.segments.len(), 1);
        assert_eq!(out.segments[0].text, "one two");
        assert_eq!(out.segments[0].start_ts, 0.0);
        assert_eq!(out.segments[0].end_ts, 2.0);
    }

    /// Accumulated text without timestamp provenance is not lawful input.
    #[test]
    fn seam_merge_rejects_text_without_segment_provenance() {
        let mut out = RawTranscript {
            text: "one two three".into(),
            energy: None,
            ..Default::default()
        };
        let next = RawTranscript {
            text: "two three four".into(),
            segments: vec![TranscriptSegment {
                text: "four".into(),
                start_ts: 3.0,
                end_ts: 4.0,
            }],
            energy: None,
            ..Default::default()
        };

        let error = merge_chunk_transcripts(&mut out, next, 3.0)
            .expect_err("accumulated text without segments must fail closed");

        assert!(
            error
                .to_string()
                .contains("overlap assembly requires segment provenance for accumulated text")
        );
        assert_eq!(out.text, "one two three");
        assert!(out.segments.is_empty());
    }

    /// A refused segmentless chunk cannot poison a later timestamped retry.
    #[test]
    fn seam_merge_segmentless_failure_preserves_state_for_timestamped_retry() {
        let mut out = RawTranscript {
            text: "one two".into(),
            segments: vec![TranscriptSegment {
                text: "one two".into(),
                start_ts: 0.0,
                end_ts: 2.0,
            }],
            energy: None,
            ..Default::default()
        };
        let middle = RawTranscript {
            text: "two middle three".into(),
            energy: None,
            ..Default::default()
        };
        let tail = RawTranscript {
            text: "two replay four five".into(),
            segments: vec![
                TranscriptSegment {
                    text: "two replay".into(),
                    start_ts: 1.5,
                    end_ts: 2.0,
                },
                TranscriptSegment {
                    text: "four five".into(),
                    start_ts: 3.0,
                    end_ts: 4.0,
                },
            ],
            energy: None,
            ..Default::default()
        };

        let error = merge_chunk_transcripts(&mut out, middle, 2.0)
            .expect_err("segmentless middle decode must fail closed");
        assert!(
            error
                .to_string()
                .contains("overlap assembly refused non-empty decode without timestamped segments")
        );
        assert_eq!(out.text, "one two");
        assert_eq!(out.segments.len(), 1);
        assert_eq!(out.segments[0].text, "one two");

        merge_chunk_transcripts(&mut out, tail, 2.0)
            .expect("timestamped retry must succeed after segmentless failure");

        assert_eq!(out.text, "one two four five");
        assert_eq!(out.segments.len(), 2);
        assert_eq!(out.segments[0].text, "one two");
        assert_eq!(out.segments[1].text, "four five");
        assert!(!out.text.contains("middle"));
    }
}

#[cfg(test)]
mod local_execution_control_tests {
    use super::*;
    use crate::stt::{LocalExecutionBoundary, LocalExecutionControl};

    /// Tiny CPU weights exercise the real encoder/decoder without a model
    /// download, Metal, VAD runtime or process-global engine mutation.
    fn engine() -> LocalWhisperEngine {
        let config = Config {
            num_mel_bins: 80,
            max_source_positions: 1500,
            d_model: 4,
            encoder_attention_heads: 1,
            encoder_layers: 1,
            vocab_size: 5,
            max_target_positions: 16,
            decoder_attention_heads: 1,
            decoder_layers: 1,
            suppress_tokens: Vec::new(),
        };
        let device = Device::Cpu;
        let vb = candle_nn::VarBuilder::zeros(candle_core::DType::F32, &device);
        let model = Model::load(&vb, config.clone()).unwrap();
        let wordlevel = tokenizers::models::wordlevel::WordLevel::builder()
            .vocab(
                [
                    ("hello".to_string(), 0),
                    ("<|startoftranscript|>".to_string(), 1),
                    ("<|endoftext|>".to_string(), 2),
                    ("<|transcribe|>".to_string(), 3),
                    ("unknown".to_string(), 4),
                ]
                .into_iter()
                .collect(),
            )
            .unk_token("unknown".into())
            .build()
            .unwrap();
        LocalWhisperEngine {
            model,
            tokenizer: Tokenizer::new(wordlevel),
            device,
            config,
            mel_filters: vec![0.0; 80 * 201],
            ts_range: None,
            engine_provenance: TranscriptionEngineVerdict::whisper(
                TranscriptionEngineMode::RuntimeFallback,
            ),
            decoding_params: DecodingParams {
                initial_prompt: Some("previous".into()),
                ..DecodingParams::default()
            },
            alignment_heads: Vec::new(),
            capture_word_alignment: false,
            captured_tokens: Vec::new(),
            captured_encoder: None,
            captured_sample_len: 0,
        }
    }

    #[test]
    fn production_window_and_token_cancellation_restore_request_state() {
        for boundary in [
            LocalExecutionBoundary::Window,
            LocalExecutionBoundary::Token,
        ] {
            let mut engine = engine();
            let control = LocalExecutionControl::cancelling_at(boundary);
            let result = engine.with_request(Some("hello".into()), |engine| {
                engine.transcribe_long_with_language_segments_using_silences(
                    &[0.25; 3200],
                    16_000,
                    Some("en"),
                    &[],
                    &mut |_| Ok(()),
                    &control,
                )
            });
            assert!(result.unwrap_err().to_string().contains("cancelled"));
            assert!(
                control.check().is_err(),
                "the requested production boundary was reached"
            );
            assert_eq!(
                engine.decoding_params.initial_prompt.as_deref(),
                Some("previous")
            );
            // An independent successor still executes the same decoder. It
            // cannot inherit the prior request's cancellation or prompt.
            let successor = engine
                .with_request(None, |engine| {
                    engine.transcribe_samples_16k_raw(
                        &[0.25; 3200],
                        Some("en"),
                        false,
                        &LocalExecutionControl::default(),
                    )
                })
                .unwrap();
            assert!(!successor.text.is_empty());
            assert_eq!(
                engine.decoding_params.initial_prompt.as_deref(),
                Some("previous")
            );
        }
    }

    #[test]
    fn request_scope_restores_prompt_after_error_and_unwind() {
        let mut engine = engine();
        let failed: Result<()> = engine.with_request(Some("temporary".into()), |_| {
            Err(anyhow!("injected decoder failure"))
        });
        assert!(failed.is_err());
        assert_eq!(
            engine.decoding_params.initial_prompt.as_deref(),
            Some("previous")
        );
        let unwind = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _: Result<()> = engine.with_request(None, |_| panic!("native worker panic"));
        }));
        assert!(unwind.is_err());
        assert_eq!(
            engine.decoding_params.initial_prompt.as_deref(),
            Some("previous")
        );
    }
}
