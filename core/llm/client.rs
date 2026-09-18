//! HTTP client for cloud STT providers.
//!
//! Features:
//! - Explicit external endpoint (no backend discovery)
//! - Multipart file upload for transcription
//! - Retry logic with exponential backoff
//! - Proper error handling and logging

use anyhow::{Context, Result};
use futures_util::StreamExt;
use reqwest::Client;
use reqwest::multipart::{Form, Part};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::{Duration, Instant};
use tokio::fs::File;
use tokio::io::AsyncReadExt;
use tracing::{debug, error, info, warn};

use crate::pipeline::contracts::{TranscriptionConfidenceFlag, TranscriptionSource};

/// Canonicalize path before async file operations (defense-in-depth).
/// Uses sync std::fs::canonicalize which is fast, then async open.
fn canonicalize_path(path: &Path) -> Result<PathBuf> {
    path.canonicalize()
        .with_context(|| format!("Failed to resolve path: {}", path.display()))
}

/// Maximum retry attempts for transcription requests
const TRANSCRIPTION_MAX_RETRIES: u32 = 3;

/// Base delay between retry attempts (multiplied by attempt number)
const TRANSCRIPTION_RETRY_DELAY_MS: u64 = 500;

// Note: Retry constants and format_text moved to ai_formatting.rs module

/// Transcription response structure
#[derive(Debug, Deserialize)]
struct TranscribeResponse {
    text: String,
}

/// Typed cloud-STT verdict emitted at the client boundary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CloudTranscriptionVerdict {
    pub text: String,
    pub source: TranscriptionSource,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub confidence_flags: Vec<TranscriptionConfidenceFlag>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latency_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_name: Option<String>,
}

impl CloudTranscriptionVerdict {
    /// Build a verdict for text that came off the cloud path. `source` is fixed
    /// to `Cloud` here — the constructor is the single place that stamps
    /// provenance, so a cloud result can never be mislabelled downstream.
    /// Confidence flags start empty; the cloud protocols carry no per-token
    /// scores today.
    fn new(text: String, latency_ms: Option<u64>, model_name: Option<String>) -> Self {
        Self {
            text,
            source: TranscriptionSource::Cloud,
            confidence_flags: Vec::new(),
            latency_ms,
            model_name,
        }
    }
}

/// NDJSON response line: `stt-jsonl-v1` (`type` = hello/ack/transcript.final/
/// stream.closed/error) or the legacy `is_final` shape.
#[derive(Deserialize, Debug)]
struct NdjsonChunk {
    #[serde(rename = "type")]
    kind: Option<String>,
    text: Option<String>,
    is_final: Option<bool>,
    error: Option<String>,
    message: Option<String>,
    /// `stt-jsonl-v1` server-side identity of this stream (`resp_stt_…`),
    /// carried on `transcript.final` and `stream.closed`. Logged so a take can
    /// be traced on the gateway; never used for routing.
    response_id: Option<String>,
}

/// Audio validation error type for pre-flight checks
#[derive(Debug, Clone)]
pub enum AudioValidationError {
    /// Audio file is too short (likely to cause Whisper hallucinations)
    TooShort { size_bytes: usize, min_bytes: usize },
    /// Audio file is too large for backend upload limit
    TooLarge { size_mb: f64, max_mb: usize },
    /// Audio file is empty
    Empty,
}

impl std::fmt::Display for AudioValidationError {
    /// Human-readable validation failure for logs and user-facing bail messages.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AudioValidationError::TooShort {
                size_bytes,
                min_bytes,
            } => {
                write!(
                    f,
                    "Audio too short ({} bytes, minimum {} bytes)",
                    size_bytes, min_bytes
                )
            }
            AudioValidationError::TooLarge { size_mb, max_mb } => {
                write!(
                    f,
                    "Audio too large ({:.1} MB, maximum {} MB)",
                    size_mb, max_mb
                )
            }
            AudioValidationError::Empty => {
                write!(f, "Audio file is empty")
            }
        }
    }
}

impl std::error::Error for AudioValidationError {}

