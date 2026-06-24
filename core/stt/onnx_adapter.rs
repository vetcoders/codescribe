//! ONNX Whisper adapter — speech-to-text via ort (ONNX Runtime).
//!
//! **STATUS: EXPERIMENTAL** — Candle q8 (MLX) remains the production default.
//! Benchmark (2026-02-11, 10 Polish files) showed ONNX +3.6–3.9pp WER vs Candle.
//! Enable via `CODESCRIBE_STT_ENGINE=onnx` for testing only.
//!
//! Uses `onnx-community/whisper-large-v3-turbo` ONNX export with optional CoreML EP.
//! Set `CODESCRIBE_ONNX_CPU_ONLY=1` to force CPU (CoreML is unstable on Tahoe beta).
//! Set `CODESCRIBE_ONNX_QUANT=int8|q4|quantized|fp16` to force a quantization variant.
//!
//! Implements the same `TranscriptionAdapter` trait as the candle-based
//! `WhisperSingletonAdapter`, making it a drop-in replacement.

use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use anyhow::{Context, Result, anyhow, ensure};
use ndarray::{Array2, Array3};
use ort::session::Session;
use ort::value::Value;
use tokenizers::Tokenizer;
use tracing::{debug, info, warn};

use candle_transformers::models::whisper::{self as whisper_audio, Config as WhisperConfig};

use crate::audio::loader as audio_loader;
use crate::pipeline::contracts::{RawTranscript, SpeechUtterance, TranscriptionAdapter};
use crate::stt::whisper::DecodingParams;
use crate::stt::whisper::timestamps::{self, TimestampRange};

// ── Constants ────────────────────────────────────────────────────────────────

/// Maximum tokens to generate per segment.
const MAX_NEW_TOKENS: usize = 448;

/// No-speech probability threshold — above this we return empty.
const NO_SPEECH_THRESHOLD: f32 = 0.6;

/// Number of mel bins for whisper-large-v3-turbo.
const NUM_MEL_BINS: usize = 128;

/// ONNX encoder expects exactly 3000 mel frames (30s * 16kHz / hop_length=160).
const ONNX_N_FRAMES: usize = 3000;

/// Minimum generated tokens before allowing EOT (prevent premature stop).
const MIN_TOKENS_BEFORE_EOT: usize = 16;

/// No-repeat n-gram size (matching candle engine).
const NO_REPEAT_NGRAM_SIZE: usize = 5;

/// Blank tokens to suppress early (matching candle engine).
const SUPPRESS_BLANK_TOKENS: [usize; 2] = [220, 50256];

/// Warn once when users set an initial prompt env override while ONNX is active.
/// ONNX path currently ignores this feature (experimental parity gap vs candle).
fn warn_if_initial_prompt_ignored() {
    static WARNED: OnceLock<()> = OnceLock::new();

    for key in [
        "CODESCRIBE_WHISPER_INITIAL_PROMPT",
        "WHISPER_INITIAL_PROMPT",
    ] {
        if let Ok(prompt) = std::env::var(key)
            && !prompt.trim().is_empty()
        {
            WARNED.get_or_init(|| {
                warn!(
                    "{} is set, but ONNX adapter does not support initial_prompt; value is ignored",
                    key
                );
            });
            break;
        }
    }
}

// ── Resolved token IDs (from tokenizer at init) ─────────────────────────────

/// Token IDs resolved dynamically from tokenizer.json.
struct ResolvedTokens {
    sot: u32,              // <|startoftranscript|>
    eot: u32,              // <|endoftext|>
    transcribe: u32,       // <|transcribe|>
    nospeech: Option<u32>, // <|nospeech|>
}

impl ResolvedTokens {
    fn from_tokenizer(tokenizer: &Tokenizer) -> Result<Self> {
        let sot = tokenizer
            .token_to_id("<|startoftranscript|>")
            .context("Tokenizer missing <|startoftranscript|>")?;
        let eot = tokenizer
            .token_to_id("<|endoftext|>")
            .context("Tokenizer missing <|endoftext|>")?;
        let transcribe = tokenizer
            .token_to_id("<|transcribe|>")
            .context("Tokenizer missing <|transcribe|>")?;
        let nospeech = tokenizer.token_to_id("<|nospeech|>");
        Ok(Self {
            sot,
            eot,
            transcribe,
            nospeech,
        })
    }

    /// Resolve language token: <|pl|>, <|en|>, etc.
    fn language_token(&self, tokenizer: &Tokenizer, lang: &str) -> Option<u32> {
        let tag = format!("<|{}|>", lang.to_lowercase());
        tokenizer.token_to_id(&tag)
    }
}

// ── OnnxEngine (internal, holds mutable sessions) ────────────────────────────

