//! HTTP client for communicating with CodeScribe Python backend (FastAPI + MLX Whisper)
//! or external WhisperX servers.
//!
//! Features:
//! - Automatic server discovery across multiple ports
//! - Support for external WhisperX servers (8443, 8444, 8445)
//! - Health checks with caching
//! - Multipart file upload for transcription
//! - Retry logic with exponential backoff
//! - Proper error handling and logging

use anyhow::{Context, Result};
use reqwest::multipart::{Form, Part};
use reqwest::Client;
use serde::Deserialize;
use std::path::Path;
use std::sync::OnceLock;
use std::time::Duration;
use tokio::fs::File;
use tokio::io::AsyncReadExt;
use tracing::{debug, error, info, warn};

/// Maximum retry attempts for transcription requests
const TRANSCRIPTION_MAX_RETRIES: u32 = 3;

/// Base delay between retry attempts (multiplied by attempt number)
const TRANSCRIPTION_RETRY_DELAY_MS: u64 = 500;

/// Cached server URL after successful discovery
static SERVER_URL: OnceLock<String> = OnceLock::new();

/// Ports to probe for backend server (in order of preference)
/// 8237 is the default Python backend port
const PROBE_PORTS: &[u16] = &[8237, 8238, 7237, 6237, 5237];

// Note: Retry constants and format_text moved to ai_formatting.rs module

/// Health check response structure
#[derive(Debug, Deserialize)]
struct HealthResponse {
    ok: bool,
}