/// Validate audio data before sending to backend
///
/// Pre-flight checks to catch common issues:
/// - Empty or too short audio (causes Whisper hallucinations)
/// - Audio exceeding backend upload limit (413 errors)
///
/// # Arguments
/// * `audio_data` - Raw audio bytes to validate
///
/// # Returns
/// Ok(()) if valid, or AudioValidationError with details
pub fn validate_audio(audio_data: &[u8]) -> std::result::Result<(), AudioValidationError> {
    // Empty file check
    if audio_data.is_empty() {
        return Err(AudioValidationError::Empty);
    }

    // Minimum size check - very short audio causes Whisper hallucinations
    // 1KB is roughly 0.06 seconds of WAV audio at 16kHz mono
    /// Floor size below which Whisper often hallucinates (~0.06s at 16 kHz mono).
    const MIN_AUDIO_BYTES: usize = 1024;
    if audio_data.len() < MIN_AUDIO_BYTES {
        return Err(AudioValidationError::TooShort {
            size_bytes: audio_data.len(),
            min_bytes: MIN_AUDIO_BYTES,
        });
    }

    // Maximum size check - backend has upload limit (configurable via env)
    let max_mb: usize = std::env::var("BACKEND_MAX_UPLOAD_MB")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(20); // Default 20MB limit

    let size_bytes = audio_data.len();
    let size_mb = size_bytes as f64 / (1024.0 * 1024.0);

    if size_bytes > max_mb * 1024 * 1024 {
        return Err(AudioValidationError::TooLarge { size_mb, max_mb });
    }

    Ok(())
}

/// Get or create HTTP client with sensible defaults
fn get_client() -> &'static Client {
    /// Process-wide reqwest client: 120s request, 5s connect, 90s pool idle.
    static CLIENT: OnceLock<Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        Client::builder()
            .timeout(Duration::from_secs(120)) // Long timeout for transcription
            .connect_timeout(Duration::from_secs(5))
            .pool_idle_timeout(Duration::from_secs(90))
            .build()
            .expect("Failed to create HTTP client")
    })
}

/// Transcribe audio file using external STT with retry logic
///
/// # Arguments
/// * `path` - Path to audio file (WAV, MP3, M4A, etc.)
/// * `language` - Optional language code (e.g., "pl", "en"). If None, auto-detect.
/// * `endpoint_url` - Full STT endpoint URL
/// * `api_key` - API key for authentication
///
/// # Returns
/// Transcribed text or error
///
/// # Features
/// - Pre-flight validation (size checks to prevent 413 and hallucinations)
/// - Automatic retry with exponential backoff (up to 3 attempts)
/// - Tray status updates for visual feedback during retries
///
/// # Example
/// ```no_run
/// use std::path::Path;
/// use codescribe_core::client;
///
/// # #[tokio::main]
/// # async fn main() -> anyhow::Result<()> {
/// let transcript = client::transcribe_cloud(
///     Path::new("recording.wav"),
///     Some("pl"),
///     "https://api.example.com/v1/audio/transcriptions",
///     "api-key",
/// ).await?;
/// println!("Transcript: {}", transcript.text);
/// # Ok(())
/// # }
/// ```
/// Endpoint and credential come from `Config::stt_lane`.
pub async fn transcribe_cloud(
    path: &Path,
    language: Option<&str>,
    endpoint_url: &str,
    api_key: &str,
) -> Result<CloudTranscriptionVerdict> {
    info!("transcribe_cloud() START for path: {:?}", path);

    transcribe_external(path, language, endpoint_url, api_key).await
}

/// Check if an error is retryable (network issues, timeouts, server errors)
fn is_retryable_error(error: &anyhow::Error) -> bool {
    let error_str = error.to_string().to_lowercase();

    // Network/connection errors are retryable
    if error_str.contains("connection")
        || error_str.contains("timeout")
        || error_str.contains("network")
        || error_str.contains("reset")
        || error_str.contains("refused")
    {
        return true;
    }

    // Server errors (5xx) are retryable
    if error_str.contains("500")
        || error_str.contains("502")
        || error_str.contains("503")
        || error_str.contains("504")
    {
        return true;
    }

    // 413 (file too large) is NOT retryable - should have been caught by validation
    // 400/401/403/404 are NOT retryable - client errors
    false
}

