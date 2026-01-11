//! E2E tests for LibraxisAI cloud transcription
//!
//! Tests WebSocket and NDJSON streaming endpoints at api.libraxis.cloud
//! Run with: cargo test --test e2e_libraxis -- --nocapture
//!
//! Required env vars (to actually run the tests):
//!   LIBRAXIS_API_KEY - API key for authentication
//!   TEST_AUDIO_FILE  - Path to test audio file (mp3/wav/webm)
//!
//! If the required env vars are not set, the tests will be skipped.

use anyhow::{Context, Result};
use futures_util::{SinkExt, StreamExt};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::time::{Duration, Instant};
use tokio_tungstenite::{connect_async, tungstenite::Message};

/// LibraxisAI API base URL
const LIBRAXIS_API_BASE: &str = "api.libraxis.cloud";


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

/// AI formatting request (OpenAI-compatible chat completion)
#[derive(Serialize)]
struct FormatRequest {
    model: String,
    messages: Vec<ChatMessage>,
    max_tokens: u32,
    temperature: f32,
}

#[derive(Serialize)]
struct ChatMessage {
    role: String,
    content: String,
}

/// AI formatting response
#[derive(Deserialize, Debug)]
struct FormatResponse {
    choices: Vec<Choice>,
}

#[derive(Deserialize, Debug)]
struct Choice {
    message: MessageContent,
}

#[derive(Deserialize, Debug)]
struct MessageContent {
    content: String,
}

