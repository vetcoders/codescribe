//! LibraxisAI Cloud STT Client
//!
//! Default transcription backend for CodeScribe using api.libraxis.cloud
//!
//! Supports:
//! - WebSocket streaming (real-time, bidirectional)
//! - NDJSON streaming (HTTP, simpler)
//!
//! WebSocket is preferred for live recording, NDJSON for file uploads.

use anyhow::{Context, Result};
use futures_util::{SinkExt, StreamExt};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::sync::OnceLock;
use std::time::{Duration, Instant};
use tokio_tungstenite::{connect_async, tungstenite::Message};
use tracing::{debug, error, info, warn};

/// LibraxisAI API base URL
pub const LIBRAXIS_API_HOST: &str = "api.libraxis.cloud";


/// WebSocket config message
#[derive(Serialize)]
struct WsConfig {
    #[serde(rename = "type")]
    msg_type: &'static str,
    language: String,
    api_key: String,
}

/// WebSocket end signal
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

/// Transcription result with metadata
#[derive(Debug, Clone)]
pub struct TranscriptionResult {
    pub text: String,
    pub duration_ms: u64,
    pub partial_count: u32,
    pub protocol: &'static str,
}

/// Get HTTP client singleton
fn get_client() -> &'static Client {
    static CLIENT: OnceLock<Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        Client::builder()
            .timeout(Duration::from_secs(300)) // 5 min for long audio
            .connect_timeout(Duration::from_secs(10))
            .pool_idle_timeout(Duration::from_secs(90))
            .build()
            .expect("Failed to create HTTP client")
    })
}

/// Get API key from env (required - no hardcoded default!)
pub fn get_api_key() -> Result<String> {
    std::env::var("LIBRAXIS_API_KEY")
        .context("LIBRAXIS_API_KEY env var not set - required for LibraxisAI API")
}

/// Callback for partial transcription updates (for UI feedback)
pub type PartialCallback = Box<dyn Fn(&str) + Send + Sync>;

/// Transcribe audio via WebSocket streaming
///
/// Best for: Real-time recording with live feedback
///
/// Protocol:
/// 1. Connect to wss://api.libraxis.cloud/v1/audio/transcribe
/// 2. Send config message with api_key and language
/// 3. Send audio as binary chunks
/// 4. Send end signal
/// 5. Receive partial/final responses
pub async fn transcribe_websocket(
    audio_data: Vec<u8>,
    language: &str,
    on_partial: Option<PartialCallback>,
) -> Result<TranscriptionResult> {
    let api_key = get_api_key()?;
    let url = format!("wss://{}/v1/audio/transcribe", LIBRAXIS_API_HOST);

    info!("[LibraxisAI WS] Connecting to {}", url);
    let start = Instant::now();

    let (mut ws, response) = connect_async(&url)
        .await
        .context("Failed to connect to LibraxisAI WebSocket")?;

    debug!(
        "[LibraxisAI WS] Connected in {:?}, status: {:?}",
        start.elapsed(),
        response.status()
    );

    // 1. Send config
    let config = WsConfig {
        msg_type: "config",
        language: language.to_string(),
        api_key,
    };
    ws.send(Message::Text(serde_json::to_string(&config)?))
        .await
        .context("Failed to send config")?;

    // 2. Send audio binary
    info!(
        "[LibraxisAI WS] Sending {} bytes ({:.2} MB)",
        audio_data.len(),
        audio_data.len() as f64 / 1_000_000.0
    );
    ws.send(Message::Binary(audio_data.into()))
        .await
        .context("Failed to send audio")?;

    // 3. Signal end
    let end = WsEnd { msg_type: "end" };
    ws.send(Message::Text(serde_json::to_string(&end)?))
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
                            debug!("[LibraxisAI WS] partial #{}: {} chars", partial_count, t.len());
                            if let Some(ref cb) = on_partial {
                                cb(t);
                            }
                        }
                    }
                    "final" => {
                        if let Some(t) = resp.text {
                            final_text = t;
                            info!(
                                "[LibraxisAI WS] Final received: {} chars, {} partials",
                                final_text.len(),
                                partial_count
                            );
                        }
                        break;
                    }
                    "error" => {
                        let err_msg = resp.error.unwrap_or_else(|| "Unknown error".to_string());
                        error!("[LibraxisAI WS] Error: {}", err_msg);
                        anyhow::bail!("LibraxisAI STT error: {}", err_msg);
                    }
                    other => {
                        warn!("[LibraxisAI WS] Unknown message type: {}", other);
                    }
                }
            }
            Message::Close(frame) => {
                debug!("[LibraxisAI WS] Connection closed: {:?}", frame);
                break;
            }
            Message::Ping(data) => {
                let _ = ws.send(Message::Pong(data)).await;
            }
            _ => {}
        }
    }

    let _ = ws.close(None).await;
    let duration_ms = start.elapsed().as_millis() as u64;

    info!(
        "[LibraxisAI WS] Complete in {}ms: {} chars",
        duration_ms,
        final_text.len()
    );

    if final_text.is_empty() {
        anyhow::bail!("No transcription received from LibraxisAI");
    }

    Ok(TranscriptionResult {
        text: final_text,
        duration_ms,
        partial_count,
        protocol: "websocket",
    })
}

