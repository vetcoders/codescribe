//! HTTP client for communicating with CodeScribe Python backend (FastAPI + MLX Whisper)
//!
//! Features:
//! - Automatic server discovery across multiple ports
//! - Health checks with caching
//! - Multipart file upload for transcription
//! - Retry logic with exponential backoff
//! - Proper error handling and logging

use anyhow::{Context, Result};
use reqwest::multipart::{Form, Part};
use reqwest::{Client, StatusCode};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::OnceLock;
use std::time::Duration;
use tokio::fs::File;
use tokio::io::AsyncReadExt;
use tracing::{debug, info, warn};

/// Cached server URL after successful discovery
static SERVER_URL: OnceLock<String> = OnceLock::new();

/// Ports to probe for backend server (in order of preference)
/// 8238 is the default Python whisper_server port
const PROBE_PORTS: &[u16] = &[8238, 8237, 7237, 6237, 5237];

/// Maximum retry attempts for transient errors
const MAX_RETRIES: u32 = 3;

/// Initial backoff duration in milliseconds
const INITIAL_BACKOFF_MS: u64 = 100;

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

/// Format request structure
#[derive(Debug, Serialize)]
struct FormatRequest {
    text: String,
    assistive: bool,
}

/// Format response structure
#[derive(Debug, Deserialize)]
struct FormatResponse {
    formatted: String,
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
/// Tries ports in order: 8238, 8237, 7237, 6237, 5237
/// Returns the first responding server URL or None
///
/// Retries each port up to 3 times with 200ms delay to handle race conditions
/// where backend just started but isn't fully accepting connections yet.
async fn discover_server() -> Option<String> {
    let client = get_client();
    const RETRIES_PER_PORT: u32 = 3;
    const RETRY_DELAY_MS: u64 = 200;

    for port in PROBE_PORTS {
        for attempt in 1..=RETRIES_PER_PORT {
            let url = format!("http://127.0.0.1:{}/healthz", port);
            debug!(
                "Probing server at {} (attempt {}/{})",
                url, attempt, RETRIES_PER_PORT
            );

            match client.get(&url).send().await {
                Ok(response) if response.status().is_success() => {
                    if let Ok(health) = response.json::<HealthResponse>().await {
                        if health.ok {
                            let base_url = format!("http://127.0.0.1:{}", port);
                            info!(
                                "Discovered backend server at {} (attempt {})",
                                base_url, attempt
                            );
                            return Some(base_url);
                        }
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
/// - `Ok(true)` if backend responds with {"ok": true}
/// - `Ok(false)` if backend responds but is unhealthy
/// - `Err(_)` if cannot connect or parse response
pub async fn check_health() -> Result<bool> {
    let base_url = get_server_url().await?;
    let url = format!("{}/healthz", base_url);

    let response = get_client()
        .get(&url)
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

    Ok(health.ok)
}

/// Execute request with retry logic for transient errors
///
/// Retries on HTTP 502, 503, 504 with exponential backoff
async fn retry_request<F, Fut, T>(mut request_fn: F) -> Result<T>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<T>>,
{
    let mut attempts = 0;
    let mut backoff_ms = INITIAL_BACKOFF_MS;

    loop {
        attempts += 1;

        match request_fn().await {
            Ok(result) => return Ok(result),
            Err(e) => {
                // Check if error is retryable
                let is_retryable = e
                    .downcast_ref::<reqwest::Error>()
                    .and_then(|req_err| req_err.status())
                    .map(|status| {
                        matches!(
                            status,
                            StatusCode::BAD_GATEWAY
                                | StatusCode::SERVICE_UNAVAILABLE
                                | StatusCode::GATEWAY_TIMEOUT
                        )
                    })
                    .unwrap_or(false);

                if !is_retryable || attempts >= MAX_RETRIES {
                    return Err(e);
                }

                warn!(
                    "Request failed with retryable error, attempt {}/{}: {}",
                    attempts, MAX_RETRIES, e
                );

                // Exponential backoff
                tokio::time::sleep(Duration::from_millis(backoff_ms)).await;
                backoff_ms *= 2;
            }
        }
    }
}

/// Transcribe audio file using backend STT service
///
/// # Arguments
/// * `path` - Path to audio file (WAV, MP3, M4A, etc.)
/// * `language` - Optional language code (e.g., "pl", "en"). If None, auto-detect.
///
/// # Returns
/// Transcribed text or error
///
/// # Example
/// ```no_run
/// use std::path::Path;
///
/// let transcript = client::transcribe(Path::new("recording.wav"), Some("pl")).await?;
/// println!("Transcript: {}", transcript);
/// ```
pub async fn transcribe(path: &Path, language: Option<&str>) -> Result<String> {
    let base_url = get_server_url().await?;
    let url = format!("{}/transcribe", base_url);

    // Read file into memory (path comes from internal recorder, not user input)
    let mut file = File::open(path) // nosemgrep: tainted-path
        .await
        .context("Failed to open audio file")?;

    let mut buffer = Vec::new();
    file.read_to_end(&mut buffer)
        .await
        .context("Failed to read audio file")?;

    let filename = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("recording.wav");

    // Build multipart form
    // Backend expects "audio" field, not "file"
    let file_part = Part::bytes(buffer)
        .file_name(filename.to_string())
        .mime_str("audio/wav")
        .context("Failed to set MIME type")?;

    let mut form = Form::new().part("audio", file_part);

    if let Some(lang) = language {
        form = form.text("language", lang.to_string());
    }

    debug!("Sending transcription request for {}", filename);

    // Single request (Form cannot be cloned for retry)
    let response = get_client()
        .post(&url)
        .multipart(form)
        .send()
        .await
        .context("Failed to send transcription request")?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response
            .text()
            .await
            .unwrap_or_else(|_| "(no body)".to_string());
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

/// Format text using backend formatting service
///
/// # Arguments
/// * `text` - Raw text to format
/// * `assistive` - Enable assistive mode (more aggressive formatting, punctuation)
///
/// # Returns
/// Formatted text or error
///
/// # Example
/// ```no_run
/// let formatted = client::format_text("hello world how are you", true).await?;
/// println!("Formatted: {}", formatted);
/// ```
pub async fn format_text(text: &str, assistive: bool) -> Result<String> {
    let base_url = get_server_url().await?;
    let url = format!("{}/format", base_url);

    let request_body = FormatRequest {
        text: text.to_string(),
        assistive,
    };

    debug!("Sending format request, assistive={}", assistive);

    // Execute with retry logic
    let response = retry_request(|| async {
        let resp = get_client()
            .post(&url)
            .json(&request_body)
            .send()
            .await
            .context("Failed to send format request")?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp
                .text()
                .await
                .unwrap_or_else(|_| "(no body)".to_string());
            anyhow::bail!("Format request failed with status {}: {}", status, body);
        }

        Ok(resp)
    })
    .await?;

    let format_response: FormatResponse = response
        .json()
        .await
        .context("Failed to parse format response")?;

    info!("Text formatting successful");

    Ok(format_response.formatted)
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
}