/// Internal engine holding ort sessions, tokenizer, and mel filters.
///
/// `Session::run()` requires `&mut self`, so this struct lives behind a Mutex
/// in the global singleton. The public `OnnxWhisperAdapter` is a zero-sized
/// type that locks the Mutex to get `&mut OnnxEngine` access.
struct OnnxEngine {
    encoder: Session,
    decoder: Session,
    tokenizer: Tokenizer,
    tokens: ResolvedTokens,
    ts_range: Option<TimestampRange>,
    decoding_params: DecodingParams,
    mel_filters: Vec<f32>,
    whisper_config: WhisperConfig,
}

// ── Singleton ────────────────────────────────────────────────────────────────

static ENGINE: OnceLock<Mutex<OnnxEngine>> = OnceLock::new();

/// Initialize the ONNX Whisper engine singleton.
///
/// Thread-safe: `Once` ensures exactly one caller builds the engine
/// while others block. If init fails, the error is cached and returned
/// on subsequent calls (no retry — model path won't change mid-run).
pub fn init() -> Result<()> {
    static INIT_ONCE: std::sync::Once = std::sync::Once::new();
    static INIT_ERR: OnceLock<String> = OnceLock::new();

    INIT_ONCE.call_once(
        || match resolve_model_path().and_then(|p| OnnxEngine::new(&p)) {
            Ok(engine) => {
                let _ = ENGINE.set(Mutex::new(engine));
            }
            Err(e) => {
                let _ = INIT_ERR.set(format!("{:#}", e));
            }
        },
    );

    if let Some(err) = INIT_ERR.get() {
        anyhow::bail!("ONNX init failed: {}", err);
    }
    Ok(())
}

/// Transcribe long audio via the ONNX engine with segment metadata.
pub(crate) fn transcribe_long_with_segments(
    audio: &[f32],
    sample_rate: u32,
    language: Option<&str>,
) -> Result<RawTranscript> {
    init()?;
    let engine = ENGINE.get().context("ONNX engine not initialized")?;
    let mut guard = engine
        .lock()
        .map_err(|e| anyhow!("ONNX lock error: {}", e))?;
    guard.transcribe_long_raw(audio, sample_rate, language)
}

/// Transcribe a single chunk via the ONNX engine (blocking lock).
// FORGOTTEN-GEM(vc-prune 2026-06-10): parked sync transcription contract —
// see core/stt/mod.rs::candle_transcribe_chunk for the cluster rationale.
#[allow(dead_code)]
pub(crate) fn transcribe_chunk(
    audio: &[f32],
    sample_rate: u32,
    language: Option<&str>,
) -> Result<String> {
    init()?;
    let engine = ENGINE.get().context("ONNX engine not initialized")?;
    let mut guard = engine
        .lock()
        .map_err(|e| anyhow!("ONNX lock error: {}", e))?;
    guard.transcribe_internal(audio, sample_rate, language)
}

/// Transcribe long audio via the ONNX engine (try_lock) with segment metadata.
#[allow(dead_code)]
pub(crate) fn try_transcribe_long_with_segments(
    audio: &[f32],
    sample_rate: u32,
    language: Option<&str>,
) -> Result<RawTranscript> {
    init()?;
    let engine = ENGINE.get().context("ONNX engine not initialized")?;
    let mut guard = engine
        .try_lock()
        .map_err(|_| anyhow!("ONNX engine busy, skipping correction"))?;
    guard.transcribe_long_raw(audio, sample_rate, language)
}

/// Resolve ONNX model path from env or HF cache.
fn resolve_model_path() -> Result<PathBuf> {
    // 1. Explicit env override
    if let Ok(path) = std::env::var("CODESCRIBE_ONNX_MODEL_PATH") {
        let p = PathBuf::from(path.trim());
        if p.exists() {
            return Ok(p);
        }
        warn!(
            "CODESCRIBE_ONNX_MODEL_PATH={} does not exist, trying HF cache",
            p.display()
        );
    }

    // 2. HF cache — look for snapshot containing onnx/ subdirectory
    let repo = std::env::var("CODESCRIBE_ONNX_REPO")
        .unwrap_or_else(|_| "onnx-community/whisper-large-v3-turbo".to_string());
    crate::hf_cache::find_snapshot(&repo, &["onnx"]).context(
        "ONNX model not found in HF cache. Run: hf download onnx-community/whisper-large-v3-turbo",
    )
}

// ── OnnxEngine impl ─────────────────────────────────────────────────────────