/// Get API key from environment (returns None when not configured).
fn get_api_key() -> Option<String> {
    std::env::var("LIBRAXIS_API_KEY")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// Get test audio file path (returns None when not configured or missing).
fn get_test_audio_path() -> Option<PathBuf> {
    let path = std::env::var("TEST_AUDIO_FILE").ok().map(PathBuf::from)?;
    if path.exists() {
        Some(path)
    } else {
        None
    }
}

/// Load test audio file
async fn load_test_audio(path: &PathBuf) -> Result<Vec<u8>> {
    println!("Loading test audio from: {:?}", path);

    let data = tokio::fs::read(path)
        .await
        .with_context(|| format!("Failed to read test audio file: {:?}", path))?;

    println!("Loaded {} bytes ({:.2} MB)", data.len(), data.len() as f64 / 1_000_000.0);
    Ok(data)
}

// ============================================================================
// WebSocket STT Tests
// ============================================================================

/// Transcribe audio via WebSocket streaming
///
/// Protocol:
/// 1. Connect to wss://api.libraxis.cloud/v1/audio/transcribe
/// 2. Send config message with api_key and language
/// 3. Send audio as binary
/// 4. Send end signal
/// 5. Receive partial/final responses
pub async fn transcribe_websocket(
    audio_data: Vec<u8>,
    api_key: &str,
    language: &str,
) -> Result<String> {
    let url = format!("wss://{}/v1/audio/transcribe", LIBRAXIS_API_BASE);
    println!("\n[WebSocket] Connecting to: {}", url);

    let start = Instant::now();
    let (mut ws, response) = connect_async(&url)
        .await
        .context("Failed to connect to WebSocket")?;

    println!(
        "[WebSocket] Connected in {:?}, status: {:?}",
        start.elapsed(),
        response.status()
    );

    // 1. Send config
    let config = WsConfig {
        msg_type: "config",
        language: language.to_string(),
        api_key: api_key.to_string(),
    };
    let config_json = serde_json::to_string(&config)?;
    println!("[WebSocket] Sending config: language={}", language);
    ws.send(Message::Text(config_json.into())).await?;

    // 2. Send audio binary
    println!(
        "[WebSocket] Sending audio: {} bytes ({:.2} MB)",
        audio_data.len(),
        audio_data.len() as f64 / 1_000_000.0
    );
    let send_start = Instant::now();
    ws.send(Message::Binary(audio_data.into())).await?;
    println!("[WebSocket] Audio sent in {:?}", send_start.elapsed());

    // 3. Signal end
    let end = WsEnd { msg_type: "end" };
    ws.send(Message::Text(serde_json::to_string(&end)?.into())).await?;
    println!("[WebSocket] End signal sent, waiting for transcription...");

    // 4. Collect responses
    let mut final_text = String::new();
    let mut partial_count = 0;
    let transcribe_start = Instant::now();

    while let Some(msg) = ws.next().await {
        match msg? {
            Message::Text(txt) => {
                let resp: WsResponse = serde_json::from_str(&txt)
                    .with_context(|| format!("Failed to parse response: {}", txt))?;

                match resp.msg_type.as_str() {
                    "partial" => {
                        partial_count += 1;
                        if let Some(t) = &resp.text {
                            // Show first few words of partial
                            let preview: String = t.chars().take(50).collect();
                            if partial_count <= 5 || partial_count % 10 == 0 {
                                println!("[WebSocket] partial #{}: {}...", partial_count, preview);
                            }
                        }
                    }
                    "final" => {
                        if let Some(t) = resp.text {
                            final_text = t;
                            println!(
                                "\n[WebSocket] FINAL received in {:?}",
                                transcribe_start.elapsed()
                            );
                            println!(
                                "[WebSocket] Total partials: {}, Final length: {} chars",
                                partial_count,
                                final_text.len()
                            );
                        }
                        break;
                    }
                    "error" => {
                        let err_msg = resp.error.unwrap_or_else(|| "Unknown error".to_string());
                        anyhow::bail!("[WebSocket] STT error: {}", err_msg);
                    }
                    other => {
                        println!("[WebSocket] Unknown message type: {}", other);
                    }
                }
            }
            Message::Close(frame) => {
                println!("[WebSocket] Connection closed: {:?}", frame);
                break;
            }
            Message::Ping(data) => {
                ws.send(Message::Pong(data)).await?;
            }
            _ => {}
        }
    }

    let _ = ws.close(None).await;
    println!(
        "[WebSocket] Total time: {:?}\n",
        start.elapsed()
    );

    if final_text.is_empty() {
        anyhow::bail!("No final transcription received");
    }

    Ok(final_text)
}

// ============================================================================
// NDJSON Streaming Tests
// ============================================================================

/// Transcribe audio via NDJSON streaming HTTP
///
/// Protocol:
/// 1. POST to https://api.libraxis.cloud/v1/audio/transcribe:stream
/// 2. Headers: x-api-key, Content-Type: audio/mp3 (or wav, webm)
/// 3. Query param: language
/// 4. Body: raw audio bytes
/// 5. Response: NDJSON stream with partial/final chunks
pub async fn transcribe_ndjson(
    audio_data: Vec<u8>,
    api_key: &str,
    language: &str,
) -> Result<String> {
    let url = format!(
        "https://{}/v1/audio/transcribe:stream",
        LIBRAXIS_API_BASE
    );
    println!("\n[NDJSON] POST to: {}", url);

    let client = Client::builder()
        .timeout(Duration::from_secs(300)) // 5 min timeout for long audio
        .build()?;

    let start = Instant::now();
    let response = client
        .post(&url)
        .header("x-api-key", api_key)
        .header("Content-Type", "audio/mp3")
        .query(&[("language", language)])
        .body(audio_data.clone())
        .send()
        .await
        .context("Failed to send NDJSON request")?;

    println!(
        "[NDJSON] Response status: {} in {:?}",
        response.status(),
        start.elapsed()
    );

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        anyhow::bail!("[NDJSON] Request failed with {}: {}", status, body);
    }

    // Stream and parse NDJSON
    let mut stream = response.bytes_stream();
    let mut buffer = Vec::new();
    let mut final_text = String::new();
    let mut partial_count = 0;

    println!("[NDJSON] Streaming response...");
    let stream_start = Instant::now();

    while let Some(chunk) = stream.next().await {
        let bytes = chunk.context("Failed to read response chunk")?;
        buffer.extend_from_slice(&bytes);

        // Parse complete NDJSON lines
        while let Some(pos) = buffer.iter().position(|&b| b == b'\n') {
            let line: Vec<u8> = buffer.drain(..=pos).collect();
            let line_str = String::from_utf8_lossy(&line);
            let line_str = line_str.trim();

            if line_str.is_empty() {
                continue;
            }

            if let Ok(chunk) = serde_json::from_str::<NdjsonChunk>(line_str) {
                if let Some(err) = chunk.error {
                    anyhow::bail!("[NDJSON] STT error: {}", err);
                }

                if let Some(text) = chunk.text {
                    if chunk.is_final.unwrap_or(false) {
                        final_text = text;
                        println!(
                            "\n[NDJSON] FINAL received in {:?}",
                            stream_start.elapsed()
                        );
                        println!(
                            "[NDJSON] Total partials: {}, Final length: {} chars",
                            partial_count,
                            final_text.len()
                        );
                    } else {
                        partial_count += 1;
                        if partial_count <= 5 || partial_count % 10 == 0 {
                            let preview: String = text.chars().take(50).collect();
                            println!("[NDJSON] partial #{}: {}...", partial_count, preview);
                        }
                    }
                }
            }
        }
    }

    println!("[NDJSON] Total time: {:?}\n", start.elapsed());

    if final_text.is_empty() {
        anyhow::bail!("No final transcription received");
    }

    Ok(final_text)
}

