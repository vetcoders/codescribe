//! E2E tests for SSE streaming with real OpenAI/Libraxis Responses API
//!
//! Tests both FORMATTING and ASSISTIVE modes with real API calls.
//! Config loaded from: ~/.codescribe/.env
//!
//! Run: make test-sse
//!
//! Created by Vetcoders (c)2026

use codescribe::{ai_formatting, config::Config};
use serial_test::serial;
use tracing_subscriber::EnvFilter;

/// Load environment from ~/.codescribe/.env if not already set
fn load_codescribe_env() {
    let Ok(home) = std::env::var("HOME") else {
        eprintln!("HOME not set");
        return;
    };
    let env_path = std::path::PathBuf::from(home).join(".codescribe/.env");

    if !env_path.exists() {
        eprintln!("Config not found: {:?}", env_path);
        return;
    }

    if let Ok(content) = std::fs::read_to_string(&env_path) {
        for line in content.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if let Some((key, value)) = line.split_once('=') {
                let key = key.trim();
                let value = value.trim();
                // Only set if not already present
                if std::env::var(key).is_err() {
                    unsafe { std::env::set_var(key, value) };
                }
            }
        }
        eprintln!("Loaded env from: {:?}", env_path);
    }
}

/// Test real SSE streaming for FORMATTING mode
/// Run with: make test-sse
#[tokio::test]
#[serial]
#[ignore = "Requires real API key - run with make test-sse"]
async fn e2e_sse_streaming_real_formatting() {
    load_codescribe_env();

    let runtime_settings = Config::load_runtime_snapshot().expect("seal runtime settings");
    let lane = runtime_settings.llm_lanes().formatting();
    if !lane.available() {
        eprintln!("Skipping: formatting lane unavailable");
        return;
    }

    unsafe {
        std::env::set_var("CODESCRIBE_AI_MAX_RETRIES", "0");
    }

    eprintln!("=== FORMATTING MODE TEST ===");
    eprintln!("Endpoint: {}", lane.endpoint());
    eprintln!("Model: {}", lane.model());

    let input = "cześć jestem Vetcoders i testuję formatowanie tekstu bez interpunkcji";
    eprintln!("Input:  {}", input);

    let result = ai_formatting::format_text(
        input,
        Some("pl"),
        false,
        runtime_settings.llm_lanes().formatting(),
    )
    .await;
    eprintln!("Output: {}", result);

    assert!(!result.is_empty(), "Should return formatted text");
    assert!(
        result != input,
        "Formatted text should differ from raw input"
    );
}

/// Test real SSE streaming for ASSISTIVE mode
/// Run with: make test-sse
#[tokio::test]
#[serial]
#[ignore = "Requires real API key - run with make test-sse"]
async fn e2e_sse_streaming_real_assistive() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::new("codescribe_core::llm=debug"))
        .with_writer(std::io::stderr)
        .try_init();
    load_codescribe_env();

    let runtime_settings = Config::load_runtime_snapshot().expect("seal runtime settings");
    let lane = runtime_settings.llm_lanes().assistive();
    if !lane.available() {
        eprintln!("Skipping: assistive lane unavailable");
        return;
    }

    unsafe {
        std::env::set_var("CODESCRIBE_AI_MAX_RETRIES", "0");
    }

    eprintln!("=== ASSISTIVE MODE TEST ===");
    eprintln!("Endpoint: {}", lane.endpoint());
    eprintln!("Model: {}", lane.model());

    let input = "jak napisać funkcję w Rust która odwraca string";
    eprintln!("Input:  {}", input);

    let fmt_result = ai_formatting::format_text_with_status(
        input,
        Some("pl"),
        true,
        runtime_settings.llm_lanes().assistive(),
        None,
    )
    .await;
    eprintln!("Output: {}", fmt_result.text);
    eprintln!("Status: {:?}", fmt_result.status);

    let result = fmt_result.text;
    assert!(!result.is_empty(), "Should return AI response");
    assert!(
        result.len() > input.len(),
        "Assistive response should be longer than input question (status={:?})",
        fmt_result.status
    );
}