impl OnnxEngine {
    /// Create a new ONNX engine from a model directory.
    ///
    /// Expected files:
    /// - `onnx/encoder_model_q4.onnx` (or other quantization variant)
    /// - `onnx/decoder_model_merged_q4.onnx` (merged = handles both first-token and with-cache)
    /// - `tokenizer.json`
    /// - `config.json`
    fn new(model_path: &Path) -> Result<Self> {
        info!("Loading ONNX Whisper model from: {}", model_path.display());

        // Detect best available encoder variant (prefer q4 > q4f16 > fp16 > fp32)
        let onnx_dir = model_path.join("onnx");
        let encoder_path = find_best_variant(&onnx_dir, "encoder_model")?;
        let decoder_path = find_best_variant(&onnx_dir, "decoder_model_merged")?;

        info!(
            "  Encoder: {}",
            encoder_path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
        );
        info!(
            "  Decoder: {}",
            decoder_path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
        );

        // Load ONNX sessions with CoreML EP
        let encoder = create_session(&encoder_path).context("Failed to load ONNX encoder")?;
        let decoder = create_session(&decoder_path).context("Failed to load ONNX decoder")?;

        // Load tokenizer via safe_path (path traversal protection).
        let tokenizer_path = model_path.join("tokenizer.json");
        let tokenizer_json = crate::safe_path::safe_read_to_string(&tokenizer_path)
            .context("Failed to read tokenizer.json")?;
        let tokenizer = Tokenizer::from_bytes(tokenizer_json.as_bytes())
            .map_err(|e| anyhow!("Failed to load tokenizer: {}", e))?;

        // Resolve special token IDs from tokenizer
        let tokens = ResolvedTokens::from_tokenizer(&tokenizer)?;
        let ts_range = TimestampRange::from_tokenizer(&tokenizer);
        let decoding_params = DecodingParams::default();

        // Load mel filters from mel_filters.npz if available, otherwise compute
        let mel_filters = load_or_compute_mel_filters(model_path)?;

        // Read config.json via safe_path (path traversal protection)
        let config_path = model_path.join("config.json");
        let config_file =
            crate::safe_path::safe_open(&config_path).context("Failed to open config.json")?;
        let config_json: serde_json::Value = serde_json::from_reader(config_file)?;
        let whisper_config = build_whisper_config(&config_json);

        let ep = if std::env::var("CODESCRIBE_ONNX_CPU_ONLY").as_deref() == Ok("1") {
            "CPU"
        } else {
            "CoreML"
        };
        info!("ONNX Whisper engine initialized ({} EP)", ep);

        Ok(Self {
            encoder,
            decoder,
            tokenizer,
            tokens,
            ts_range,
            decoding_params,
            mel_filters,
            whisper_config,
        })
    }

    /// Transcribe audio samples using ONNX encoder + decoder.
    ///
    /// Uses full-sequence decoding (no KV cache) for correctness.
    /// The merged decoder model receives all accumulated tokens each step
    /// with `use_cache_branch=false`, so no past_key_values management needed.
    /// This is O(n²) in token count but correct — KV cache optimization can
    /// come later after we verify quality.
    #[allow(dead_code)]
    fn transcribe_internal(
        &mut self,
        samples: &[f32],
        sample_rate: u32,
        language: Option<&str>,
    ) -> Result<String> {
        Ok(self
            .transcribe_internal_raw(samples, sample_rate, language)?
            .text)
    }

    #[allow(dead_code)]
    fn transcribe_internal_raw(
        &mut self,
        samples: &[f32],
        sample_rate: u32,
        language: Option<&str>,
    ) -> Result<RawTranscript> {
        ensure!(!samples.is_empty(), "audio is empty");
        let samples_16k = audio_loader::resample_to_16k(samples, sample_rate);
        self.transcribe_internal_16k_raw(&samples_16k, language)
    }