// ============================================================================
// AI Formatting Tests
// ============================================================================

/// Format transcription using LibraxisAI LLM
///
/// Uses chat completion API to clean up and format raw transcription
pub async fn format_with_ai(transcript: &str, api_key: &str) -> Result<String> {
    let url = format!("https://{}/v1/chat/completions", LIBRAXIS_API_BASE);
    println!("\n[AI Format] POST to: {}", url);

    let client = Client::builder()
        .timeout(Duration::from_secs(120))
        .build()?;

    let system_prompt = r#"Jesteś asystentem formatującym transkrypcje. Twoim zadaniem jest:
1. Poprawić interpunkcję i kapitalizację
2. Usunąć powtórzenia i wypełniacze (yyy, eee, no, znaczy)
3. Zachować oryginalny sens i styl wypowiedzi
4. Nie dodawać własnych treści

Zwróć tylko sformatowany tekst, bez komentarzy."#;

    let request = FormatRequest {
        model: "chat".to_string(), // LibraxisAI model alias
        messages: vec![
            ChatMessage {
                role: "system".to_string(),
                content: system_prompt.to_string(),
            },
            ChatMessage {
                role: "user".to_string(),
                content: format!("Sformatuj następującą transkrypcję:\n\n{}", transcript),
            },
        ],
        max_tokens: 4096,
        temperature: 0.3,
    };

    let start = Instant::now();
    let response = client
        .post(&url)
        .header("x-api-key", api_key)
        .header("Content-Type", "application/json")
        .json(&request)
        .send()
        .await
        .context("Failed to send AI format request")?;

    println!(
        "[AI Format] Response status: {} in {:?}",
        response.status(),
        start.elapsed()
    );

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        anyhow::bail!("[AI Format] Request failed with {}: {}", status, body);
    }

    let format_response: FormatResponse = response
        .json()
        .await
        .context("Failed to parse AI format response")?;

    let formatted = format_response
        .choices
        .first()
        .map(|c| c.message.content.clone())
        .unwrap_or_default();

    println!(
        "[AI Format] Formatted text: {} chars (original: {} chars)",
        formatted.len(),
        transcript.len()
    );

    Ok(formatted)
}

// ============================================================================
// Test Functions
// ============================================================================