/// Transcribe audio via NDJSON streaming HTTP
///
/// Best for: File uploads, simpler protocol, HTTP/2 compatible
///
/// Protocol:
/// 1. POST to https://api.libraxis.cloud/v1/audio/transcribe:stream
/// 2. Headers: x-api-key, Content-Type
/// 3. Query param: language
/// 4. Body: raw audio bytes
/// 5. Response: NDJSON stream
pub async fn transcribe_ndjson(
    audio_data: Vec<u8>,
    language: &str,
    content_type: &str,
    on_partial: Option<PartialCallback>,
) -> Result<TranscriptionResult> {
    let api_key = get_api_key()?;
    let url = format!("https://{}/v1/audio/transcribe:stream", LIBRAXIS_API_HOST);

    info!(
        "[LibraxisAI NDJSON] POST {} ({} bytes, {})",
        url,
        audio_data.len(),
        content_type
    );
    let start = Instant::now();

    let response = get_client()
        .post(&url)
        .header("x-api-key", &api_key)
        .header("Content-Type", content_type)
        .query(&[("language", language)])
        .body(audio_data)
        .send()
        .await
        .context("Failed to send NDJSON request")?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        error!("[LibraxisAI NDJSON] Error {}: {}", status, body);
        anyhow::bail!("LibraxisAI NDJSON request failed: {} - {}", status, body);
    }

    // Stream and parse NDJSON
    let mut stream = response.bytes_stream();
    let mut buffer = Vec::new();
    let mut final_text = String::new();
    let mut partial_count = 0u32;

    while let Some(chunk) = stream.next().await {
        let bytes = chunk.context("Failed to read NDJSON chunk")?;
        buffer.extend_from_slice(&bytes);

        // Parse complete lines
        while let Some(pos) = buffer.iter().position(|&b| b == b'\n') {
            let line: Vec<u8> = buffer.drain(..=pos).collect();
            let line_str = String::from_utf8_lossy(&line);
            let line_str = line_str.trim();

            if line_str.is_empty() {
                continue;
            }

            if let Ok(chunk) = serde_json::from_str::<NdjsonChunk>(line_str) {
                if let Some(err) = chunk.error {
                    error!("[LibraxisAI NDJSON] Error in stream: {}", err);
                    anyhow::bail!("LibraxisAI STT error: {}", err);
                }

                if let Some(text) = chunk.text {
                    if chunk.is_final.unwrap_or(false) {
                        final_text = text;
                        info!(
                            "[LibraxisAI NDJSON] Final: {} chars, {} partials",
                            final_text.len(),
                            partial_count
                        );
                    } else {
                        partial_count += 1;
                        debug!(
                            "[LibraxisAI NDJSON] partial #{}: {} chars",
                            partial_count,
                            text.len()
                        );
                        if let Some(ref cb) = on_partial {
                            cb(&text);
                        }
                    }
                }
            }
        }
    }

    let duration_ms = start.elapsed().as_millis() as u64;

    info!(
        "[LibraxisAI NDJSON] Complete in {}ms: {} chars",
        duration_ms,
        final_text.len()
    );

    if final_text.is_empty() {
        anyhow::bail!("No transcription received from LibraxisAI");
    }

    Ok(TranscriptionResult {
        text: final_text,
        duration_ms,
        partial_count,
        protocol: "ndjson",
    })
}

/// High-level transcribe function - auto-selects protocol
///
/// Uses WebSocket for real-time (when audio is small or streaming)
/// Uses NDJSON for large files (more reliable for big uploads)
pub async fn transcribe(
    audio_data: Vec<u8>,
    language: &str,
    content_type: &str,
    on_partial: Option<PartialCallback>,
) -> Result<TranscriptionResult> {
    // Use NDJSON for files > 5MB (more reliable for large uploads)
    // Use WebSocket for smaller files (faster feedback)
    let use_websocket = audio_data.len() < 5 * 1024 * 1024;

    if use_websocket {
        transcribe_websocket(audio_data, language, on_partial).await
    } else {
        transcribe_ndjson(audio_data, language, content_type, on_partial).await
    }
}

/// Check LibraxisAI API health
pub async fn health_check() -> Result<bool> {
    let url = format!("https://{}/healthz", LIBRAXIS_API_HOST);

    let response = get_client()
        .get(&url)
        .timeout(Duration::from_secs(5))
        .send()
        .await
        .context("Failed to reach LibraxisAI")?;

    Ok(response.status().is_success())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_health_check() {
        match health_check().await {
            Ok(healthy) => println!("LibraxisAI health: {}", healthy),
            Err(e) => println!("LibraxisAI not reachable: {}", e),
        }
    }

    #[test]
    fn test_get_api_key_requires_env() {
        // Without env var, should return error (no hardcoded default!)
        std::env::remove_var("LIBRAXIS_API_KEY");
        assert!(get_api_key().is_err());
    }
}