    fn transcribe_internal_16k_raw(
        &mut self,
        samples_16k: &[f32],
        language: Option<&str>,
    ) -> Result<RawTranscript> {
        ensure!(!samples_16k.is_empty(), "audio is empty");
        warn_if_initial_prompt_ignored();

        // 1. Compute mel spectrogram
        let mel =
            whisper_audio::audio::pcm_to_mel(&self.whisper_config, samples_16k, &self.mel_filters);
        let n_mels = self.whisper_config.num_mel_bins;
        let n_frames = mel.len() / n_mels;

        // 2. Pad or trim mel to exactly ONNX_N_FRAMES (3000).
        //    pcm_to_mel pads to multiples of 1500+1500, so variable-length chunks
        //    produce 4500, 6000, etc. ONNX encoder input is fixed [1, 128, 3000].
        let mel = if n_frames == ONNX_N_FRAMES {
            mel
        } else if n_frames > ONNX_N_FRAMES {
            // Trim: take first 3000 frames from each mel bin row
            let mut trimmed = Vec::with_capacity(n_mels * ONNX_N_FRAMES);
            for bin in 0..n_mels {
                let start = bin * n_frames;
                trimmed.extend_from_slice(&mel[start..start + ONNX_N_FRAMES]);
            }
            trimmed
        } else {
            // Pad: zero-fill remaining frames in each mel bin row
            let mut padded = vec![0.0f32; n_mels * ONNX_N_FRAMES];
            for bin in 0..n_mels {
                let src_start = bin * n_frames;
                let dst_start = bin * ONNX_N_FRAMES;
                padded[dst_start..dst_start + n_frames]
                    .copy_from_slice(&mel[src_start..src_start + n_frames]);
            }
            padded
        };

        // 3. Shape mel as [1, num_mel_bins, ONNX_N_FRAMES] for encoder
        let mel_array = Array3::from_shape_vec([1, n_mels, ONNX_N_FRAMES], mel)
            .context("Failed to reshape mel spectrogram")?;

        let mel_value = Value::from_array(mel_array)?;

        // 4. Run encoder
        let encoder_outputs = self.encoder.run(ort::inputs![mel_value])?;
        let encoder_hidden = encoder_outputs
            .get("last_hidden_state")
            .context("Encoder missing 'last_hidden_state'")?;

        // 5. Prepare decoder initial tokens (resolved from tokenizer, not hardcoded)
        let mut initial_tokens: Vec<i64> = vec![self.tokens.sot as i64];
        if let Some(lang) = language
            && let Some(lang_tok) = self.tokens.language_token(&self.tokenizer, lang)
        {
            initial_tokens.push(lang_tok as i64);
        }
        initial_tokens.push(self.tokens.transcribe as i64);
        let timestamps_enabled = self.decoding_params.emit_timestamps && self.ts_range.is_some();
        if !timestamps_enabled && let Some(no_ts) = self.tokenizer.token_to_id("<|notimestamps|>") {
            initial_tokens.push(no_ts as i64);
        }

        // 6. Greedy decoder loop — always full-sequence (no KV cache)
        //    This matches candle engine's approach: full token sequence each step,
        //    with suppress_blank, n-gram blocking, and proper softmax for no-speech.
        let mut all_tokens: Vec<u32> = Vec::new(); // generated tokens (after initial)
        let eot = self.tokens.eot;
        let nospeech = self.tokens.nospeech;

        // Cap total sequence at model's positional embedding size
        let max_seq = self.whisper_config.max_target_positions;
        let max_gen = max_seq
            .saturating_sub(initial_tokens.len())
            .min(MAX_NEW_TOKENS);

        for step in 0..max_gen {
            // Build full input: initial_tokens + all generated tokens so far
            let mut input_seq = initial_tokens.clone();
            input_seq.extend(all_tokens.iter().map(|&t| t as i64));
            let seq_len = input_seq.len();

            let input_ids = Array2::from_shape_vec([1, seq_len], input_seq)?;
            let input_value = Value::from_array(input_ids)?;

            // Always use_cache_branch=false — no KV cache, full recompute each step
            let cache_flag = Value::from_array(ndarray::Array1::from_vec(vec![false]))?;

            let decoder_outputs = self.decoder.run(ort::inputs![
                "input_ids" => input_value,
                "encoder_hidden_states" => encoder_hidden,
                "use_cache_branch" => cache_flag,
            ])?;

            // Extract logits from last position
            let logits_value = decoder_outputs
                .get("logits")
                .context("Decoder missing 'logits'")?;
            let (logits_shape, logits_data) = logits_value.try_extract_tensor::<f32>()?;

            // Guard against malformed model output: shape values come from the model
            // and must not be trusted to panic-index. See `last_position_logits`.
            let mut logits_vec: Vec<f32> = last_position_logits(logits_shape, logits_data)?;

            // No-speech check on first step (proper softmax, matching candle engine)
            if step == 0
                && let Some(nos) = nospeech
            {
                let nos_idx = nos as usize;
                if nos_idx < logits_vec.len() {
                    let max_val = logits_vec.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
                    let exp_sum: f32 = logits_vec.iter().map(|&x| (x - max_val).exp()).sum();
                    let nos_prob = (logits_vec[nos_idx] - max_val).exp() / exp_sum;
                    debug!(
                        "No-speech softmax: nos_prob={:.4}, threshold={}, logit={:.2}, max={:.2}",
                        nos_prob, NO_SPEECH_THRESHOLD, logits_vec[nos_idx], max_val
                    );
                    if nos_prob > NO_SPEECH_THRESHOLD {
                        debug!("No speech detected (prob={:.3})", nos_prob);
                        return Ok(RawTranscript::default());
                    }
                }
            }

            // Suppress blank tokens early (matching candle engine)
            if all_tokens.len() < 4 {
                for &tok in &SUPPRESS_BLANK_TOKENS {
                    if tok < logits_vec.len() {
                        logits_vec[tok] = f32::NEG_INFINITY;
                    }
                }
            }

            // N-gram blocking (matching candle engine, no_repeat_ngram_size=5)
            if NO_REPEAT_NGRAM_SIZE > 0 && all_tokens.len() >= NO_REPEAT_NGRAM_SIZE {
                let prefix_start = all_tokens.len() + 1 - NO_REPEAT_NGRAM_SIZE;
                let prefix = &all_tokens[prefix_start..];
                let search_end = all_tokens.len() - NO_REPEAT_NGRAM_SIZE + 1;
                for i in 0..search_end {
                    if all_tokens[i..i + NO_REPEAT_NGRAM_SIZE - 1] == *prefix {
                        let blocked = all_tokens[i + NO_REPEAT_NGRAM_SIZE - 1] as usize;
                        if blocked < logits_vec.len() {
                            logits_vec[blocked] = f32::NEG_INFINITY;
                        }
                    }
                }
            }

            // Suppress early EOT (prevent premature stop, matching candle engine)
            if all_tokens.len() < MIN_TOKENS_BEFORE_EOT {
                let eot_idx = eot as usize;
                if eot_idx < logits_vec.len() {
                    logits_vec[eot_idx] = f32::NEG_INFINITY;
                }
            }

            // Greedy: pick highest logit
            let next_token = logits_vec
                .iter()
                .enumerate()
                .max_by(|(_, a), (_, b)| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal))
                .map(|(idx, _)| idx as u32)
                .unwrap_or(eot);

            if next_token == eot {
                break;
            }

            all_tokens.push(next_token);
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
                    .map_err(|e| anyhow!("Tokenizer decode failed: {}", e))?,
                Vec::new(),
            )
        };

        Ok(RawTranscript {
            text: text.trim().to_string(),
            segments,
            ..Default::default()
        })
    }

    fn transcribe_long_raw(
        &mut self,
        samples: &[f32],
        sample_rate: u32,
        language: Option<&str>,
    ) -> Result<RawTranscript> {
        let samples_16k = audio_loader::resample_to_16k(samples, sample_rate);

        // 30s = encoder native window, 5s overlap for context continuity
        let chunk_samples = 16000 * 30; // 30 seconds = ONNX_N_FRAMES * HOP_LENGTH
        let overlap_samples = 16000 * 5; // 5 seconds overlap
        let step = chunk_samples - overlap_samples;

        if samples_16k.len() <= chunk_samples {
            // Short audio — single pass (pad-or-trim handles the rest)
            let mut transcript = self.transcribe_internal_16k_raw(&samples_16k, language)?;
            transcript.text = crate::stt::whisper::dedup_repetitions(&transcript.text);
            return Ok(transcript);
        }

        let mut out = String::new();
        let mut all_segments = Vec::new();
        let mut offset = 0usize;

        while offset < samples_16k.len() {
            let end = (offset + chunk_samples).min(samples_16k.len());
            let chunk = &samples_16k[offset..end];

            if chunk.len() < 1600 {
                // Less than 0.1s — skip
                break;
            }

            let transcript = self.transcribe_internal_16k_raw(chunk, language)?;
            if !transcript.text.is_empty() {
                crate::stt::whisper::append_with_overlap_dedup(&mut out, &transcript.text);
            }
            if !transcript.segments.is_empty() {
                let offset_sec = offset as f32 / 16_000.0;
                all_segments.extend(transcript.segments.into_iter().map(|mut s| {
                    s.start_ts += offset_sec;
                    s.end_ts += offset_sec;
                    s
                }));
            }

            offset += step;
        }

        let trimmed = out.trim();
        Ok(RawTranscript {
            text: crate::stt::whisper::dedup_repetitions(trimmed),
            segments: all_segments,
            ..Default::default()
        })
    }
}