#[tokio::test]
async fn test_websocket_transcription() -> Result<()> {
    println!("\n{}", "=".repeat(60));
    println!("TEST: WebSocket STT Transcription");
    println!("{}", "=".repeat(60));

    let Some(api_key) = get_api_key() else {
        println!("Skipping: LIBRAXIS_API_KEY not set");
        return Ok(());
    };
    let Some(audio_path) = get_test_audio_path() else {
        println!("Skipping: TEST_AUDIO_FILE not set or file missing");
        return Ok(());
    };
    let audio_data = load_test_audio(&audio_path).await?;

    let transcript = transcribe_websocket(audio_data, &api_key, "pl").await?;

    println!("\n--- TRANSCRIPT (WebSocket) ---");
    println!("{}", transcript);
    println!("--- END TRANSCRIPT ---\n");

    assert!(!transcript.is_empty(), "Transcript should not be empty");
    assert!(
        transcript.len() > 50,
        "Transcript seems too short: {} chars",
        transcript.len()
    );

    Ok(())
}

#[tokio::test]
async fn test_ndjson_transcription() -> Result<()> {
    println!("\n{}", "=".repeat(60));
    println!("TEST: NDJSON Streaming Transcription");
    println!("{}", "=".repeat(60));

    let Some(api_key) = get_api_key() else {
        println!("Skipping: LIBRAXIS_API_KEY not set");
        return Ok(());
    };
    let Some(audio_path) = get_test_audio_path() else {
        println!("Skipping: TEST_AUDIO_FILE not set or file missing");
        return Ok(());
    };
    let audio_data = load_test_audio(&audio_path).await?;

    let transcript = transcribe_ndjson(audio_data, &api_key, "pl").await?;

    println!("\n--- TRANSCRIPT (NDJSON) ---");
    println!("{}", transcript);
    println!("--- END TRANSCRIPT ---\n");

    assert!(!transcript.is_empty(), "Transcript should not be empty");
    assert!(
        transcript.len() > 50,
        "Transcript seems too short: {} chars",
        transcript.len()
    );

    Ok(())
}

#[tokio::test]
async fn test_ai_formatting() -> Result<()> {
    println!("\n{}", "=".repeat(60));
    println!("TEST: AI Formatting");
    println!("{}", "=".repeat(60));

    let Some(api_key) = get_api_key() else {
        println!("Skipping: LIBRAXIS_API_KEY not set");
        return Ok(());
    };

    // Test with sample unformatted text
    let raw_transcript = r#"no więc eee znaczy pies ma problem z eee z oddychaniem tak
no i widać że eee ma problemy z chodzeniem znaczy kuleje na tylną łapę
i eee no właściciel mówi że to od jakiegoś tygodnia tak się dzieje"#;

    let formatted = format_with_ai(raw_transcript, &api_key).await?;

    println!("\n--- ORIGINAL ---");
    println!("{}", raw_transcript);
    println!("\n--- FORMATTED ---");
    println!("{}", formatted);
    println!("--- END ---\n");

    assert!(!formatted.is_empty(), "Formatted text should not be empty");
    // Check that some filler words were removed
    assert!(
        formatted.matches("eee").count() < raw_transcript.matches("eee").count(),
        "Formatting should reduce filler words"
    );

    Ok(())
}

