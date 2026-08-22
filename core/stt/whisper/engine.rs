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
use crate::pipeline::stream_postprocess::{
    StreamPostProcessStats, StreamPostProcessor, WHISPER_INITIAL_PROMPT_TOKEN_BUDGET,
    final_pass_guardrail_reason,
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

/// Callback for streaming chunk results (called after each chunk is transcribed)
pub type ChunkCallback<'a> = &'a dyn Fn(&str);

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
/// Whisper's marker introducing previous-context tokens. Everything between it
/// and the decode prefix is treated by the model as prior context, not as text
/// to transcribe.
const WHISPER_START_OF_PREVIOUS_TOKEN: &str = "<|startofprev|>";

/// Token budget for the in-loop runaway watchdog given the chunk audio length.
///
/// Derived from the shared words-per-second cap
/// (`quality_gate::MAX_WORDS_PER_SEC`) times tokens-per-word and a generous
/// safety margin. When generated tokens exceed this budget the decode loop bails
/// instead of paying the full O(n^2)/O(n^3) cost of a runaway hallucination.
fn runaway_token_budget(audio_sec: f32) -> usize {
    let raw = (crate::pipeline::streaming::quality_gate::MAX_WORDS_PER_SEC
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

/// Run the requested final pass over a raw transcript.
///
/// [`FinalPassMode::None`] returns the raw text untouched.
/// `EmbeddedLexiconCleanup` runs the stream post-processor; if cleanup empties
/// the text the result is a `Dropped` verdict with empty output, which the
/// caller must treat as "no speech" rather than as a transcript.
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

/// Fold a Silero VAD filtering result back into the transcript.
///
/// Segments are always replaced, but the **text** is preserved from `raw` when
/// nothing was actually dropped and the filtered text is merely an equivalent
/// or a strict subset. Rebuilding text from segments loses punctuation and
/// casing, so it is only accepted when a real drop justifies it.
fn apply_silero_filter_outcome(
    raw: &RawTranscript,
    filtered_text: String,
    filtered_segments: Vec<crate::pipeline::contracts::TranscriptSegment>,
    dropped_count: u32,
) -> RawTranscript {
    let mut filtered = raw.clone();
    let should_preserve_raw_text = dropped_count == 0
        && (is_text_equivalent(&filtered_text, &raw.text)
            || is_strict_text_subset(&filtered_text, &raw.text));
    filtered.text = if should_preserve_raw_text {
        raw.text.clone()
    } else {
        filtered_text
    };
    filtered.segments = filtered_segments;
    filtered
}

/// Whether `candidate` is a non-empty, strictly smaller fragment of
/// `full_text` once both are normalized. Equality is deliberately excluded —
/// that case is [`is_text_equivalent`].
fn is_strict_text_subset(candidate: &str, full_text: &str) -> bool {
    let candidate = normalize_transcript_text(candidate);
    let full_text = normalize_transcript_text(full_text);
    !candidate.is_empty() && candidate != full_text && full_text.contains(&candidate)
}

/// Whether two non-empty texts are the same modulo casing, punctuation and
/// whitespace.
fn is_text_equivalent(candidate: &str, full_text: &str) -> bool {
    let candidate = normalize_transcript_text(candidate);
    let full_text = normalize_transcript_text(full_text);
    !candidate.is_empty() && candidate == full_text
}

/// Reduce a transcript to lowercase alphanumeric words joined by single spaces.
///
/// Comparison-only helper: it deliberately destroys punctuation and casing so
/// that two renderings of the same speech compare equal. Never store its output
/// as a transcript.
fn normalize_transcript_text(text: &str) -> String {
    text.split_whitespace()
        .filter_map(|token| {
            let mut normalized = String::new();
            for ch in token.chars() {
                if ch.is_alphanumeric() {
                    normalized.extend(ch.to_lowercase());
                }
            }
            if normalized.is_empty() {
                None
            } else {
                Some(normalized)
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Whether decoder control tokens should be suppressed at this decode step.
///
/// Only before the first generated token: suppressing them later would stop the
/// model from ever emitting `<|endoftext|>` and turn a normal utterance into a
/// runaway decode.
fn should_suppress_decoder_control_tokens(generated_tokens: usize) -> bool {
    generated_tokens == 0
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
}

impl LocalWhisperEngine {
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

        let vb = unsafe {
            let tensors = candle_core::safetensors::MmapedSafetensors::new(&weights_path)?;
            let mut raw_tensors: HashMap<String, Tensor> = HashMap::new();

            // Load the verified unquantized tensors on CPU before device transfer.
            let read_started = std::time::Instant::now();
            for (name, view) in tensors.tensors() {
                if name == "alignment_heads" {
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
        let raw_tensors = candle_core::safetensors::load_buffer(embedded.weights, &Device::Cpu)
            .context("Failed to deserialize embedded weights")?;

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
    /// The complete path: load and resample the audio, transcribe (chunked for
    /// long input), apply the Silero VAD filter, then run the requested final
    /// pass. The returned [`TranscriptionVerdict`] carries both the delivered
    /// text and the raw text, so a rejected final pass stays auditable.
    ///
    /// `language` of `None` triggers detection from the audio itself.
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
        )?;
        super::timing::record_inference_ms(inference_started.elapsed().as_millis() as u64);
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

            let dropped_count = outcome.dropped_count;
            let filtered =
                apply_silero_filter_outcome(&raw, outcome.text, outcome.segments, dropped_count);
            (filtered, dropped_count)
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

        self.transcribe_samples_16k_raw(&samples, language, debug_tokens)
    }

    /// Transcribe arbitrarily long audio in VAD-aligned, overlapping windows.
    ///
    /// The overlap exists so a word split across a boundary is still heard
    /// whole; [`merge_chunk_transcripts`] then removes the duplicated region by
    /// segment time, falling back to [`append_with_overlap_dedup`] only when a
    /// decoder does not provide segments.
    /// Segment timestamps are rebased onto the full recording, `avg_logprob` is
    /// averaged across chunks, and `compression_ratio` reports the **worst**
    /// chunk — one hallucinating window must not be hidden by good neighbours.
    pub fn transcribe_long_with_language_segments(
        &mut self,
        audio: &[f32],
        sample_rate: u32,
        language: Option<&str>,
    ) -> Result<RawTranscript> {
        let (_, stats) = crate::vad::extract_speech(audio, sample_rate);
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
        )
    }

    /// Decode long audio using silence spans already measured by the caller.
    ///
    /// File transcription passes its existing Silero result here so window
    /// planning does not add a second VAD run to the stop-path budget.
    fn transcribe_long_with_language_segments_using_silences(
        &mut self,
        audio: &[f32],
        sample_rate: u32,
        language: Option<&str>,
        silence_spans: &[(f32, f32)],
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

        let total_secs = samples.len() as f32 / 16_000.0;
        let windows = plan_vad_aligned_windows(silence_spans, total_secs);
        tracing::debug!(
            window_count = windows.len(),
            silence_span_count = silence_spans.len(),
            "planned VAD-aligned long-file decode windows"
        );

        let mut merged = RawTranscript::default();
        let mut covered_until_secs = 0.0_f32;
        let mut logprob_sum = 0.0_f32;
        let mut logprob_count = 0_u32;
        let mut worst_compression = 0.0_f32;
        let mut any_quality_gate_dropped = false;

        for (start_sec, end_sec) in windows {
            let start = ((start_sec * 16_000.0).round() as usize).min(samples.len());
            let end = ((end_sec * 16_000.0).round() as usize).min(samples.len());
            if end <= start {
                continue;
            }
            let chunk = &samples[start..end];
            let mut transcript = self.transcribe_samples_16k_raw(chunk, language, debug_tokens)?;

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
                let offset_sec = start as f32 / 16_000.0;
                transcript.segments.iter_mut().for_each(|segment| {
                    segment.start_ts += offset_sec;
                    segment.end_ts += offset_sec;
                });
            }

            let overlap_end_secs = covered_until_secs.max(start_sec);
            merge_chunk_transcripts(&mut merged, transcript, overlap_end_secs);
            covered_until_secs = covered_until_secs.max(end_sec);
        }

        Ok(RawTranscript {
            text: dedup_repetitions(merged.text.trim()),
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

    /// Text-only wrapper over [`Self::transcribe_samples_16k_raw`].
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

    /// The decode loop: mel spectrogram, encoder pass, then greedy decoding of
    /// one audio window into text, segments and quality signals.
    ///
    /// Three guards run inside the loop and are the reason this function is not
    /// a thin wrapper over the model:
    /// - a runaway watchdog ([`runaway_token_budget`]) bails before a
    ///   hallucination costs the full quadratic decode,
    /// - [`NgramBlocker`] suppresses repeated n-grams incrementally,
    /// - the quality gate ([`should_drop_for_quality_gate`]) can discard a
    ///   window whose logprob and compression ratio both look pathological.
    ///
    /// `debug_tokens` logs the raw token stream for diagnosis.
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
        let no_timestamps_token = self.tokenizer.token_to_id("<|notimestamps|>");
        let start_of_previous_token = self.tokenizer.token_to_id(WHISPER_START_OF_PREVIOUS_TOKEN);

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
        let encoder_output = self.model.encoder.forward(&mel, true)?;

        // Decoder loop – allow up to the configured maximum target positions minus initial tokens
        let max_new_tokens = self
            .config
            .max_target_positions
            .saturating_sub(tokens.len());
        let ngram_size = self.decoding_params.no_repeat_ngram_size;
        let mut ngram_blocker = NgramBlocker::new(ngram_size);

        // Runaway watchdog: cap generated tokens at a generous multiple of the
        // plausible word rate for this chunk's audio length, so a hallucinating
        // decode bails early instead of grinding to max_new_tokens at O(n^2) cost.
        let audio_sec = samples_16k.len() as f32 / whisper::SAMPLE_RATE as f32;
        let runaway_budget = runaway_token_budget(audio_sec);
        let mut runaway_tripped = false;

        let mut sum_logprob = 0.0f32;
        let mut token_count = 0usize;

        for step in 0..max_new_tokens {
            if all_tokens.len() >= runaway_budget {
                tracing::warn!(
                    "Runaway watchdog tripped: {} tokens for {:.2}s audio (budget {})",
                    all_tokens.len(),
                    audio_sec,
                    runaway_budget
                );
                runaway_tripped = true;
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

            // Apply no_repeat_ngram blocking (faster-whisper style).
            // Block tokens that would create a repeated n-gram. Uses an
            // incremental lookup (see NgramBlocker) instead of a full O(n) scan
            // of all_tokens per step.
            for &blocked_token in ngram_blocker.blocked_tokens(&all_tokens) {
                let idx = blocked_token as usize;
                if idx < logits_vec.len() {
                    logits_vec[idx] = f32::NEG_INFINITY;
                }
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
            ngram_blocker.observe(&all_tokens);
        }

        // Runaway decode: drop the transcript rather than emit a hallucinated
        // wall of text. Mirrors the post-hoc quality gate's dropped contract.
        if runaway_tripped {
            let avg_logprob = (token_count > 0).then(|| sum_logprob / token_count as f32);
            return Ok(RawTranscript {
                avg_logprob,
                quality_gate_dropped: true,
                ..Default::default()
            });
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

/// Normalize a word for overlap comparison: lowercase, alphanumerics only.
///
/// Falls back to the lowercased original when stripping would leave nothing, so
/// a purely punctuation token still compares as itself instead of matching
/// every other punctuation token.
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

/// Incremental no-repeat n-gram blocker.
///
/// Replaces the per-step O(n) full scan of `all_tokens` (which made the decode
/// loop O(n^2) in blocking cost) with an O(1)-amortized map from each completed
/// `(ngram_size - 1)`-gram to the set of tokens that have followed it. After
/// every emitted token the new trailing window is recorded; before each step the
/// current tail's `(ngram_size - 1)`-gram is looked up to find tokens to block.
///
/// Behavior is identical to the previous full-scan: it blocks exactly the tokens
/// that ever followed the current `(ngram_size - 1)`-gram tail elsewhere in the
/// generated sequence. `ngram_size == 0` disables blocking; sequences shorter
/// than `ngram_size` produce no blocks.
struct NgramBlocker {
    ngram_size: usize,
    // (n-1)-gram window -> tokens observed immediately after it.
    seen: HashMap<Vec<u32>, Vec<u32>>,
    emitted: usize,
}

impl NgramBlocker {
    /// Create a blocker for `ngram_size`; `0` disables blocking entirely.
    fn new(ngram_size: usize) -> Self {
        Self {
            ngram_size,
            seen: HashMap::new(),
            emitted: 0,
        }
    }

    /// Tokens to block given the full generated sequence so far. Mirrors the
    /// prefix = last (n-1) tokens lookup of the original scan.
    fn blocked_tokens(&self, all_tokens: &[u32]) -> &[u32] {
        if self.ngram_size == 0 || all_tokens.len() < self.ngram_size {
            return &[];
        }
        let prefix = &all_tokens[all_tokens.len() + 1 - self.ngram_size..];
        self.seen.get(prefix).map(Vec::as_slice).unwrap_or(&[])
    }

    /// Record the newly completed (n-1)-gram windows after `all_tokens` grew by
    /// one. Must be called after each push to `all_tokens`.
    fn observe(&mut self, all_tokens: &[u32]) {
        // A successor at position `len-1` completes the window
        // all_tokens[len-1-(n-1) .. len-1] -> all_tokens[len-1].
        if self.ngram_size == 0 {
            self.emitted = all_tokens.len();
            return;
        }
        // Catch up if observe was skipped (defensive; loop calls every push).
        let win = self.ngram_size - 1;
        while self.emitted < all_tokens.len() {
            let succ_pos = self.emitted;
            if succ_pos >= win {
                let key = all_tokens[succ_pos - win..succ_pos].to_vec();
                let succ = all_tokens[succ_pos];
                let entry = self.seen.entry(key).or_default();
                if !entry.contains(&succ) {
                    entry.push(succ);
                }
            }
            self.emitted += 1;
        }
    }
}

/// Ratio of raw length to gzip-compressed length.
///
/// A hallucinated loop compresses far better than natural speech, so a high
/// ratio is the repetition signal half of the quality gate. Empty text yields
/// `0.0`.
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

/// Whether a decoded window should be discarded as hallucinated.
///
/// Requires **both** signals: low confidence (`avg_logprob` under threshold)
/// and high repetition (`compression_ratio` over threshold). Either alone is
/// common in legitimate speech — quiet audio scores low, a chant compresses
/// well — so demanding both is what keeps the gate from eating real words.
fn should_drop_for_quality_gate(
    avg_logprob: Option<f32>,
    compression_ratio: f32,
    params: &DecodingParams,
) -> bool {
    let low_logprob = avg_logprob.is_some_and(|avg| avg < params.logprob_threshold);
    let high_compression = compression_ratio > params.compression_ratio_threshold;
    low_logprob && high_compression
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

/// Dedup helpers, n-gram parity, quality gate, Silero filter, and final-pass tests.
#[cfg(test)]
mod dedup_tests {
    use super::*;

    /// Adjacent identical words collapse to a single occurrence.
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

    /// Adjacent repeated multi-word phrases collapse once, punctuation-tolerant.
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

    /// Phrase pass then word pass removes both kinds of Whisper stutter.
    #[test]
    fn test_dedup_repetitions_combined() {
        let input = "który zajmuje który zajmuje 56 GB. 56 GB. test test";
        let expected = "który zajmuje 56 GB. test";
        assert_eq!(dedup_repetitions(input), expected);
    }

    /// Reference implementation: the original full-scan n-gram block, used only
    /// to prove the incremental NgramBlocker produces an identical block set.
    fn reference_blocked(ngram_size: usize, all_tokens: &[u32]) -> Vec<u32> {
        let mut blocked = Vec::new();
        if ngram_size > 0 && all_tokens.len() >= ngram_size {
            let prefix_start = all_tokens.len() + 1 - ngram_size;
            let prefix = &all_tokens[prefix_start..];
            let search_end = all_tokens.len() - ngram_size + 1;
            for i in 0..search_end {
                if all_tokens[i..i + ngram_size - 1] == *prefix {
                    blocked.push(all_tokens[i + ngram_size - 1]);
                }
            }
        }
        blocked
    }

    /// Step through `seq` and assert incremental blocker matches full-scan blocks.
    fn assert_ngram_parity(ngram_size: usize, seq: &[u32]) {
        let mut blocker = NgramBlocker::new(ngram_size);
        let mut all: Vec<u32> = Vec::new();
        for &t in seq {
            // Lookup happens against the sequence as it stood before pushing t.
            // Compare as sets: blocking a token is idempotent, so duplicate
            // hits in the reference scan and the deduped incremental list have
            // identical effect on the logits.
            let mut inc: Vec<u32> = blocker.blocked_tokens(&all).to_vec();
            let mut refr = reference_blocked(ngram_size, &all);
            inc.sort_unstable();
            inc.dedup();
            refr.sort_unstable();
            refr.dedup();
            assert_eq!(
                inc,
                refr,
                "block-set mismatch (n={ngram_size}) at len {}: inc={inc:?} ref={refr:?}",
                all.len()
            );
            all.push(t);
            blocker.observe(&all);
        }
    }

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

    /// Incremental n-gram blocker matches full-scan blocks across sizes and edges.
    #[test]
    fn ngram_block_parity() {
        // Repetition-heavy synthetic sequence exercises the block path.
        let seq = [5u32, 6, 7, 5, 6, 7, 5, 6, 7, 8, 9, 8, 9, 8, 9, 8];
        for n in [0usize, 1, 2, 3, 5] {
            assert_ngram_parity(n, &seq);
        }
        // Sequence shorter than n -> no blocks.
        assert_ngram_parity(5, &[1, 2, 3]);
        // Empty.
        assert_ngram_parity(3, &[]);
        // Single distinct token repeated (worst case for ngram_size==1).
        assert_ngram_parity(1, &[42, 42, 42, 42]);
    }

    /// Drop requires both low avg logprob and high compression ratio together.
    #[test]
    fn quality_gate_requires_both_logprob_and_compression_signals() {
        let params = DecodingParams::default();
        assert!(!should_drop_for_quality_gate(Some(-0.2), 3.0, &params));
        assert!(!should_drop_for_quality_gate(Some(-3.0), 1.4, &params));
        assert!(should_drop_for_quality_gate(Some(-3.0), 3.0, &params));
    }

    /// Zero dropped segments keeps the original raw text (not the re-joined filter).
    #[test]
    fn silero_filter_preserves_raw_text_when_no_segments_were_dropped() {
        let segments = vec![crate::pipeline::contracts::TranscriptSegment {
            text: "close chart".to_string(),
            start_ts: 0.0,
            end_ts: 1.2,
        }];
        let raw = RawTranscript {
            text: "Close chart, and add plan.".to_string(),
            segments: segments.clone(),
            ..Default::default()
        };

        let filtered = apply_silero_filter_outcome(&raw, "close chart".to_string(), segments, 0);

        assert_eq!(filtered.text, raw.text);
        assert_eq!(filtered.segments, raw.segments);
    }

    /// Case/punctuation-only filter text still preserves raw when nothing was dropped.
    #[test]
    fn silero_filter_preserves_raw_text_when_no_drop_only_case_or_punctuation_differs() {
        let segments = vec![crate::pipeline::contracts::TranscriptSegment {
            text: "close chart and add plan".to_string(),
            start_ts: 0.0,
            end_ts: 1.2,
        }];
        let raw = RawTranscript {
            text: "Close chart, and add plan.".to_string(),
            segments: segments.clone(),
            ..Default::default()
        };

        let filtered =
            apply_silero_filter_outcome(&raw, "close chart and add plan".to_string(), segments, 0);

        assert_eq!(filtered.text, raw.text);
        assert_eq!(filtered.segments, raw.segments);
        assert!(!is_strict_text_subset(
            "close chart and add plan",
            "Close chart, and add plan."
        ));
    }

    /// When segments are dropped, filtered text and segment list replace raw.
    #[test]
    fn silero_filter_uses_filtered_text_when_segments_were_dropped() {
        let raw_segments = vec![
            crate::pipeline::contracts::TranscriptSegment {
                text: "close chart".to_string(),
                start_ts: 0.0,
                end_ts: 1.2,
            },
            crate::pipeline::contracts::TranscriptSegment {
                text: "subscribe".to_string(),
                start_ts: 1.2,
                end_ts: 2.0,
            },
        ];
        let filtered_segments = vec![raw_segments[0].clone()];
        let raw = RawTranscript {
            text: "Close chart, and subscribe.".to_string(),
            segments: raw_segments,
            ..Default::default()
        };

        let filtered = apply_silero_filter_outcome(
            &raw,
            "close chart".to_string(),
            filtered_segments.clone(),
            1,
        );

        assert_eq!(filtered.text, "close chart");
        assert_eq!(filtered.segments, filtered_segments);
    }

    /// Control-token suppression applies only at decode step zero.
    #[test]
    fn decoder_control_tokens_are_only_suppressed_before_first_token() {
        assert!(should_suppress_decoder_control_tokens(0));
        assert!(!should_suppress_decoder_control_tokens(1));
        assert!(!should_suppress_decoder_control_tokens(15));
    }

    /// Embedded lexicon cleanup reports Changed with rewrite counts.
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
) {
    if next.segments.is_empty() {
        append_with_overlap_dedup(&mut out.text, &next.text);
        // Segment truth is no longer complete. Clear it so every later chunk
        // remains on the text fallback instead of rebuilding the transcript
        // from a partial segment set and dropping this chunk.
        out.segments.clear();
        return;
    }

    if !out.text.trim().is_empty() && out.segments.is_empty() {
        append_with_overlap_dedup(&mut out.text, &next.text);
        return;
    }

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
            ..Default::default()
        };
        merge_chunk_transcripts(&mut out, next, 25.0);
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
            ..Default::default()
        };

        merge_chunk_transcripts(&mut out, next, 10.0);

        assert_eq!(out.segments.len(), 3);
        assert_eq!(
            out.text,
            "trusted earlier middle boundary bridge clean tail"
        );
        assert_eq!(out.text.matches("boundary bridge").count(), 1);
        assert!(!out.text.contains("  "));
    }

    /// Timestamp-less decoders retain the established text overlap fallback.
    #[test]
    fn seam_merge_uses_text_fallback_without_segments() {
        let mut out = RawTranscript {
            text: "one two three".into(),
            ..Default::default()
        };
        let next = RawTranscript {
            text: "two three four".into(),
            ..Default::default()
        };

        merge_chunk_transcripts(&mut out, next, 3.0);

        assert_eq!(out.text, "one two three four");
        assert!(out.segments.is_empty());
    }

    /// Once an earlier chunk lacked timestamps, keep one text-only source of
    /// truth instead of entering a mixed state that can drop the old prefix on
    /// a later segment-aware merge.
    #[test]
    fn seam_merge_keeps_mixed_sequences_on_the_text_fallback() {
        let mut out = RawTranscript {
            text: "one two three".into(),
            ..Default::default()
        };
        let next = RawTranscript {
            text: "two three four".into(),
            segments: vec![TranscriptSegment {
                text: "four".into(),
                start_ts: 3.0,
                end_ts: 4.0,
            }],
            ..Default::default()
        };

        merge_chunk_transcripts(&mut out, next, 3.0);

        assert_eq!(out.text, "one two three four");
        assert!(out.segments.is_empty());
    }

    /// A timestamp-less middle chunk invalidates the accumulated segment map;
    /// a later timestamped chunk must not rebuild text without that middle.
    #[test]
    fn seam_merge_stays_text_only_after_a_middle_chunk_loses_segments() {
        let mut out = RawTranscript {
            text: "one two".into(),
            segments: vec![TranscriptSegment {
                text: "one two".into(),
                start_ts: 0.0,
                end_ts: 2.0,
            }],
            ..Default::default()
        };
        let middle = RawTranscript {
            text: "two three".into(),
            ..Default::default()
        };
        let tail = RawTranscript {
            text: "three four".into(),
            segments: vec![TranscriptSegment {
                text: "four".into(),
                start_ts: 3.0,
                end_ts: 4.0,
            }],
            ..Default::default()
        };

        merge_chunk_transcripts(&mut out, middle, 2.0);
        merge_chunk_transcripts(&mut out, tail, 3.0);

        assert_eq!(out.text, "one two three four");
        assert!(out.segments.is_empty());
    }
}