/// Transcribe audio using external STT API
///
/// Supports multiple protocols based on endpoint URL:
/// - WebSocket schemes are rejected; they belong to the separate Live lane
/// - URL ending with `:stream` → NDJSON streaming HTTP
/// - Otherwise → OpenAI-compatible multipart upload
///
/// # Arguments
/// * `path` - Path to audio file
/// * `language` - Optional language code
/// * `endpoint_url` - Full URL to the transcription endpoint
/// * `api_key` - API key for authentication
async fn transcribe_external(
    path: &Path,
    language: Option<&str>,
    endpoint_url: &str,
    api_key: &str,
) -> Result<CloudTranscriptionVerdict> {
    info!("Using external STT endpoint: {}", endpoint_url);

    let canonical_path = canonicalize_path(path)?;
    let lang = language.unwrap_or("pl");

    // File lane only: `:stream` is the NDJSON variant, anything else is multipart.
    // Live sockets belong to the Live lane (`Config::stt_lane(SttLane::Live)`).
    if endpoint_url.starts_with("ws") {
        anyhow::bail!("a WebSocket socket is not a file transcription endpoint: {endpoint_url}");
    }
    // Resolve at the common file transport boundary so every caller follows
    // the vendor's OAuth-first policy, including any direct internal callers.
    let auth = if let Some(vendor) = super::speech::vendor_for_endpoint(endpoint_url) {
        Some(super::speech::resolve_vendor_auth(vendor, Some(api_key)).await?)
    } else {
        None
    };
    let api_key = auth.as_ref().map_or(api_key, |auth| auth.bearer.as_str());
    if endpoint_url.ends_with(":stream") {
        // NDJSON streaming HTTP: the file is decoded and sent in segments, so
        // the whole-file upload cap of the multipart lane does not apply.
        transcribe_ndjson(endpoint_url, api_key, &canonical_path, lang).await
    } else {
        // OpenAI-compatible multipart upload: one body, one backend cap.
        // nosemgrep: rust.actix.path-traversal.tainted-path.tainted-path (path canonicalized above)
        let mut file = File::open(&canonical_path)
            .await
            .context("Failed to open audio file")?;
        let mut buffer = Vec::new();
        file.read_to_end(&mut buffer)
            .await
            .context("Failed to read audio file")?;
        if let Err(validation_error) = validate_audio(&buffer) {
            error!("Audio validation failed: {}", validation_error);
            crate::status::notify_status(crate::status::StatusSignal::Error);
            anyhow::bail!("Audio validation failed: {}", validation_error);
        }
        let filename = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("recording.wav");
        transcribe_multipart(endpoint_url, api_key, buffer, lang, filename).await
    }
}

// ============================================================================
// NDJSON Streaming HTTP STT
// ============================================================================

/// Wire rate of the `:stream` lane (`stt-jsonl-v1`): PCM16 mono at 16 kHz.
const NDJSON_SAMPLE_RATE: u32 = 16_000;
/// Segment length for the `:stream` file lane. Measured 2026-09-09 on
/// api.libraxis.cloud: 10-minute PCM16@16k segments (19 MB, 25 MB NDJSON)
/// transcribe; one 20-minute body (51 MB) dies with nginx 500.
const NDJSON_SEGMENT_SECONDS: usize = 600;
/// A cut is moved back into the quietest 20 ms frame of this window so a
/// segment boundary lands between words, not inside one.
const NDJSON_SEGMENT_SEARCH_SECONDS: usize = 5;
const NDJSON_QUIET_FRAME_SAMPLES: usize = 320;
/// PCM bytes per `chunk` line, the size the operator's proven shell client uses.
const NDJSON_CHUNK_BYTES: usize = 128 * 1024;