// ── OnnxWhisperAdapter (zero-sized public type) ──────────────────────────────

/// Zero-sized adapter wrapping the global ONNX engine singleton.
///
/// Mirrors `WhisperSingletonAdapter` pattern: the struct itself is trivially
/// Send+Sync, and `transcribe()` locks the internal Mutex to get `&mut` access.
pub struct OnnxWhisperAdapter;

impl OnnxWhisperAdapter {
    pub fn new() -> Self {
        Self
    }
}

impl Default for OnnxWhisperAdapter {
    fn default() -> Self {
        Self
    }
}

impl TranscriptionAdapter for OnnxWhisperAdapter {
    fn transcribe(
        &self,
        utterance: &SpeechUtterance,
        language: Option<&str>,
    ) -> Result<RawTranscript> {
        let engine = ENGINE
            .get()
            .context("ONNX Whisper engine not initialized. Call onnx_adapter::init() first.")?;
        let mut guard = engine
            .lock()
            .map_err(|e| anyhow!("ONNX engine mutex poisoned: {}", e))?;

        guard.transcribe_long_raw(&utterance.samples, utterance.sample_rate, language)
    }
}

// ── Helper functions ────────────────────────────────────────────────────────

/// Create an ort Session with CoreML EP preference (or CPU-only if forced).
///
/// Set `CODESCRIBE_ONNX_CPU_ONLY=1` to skip CoreML and use CPU only.
fn create_session(model_path: &Path) -> Result<Session> {
    let cpu_only = std::env::var("CODESCRIBE_ONNX_CPU_ONLY").as_deref() == Ok("1");

    let builder = Session::builder()?.with_intra_threads(2)?;

    let session = if cpu_only {
        info!("ONNX: CPU-only mode (CODESCRIBE_ONNX_CPU_ONLY=1)");
        builder
    } else {
        use ort::execution_providers::CoreMLExecutionProvider;
        builder.with_execution_providers([CoreMLExecutionProvider::default().build()])?
    }
    .commit_from_file(model_path)
    .with_context(|| format!("Failed to load ONNX model: {}", model_path.display()))?;

    Ok(session)
}

