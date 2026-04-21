//! HTTP client for cloud STT providers.
//!
//! Features:
//! - Explicit external endpoint (no backend discovery)
//! - Multipart file upload for transcription
//! - Retry logic with exponential backoff
//! - Proper error handling and logging

use anyhow::{Context, Result};
use futures_util::{SinkExt, StreamExt};
use reqwest::Client;
use reqwest::multipart::{Form, Part};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::{Duration, Instant};
use tokio::fs::File;
use tokio::io::AsyncReadExt;
use tokio_tungstenite::{connect_async, tungstenite::Message};
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

// ============================================================================
// WebSocket STT Protocol Structures
// ============================================================================

/// WebSocket config message (sent first)
#[derive(Serialize)]
struct WsConfig {
    #[serde(rename = "type")]
    msg_type: &'static str,
    language: String,
    api_key: String,
}

/// WebSocket end signal (sent after audio)
#[derive(Serialize)]
struct WsEnd {
    #[serde(rename = "type")]
    msg_type: &'static str,
}

/// WebSocket response message
#[derive(Deserialize, Debug)]
struct WsResponse {
    #[serde(rename = "type")]
    msg_type: String,
    text: Option<String>,
    error: Option<String>,
}

/// NDJSON chunk response
#[derive(Deserialize, Debug)]
struct NdjsonChunk {
    text: Option<String>,
    is_final: Option<bool>,
    error: Option<String>,
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

/// Check if local Whisper engine is ready.
///
/// Returns:
/// - `Ok(true)` if the engine is initialized (or initializes successfully)
/// - `Ok(false)` if initialization fails
pub async fn check_health() -> Result<bool> {
    if crate::stt::whisper::singleton::is_initialized() {
        return Ok(true);
    }

    match crate::stt::whisper::init() {
        Ok(()) => Ok(true),
        Err(e) => {
            warn!("Whisper engine not ready: {}", e);
            Ok(false)
        }
    }
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
/// // nosemgrep: javascript.lang.security.detect-insecure-websocket.detect-insecure-websocket
/// - `wss://` or `ws://` → WebSocket streaming (ws:// for localhost dev, wss:// for production)
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

    // Read file into memory (shared by all protocols)
    let canonical_path = canonicalize_path(path)?;
    // nosemgrep: rust.actix.path-traversal.tainted-path.tainted-path (path canonicalized above)
    let mut file = File::open(&canonical_path)
        .await
        .context("Failed to open audio file")?;

    let mut buffer = Vec::new();
    file.read_to_end(&mut buffer)
        .await
        .context("Failed to read audio file")?;

    // Pre-flight validation
    if let Err(validation_error) = validate_audio(&buffer) {
        error!("Audio validation failed: {}", validation_error);
        crate::status::notify_status(crate::status::StatusSignal::Error);
        anyhow::bail!("Audio validation failed: {}", validation_error);
    }

    let lang = language.unwrap_or("pl");

    // nosemgrep: javascript.lang.security.detect-insecure-websocket.detect-insecure-websocket
    // Dispatch based on protocol (ws:// for localhost, wss:// for production)
    // nosemgrep: javascript.lang.security.detect-insecure-websocket.detect-insecure-websocket
    if endpoint_url.starts_with("wss://") || endpoint_url.starts_with("ws://") {
        // WebSocket streaming
        transcribe_websocket(endpoint_url, api_key, buffer, lang).await
    } else if endpoint_url.ends_with(":stream") {
        // NDJSON streaming HTTP
        transcribe_ndjson(endpoint_url, api_key, buffer, lang).await
    } else {
        // OpenAI-compatible multipart upload
        let filename = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("recording.wav");
        transcribe_multipart(endpoint_url, api_key, buffer, lang, filename).await
    }
}

// ============================================================================
// WebSocket Streaming STT
// ============================================================================

/// Transcribe audio via WebSocket streaming
///
/// Protocol:
/// 1. Connect to WebSocket
/// 2. Send config JSON: {"type": "config", "language": "...", "api_key": "..."}
/// 3. Send audio as binary message
/// 4. Send end signal: {"type": "end"}
/// 5. Receive partial/final responses until final or close
async fn transcribe_websocket(
    url: &str,
    api_key: &str,
    audio_data: Vec<u8>,
    language: &str,
) -> Result<CloudTranscriptionVerdict> {
    let start = Instant::now();
    info!(
        "[WS STT] Connecting to {} ({} bytes, lang={})",
        url,
        audio_data.len(),
        language
    );

    let (mut ws, response) = connect_async(url)
        .await
        .context("Failed to connect to WebSocket STT endpoint")?;

    debug!(
        "[WS STT] Connected in {:?}, status: {:?}",
        start.elapsed(),
        response.status()
    );

    // 1. Send config
    let config = WsConfig {
        msg_type: "config",
        language: language.to_string(),
        api_key: api_key.to_string(),
    };
    ws.send(Message::Text(serde_json::to_string(&config)?.into()))
        .await
        .context("Failed to send WebSocket config")?;

    // 2. Send audio binary
    info!(
        "[WS STT] Sending {} bytes ({:.2} MB)",
        audio_data.len(),
        audio_data.len() as f64 / 1_000_000.0
    );
    ws.send(Message::Binary(audio_data.into()))
        .await
        .context("Failed to send audio data")?;

    // 3. Signal end
    let end = WsEnd { msg_type: "end" };
    ws.send(Message::Text(serde_json::to_string(&end)?.into()))
        .await
        .context("Failed to send end signal")?;

    // 4. Collect responses
    let mut final_text = String::new();
    let mut partial_count = 0u32;

    while let Some(msg) = ws.next().await {
        match msg? {
            Message::Text(txt) => {
                let resp: WsResponse = serde_json::from_str(&txt)
                    .with_context(|| format!("Failed to parse WS response: {}", txt))?;

                match resp.msg_type.as_str() {
                    "partial" => {
                        partial_count += 1;
                        if let Some(t) = &resp.text {
                            debug!("[WS STT] partial #{}: {} chars", partial_count, t.len());
                            // TODO: callback for real-time UI updates
                        }
                    }
                    "final" => {
                        if let Some(t) = resp.text {
                            final_text = t;
                            info!(
                                "[WS STT] Final: {} chars after {} partials",
                                final_text.len(),
                                partial_count
                            );
                        }
                        break;
                    }
                    "error" => {
                        let err_msg = resp.error.unwrap_or_else(|| "Unknown error".to_string());
                        error!("[WS STT] Error: {}", err_msg);
                        anyhow::bail!("WebSocket STT error: {}", err_msg);
                    }
                    other => {
                        warn!("[WS STT] Unknown message type: {}", other);
                    }
                }
            }
            Message::Close(frame) => {
                debug!("[WS STT] Connection closed: {:?}", frame);
                break;
            }
            Message::Ping(data) => {
                let _ = ws.send(Message::Pong(data)).await;
            }
            _ => {}
        }
    }

    let _ = ws.close(None).await;
    let duration_ms = start.elapsed().as_millis();

    info!(
        "[WS STT] Complete in {}ms: {} chars",
        duration_ms,
        final_text.len()
    );

    if final_text.is_empty() {
        anyhow::bail!("No transcription received from WebSocket STT");
    }

    Ok(CloudTranscriptionVerdict::new(
        final_text,
        Some(duration_ms.min(u128::from(u64::MAX)) as u64),
        None,
    ))
}

// ============================================================================
// NDJSON Streaming HTTP STT
// ============================================================================

/// Transcribe audio via NDJSON streaming HTTP
///
/// Protocol:
/// 1. POST raw audio with Content-Type and x-api-key header
/// 2. Stream response, parse newline-delimited JSON chunks
/// 3. Return text from final chunk (is_final: true)
async fn transcribe_ndjson(
    url: &str,
    api_key: &str,
    audio_data: Vec<u8>,
    language: &str,
) -> Result<CloudTranscriptionVerdict> {
    use base64::{Engine, engine::general_purpose::STANDARD as BASE64};

    let start = Instant::now();

    // Parse WAV header to extract sample rate and PCM data
    // WAV format: RIFF header (12 bytes) + fmt chunk (24+ bytes) + data chunk
    if audio_data.len() < 44 {
        anyhow::bail!("Audio data too short for WAV header");
    }

    // Verify RIFF header
    if &audio_data[0..4] != b"RIFF" || &audio_data[8..12] != b"WAVE" {
        anyhow::bail!("Invalid WAV file format");
    }

    // Extract sample rate from fmt chunk (bytes 24-27, little-endian)
    let sample_rate = u32::from_le_bytes([
        audio_data[24],
        audio_data[25],
        audio_data[26],
        audio_data[27],
    ]);

    // Find data chunk start (skip header, typically 44 bytes but can vary)
    let mut data_start = 12; // After "WAVE"
    while data_start + 8 < audio_data.len() {
        let chunk_id = &audio_data[data_start..data_start + 4];
        let chunk_size = u32::from_le_bytes([
            audio_data[data_start + 4],
            audio_data[data_start + 5],
            audio_data[data_start + 6],
            audio_data[data_start + 7],
        ]) as usize;

        if chunk_id == b"data" {
            data_start += 8; // Skip "data" + size
            break;
        }
        data_start += 8 + chunk_size;
    }

    let pcm_data = &audio_data[data_start..];

    info!(
        "[NDJSON STT] POST {} ({} bytes PCM @ {}Hz, lang={})",
        url,
        pcm_data.len(),
        sample_rate,
        language
    );

    // Build NDJSON payload with base64 audio
    // Single chunk with all audio (could be chunked for streaming in future)
    let audio_base64 = BASE64.encode(pcm_data);

    let chunk_json = serde_json::json!({
        "type": "chunk",
        "audio_base64": audio_base64,
        "sample_rate": sample_rate,
        "encoding": "pcm16",
        "language": language,
        "last": true
    });

    let end_json = serde_json::json!({"type": "end"});

    let ndjson_body = format!("{}\n{}\n", chunk_json, end_json);

    debug!(
        "[NDJSON STT] Sending {} bytes NDJSON ({} bytes base64)",
        ndjson_body.len(),
        audio_base64.len()
    );

    let response = get_client()
        .post(url)
        .header("x-api-key", api_key)
        .header("Content-Type", "application/x-ndjson")
        .body(ndjson_body)
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
    let mut final_text = String::new();
    let mut partial_count = 0u32;

    while let Some(chunk) = stream.next().await {
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
                    break;
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
                if let Some(err) = chunk.error {
                    error!("[NDJSON STT] Error in stream: {}", err);
                    anyhow::bail!("NDJSON STT error: {}", err);
                }

                if let Some(text) = chunk.text {
                    if chunk.is_final.unwrap_or(false) {
                        final_text = text;
                        info!(
                            "[NDJSON STT] Final: {} chars after {} partials",
                            final_text.len(),
                            partial_count
                        );
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

    let duration_ms = start.elapsed().as_millis();
    info!(
        "[NDJSON STT] Complete in {}ms: {} chars",
        duration_ms,
        final_text.len()
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

// ============================================================================
// OpenAI-compatible Multipart Upload STT
// ============================================================================

/// Transcribe audio via OpenAI-compatible multipart upload
///
/// Standard HTTP POST with multipart/form-data:
/// - file: audio file
/// - model: whisper model name
/// - language: optional language code
async fn transcribe_multipart(
    url: &str,
    api_key: &str,
    audio_data: Vec<u8>,
    language: &str,
    filename: &str,
) -> Result<CloudTranscriptionVerdict> {
    let start = Instant::now();
    info!(
        "[Multipart STT] POST {} ({} bytes, lang={})",
        url,
        audio_data.len(),
        language
    );

    // Retry loop
    let mut last_error: Option<anyhow::Error> = None;

    for attempt in 1..=TRANSCRIPTION_MAX_RETRIES {
        // Build multipart form fresh for each attempt
        let file_part = Part::bytes(audio_data.clone())
            .file_name(filename.to_string())
            .mime_str("audio/wav")
            .context("Failed to set MIME type")?;

        // Model from env WHISPER_MODEL or default to non-turbo large-v3
        let whisper_model = std::env::var("WHISPER_MODEL")
            .unwrap_or_else(|_| "mlx-community/whisper-large-v3-mlx".to_string());

        let form = Form::new()
            .part("file", file_part)
            .text("model", whisper_model.clone())
            .text("language", language.to_string());

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
                    Some(whisper_model.clone()),
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
    let response = get_client()
        .post(url)
        .header("x-api-key", api_key)
        .multipart(form)
        .send()
        .await
        .context("Failed to send transcription request to external STT")?;

    if !response.status().is_success() {
        let status = response.status();
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validate_audio_empty() {
        let result = validate_audio(&[]);
        assert!(matches!(result, Err(AudioValidationError::Empty)));
    }

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

    #[test]
    fn test_validate_audio_valid() {
        let result = validate_audio(&[0u8; 2048]); // 2KB > 1KB minimum
        assert!(result.is_ok());
    }

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

    #[test]
    fn test_is_retryable_error_network() {
        let error = anyhow::anyhow!("connection refused");
        assert!(is_retryable_error(&error));
    }

    #[test]
    fn test_is_retryable_error_timeout() {
        let error = anyhow::anyhow!("request timeout");
        assert!(is_retryable_error(&error));
    }

    #[test]
    fn test_is_retryable_error_server() {
        let error = anyhow::anyhow!("status 503: Service Unavailable");
        assert!(is_retryable_error(&error));
    }

    #[test]
    fn test_is_not_retryable_client_error() {
        let error = anyhow::anyhow!("status 400: Bad Request");
        assert!(!is_retryable_error(&error));
    }

    #[test]
    fn test_is_not_retryable_413() {
        // 413 should not be retried - file too large is a client issue
        let error = anyhow::anyhow!("status 413: Payload Too Large");
        assert!(!is_retryable_error(&error));
    }
}