/// Transcribe an audio file via the NDJSON streaming HTTP lane (`stt-jsonl-v1`).
///
/// The file is decoded and resampled to 16 kHz mono, cut into segments of at
/// most [`NDJSON_SEGMENT_SECONDS`] at quiet points, and every segment is one
/// request: `set` → `chunk`× → `end`, answered by `transcript.final`. The
/// segment texts are joined in order. Long takes never travel as one body.
async fn transcribe_ndjson(
    url: &str,
    api_key: &str,
    path: &Path,
    language: &str,
) -> Result<CloudTranscriptionVerdict> {
    let start = Instant::now();
    let decode_path = path.to_path_buf();
    let (samples, sample_rate) =
        tokio::task::spawn_blocking(move || crate::audio::load_audio_file(&decode_path))
            .await
            .context("audio decode task join error")??;
    if samples.is_empty() {
        anyhow::bail!("Audio validation failed: {}", AudioValidationError::Empty);
    }
    let samples = crate::audio::resample_to_16k(&samples, sample_rate);
    let pcm: Vec<i16> = samples
        .iter()
        .map(|s| (s.clamp(-1.0, 1.0) * f32::from(i16::MAX)) as i16)
        .collect();
    let segments = split_pcm_at_quiet_points(
        &pcm,
        NDJSON_SEGMENT_SECONDS * NDJSON_SAMPLE_RATE as usize,
        NDJSON_SEGMENT_SEARCH_SECONDS * NDJSON_SAMPLE_RATE as usize,
    );
    info!(
        "[NDJSON STT] POST {} ({:.1}s @ {}Hz → {} segment(s), lang={})",
        url,
        pcm.len() as f64 / f64::from(NDJSON_SAMPLE_RATE),
        NDJSON_SAMPLE_RATE,
        segments.len(),
        language
    );

    let mut texts: Vec<String> = Vec::with_capacity(segments.len());
    for (index, segment) in segments.iter().enumerate() {
        let text = transcribe_ndjson_segment(url, api_key, segment, language)
            .await
            .with_context(|| format!("segment {}/{}", index + 1, segments.len()))?;
        info!(
            "[NDJSON STT] segment {}/{}: {:.1}s → {} chars",
            index + 1,
            segments.len(),
            segment.len() as f64 / f64::from(NDJSON_SAMPLE_RATE),
            text.chars().count()
        );
        let text = text.trim();
        if !text.is_empty() {
            texts.push(text.to_string());
        }
    }
    let final_text = texts.join(" ");

    let duration_ms = start.elapsed().as_millis();
    info!(
        "[NDJSON STT] Complete in {}ms: {} chars over {} segment(s)",
        duration_ms,
        final_text.len(),
        segments.len()
    );

    if final_text.is_empty() {
        anyhow::bail!("No transcription received from NDJSON STT");
    }

    Ok(CloudTranscriptionVerdict::new(
        final_text,
        Some(duration_ms.min(u128::from(u64::MAX)) as u64),
        None,
    ))
}

/// Cut PCM into slices of at most `segment_len` samples. Each cut is moved
/// back, within `search_len` samples of the boundary, to the start of the
/// quietest [`NDJSON_QUIET_FRAME_SAMPLES`] frame. Slices concatenate back to
/// the input exactly.
fn split_pcm_at_quiet_points(pcm: &[i16], segment_len: usize, search_len: usize) -> Vec<&[i16]> {
    let mut segments = Vec::new();
    let mut pos = 0;
    while pcm.len() - pos > segment_len {
        let boundary = pos + segment_len;
        let window_start = boundary.saturating_sub(search_len).max(pos + 1);
        let mut cut = boundary;
        let mut quietest = u64::MAX;
        let mut frame_start = window_start;
        while frame_start + NDJSON_QUIET_FRAME_SAMPLES <= boundary {
            let energy: u64 = pcm[frame_start..frame_start + NDJSON_QUIET_FRAME_SAMPLES]
                .iter()
                .map(|s| u64::from(s.unsigned_abs()))
                .sum();
            if energy < quietest {
                quietest = energy;
                cut = frame_start;
            }
            frame_start += NDJSON_QUIET_FRAME_SAMPLES;
        }
        segments.push(&pcm[pos..cut]);
        pos = cut;
    }
    segments.push(&pcm[pos..]);
    segments
}