/// Find the best available quantization variant for a model prefix.
///
/// Override with `CODESCRIBE_ONNX_QUANT=int8` (or q4, fp16, etc.)
/// to force a specific variant instead of auto-detection.
fn find_best_variant(onnx_dir: &Path, prefix: &str) -> Result<PathBuf> {
    // Allow explicit variant override via env
    if let Ok(forced) = std::env::var("CODESCRIBE_ONNX_QUANT") {
        let forced = forced.trim().to_lowercase();
        let filename = if forced == "fp32" || forced == "default" {
            format!("{}.onnx", prefix)
        } else {
            format!("{}_{}.onnx", prefix, forced)
        };
        let path = onnx_dir.join(&filename);
        if path.exists() {
            info!("Using forced ONNX variant: {}", filename);
            return Ok(path);
        }
        warn!(
            "Forced ONNX variant '{}' not found at {}, falling back to auto-detect",
            filename,
            path.display()
        );
    }

    // Auto-detect: preference order q4 > q4f16 > int8 > fp16 > fp32
    let variants = [
        format!("{}_q4.onnx", prefix),
        format!("{}_q4f16.onnx", prefix),
        format!("{}_int8.onnx", prefix),
        format!("{}_fp16.onnx", prefix),
        format!("{}.onnx", prefix),
    ];

    for variant in &variants {
        let path = onnx_dir.join(variant);
        if path.exists() {
            return Ok(path);
        }
    }

    // Check if fp32 has external data format
    let base = onnx_dir.join(format!("{}.onnx", prefix));
    let data = onnx_dir.join(format!("{}.onnx_data", prefix));
    if base.exists() && data.exists() {
        return Ok(base);
    }

    anyhow::bail!(
        "No ONNX model found for {} in {}. Available variants: {:?}",
        prefix,
        onnx_dir.display(),
        variants
    )
}

/// Load mel filters from mel_filters.npz or compute standard filterbank.
fn load_or_compute_mel_filters(model_path: &Path) -> Result<Vec<f32>> {
    let npz_path = model_path.join("mel_filters.npz");
    if npz_path.exists() {
        info!("Loading mel filters from mel_filters.npz");
        let file = crate::safe_path::safe_open(&npz_path)?;
        return load_mel_filters_from_reader(file, NUM_MEL_BINS);
    }

    // Compute standard mel filterbank from parameters
    info!(
        "Computing {} mel filters (n_fft=400, sr=16000)",
        NUM_MEL_BINS
    );
    Ok(compute_mel_filterbank(NUM_MEL_BINS, 400, 16000))
}

/// Build a minimal WhisperConfig for pcm_to_mel.
fn build_whisper_config(config_json: &serde_json::Value) -> WhisperConfig {
    let num_mel_bins = config_json["num_mel_bins"].as_u64().unwrap_or(128) as usize;
    let max_source_positions =
        config_json["max_source_positions"].as_u64().unwrap_or(1500) as usize;
    let d_model = config_json["d_model"].as_u64().unwrap_or(1280) as usize;
    let encoder_attention_heads = config_json["encoder_attention_heads"]
        .as_u64()
        .unwrap_or(20) as usize;
    let encoder_layers = config_json["encoder_layers"].as_u64().unwrap_or(32) as usize;
    let decoder_attention_heads = config_json["decoder_attention_heads"]
        .as_u64()
        .unwrap_or(20) as usize;
    let decoder_layers = config_json["decoder_layers"].as_u64().unwrap_or(4) as usize;
    let vocab_size = config_json["vocab_size"].as_u64().unwrap_or(51866) as usize;
    let max_target_positions = config_json["max_target_positions"].as_u64().unwrap_or(448) as usize;

    WhisperConfig {
        num_mel_bins,
        max_source_positions,
        d_model,
        encoder_attention_heads,
        encoder_layers,
        decoder_attention_heads,
        decoder_layers,
        vocab_size,
        max_target_positions,
        suppress_tokens: Vec::new(),
    }
}

// ── Decoder logits extraction ────────────────────────────────────────────────