/// Transcription response structure
#[derive(Debug, Deserialize)]
struct TranscribeResponse {
    text: String,
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

/// Discover backend server by probing known ports
///
/// Tries ports in order: 8237, 8238, 7237, 6237, 5237
/// Returns the first responding server URL or None
///
/// Retries each port up to 5 times with 500ms delay to handle race conditions
/// where backend just started but isn't fully accepting connections yet.
async fn discover_server() -> Option<String> {
    let client = get_client();
    const RETRIES_PER_PORT: u32 = 5;
    const RETRY_DELAY_MS: u64 = 500;

    for port in PROBE_PORTS {
        for attempt in 1..=RETRIES_PER_PORT {
            let url = format!("http://127.0.0.1:{}/healthz", port);
            debug!(
                "Probing server at {} (attempt {}/{})",
                url, attempt, RETRIES_PER_PORT
            );

            match client.get(&url).send().await {
                Ok(response) if response.status().is_success() => {
                    // Server is responding - accept even if model not loaded yet
                    // (ok=false means server running but model initializing)
                    if let Ok(health) = response.json::<HealthResponse>().await {
                        let base_url = format!("http://127.0.0.1:{}", port);
                        if health.ok {
                            info!(
                                "Discovered backend server at {} (fully ready, attempt {})",
                                base_url, attempt
                            );
                        } else {
                            info!(
                                "Discovered backend server at {} (model loading, attempt {})",
                                base_url, attempt
                            );
                        }
                        return Some(base_url);
                    }
                }
                Ok(response) => {
                    debug!(
                        "Port {} responded with status {} (attempt {})",
                        port,
                        response.status(),
                        attempt
                    );
                }
                Err(e) => {
                    debug!("Port {} not responding: {} (attempt {})", port, e, attempt);
                }
            }

            // Retry with delay (except on last attempt)
            if attempt < RETRIES_PER_PORT {
                tokio::time::sleep(Duration::from_millis(RETRY_DELAY_MS)).await;
            }
        }
    }

    warn!(
        "No backend server found on any probe port after {} retries per port",
        RETRIES_PER_PORT
    );
    None
}

/// Get base server URL (cached or discovered)
async fn get_server_url() -> Result<String> {
    // Check cache first
    if let Some(url) = SERVER_URL.get() {
        return Ok(url.clone());
    }

    // Discover and cache
    let url = discover_server()
        .await
        .context("Backend server not found - ensure Python backend is running")?;

    // Try to cache (ignore if already set by another thread)
    let _ = SERVER_URL.set(url.clone());

    Ok(url)
}

/// Check if backend is healthy
///
/// Returns:
/// - `Ok(true)` if backend responds with {"ok": true} (model loaded)
/// - `Ok(false)` if backend responds but model still loading
/// - `Err(_)` if cannot connect or parse response
pub async fn check_health() -> Result<bool> {
    let base_url = get_server_url().await?;
    let url = format!("{}/healthz", base_url);

    // Use short timeout for health check to avoid stale connections
    let response = get_client()
        .get(&url)
        .timeout(Duration::from_secs(2))
        .send()
        .await
        .context("Failed to send health check request")?;

    if !response.status().is_success() {
        return Ok(false);
    }

    let health: HealthResponse = response
        .json()
        .await
        .context("Failed to parse health check response")?;

    if !health.ok {
        info!("Backend responding but model still loading");
    }

    Ok(health.ok)
}

/// Transcribe audio file using backend STT service with retry logic
///
/// # Arguments
/// * `path` - Path to audio file (WAV, MP3, M4A, etc.)
/// * `language` - Optional language code (e.g., "pl", "en"). If None, auto-detect.
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
///
/// let transcript = client::transcribe(Path::new("recording.wav"), Some("pl")).await?;
/// println!("Transcript: {}", transcript);
/// ```
pub async fn transcribe(path: &Path, language: Option<&str>) -> Result<String> {
    info!("transcribe() START for path: {:?}", path);

    // Note: Path validation is handled at the controller level via ValidatedAudioPath
    // before this function is called. See controller.rs::process_recording()

    // Check if external STT endpoint is configured via STT_ENDPOINT
    // This should be a full URL (e.g., https://api.libraxis.cloud/stt/v1/transcribe)
    if let Ok(endpoint_url) = std::env::var("STT_ENDPOINT") {
        let api_key = std::env::var("STT_API_KEY")
            .context("STT_API_KEY required when STT_ENDPOINT is set")?;
        return transcribe_external(path, language, &endpoint_url, &api_key).await;
    }

    // Local Python backend uses /transcribe with "audio" field
    info!("Getting server URL...");
    let base_url = get_server_url().await?;
    info!("Server URL: {}", base_url);
    let url = format!("{}/transcribe", base_url);
    let field_name = "audio";

    // Read file into memory
    info!("Opening file: {:?}", path);
    let mut file = File::open(path)
        .await
        .context("Failed to open audio file")?;
    info!("File opened successfully");

    let mut buffer = Vec::new();
    file.read_to_end(&mut buffer)
        .await
        .context("Failed to read audio file")?;
    info!("File read: {} bytes", buffer.len());

    // Pre-flight validation to catch issues before sending
    if let Err(validation_error) = validate_audio(&buffer) {
        error!("Audio validation failed: {}", validation_error);
        // Update tray to show error state
        let _ = crate::tray::update_tray_status(crate::tray::TrayStatus::Error);
        anyhow::bail!("Audio validation failed: {}", validation_error);
    }
    info!("Audio validation passed");

    let filename = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("recording.wav");

    info!(
        "Sending transcription request to {} for {} ({} bytes)",
        url,
        filename,
        buffer.len()
    );

    // Retry loop - we recreate the Form for each attempt since it cannot be cloned
    let mut last_error: Option<anyhow::Error> = None;

    for attempt in 1..=TRANSCRIPTION_MAX_RETRIES {
        // Build multipart form fresh for each attempt
        let file_part = Part::bytes(buffer.clone())
            .file_name(filename.to_string())
            .mime_str("audio/wav")
            .context("Failed to set MIME type")?;

        let mut form = Form::new().part(field_name, file_part);

        if let Some(lang) = language {
            form = form.text("language", lang.to_string());
        }

        debug!(
            "Transcription attempt {}/{} for {}",
            attempt, TRANSCRIPTION_MAX_RETRIES, filename
        );

        match transcribe_request(&url, form).await {
            Ok(text) => {
                if attempt > 1 {
                    info!(
                        "Transcription succeeded on attempt {}/{}",
                        attempt, TRANSCRIPTION_MAX_RETRIES
                    );
                }
                return Ok(text);
            }
            Err(e) => {
                let is_retryable = is_retryable_error(&e);
                warn!(
                    "Transcription attempt {}/{} failed: {} (retryable: {})",
                    attempt, TRANSCRIPTION_MAX_RETRIES, e, is_retryable
                );

                if attempt < TRANSCRIPTION_MAX_RETRIES && is_retryable {
                    // Update tray with retry status
                    let _ = crate::tray::update_tray_status(crate::tray::TrayStatus::Thinking);

                    // Exponential backoff delay
                    let delay_ms = TRANSCRIPTION_RETRY_DELAY_MS * attempt as u64;
                    info!(
                        "Retrying transcription in {}ms (attempt {}/{})",
                        delay_ms,
                        attempt + 1,
                        TRANSCRIPTION_MAX_RETRIES
                    );
                    tokio::time::sleep(Duration::from_millis(delay_ms)).await;
                }

                last_error = Some(e);
            }
        }
    }

    // All retries exhausted
    let _ = crate::tray::update_tray_status(crate::tray::TrayStatus::Error);
    Err(last_error.unwrap_or_else(|| anyhow::anyhow!("Transcription failed after all retries")))
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

/// Send a single transcription request (used by retry loop)
async fn transcribe_request(url: &str, form: Form) -> Result<String> {
    let response = match get_client().post(url).multipart(form).send().await {
        Ok(r) => r,
        Err(e) => {
            error!("HTTP request failed: {:?}", e);
            anyhow::bail!("Failed to send transcription request: {}", e);
        }
    };

    let status = response.status();
    debug!("Transcription response status: {}", status);

    if !status.is_success() {
        let body = response
            .text()
            .await
            .unwrap_or_else(|_| "(no body)".to_string());
        error!("Transcription failed - status: {}, body: {}", status, body);
        anyhow::bail!("Transcription failed with status {}: {}", status, body);
    }

    let transcribe_response: TranscribeResponse = response
        .json()
        .await
        .context("Failed to parse transcription response")?;

    info!(
        "Transcription successful, length: {} chars",
        transcribe_response.text.len()
    );

    Ok(transcribe_response.text)
}

/// Transcribe audio using external STT API (OpenAI-compatible endpoint)
///
/// Uses the full endpoint URL directly with x-api-key header.
/// Includes pre-flight validation and retry logic with exponential backoff.
///
/// # Arguments
/// * `path` - Path to audio file
/// * `language` - Optional language code
/// * `endpoint_url` - Full URL to the transcription endpoint (not base URL)
/// * `api_key` - API key for authentication
async fn transcribe_external(
    path: &Path,
    language: Option<&str>,
    endpoint_url: &str,
    api_key: &str,
) -> Result<String> {
    info!("Using external STT endpoint: {}", endpoint_url);

    // Path is already validated by caller (transcribe function validates at entry point)
    // Read file into memory
    let mut file = File::open(path)
        .await
        .context("Failed to open audio file")?;

    let mut buffer = Vec::new();
    file.read_to_end(&mut buffer)
        .await
        .context("Failed to read audio file")?;

    // Pre-flight validation
    if let Err(validation_error) = validate_audio(&buffer) {
        error!("Audio validation failed: {}", validation_error);
        let _ = crate::tray::update_tray_status(crate::tray::TrayStatus::Error);
        anyhow::bail!("Audio validation failed: {}", validation_error);
    }

    let filename = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("recording.wav");

    info!(
        "Sending transcription request to {} for {} ({} bytes)",
        endpoint_url,
        filename,
        buffer.len()
    );

    // Retry loop
    let mut last_error: Option<anyhow::Error> = None;

    for attempt in 1..=TRANSCRIPTION_MAX_RETRIES {
        // Build multipart form fresh for each attempt (OpenAI-compatible format)
        let file_part = Part::bytes(buffer.clone())
            .file_name(filename.to_string())
            .mime_str("audio/wav")
            .context("Failed to set MIME type")?;

        let mut form = Form::new()
            .part("file", file_part)
            .text("model", "whisper-large-v3");

        if let Some(lang) = language {
            form = form.text("language", lang.to_string());
        }

        debug!(
            "External STT transcription attempt {}/{} for {}",
            attempt, TRANSCRIPTION_MAX_RETRIES, filename
        );

        match transcribe_external_request(endpoint_url, api_key, form).await {
            Ok(text) => {
                if attempt > 1 {
                    info!(
                        "External STT transcription succeeded on attempt {}/{}",
                        attempt, TRANSCRIPTION_MAX_RETRIES
                    );
                }
                return Ok(text);
            }
            Err(e) => {
                let is_retryable = is_retryable_error(&e);
                warn!(
                    "External STT transcription attempt {}/{} failed: {} (retryable: {})",
                    attempt, TRANSCRIPTION_MAX_RETRIES, e, is_retryable
                );

                if attempt < TRANSCRIPTION_MAX_RETRIES && is_retryable {
                    let _ = crate::tray::update_tray_status(crate::tray::TrayStatus::Thinking);

                    let delay_ms = TRANSCRIPTION_RETRY_DELAY_MS * attempt as u64;
                    info!(
                        "Retrying external STT transcription in {}ms (attempt {}/{})",
                        delay_ms,
                        attempt + 1,
                        TRANSCRIPTION_MAX_RETRIES
                    );
                    tokio::time::sleep(Duration::from_millis(delay_ms)).await;
                }

                last_error = Some(e);
            }
        }
    }

    let _ = crate::tray::update_tray_status(crate::tray::TrayStatus::Error);
    Err(last_error
        .unwrap_or_else(|| anyhow::anyhow!("External STT transcription failed after all retries")))
}

/// Send a single external STT transcription request (used by retry loop)
async fn transcribe_external_request(url: &str, api_key: &str, form: Form) -> Result<String> {
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

// Note: format_text moved to ai_formatting.rs module for OpenAI/Libraxis support

/// Model set response structure
#[derive(Debug, Deserialize)]
struct ModelSetResponse {
    ok: bool,
    #[serde(default)]
    variant: Option<String>,
    #[serde(default)]
    path: Option<String>,
    #[serde(default)]
    error: Option<String>,
}

/// Get current Whisper model variant from backend
pub async fn get_current_model() -> Result<String> {
    let base_url = get_server_url().await?;
    let url = format!("{}/model", base_url);

    let response = get_client()
        .get(&url)
        .timeout(Duration::from_secs(2))
        .send()
        .await
        .context("Failed to get current model")?;

    #[derive(Deserialize)]
    struct ModelInfo {
        variant: String,
    }

    let info: ModelInfo = response
        .json()
        .await
        .context("Failed to parse model info")?;
    Ok(info.variant)
}

/// Set Whisper model variant
///
/// # Arguments
/// * `variant` - Model variant (small, medium, large-v3, large-v3-turbo)
///
/// # Returns
/// Ok(()) on success, error if model not found or switch failed
pub async fn set_whisper_model(variant: &str) -> Result<()> {
    let base_url = get_server_url().await?;
    let url = format!("{}/model/set", base_url);

    debug!("Setting Whisper model to: {}", variant);

    let response = get_client()
        .post(&url)
        .json(&serde_json::json!({ "variant": variant }))
        .send()
        .await
        .context("Failed to send model set request")?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response
            .text()
            .await
            .unwrap_or_else(|_| "(no body)".to_string());
        anyhow::bail!("Model set request failed with status {}: {}", status, body);
    }

    let set_response: ModelSetResponse = response
        .json()
        .await
        .context("Failed to parse model set response")?;

    if !set_response.ok {
        anyhow::bail!(
            "Failed to set model: {}",
            set_response
                .error
                .unwrap_or_else(|| "unknown error".to_string())
        );
    }

    info!(
        "Whisper model switched to: {} at {:?}",
        set_response.variant.unwrap_or_default(),
        set_response.path
    );

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_health_check() {
        // This test requires backend to be running
        match check_health().await {
            Ok(healthy) => println!("Backend health: {}", healthy),
            Err(e) => println!("Backend not available: {}", e),
        }
    }

    #[tokio::test]
    async fn test_server_discovery() {
        match discover_server().await {
            Some(url) => println!("Discovered server: {}", url),
            None => println!("No server found"),
        }
    }

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
        std::env::set_var("BACKEND_MAX_UPLOAD_MB", "1");
        let result = validate_audio(&vec![0u8; 2 * 1024 * 1024]); // 2MB > 1MB limit
        std::env::remove_var("BACKEND_MAX_UPLOAD_MB");

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