/// One `stt-jsonl-v1` request for one PCM16@16k segment.
async fn transcribe_ndjson_segment(
    url: &str,
    api_key: &str,
    pcm: &[i16],
    language: &str,
) -> Result<String> {
    use base64::{Engine, engine::general_purpose::STANDARD as BASE64};

    let bytes: Vec<u8> = pcm.iter().flat_map(|s| s.to_le_bytes()).collect();
    let mut body = String::with_capacity(bytes.len() * 4 / 3 + 1024);
    body.push_str(
        &serde_json::json!({
            "type": "set",
            "language": language,
            "sample_rate": NDJSON_SAMPLE_RATE,
            "encoding": "pcm16",
            "vad": false
        })
        .to_string(),
    );
    body.push('\n');
    for chunk in bytes.chunks(NDJSON_CHUNK_BYTES) {
        body.push_str(
            &serde_json::json!({"type": "chunk", "audio_base64": BASE64.encode(chunk)}).to_string(),
        );
        body.push('\n');
    }
    body.push_str(&serde_json::json!({"type": "end"}).to_string());
    body.push('\n');
    debug!(
        "[NDJSON STT] Sending {} bytes NDJSON ({} bytes PCM)",
        body.len(),
        bytes.len()
    );

    let request = get_client()
        .post(url)
        .header("Content-Type", "application/x-ndjson");
    let request = match crate::stt::tail_provider::stt_auth_mode(url) {
        crate::stt::tail_provider::SttAuthMode::Unauthenticated => request,
        crate::stt::tail_provider::SttAuthMode::Bearer => request.bearer_auth(api_key),
        crate::stt::tail_provider::SttAuthMode::ApiKey => request.header("x-api-key", api_key),
    };
    let response = request
        .body(body)
        .send()
        .await
        .context("Failed to send NDJSON STT request")?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        error!("[NDJSON STT] Error {}: {}", status, body);
        anyhow::bail!("NDJSON STT request failed: {} - {}", status, body);
    }

    // Stream and parse NDJSON
    let mut stream = response.bytes_stream();
    let mut buffer = Vec::new();
    let mut final_text: Option<String> = None;
    let mut partial_count = 0u32;

    'lines: while let Some(chunk) = stream.next().await {
        let bytes = chunk.context("Failed to read NDJSON chunk")?;
        buffer.extend_from_slice(&bytes);

        // Process complete lines
        while let Some(pos) = buffer.iter().position(|&b| b == b'\n') {
            let line: Vec<u8> = buffer.drain(..=pos).collect();
            let line_str = String::from_utf8_lossy(&line);
            let line_str = line_str.trim();

            if line_str.is_empty() {
                continue;
            }

            // Handle SSE format: "data: {...}" or "event: ..." or plain NDJSON
            let json_str = if line_str.starts_with("data:") {
                let data = line_str.strip_prefix("data:").unwrap().trim();
                if data == "[DONE]" {
                    debug!("[NDJSON STT] Received [DONE] marker");
                    break 'lines;
                }
                data
            } else if line_str.starts_with("event:") {
                // Skip SSE event lines
                continue;
            } else {
                // Plain NDJSON (no prefix)
                line_str
            };

            if let Ok(chunk) = serde_json::from_str::<NdjsonChunk>(json_str) {
                if chunk.kind.as_deref() == Some("error") || chunk.error.is_some() {
                    let err = chunk
                        .error
                        .or(chunk.message)
                        .unwrap_or_else(|| "unspecified".to_string());
                    error!("[NDJSON STT] Error in stream: {}", err);
                    anyhow::bail!("NDJSON STT error: {}", err);
                }
                if chunk.kind.as_deref() == Some("stream.closed") {
                    info!(
                        "[NDJSON STT] stream.closed response_id={}",
                        chunk.response_id.as_deref().unwrap_or("none")
                    );
                    break 'lines;
                }
                if let Some(text) = chunk.text {
                    if chunk.kind.as_deref() == Some("transcript.final")
                        || chunk.is_final.unwrap_or(false)
                    {
                        info!(
                            "[NDJSON STT] Final: {} chars after {} partials response_id={}",
                            text.len(),
                            partial_count,
                            chunk.response_id.as_deref().unwrap_or("none")
                        );
                        final_text = Some(text);
                    } else {
                        partial_count += 1;
                        debug!(
                            "[NDJSON STT] partial #{}: {} chars",
                            partial_count,
                            text.len()
                        );
                    }
                }
            }
        }
    }

    final_text.ok_or_else(|| anyhow::anyhow!("No transcript.final received from NDJSON STT"))
}

// ============================================================================
// OpenAI-compatible Multipart Upload STT
// ============================================================================

/// Transcribe audio via OpenAI-compatible multipart upload
///
/// Standard HTTP POST with multipart/form-data:
/// - file: audio file
/// - model: whisper model name
/// - language: optional language code
pub(crate) fn stt_model(url: &str, override_model: Option<&str>) -> Option<String> {
    use super::provider::ProviderKind;
    let vendor = super::speech::vendor_for_endpoint(url);
    if vendor == Some(ProviderKind::XaiResponses) {
        return None; // xAI STT has no model request field.
    }
    let default = if vendor == Some(ProviderKind::OpenAiResponses) {
        super::vendors::openai::DEFAULT_STT_MODEL
    } else {
        "mlx-community/whisper-large-v3-mlx"
    };
    Some(
        override_model
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or(default)
            .to_string(),
    )
}