/// Extract the logits row for the last sequence position from a decoder output
/// tensor, validating the shape so malformed model output yields a readable
/// `Err` instead of a panic.
///
/// Expected layout: `[batch, seq_len, vocab_size]` (rank >= 2; the last axis is
/// vocab, the second axis is the sequence length). Shape values originate from
/// the ONNX model and must not be trusted to panic-index.
fn last_position_logits(shape: &[i64], data: &[f32]) -> Result<Vec<f32>> {
    let raw_vocab_size = *shape
        .last()
        .ok_or_else(|| anyhow!("decoder logits tensor has empty shape"))?;
    ensure!(
        shape.len() >= 2,
        "decoder logits tensor has rank {} (expected >= 2): {:?}",
        shape.len(),
        shape
    );
    ensure!(
        raw_vocab_size > 0,
        "decoder logits vocab dimension must be positive, got {}",
        raw_vocab_size
    );
    let vocab_size = raw_vocab_size as usize;

    let raw_out_seq_len = shape[1];
    ensure!(
        raw_out_seq_len > 0,
        "decoder logits seq_len must be positive, got {}",
        raw_out_seq_len
    );
    let out_seq_len = raw_out_seq_len as usize;
    let last_index = out_seq_len - 1;
    let last_pos_offset = last_index
        .checked_mul(vocab_size)
        .ok_or_else(|| anyhow!("decoder logits offset overflow"))?;
    let range_end = last_pos_offset
        .checked_add(vocab_size)
        .ok_or_else(|| anyhow!("decoder logits range overflow"))?;
    let range = last_pos_offset..range_end;
    let last_logits = data.get(range.clone()).ok_or_else(|| {
        anyhow!(
            "decoder logits slice {:?} out of bounds (data len {}, shape {:?})",
            range,
            data.len(),
            shape
        )
    })?;
    Ok(last_logits.to_vec())
}

// ── Mel filterbank computation ───────────────────────────────────────────────

/// Compute standard mel filterbank matrix.
///
/// Returns flat Vec of shape [n_mels, n_fft/2 + 1].
fn compute_mel_filterbank(n_mels: usize, n_fft: usize, sample_rate: u32) -> Vec<f32> {
    let n_freqs = n_fft / 2 + 1;
    let sr = sample_rate as f64;

    // Mel scale conversion
    let hz_to_mel = |hz: f64| -> f64 { 2595.0 * (1.0 + hz / 700.0).log10() };
    let mel_to_hz = |mel: f64| -> f64 { 700.0 * (10.0_f64.powf(mel / 2595.0) - 1.0) };

    let mel_min = hz_to_mel(0.0);
    let mel_max = hz_to_mel(sr / 2.0);

    // n_mels + 2 equally spaced points in mel scale
    let mel_points: Vec<f64> = (0..=n_mels + 1)
        .map(|i| mel_min + (mel_max - mel_min) * i as f64 / (n_mels + 1) as f64)
        .collect();
    let hz_points: Vec<f64> = mel_points.iter().map(|&m| mel_to_hz(m)).collect();

    // Convert to FFT bin indices
    let bin_points: Vec<f64> = hz_points.iter().map(|&hz| hz * n_fft as f64 / sr).collect();

    let mut filters = vec![0.0f32; n_mels * n_freqs];

    for i in 0..n_mels {
        let left = bin_points[i];
        let center = bin_points[i + 1];
        let right = bin_points[i + 2];

        for j in 0..n_freqs {
            let freq = j as f64;
            let weight = if freq >= left && freq <= center {
                (freq - left) / (center - left + 1e-10)
            } else if freq > center && freq <= right {
                (right - freq) / (right - center + 1e-10)
            } else {
                0.0
            };
            filters[i * n_freqs + j] = weight as f32;
        }
    }

    // Slaney-style normalization
    for i in 0..n_mels {
        let left_hz = hz_points[i];
        let right_hz = hz_points[i + 2];
        let enorm = 2.0 / (right_hz - left_hz + 1e-10);
        for j in 0..n_freqs {
            filters[i * n_freqs + j] *= enorm as f32;
        }
    }

    filters
}