/// Test SSE streaming with KURIER mode (pass-through dictation)
#[tokio::test]
#[serial]
#[ignore = "Requires real API key - run with make test-sse"]
async fn e2e_sse_streaming_kurier_mode() {
    load_codescribe_env();

    let runtime_settings = Config::load_runtime_snapshot().expect("seal runtime settings");
    let lane = runtime_settings.llm_lanes().assistive();
    if !lane.available() {
        eprintln!("Skipping: assistive lane unavailable");
        return;
    }

    unsafe {
        std::env::set_var("CODESCRIBE_AI_MAX_RETRIES", "0");
    }

    eprintln!("=== KURIER MODE TEST (pass-through) ===");

    // KURIER trigger: "przekaż" or dictation without question
    let input = "przekaż do Maćka że spotkanie jest przełożone na piątek o dziesiątej";
    eprintln!("Input:  {}", input);

    let result = ai_formatting::format_text(
        input,
        Some("pl"),
        true,
        runtime_settings.llm_lanes().assistive(),
    )
    .await;
    eprintln!("Output: {}", result);

    assert!(!result.is_empty(), "Should return formatted message");
    // KURIER should NOT add AI commentary, just format/pass through
    assert!(
        !result.to_lowercase().contains("oczywiście")
            && !result.to_lowercase().contains("rozumiem"),
        "KURIER mode should not add AI commentary"
    );
}

/// Direct SSE parsing test - validates raw stream handling
#[tokio::test]
#[serial]
#[ignore = "Requires real API key - run with make test-sse"]
async fn e2e_sse_direct_stream_parsing() {
    use futures_util::StreamExt;
    use reqwest::Client;
    use serde_json::json;

    load_codescribe_env();

    let runtime_settings = Config::load_runtime_snapshot().expect("seal runtime settings");
    let lane = runtime_settings.llm_lanes().formatting();
    let Some(api_key) = lane.credential().api_key() else {
        eprintln!("Skipping: No API key found");
        return;
    };

    let endpoint = lane.endpoint();
    let model = lane.model();

    eprintln!("=== DIRECT SSE STREAM TEST ===");
    eprintln!("Endpoint: {}", endpoint);
    eprintln!("Model: {}", model);

    let client = Client::new();
    let response = client
        .post(&endpoint)
        .header("Authorization", format!("Bearer {}", api_key))
        .header("Content-Type", "application/json")
        .header("Accept", "text/event-stream")
        .json(&json!({
            "model": model,
            "input": [{"role": "user", "content": [{"type": "input_text", "text": "Powiedz cześć po polsku"}]}],
            "stream": true
        }))
        .send()
        .await
        .expect("Request failed");

    assert!(
        response.status().is_success(),
        "HTTP error: {}",
        response.status()
    );

    let mut stream = response.bytes_stream();
    let mut collected = String::new();
    let mut chunk_count = 0;
    let mut event_types: Vec<String> = Vec::new();

    eprintln!("--- SSE Events ---");

    while let Some(chunk) = stream.next().await {
        let bytes = chunk.expect("Stream error");
        let text = String::from_utf8_lossy(&bytes);

        for line in text.lines() {
            if let Some(data) = line.strip_prefix("data: ") {
                if data == "[DONE]" {
                    eprintln!("  [DONE]");
                    break;
                }
                if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(data) {
                    let chunk_type = parsed["type"].as_str().unwrap_or("unknown");
                    event_types.push(chunk_type.to_string());

                    match chunk_type {
                        "response.output_text.delta" => {
                            if let Some(delta) = parsed["delta"].as_str() {
                                collected.push_str(delta);
                                chunk_count += 1;
                                eprint!("{}", delta);
                            }
                        }
                        "response.completed" => {
                            eprintln!("\n  [response.completed]");
                        }
                        _ => {}
                    }
                }
            }
        }
    }

    eprintln!("\n--- Results ---");
    eprintln!("Chunks received: {}", chunk_count);
    eprintln!(
        "Event types: {:?}",
        event_types.iter().collect::<std::collections::HashSet<_>>()
    );
    eprintln!("Collected text: {}", collected);

    assert!(!collected.is_empty(), "Should collect text from SSE stream");
    assert!(chunk_count > 0, "Should receive multiple delta chunks");
    assert!(
        event_types.contains(&"response.output_text.delta".to_string()),
        "Should receive delta events"
    );
}