async fn transcribe_multipart(
    url: &str,
    api_key: &str,
    audio_data: Vec<u8>,
    language: &str,
    filename: &str,
) -> Result<CloudTranscriptionVerdict> {
    let start = Instant::now();
    let vocabulary = crate::stt::request_vocabulary::codescribe_stt_vocabulary(url);
    info!(
        "[Multipart STT] POST {} ({} bytes, lang={}, vocabulary={})",
        url,
        audio_data.len(),
        language,
        vocabulary.unwrap_or("off")
    );

    // Retry loop
    let mut last_error: Option<anyhow::Error> = None;

    for attempt in 1..=TRANSCRIPTION_MAX_RETRIES {
        // Build multipart form fresh for each attempt
        let file_part = Part::bytes(audio_data.clone())
            .file_name(filename.to_string())
            .mime_str("audio/wav")
            .context("Failed to set MIME type")?;

        let whisper_model = stt_model(url, std::env::var("WHISPER_MODEL").ok().as_deref());
        let mut form = Form::new()
            .part("file", file_part)
            .text("language", language.to_string());
        if let Some(model) = &whisper_model {
            form = form.text("model", model.clone());
        }
        if super::speech::vendor_for_endpoint(url)
            == Some(super::provider::ProviderKind::OpenAiResponses)
        {
            form = form.text("response_format", "json");
        }
        if let Some((field, value)) =
            crate::stt::request_vocabulary::codescribe_stt_vocabulary_form_part(url)
        {
            form = form.text(field, value.to_string());
        }

        debug!(
            "[Multipart STT] attempt {}/{} for {}",
            attempt, TRANSCRIPTION_MAX_RETRIES, filename
        );

        match transcribe_multipart_request(url, api_key, form).await {
            Ok(text) => {
                if attempt > 1 {
                    info!(
                        "[Multipart STT] succeeded on attempt {}/{}",
                        attempt, TRANSCRIPTION_MAX_RETRIES
                    );
                }
                return Ok(CloudTranscriptionVerdict::new(
                    text,
                    Some(start.elapsed().as_millis().min(u128::from(u64::MAX)) as u64),
                    whisper_model.clone(),
                ));
            }
            Err(e) => {
                let is_retryable = is_retryable_error(&e);
                warn!(
                    "[Multipart STT] attempt {}/{} failed: {} (retryable: {})",
                    attempt, TRANSCRIPTION_MAX_RETRIES, e, is_retryable
                );

                if attempt < TRANSCRIPTION_MAX_RETRIES && is_retryable {
                    crate::status::notify_status(crate::status::StatusSignal::Thinking);

                    let delay_ms = TRANSCRIPTION_RETRY_DELAY_MS * attempt as u64;
                    info!(
                        "[Multipart STT] retrying in {}ms (attempt {}/{})",
                        delay_ms,
                        attempt + 1,
                        TRANSCRIPTION_MAX_RETRIES
                    );
                    tokio::time::sleep(Duration::from_millis(delay_ms)).await;
                }

                last_error = Some(e);

                // Non-retryable errors should fail fast instead of looping through all attempts.
                if !is_retryable {
                    break;
                }
            }
        }
    }

    crate::status::notify_status(crate::status::StatusSignal::Error);
    Err(last_error
        .unwrap_or_else(|| anyhow::anyhow!("Multipart STT transcription failed after all retries")))
}

/// Send a single multipart STT transcription request (used by retry loop)
async fn transcribe_multipart_request(url: &str, api_key: &str, form: Form) -> Result<String> {
    let request = get_client().post(url);
    let request = match crate::stt::tail_provider::stt_auth_mode(url) {
        crate::stt::tail_provider::SttAuthMode::Unauthenticated => request,
        crate::stt::tail_provider::SttAuthMode::Bearer => request.bearer_auth(api_key),
        crate::stt::tail_provider::SttAuthMode::ApiKey => request.header("x-api-key", api_key),
    };
    let response = request
        .multipart(form)
        .send()
        .await
        .context("Failed to send transcription request to external STT")?;

    if !response.status().is_success() {
        let status = response.status();
        // Vendor error bodies can echo request contents; keep diagnostics content-free.
        if super::speech::vendor_for_endpoint(url).is_some() {
            anyhow::bail!("Vendor STT transcription failed with status {}", status);
        }
        let body = response
            .text()
            .await
            .unwrap_or_else(|_| "(no body)".to_string());
        anyhow::bail!(
            "External STT transcription failed with status {}: {}",
            status,
            body
        );
    }

    // External STT returns OpenAI-compatible response
    let transcribe_response: TranscribeResponse = response
        .json()
        .await
        .context("Failed to parse external STT transcription response")?;

    info!(
        "External STT transcription successful, length: {} chars",
        transcribe_response.text.len()
    );

    Ok(transcribe_response.text)
}