/// Load mel filters from npz file (reused from whisper engine).
fn load_mel_filters_from_reader<R: std::io::Read + std::io::Seek>(
    reader: R,
    n_mels: usize,
) -> Result<Vec<f32>> {
    use ndarray::Array2;
    use ndarray_npy::ReadNpyExt;

    let mut zip = zip::ZipArchive::new(reader)?;
    let key = format!("mel_{}", n_mels);
    let candidates = [format!("{}.npy", key), key.clone()];

    let mut buf = Vec::new();
    let mut found = false;
    for name in candidates {
        if let Ok(mut f) = zip.by_name(&name) {
            std::io::Read::read_to_end(&mut f, &mut buf)?;
            found = true;
            break;
        }
    }

    if !found {
        anyhow::bail!("mel filter {} not found in npz", n_mels);
    }

    let cursor = std::io::Cursor::new(buf);
    let array: Array2<f32> =
        <Array2<f32> as ReadNpyExt>::read_npy(cursor).context("Failed to parse mel filters npy")?;
    let (data, _) = array.into_raw_vec_and_offset();
    Ok(data)
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mel_filterbank_shape() {
        let filters = compute_mel_filterbank(128, 400, 16000);
        assert_eq!(filters.len(), 128 * 201); // 128 mels × (400/2 + 1) freqs
    }

    #[test]
    fn mel_filterbank_nonnegative() {
        let filters = compute_mel_filterbank(128, 400, 16000);
        assert!(filters.iter().all(|&f| f >= 0.0));
    }

    #[test]
    fn mel_filterbank_not_all_zero() {
        let filters = compute_mel_filterbank(128, 400, 16000);
        assert!(filters.iter().any(|&f| f > 0.0));
    }

    #[test]
    fn softmax_no_speech_is_valid_probability() {
        // Verify the softmax computation produces valid probabilities [0,1]
        let logits = vec![1.0f32, 2.0, 3.0, -1.0, 0.5];
        let max_val = logits.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
        let exp_sum: f32 = logits.iter().map(|&x| (x - max_val).exp()).sum();

        for &logit in &logits {
            let prob = (logit - max_val).exp() / exp_sum;
            assert!((0.0..=1.0).contains(&prob), "prob={} out of [0,1]", prob);
        }

        // Sum of all probs should be ~1.0
        let total: f32 = logits.iter().map(|&x| (x - max_val).exp() / exp_sum).sum();
        assert!(
            (total - 1.0).abs() < 1e-5,
            "softmax sum={}, expected 1.0",
            total
        );
    }

    #[test]
    fn last_position_logits_extracts_last_row() {
        // shape [1, 2, 3]: two positions, vocab=3. Last row is [4,5,6].
        let data = vec![1.0f32, 2.0, 3.0, 4.0, 5.0, 6.0];
        let out = last_position_logits(&[1, 2, 3], &data).expect("valid shape");
        assert_eq!(out, vec![4.0, 5.0, 6.0]);
    }

    #[test]
    fn onnx_decode_malformed_shape() {
        // Rank 1 (len < 2) → Err, not panic.
        assert!(last_position_logits(&[5], &[0.0f32; 5]).is_err());
        // Empty shape → Err.
        assert!(last_position_logits(&[], &[0.0f32; 4]).is_err());
        // seq_len == 0 → Err (no underflow on (out_seq_len - 1)).
        assert!(last_position_logits(&[1, 0, 3], &[0.0f32; 0]).is_err());
        // seq_len < 0 → Err, not usize wraparound.
        assert!(last_position_logits(&[1, -1, 3], &[0.0f32; 0]).is_err());
        // vocab == 0 → Err.
        assert!(last_position_logits(&[1, 2, 0], &[0.0f32; 4]).is_err());
        // vocab < 0 → Err, not usize wraparound.
        assert!(last_position_logits(&[1, 2, -1], &[0.0f32; 4]).is_err());
        // Shape claims more than data holds → Err (slice OOB), not panic.
        assert!(last_position_logits(&[1, 2, 3], &[0.0f32; 2]).is_err());
    }

    /// Verify adapter satisfies Send + Sync (required by TranscriptionAdapter).
    #[test]
    fn adapter_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<OnnxWhisperAdapter>();
    }

    /// Integration test: load ONNX model from HF cache and verify init.
    /// Requires `hf download onnx-community/whisper-large-v3-turbo` to be run first.
    #[test]
    #[ignore] // Run with: cargo test -p codescribe-core onnx_init_from_hf_cache -- --ignored
    fn onnx_init_from_hf_cache() {
        let result = init();
        assert!(result.is_ok(), "ONNX init failed: {:?}", result.err());

        // Verify we can get the engine lock
        let engine_lock = ENGINE.get().expect("ENGINE not initialized after init()");
        let guard = engine_lock.lock().unwrap();
        assert!(
            guard.tokens.eot > 0,
            "EOT token should be resolved to a non-zero ID"
        );
        assert!(
            guard.tokens.sot > 0,
            "SOT token should be resolved to a non-zero ID"
        );
        assert!(
            !guard.mel_filters.is_empty(),
            "Mel filters should be computed"
        );
    }

    /// End-to-end smoke test: transcribe synthetic noise via ONNX adapter.
    /// Low-amplitude random noise simulates real silence (Whisper expects analog noise, not digital zeros).
    #[test]
    #[ignore] // Requires ONNX model in HF cache
    fn onnx_transcribe_noise_silence() {
        use crate::pipeline::contracts::{SpeechUtterance, TranscriptionAdapter};
        use rand::Rng;

        init().expect("ONNX init failed");
        let adapter = OnnxWhisperAdapter::new();

        // 2 seconds of low-amplitude random noise at 16kHz (simulates real silence)
        let mut rng = rand::thread_rng();
        let noise: Vec<f32> = (0..32000).map(|_| rng.gen_range(-0.001..0.001)).collect();
        let utterance = SpeechUtterance {
            samples: noise,
            sample_rate: 16000,
            start_ts: 0.0,
            end_ts: 2.0,
        };

        let result = adapter.transcribe(&utterance, Some("pl"));
        assert!(result.is_ok(), "Transcribe failed: {:?}", result.err());

        let transcript = result.unwrap();
        eprintln!("Noise-silence transcript: '{}'", transcript.text);
        // With realistic noise, no-speech should trigger or produce minimal hallucination
        // Acceptable: empty, or very short hallucination (Whisper quirk)
        assert!(
            transcript.text.len() < 100,
            "Noise-silence produced too much text: '{}'",
            transcript.text
        );
    }
}