#[tokio::test]
async fn test_full_e2e_pipeline() -> Result<()> {
    println!("\n{}", "=".repeat(60));
    println!("TEST: Full E2E Pipeline (Transcription + AI Formatting)");
    println!("{}", "=".repeat(60));

    let Some(api_key) = get_api_key() else {
        println!("Skipping: LIBRAXIS_API_KEY not set");
        return Ok(());
    };
    let Some(audio_path) = get_test_audio_path() else {
        println!("Skipping: TEST_AUDIO_FILE not set or file missing");
        return Ok(());
    };
    let audio_data = load_test_audio(&audio_path).await?;

    // Step 1: Transcribe via WebSocket (faster for real-time)
    println!("\n[Step 1] Transcribing audio...");
    let start = Instant::now();
    let raw_transcript = transcribe_websocket(audio_data, &api_key, "pl").await?;
    let transcribe_time = start.elapsed();

    println!("\n--- RAW TRANSCRIPT ---");
    // Print first 500 chars
    let preview: String = raw_transcript.chars().take(500).collect();
    println!("{}...", preview);
    println!("[{} chars total]", raw_transcript.len());

    // Step 2: Format with AI
    println!("\n[Step 2] Formatting with AI...");
    let start = Instant::now();
    let formatted = format_with_ai(&raw_transcript, &api_key).await?;
    let format_time = start.elapsed();

    println!("\n--- FORMATTED TRANSCRIPT ---");
    let preview: String = formatted.chars().take(500).collect();
    println!("{}...", preview);
    println!("[{} chars total]", formatted.len());

    // Summary
    println!("\n{}", "=".repeat(60));
    println!("E2E PIPELINE SUMMARY");
    println!("{}", "=".repeat(60));
    println!("Transcription time: {:?}", transcribe_time);
    println!("AI formatting time: {:?}", format_time);
    println!("Total pipeline time: {:?}", transcribe_time + format_time);
    println!("Raw transcript: {} chars", raw_transcript.len());
    println!("Formatted: {} chars", formatted.len());
    println!(
        "Compression ratio: {:.1}%",
        (formatted.len() as f64 / raw_transcript.len() as f64) * 100.0
    );
    println!("{}\n", "=".repeat(60));

    assert!(!formatted.is_empty(), "Formatted text should not be empty");

    Ok(())
}

/// Compare WebSocket vs NDJSON performance
#[tokio::test]
async fn test_compare_protocols() -> Result<()> {
    println!("\n{}", "=".repeat(60));
    println!("TEST: Protocol Comparison (WebSocket vs NDJSON)");
    println!("{}", "=".repeat(60));

    let Some(api_key) = get_api_key() else {
        println!("Skipping: LIBRAXIS_API_KEY not set");
        return Ok(());
    };
    let Some(audio_path) = get_test_audio_path() else {
        println!("Skipping: TEST_AUDIO_FILE not set or file missing");
        return Ok(());
    };
    let audio_data = load_test_audio(&audio_path).await?;

    // Test WebSocket
    println!("\n[1/2] Testing WebSocket...");
    let ws_start = Instant::now();
    let ws_transcript = transcribe_websocket(audio_data.clone(), &api_key, "pl").await?;
    let ws_time = ws_start.elapsed();

    // Test NDJSON
    println!("\n[2/2] Testing NDJSON...");
    let ndjson_start = Instant::now();
    let ndjson_transcript = transcribe_ndjson(audio_data, &api_key, "pl").await?;
    let ndjson_time = ndjson_start.elapsed();

    // Compare
    println!("\n{}", "=".repeat(60));
    println!("PROTOCOL COMPARISON RESULTS");
    println!("{}", "=".repeat(60));
    println!("WebSocket: {:?} ({} chars)", ws_time, ws_transcript.len());
    println!("NDJSON:    {:?} ({} chars)", ndjson_time, ndjson_transcript.len());

    let faster = if ws_time < ndjson_time {
        "WebSocket"
    } else {
        "NDJSON"
    };
    let diff = ws_time.abs_diff(ndjson_time);
    println!("{} is faster by {:?}", faster, diff);

    // Check transcript similarity (should be nearly identical)
    let similarity = if ws_transcript == ndjson_transcript {
        100.0
    } else {
        let common_prefix = ws_transcript
            .chars()
            .zip(ndjson_transcript.chars())
            .take_while(|(a, b)| a == b)
            .count();
        (common_prefix as f64 / ws_transcript.len().max(ndjson_transcript.len()) as f64) * 100.0
    };
    println!("Transcript similarity: {:.1}%", similarity);
    println!("{}\n", "=".repeat(60));

    Ok(())
}