/// Unit tests for audio preflight, retry classification, serde.
#[cfg(test)]
mod tests {
    #[test]
    fn short_pcm_is_one_segment() {
        let pcm = vec![1000i16; 16_000 * 30];
        let segments = split_pcm_at_quiet_points(&pcm, 600 * 16_000, 5 * 16_000);
        assert_eq!(segments.len(), 1);
        assert_eq!(segments[0].len(), pcm.len());
    }

    #[test]
    fn long_pcm_cuts_inside_the_quiet_gap_before_the_boundary() {
        // 25 minutes of "speech" with a 400 ms hole at 9:58 and one at 19:57.
        let rate = 16_000usize;
        let mut pcm = vec![8000i16; 25 * 60 * rate];
        let holes = [
            (598 * rate, 598 * rate + 6_400),
            (1_197 * rate, 1_197 * rate + 6_400),
        ];
        for (from, to) in holes {
            pcm[from..to].fill(0);
        }
        let segments = split_pcm_at_quiet_points(&pcm, 600 * rate, 5 * rate);
        assert_eq!(segments.len(), 3, "25 min at 10-min segments");
        let first_cut = segments[0].len();
        let second_cut = first_cut + segments[1].len();
        assert!(
            (holes[0].0..holes[0].1).contains(&first_cut),
            "first cut {first_cut} not inside the 9:58 hole"
        );
        assert!(
            (holes[1].0..holes[1].1).contains(&second_cut),
            "second cut {second_cut} not inside the 19:57 hole"
        );
        for segment in &segments {
            assert!(segment.len() <= 600 * rate);
            assert!(!segment.is_empty());
        }
        let rebuilt: Vec<i16> = segments.concat();
        assert_eq!(rebuilt, pcm, "segments must concatenate back to the input");
    }

    #[test]
    fn ndjson_lines_parse_both_wire_shapes() {
        let modern: NdjsonChunk =
            serde_json::from_str(r#"{"type": "transcript.final", "text": "Dziękuję.", "duration_ms": null, "response_id": "r1"}"#)
                .unwrap();
        assert_eq!(modern.kind.as_deref(), Some("transcript.final"));
        assert_eq!(modern.text.as_deref(), Some("Dziękuję."));
        let legacy: NdjsonChunk =
            serde_json::from_str(r#"{"text": "x", "is_final": true}"#).unwrap();
        assert_eq!(legacy.is_final, Some(true));
        let err: NdjsonChunk =
            serde_json::from_str(r#"{"type": "error", "message": "boom"}"#).unwrap();
        assert_eq!(err.kind.as_deref(), Some("error"));
        assert_eq!(err.message.as_deref(), Some("boom"));
    }

    use super::*;

    #[test]
    fn vendor_stt_model_selection_matches_wire_contract() {
        assert_eq!(
            stt_model("https://api.openai.com/v1/audio/transcriptions", None).as_deref(),
            Some("gpt-4o-mini-transcribe")
        );
        assert_eq!(
            stt_model(
                "https://api.openai.com/v1/audio/transcriptions",
                Some("custom")
            )
            .as_deref(),
            Some("custom")
        );
        assert_eq!(stt_model("https://api.x.ai/v1/stt", Some("ignored")), None);
        assert_eq!(
            stt_model("https://custom.example/stt", None).as_deref(),
            Some("mlx-community/whisper-large-v3-mlx")
        );
    }

    #[test]
    fn xai_transcription_response_accepts_vendor_metadata() {
        let response: TranscribeResponse = serde_json::from_str(
            r#"{"text":"Repeated repeated","language":"en","duration":1.25,"words":[]}"#,
        )
        .unwrap();
        assert_eq!(response.text, "Repeated repeated");
    }

    /// Empty buffer is rejected before any network call.
    #[test]
    fn test_validate_audio_empty() {
        let result = validate_audio(&[]);
        assert!(matches!(result, Err(AudioValidationError::Empty)));
    }

    /// Sub-minimum payloads map to `TooShort` with the pinned min_bytes.
    #[test]
    fn test_validate_audio_too_short() {
        let result = validate_audio(&[0u8; 500]); // 500 bytes < 1024 minimum
        assert!(matches!(
            result,
            Err(AudioValidationError::TooShort {
                size_bytes: 500,
                min_bytes: 1024
            })
        ));
    }

    /// Buffer above the floor and under the default max is accepted.
    #[test]
    fn test_validate_audio_valid() {
        let result = validate_audio(&[0u8; 2048]); // 2KB > 1KB minimum
        assert!(result.is_ok());
    }

    /// `BACKEND_MAX_UPLOAD_MB` drives the too-large path (not retried later).
    #[test]
    fn test_validate_audio_too_large() {
        // Set a low limit for testing (1MB)
        // SAFETY: Test code runs single-threaded
        unsafe { std::env::set_var("BACKEND_MAX_UPLOAD_MB", "1") };
        let result = validate_audio(&vec![0u8; 2 * 1024 * 1024]); // 2MB > 1MB limit
        // SAFETY: Test code runs single-threaded
        unsafe { std::env::remove_var("BACKEND_MAX_UPLOAD_MB") };

        assert!(matches!(result, Err(AudioValidationError::TooLarge { .. })));
    }

    /// Connection-class errors are retryable for cloud STT.
    #[test]
    fn test_is_retryable_error_network() {
        let error = anyhow::anyhow!("connection refused");
        assert!(is_retryable_error(&error));
    }

    /// Timeout-class errors are retryable for cloud STT.
    #[test]
    fn test_is_retryable_error_timeout() {
        let error = anyhow::anyhow!("request timeout");
        assert!(is_retryable_error(&error));
    }

    /// 5xx-style errors are retryable for cloud STT.
    #[test]
    fn test_is_retryable_error_server() {
        let error = anyhow::anyhow!("status 503: Service Unavailable");
        assert!(is_retryable_error(&error));
    }

    /// 4xx client errors must not burn retry budget.
    #[test]
    fn test_is_not_retryable_client_error() {
        let error = anyhow::anyhow!("status 400: Bad Request");
        assert!(!is_retryable_error(&error));
    }

    /// 413 is a validation/size issue — never retried.
    #[test]
    fn test_is_not_retryable_413() {
        // 413 should not be retried - file too large is a client issue
        let error = anyhow::anyhow!("status 413: Payload Too Large");
        assert!(!is_retryable_error(&error));
    }

    /// Cloud verdict JSON round-trips with source and optional metadata intact.
    #[test]
    fn cloud_transcription_verdict_serde_roundtrip() {
        let verdict = CloudTranscriptionVerdict::new(
            "Hello cloud".to_string(),
            Some(187),
            Some("whisper-large-v3".to_string()),
        );
        let json = serde_json::to_string(&verdict).expect("serialize verdict");
        let restored: CloudTranscriptionVerdict =
            serde_json::from_str(&json).expect("deserialize verdict");
        assert_eq!(restored, verdict);
        assert_eq!(restored.text, "Hello cloud");
        assert_eq!(restored.source, TranscriptionSource::Cloud);
        assert_eq!(restored.latency_ms, Some(187));
        assert_eq!(restored.model_name, Some("whisper-large-v3".to_string()));
        assert!(restored.confidence_flags.is_empty());
    }

    /// Empty optionals and empty flags are omitted via `skip_serializing_if`.
    #[test]
    fn cloud_transcription_verdict_omits_empty_optional_fields_in_json() {
        let verdict = CloudTranscriptionVerdict::new("Just text".to_string(), None, None);
        let json = serde_json::to_string(&verdict).expect("serialize");
        assert!(
            !json.contains("latency_ms"),
            "None latency_ms must be omitted (got {json})"
        );
        assert!(
            !json.contains("model_name"),
            "None model_name must be omitted (got {json})"
        );
        assert!(
            !json.contains("confidence_flags"),
            "empty confidence_flags must be omitted (got {json})"
        );
    }
}
