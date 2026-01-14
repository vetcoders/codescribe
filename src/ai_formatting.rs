//! AI-powered text formatting service
//!
//! Two modes:
//! - FORMATTING (assistive=false): Clean formatting only - punctuation, capitalization,
//!   paragraphs, bullet points. Removes Whisper repetition loops. NEVER changes meaning.
//! - ASSISTIVE (assistive=true): Kurier/enhancer mode - augments and PASSES user's words
//!   forward, does NOT respond to them. Adds structure/context but message is always user's.
//!
//! Uses Responses API (/v1/responses) for:
//! - Text formatting and grammar correction
//! - Punctuation and capitalization
//! - Anti-repetition filtering (fixes Whisper loops like "Wielki, Wielki...")
//! - Language-specific formatting
//!
//! Configuration (required environment variables):
//! - LLM_HOST: Full URL to LLM endpoint (e.g., "http://localhost:11434/v1/responses")
//! - LLM_MODEL: Model name (e.g., "qwen3-coder:480b-cloud")
//! - LLM_API_KEY: API key for authentication (not needed for local Ollama)
//!
//! Legacy fallbacks: OLLAMA_HOST -> LLM_HOST, OLLAMA_MODEL -> LLM_MODEL
//!
//! Supports both cloud providers (via /v1/responses) and local Ollama (/api/chat).
//! Authentication: `Authorization: Bearer <key>` + `x-api-key: <key>` (dual-header)

use anyhow::{Context, Result};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::env;
use std::sync::{OnceLock, RwLock};
use std::time::Duration;
use tracing::{debug, info, warn};

/// HTTP client for AI providers
static AI_CLIENT: OnceLock<Client> = OnceLock::new();

#[derive(Clone)]
struct MemoryMessage {
    role: String,
    content: String,
}

static OLLAMA_MEMORY: OnceLock<RwLock<Vec<MemoryMessage>>> = OnceLock::new();
const MAX_OLLAMA_MEMORY_CHARS: usize = 4000;

fn ollama_memory() -> &'static RwLock<Vec<MemoryMessage>> {
    OLLAMA_MEMORY.get_or_init(|| RwLock::new(Vec::new()))
}

fn get_client() -> &'static Client {
    AI_CLIENT.get_or_init(|| {
        Client::builder()
            .timeout(Duration::from_secs(30))
            .connect_timeout(Duration::from_secs(5))
            .build()
            .expect("Failed to create AI HTTP client")
    })
}

/// Read env var by priority list, ensure non-empty, return detailed error
fn get_env_non_empty(candidates: &[&str], what: &str) -> Result<String> {
    for key in candidates {
        if let Ok(value) = env::var(key) {
            let trimmed = value.trim();
            if !trimmed.is_empty() {
                return Ok(trimmed.to_string());
            }
        }
    }

    anyhow::bail!(
        "{} is required. Set {} (or legacy {}).",
        what,
        candidates.first().unwrap_or(&"LLM_*"),
        candidates.get(1).unwrap_or(&"<none>")
    );
}

/// Get LLM host from environment (LLM_HOST with OLLAMA_HOST legacy fallback)
fn get_llm_host() -> Result<String> {
    get_env_non_empty(&["LLM_HOST", "OLLAMA_HOST"], "LLM host")
}

/// Get LLM model from environment (LLM_MODEL with OLLAMA_MODEL legacy fallback)
fn get_llm_model() -> Result<String> {
    get_env_non_empty(&["LLM_MODEL", "OLLAMA_MODEL"], "LLM model")
}

fn prune_memory(memory: &mut Vec<MemoryMessage>) {
    while memory
        .iter()
        .map(|m| m.content.len())
        .sum::<usize>()
        > MAX_OLLAMA_MEMORY_CHARS
    {
        if memory.is_empty() {
            break;
        }
        memory.remove(0);
    }
}

fn push_memory(role: &str, content: &str) {
    if let Ok(mut guard) = ollama_memory().write() {
        guard.push(MemoryMessage {
            role: role.to_string(),
            content: content.to_string(),
        });
        prune_memory(&mut guard);
    }
}

fn snapshot_memory() -> Vec<MemoryMessage> {
    ollama_memory()
        .read()
        .map(|g| g.clone())
        .unwrap_or_default()
}

pub fn reset_ollama_memory() {
    if let Ok(mut guard) = ollama_memory().write() {
        guard.clear();
    }
}

fn build_ollama_messages(
    system_prompt: &str,
    user_message: &str,
    assistive: bool,
) -> Vec<ChatMessage> {
    let mut messages = Vec::new();
    messages.push(ChatMessage {
        role: "system".to_string(),
        content: system_prompt.to_string(),
    });

    if assistive {
        for m in snapshot_memory() {
            messages.push(ChatMessage {
                role: m.role,
                content: m.content,
            });
        }
    }

    messages.push(ChatMessage {
        role: "user".to_string(),
        content: user_message.to_string(),
    });

    messages
}

/// Ollama request format
#[derive(Debug, Serialize)]
struct OllamaRequest {
    model: String,
    messages: Vec<ChatMessage>,
    stream: bool,
    options: OllamaOptions,
}

#[derive(Debug, Serialize)]
struct OllamaOptions {
    temperature: f32,
    num_predict: u32,
}

/// Ollama response format
#[derive(Debug, Deserialize)]
struct OllamaResponse {
    message: Option<OllamaMessage>,
    response: Option<String>,
}

#[derive(Debug, Deserialize)]
struct OllamaMessage {
    content: String,
}

/// Responses API request format (/v1/responses)
#[derive(Debug, Serialize)]
struct ResponsesRequest {
    model: String,
    input: Vec<InputItem>,
    #[serde(skip_serializing_if = "Option::is_none")]
    previous_response_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    instructions: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_output_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    stream: bool,
}

/// Input item for Responses API
#[derive(Debug, Serialize)]
struct InputItem {
    role: &'static str,
    content: Vec<InputContent>,
}

/// Content part for input messages
#[derive(Debug, Serialize)]
struct InputContent {
    #[serde(rename = "type")]
    content_type: &'static str,
    text: String,
}

/// Responses API response format
#[derive(Debug, Deserialize)]
struct ResponsesResponse {
    id: String,
    output: Vec<OutputItem>,
}

#[derive(Debug, Deserialize)]
struct OutputItem {
    #[serde(rename = "type")]
    item_type: String,
    #[serde(default)]
    content: Option<Vec<ContentPart>>,
}

#[derive(Debug, Deserialize)]
struct ContentPart {
    #[serde(rename = "type")]
    part_type: String,
    #[serde(default)]
    text: Option<String>,
}

/// SSE streaming chunk from Responses API
#[derive(Debug, Deserialize)]
struct StreamChunk {
    #[serde(rename = "type")]
    chunk_type: String,
    #[serde(default)]
    delta: Option<String>,
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    response: Option<StreamResponse>,
}

/// Response object in stream chunks (for response.completed event)
#[derive(Debug, Deserialize)]
struct StreamResponse {
    #[serde(default)]
    id: String,
}

/// Legacy chat message (for Ollama compatibility)
#[derive(Debug, Serialize, Clone)]
struct ChatMessage {
    role: String,
    content: String,
}

/// Max tokens for normal formatting
const FORMATTING_MAX_TOKENS: u32 = 2048;

/// Max tokens for assistive mode (higher for complex responses)
const ASSISTIVE_MAX_TOKENS: u32 = 4096;

/// Check if output is effectively the same as input (raw-like)
/// Returns true if normalized content (lowercase, alphanumeric only) matches.
fn is_effectively_same(input: &str, output: &str) -> bool {
    let normalize = |s: &str| -> String {
        s.chars()
            .filter(|c| c.is_alphanumeric())
            .flat_map(|c| c.to_lowercase())
            .collect()
    };
    normalize(input) == normalize(output)
}

/// Check if text has repetition loop (Whisper hallucination)
pub fn has_repetition_loop(text: &str) -> bool {
    let words: Vec<&str> = text.split_whitespace().collect();
    if words.len() < 4 {
        return false;
    }

    // Check for consecutive word repetitions
    let mut consecutive_count = 1;
    for i in 1..words.len() {
        if words[i].to_lowercase() == words[i - 1].to_lowercase() {
            consecutive_count += 1;
            if consecutive_count >= 3 {
                return true;
            }
        } else {
            consecutive_count = 1;
        }
    }

    // Check for phrase repetitions (2-3 word patterns)
    for pattern_len in 1..=3 {
        if words.len() < pattern_len * 3 {
            continue;
        }

        let mut i = 0;
        while i + pattern_len * 2 <= words.len() {
            let pattern: Vec<&str> = words[i..i + pattern_len].to_vec();
            let mut repeat_count = 1;
            let mut j = i + pattern_len;

            while j + pattern_len <= words.len() {
                let next: Vec<&str> = words[j..j + pattern_len].to_vec();
                let matches = pattern
                    .iter()
                    .zip(next.iter())
                    .all(|(a, b)| a.to_lowercase() == b.to_lowercase());

                if matches {
                    repeat_count += 1;
                    j += pattern_len;
                } else {
                    break;
                }
            }

            if repeat_count >= 3 {
                return true;
            }
            i += 1;
        }
    }

    false
}

/// Strip punctuation from a word for comparison (but keep the original)
fn normalize_word(word: &str) -> String {
    word.trim_matches(|c: char| !c.is_alphanumeric())
        .to_lowercase()
}

/// Clean up trailing punctuation from repeated patterns
/// For comma-separated repetitions, remove the comma: "roku, roku, roku" -> "roku"
/// For period-separated repetitions, keep the period: "jest. jest. jest." -> "jest."
fn clean_pattern_punctuation(words: &[&str]) -> Vec<String> {
    if words.is_empty() {
        return Vec::new();
    }

    let mut cleaned: Vec<String> = words.iter().map(|w| w.to_string()).collect();

    // Check if last word has trailing punctuation
    if let Some(last) = cleaned.last_mut() {
        // Only remove commas from repeated patterns (they're just separators)
        // Keep periods (they mark sentence endings)
        if last.ends_with(',') {
            *last = last.trim_end_matches(',').to_string();
        }
    }

    cleaned
}

/// Simple local repetition cleanup (no AI needed)
/// Removes repeated words AND repeated phrases (1-3 word patterns)
/// Handles comma-separated repetitions like "w tym roku, w tym roku, w tym roku"
pub fn remove_simple_repetitions(text: &str) -> String {
    let words: Vec<&str> = text.split_whitespace().collect();
    if words.is_empty() {
        return text.to_string();
    }

    let mut result: Vec<String> = Vec::new();
    let mut i = 0;

    while i < words.len() {
        // Try to match phrase patterns (3-word, 2-word, then 1-word)
        let mut best_pattern_len = 1;
        let mut best_repeat_count = 1;

        for pattern_len in (1..=3).rev() {
            if i + pattern_len > words.len() {
                continue;
            }

            // Normalize words for comparison (strip punctuation, lowercase)
            let pattern: Vec<String> = words[i..i + pattern_len]
                .iter()
                .map(|w| normalize_word(w))
                .collect();

            let mut repeat_count = 1;
            let mut j = i + pattern_len;

            while j + pattern_len <= words.len() {
                let next: Vec<String> = words[j..j + pattern_len]
                    .iter()
                    .map(|w| normalize_word(w))
                    .collect();

                if pattern == next {
                    repeat_count += 1;
                    j += pattern_len;
                } else {
                    break;
                }
            }

            // Prefer longer patterns with more repeats
            if repeat_count >= 2
                && (pattern_len > best_pattern_len || repeat_count > best_repeat_count)
            {
                best_pattern_len = pattern_len;
                best_repeat_count = repeat_count;
            }
        }

        // Add the pattern once, clean up punctuation if it was repeated
        let pattern_words = &words[i..i + best_pattern_len];
        if best_repeat_count >= 2 {
            // Pattern was repeated - clean trailing punctuation
            result.extend(clean_pattern_punctuation(pattern_words));
        } else {
            // Not repeated - keep as is
            result.extend(pattern_words.iter().map(|w| w.to_string()));
        }

        i += best_pattern_len * best_repeat_count;
    }

    result.join(" ")
}

/// Format text using AI provider with fallback chain
///
/// # Arguments
/// * `text` - Raw text from transcription
/// * `language` - Optional language hint (e.g., "pl", "en")
/// * `assistive` - If true, use assistive mode (AI assistant) instead of simple formatting
///
/// # Returns
/// Formatted text or original if all providers fail
pub async fn format_text(text: &str, language: Option<&str>, assistive: bool) -> String {
    // Skip very short texts (but not in assistive mode - user might say "help")
    if text.len() < 10 && !assistive {
        return text.to_string();
    }

    // Check for repetition loops - apply simple fix first
    let cleaned = if has_repetition_loop(text) {
        info!("Detected repetition loop in transcription");
        remove_simple_repetitions(text)
    } else {
        text.to_string()
    };

    // Production defaults (per acceptance): 1 retry after 5s, ~2.5s per attempt.
    // For deterministic/fast tests, allow overriding via env vars.
    let max_retries: u32 = env::var("CODESCRIBE_AI_MAX_RETRIES")
        .ok()
        .and_then(|v| v.parse::<u32>().ok())
        .unwrap_or(1);

    let retry_delay_ms: u64 = env::var("CODESCRIBE_AI_RETRY_DELAY_MS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(5000);
    let retry_delay = Duration::from_millis(retry_delay_ms);

    // Budget: ~2.5s + 5s pause + ~2.5s ≈ 10s total
    let attempt_timeout_ms: u64 = env::var("CODESCRIBE_AI_ATTEMPT_TIMEOUT_MS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(2500);
    let attempt_timeout = Duration::from_millis(attempt_timeout_ms);

    for attempt in 0..=max_retries {
        info!(
            "AI formatting attempt {} (assistive={}, input_len={})",
            attempt + 1,
            assistive,
            cleaned.len()
        );
        // Select prompt and max tokens based on mode
        let (mut system_prompt, max_tokens) = if assistive {
            if attempt == 0 {
                info!("Using assistive mode (AI assistant)");
            }
            (crate::config::prompts::get_assistive_prompt(), ASSISTIVE_MAX_TOKENS)
        } else {
            if attempt == 0 {
                info!("Using formatting mode");
            }
            (crate::config::prompts::get_formatting_prompt(), FORMATTING_MAX_TOKENS)
        };

        // If retrying, wait and strengthen instructions
        if attempt > 0 {
            info!(
                "Retry attempt {}/{} (waiting {:?})",
                attempt,
                max_retries,
                retry_delay
            );
            tokio::time::sleep(retry_delay).await;
            
            // Append critical instruction
            system_prompt.push_str("\n\nCRITICAL: You MUST format/enhance the text. Do NOT return raw input.");
        }

        // Build user message with optional language hint
        let user_message = if let Some(lang) = language {
            format!("[Language: {}]\n\n{}", lang, cleaned)
        } else {
            cleaned.clone()
        };

        // Try Ollama first if configured as AI_PROVIDER
        let mut result_opt = if has_ollama() {
            match tokio::time::timeout(
                attempt_timeout,
                call_ollama(&user_message, &system_prompt, max_tokens, assistive),
            )
            .await
            {
                Ok(Ok(formatted)) => Some(formatted),
                Ok(Err(e)) => {
                    warn!("Ollama failed (attempt {}): {}, trying other providers", attempt, e);
                    None
                }
                Err(_) => {
                    warn!("Ollama timed out after {:?} (attempt {})", attempt_timeout, attempt);
                    None
                }
            }
        } else {
            None
        };

        // Try LLM endpoint if Ollama failed/skipped
        if result_opt.is_none() {
            let endpoint = get_llm_host().unwrap_or_default();
            let use_streaming = is_openai_endpoint(&endpoint);

            // Streaming gets longer timeout (60s), sync gets 30s
            let llm_timeout = if use_streaming {
                Duration::from_secs(60)
            } else {
                Duration::from_secs(30)
            };

            if use_streaming {
                // SSE streaming for OpenAI/Libraxis
                match tokio::time::timeout(
                    llm_timeout,
                    call_llm_endpoint_streaming(&user_message, &system_prompt, max_tokens, assistive),
                )
                .await
                {
                    Ok(Ok(formatted)) => result_opt = Some(formatted),
                    Ok(Err(e)) => {
                        warn!("LLM streaming failed (attempt {}): {}", attempt, e);
                    }
                    Err(_) => {
                        warn!("LLM streaming timed out after {:?} (attempt {})", llm_timeout, attempt);
                    }
                }
            } else {
                // Sync mode for other providers
                match tokio::time::timeout(
                    llm_timeout,
                    call_llm_endpoint(&user_message, &system_prompt, max_tokens, assistive),
                )
                .await
                {
                    Ok(Ok(formatted)) => result_opt = Some(formatted),
                    Ok(Err(e)) => {
                        warn!("LLM endpoint failed (attempt {}): {}", attempt, e);
                    }
                    Err(_) => {
                        warn!("LLM endpoint timed out after {:?} (attempt {})", llm_timeout, attempt);
                    }
                }
            }
        }

        if let Some(formatted) = result_opt {
             // Analyze result quality
             let cleaned_trim = cleaned.trim();
             let formatted_trim = formatted.trim();
             let content_match = is_effectively_same(&cleaned, &formatted);

             let mut should_retry = false;
             let mut raw_like = content_match;

             if assistive {
                 // Assistive should change/expand content
                 // If it matches normalized content, it likely failed to enhance
                 if content_match {
                     warn!("Assistive mode returned content-matching output (not expanded)");
                     should_retry = true;
                 }
             } else {
                 // Formatting should preserve content but add structure
                 // If output is identical to input
                 if cleaned_trim == formatted_trim {
                      // Check if input was arguably already formatted (has punctuation)
                      let input_has_punct = cleaned_trim.ends_with('.') || cleaned_trim.ends_with('?') || cleaned_trim.ends_with('!');
                      if !input_has_punct {
                          warn!("Formatting mode returned raw echo");
                          should_retry = true;
                          raw_like = true;
                      }
                 }
             }
             
             if should_retry {
                 if attempt < max_retries {
                     warn!("Triggering retry...");
                     continue;
                 } else {
                     warn!("Max retries reached, accepting output.");
                 }
             }

            info!(
                "Formatted via AI ({} -> {} chars, assistive={}, content_match={}, raw_like={})",
                text.len(),
                formatted.len(),
                assistive,
                content_match,
                raw_like
            );
            return formatted;
        }
    }

    // All providers failed
    warn!("All AI providers/retries failed, returning cleaned text");
    cleaned
}

/// Call LLM endpoint using /v1/responses API
///
/// Requires environment variables:
/// - LLM_HOST: Full URL to endpoint (e.g., "http://localhost:11434/v1/responses")
/// - LLM_MODEL: Model name (e.g., "qwen3-coder:480b-cloud")
/// - LLM_API_KEY: API key for authentication
///
/// Legacy fallbacks: OLLAMA_HOST -> LLM_HOST, OLLAMA_MODEL -> LLM_MODEL
async fn call_llm_endpoint(
    user_message: &str,
    system_prompt: &str,
    max_tokens: u32,
    assistive: bool,
) -> Result<String> {
    let endpoint = get_llm_host()?;
    let model = get_llm_model()?;
    let api_key = env::var("LLM_API_KEY").context("LLM_API_KEY not set")?;

    if api_key.is_empty() {
        anyhow::bail!("LLM_API_KEY is empty");
    }

    // Use higher temperature for assistive mode (more creative responses)
    let temperature = if assistive { 0.3 } else { 0.1 };

    // Get previous_response_id for conversation continuity (only in assistive mode)
    let previous_response_id = if assistive {
        crate::state::conversation::get_previous_response_id()
    } else {
        None
    };

    // Build Responses API request
    let request = ResponsesRequest {
        model,
        input: vec![InputItem {
            role: "user",
            content: vec![InputContent {
                content_type: "input_text",
                text: user_message.to_string(),
            }],
        }],
        previous_response_id,
        instructions: Some(system_prompt.to_string()),
        max_output_tokens: Some(max_tokens),
        temperature: Some(temperature),
        stream: false,
    };

    debug!(
        "Calling LLM endpoint {} for {} (max_tokens={}, temp={})",
        endpoint,
        if assistive { "assistive" } else { "formatting" },
        max_tokens,
        temperature
    );

    // Dual-header authentication (both Bearer and x-api-key for compatibility)
    let response = get_client()
        .post(&endpoint)
        .header("Authorization", format!("Bearer {}", api_key))
        .header("x-api-key", &api_key)
        .header("Content-Type", "application/json")
        .json(&request)
        .send()
        .await
        .context("Request failed")?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        anyhow::bail!("HTTP {} - {}", status, body);
    }

    let responses_result: ResponsesResponse =
        response.json().await.context("Failed to parse response")?;

    // Extract text from output array
    let formatted = responses_result
        .output
        .iter()
        .filter(|o| o.item_type == "message")
        .filter_map(|o| o.content.as_ref())
        .flatten()
        .filter(|c| c.part_type == "output_text" || c.part_type == "text")
        .filter_map(|c| c.text.as_deref())
        .collect::<Vec<_>>()
        .join("")
        .trim()
        .to_string();

    if formatted.is_empty() {
        anyhow::bail!("No text content in response (id: {})", responses_result.id);
    }

    // Store response_id for conversation continuity (only in assistive mode)
    if assistive {
        crate::state::conversation::set_response_id(responses_result.id.clone());
    }

    // Sanity check - only for formatting mode (assistive can return any length)
    if !assistive {
        let max_len_multiplier = 2;
        if formatted.len() > user_message.len() * max_len_multiplier {
            anyhow::bail!("Response too long");
        }
    }

    debug!("Response id: {}", responses_result.id);
    Ok(formatted)
}

/// Check if endpoint is OpenAI-compatible (supports SSE streaming)
fn is_openai_endpoint(endpoint: &str) -> bool {
    endpoint.contains("openai.com") || endpoint.contains("libraxis")
}

/// Call LLM endpoint with SSE streaming (OpenAI Responses API)
///
/// Uses Server-Sent Events for faster first-token response.
/// Falls back to sync mode if streaming fails.
async fn call_llm_endpoint_streaming(
    user_message: &str,
    system_prompt: &str,
    max_tokens: u32,
    assistive: bool,
) -> Result<String> {
    use futures_util::StreamExt;

    let endpoint = get_llm_host()?;
    let model = get_llm_model()?;
    let api_key = env::var("LLM_API_KEY").context("LLM_API_KEY not set")?;

    let temperature = if assistive { 0.3 } else { 0.1 };

    let previous_response_id = if assistive {
        crate::state::conversation::get_previous_response_id()
    } else {
        None
    };

    let request = ResponsesRequest {
        model,
        input: vec![InputItem {
            role: "user",
            content: vec![InputContent {
                content_type: "input_text",
                text: user_message.to_string(),
            }],
        }],
        previous_response_id,
        instructions: Some(system_prompt.to_string()),
        max_output_tokens: Some(max_tokens),
        temperature: Some(temperature),
        stream: true,
    };

    debug!(
        "SSE streaming to {} for {} (max_tokens={})",
        endpoint,
        if assistive { "assistive" } else { "formatting" },
        max_tokens
    );

    let response = get_client()
        .post(&endpoint)
        .header("Authorization", format!("Bearer {}", api_key))
        .header("Content-Type", "application/json")
        .header("Accept", "text/event-stream")
        .json(&request)
        .send()
        .await
        .context("SSE request failed")?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        anyhow::bail!("HTTP {} - {}", status, body);
    }

    // Parse SSE stream
    let mut collected_text = String::new();
    let mut response_id = String::new();
    let mut stream = response.bytes_stream();

    let mut buffer = String::new();

    while let Some(chunk_result) = stream.next().await {
        let chunk = chunk_result.context("Stream read error")?;
        buffer.push_str(&String::from_utf8_lossy(&chunk));

        // Process complete lines
        while let Some(newline_pos) = buffer.find('\n') {
            let line = buffer[..newline_pos].trim().to_string();
            buffer = buffer[newline_pos + 1..].to_string();

            if line.is_empty() || line.starts_with(':') {
                continue;
            }

            // Parse SSE data lines
            if let Some(data) = line.strip_prefix("data: ") {
                if data == "[DONE]" {
                    break;
                }

                if let Ok(chunk) = serde_json::from_str::<StreamChunk>(data) {
                    match chunk.chunk_type.as_str() {
                        "response.output_text.delta" => {
                            if let Some(delta) = chunk.delta {
                                collected_text.push_str(&delta);
                            }
                        }
                        "response.output_text.done" => {
                            // Full text available - use it if we missed deltas
                            if let Some(text) = chunk.text {
                                if collected_text.is_empty() {
                                    collected_text = text;
                                }
                            }
                        }
                        "response.completed" | "response.done" => {
                            if let Some(resp) = chunk.response {
                                response_id = resp.id;
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
    }

    let formatted = collected_text.trim().to_string();

    if formatted.is_empty() {
        anyhow::bail!("No text content in SSE stream");
    }

    // Store response_id for conversation continuity
    if assistive && !response_id.is_empty() {
        crate::state::conversation::set_response_id(response_id.clone());
    }

    // Sanity check for formatting mode
    if !assistive {
        let max_len_multiplier = 2;
        if formatted.len() > user_message.len() * max_len_multiplier {
            anyhow::bail!("Response too long");
        }
    }

    debug!("SSE complete, response_id: {}", response_id);
    Ok(formatted)
}

/// Call Ollama/local LLM for text formatting/assistive mode
///
/// Uses LLM_HOST (or legacy OLLAMA_HOST) for host, LLM_MODEL (or legacy OLLAMA_MODEL) for model.
/// Ollama native API uses /api/chat endpoint format.
async fn call_ollama(
    user_message: &str,
    system_prompt: &str,
    max_tokens: u32,
    assistive: bool,
) -> Result<String> {
    let host = get_llm_host()?;
    let model = get_llm_model()?;

    // Ollama native API uses /api/chat - strip any /v1/responses suffix
    let base_host = host
        .trim_end_matches('/')
        .trim_end_matches("/v1/responses")
        .trim_end_matches("/v1");
    let endpoint = format!("{}/api/chat", base_host);

    // Use higher temperature for assistive mode
    let temperature = if assistive { 0.3 } else { 0.1 };

    let messages = build_ollama_messages(system_prompt, user_message, assistive);

    let request = OllamaRequest {
        model,
        messages,
        stream: false,
        options: OllamaOptions {
            temperature,
            num_predict: max_tokens,
        },
    };

    debug!(
        "Calling Ollama for {} (max_tokens={}, temp={})",
        if assistive { "assistive" } else { "formatting" },
        max_tokens,
        temperature
    );

    let response = get_client()
        .post(&endpoint)
        .header("Content-Type", "application/json")
        .json(&request)
        .send()
        .await
        .context("Ollama request failed")?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        anyhow::bail!("Ollama HTTP {} - {}", status, body);
    }

    let ollama_response: OllamaResponse = response
        .json()
        .await
        .context("Failed to parse Ollama response")?;

    let formatted = ollama_response
        .message
        .map(|m| m.content)
        .or(ollama_response.response)
        .unwrap_or_default()
        .trim()
        .to_string();

    if formatted.is_empty() {
        anyhow::bail!("Empty Ollama response");
    }

    if assistive {
        push_memory("user", user_message);
        push_memory("assistant", &formatted);
    }

    Ok(formatted)
}

/// Check if local LLM (Ollama native /api/chat) is configured
/// Returns true if LLM_HOST points to localhost AND doesn't use /v1/ path
/// Returns false if env vars are not set or using /v1/ endpoints (Responses API format)
fn has_ollama() -> bool {
    let host = match get_llm_host() {
        Ok(h) => h,
        Err(_) => return false, // No host configured
    };

    // Skip Ollama native format if endpoint uses /v1/ (Responses API)
    if host.contains("/v1/") {
        return false;
    }

    // Check if pointing to localhost
    host.contains("127.0.0.1") || host.contains("localhost")
}

/// Check if any AI provider is configured
/// Returns true if:
/// - Local Ollama is configured (LLM_HOST points to localhost, no API key needed)
/// - Remote LLM is configured with LLM_HOST + LLM_MODEL + LLM_API_KEY
pub fn has_api_key() -> bool {
    // Check if required env vars are set
    let has_host = get_llm_host().is_ok();
    let has_model = get_llm_model().is_ok();

    if !has_host || !has_model {
        return false;
    }

    // Ollama doesn't need an API key
    if has_ollama() {
        return true;
    }

    // Remote LLM requires API key
    env::var("LLM_API_KEY")
        .map(|k| !k.is_empty())
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_has_repetition_loop() {
        // Should detect repetitions
        assert!(has_repetition_loop("Wielki Wielki Wielki problem"));
        assert!(has_repetition_loop("Kali Kali Kali Kali bogini"));
        assert!(has_repetition_loop("to jest to jest to jest test"));

        // Should not flag normal text
        assert!(!has_repetition_loop("To jest normalny tekst"));
        assert!(!has_repetition_loop("Wielki problem do rozwiązania"));
        assert!(!has_repetition_loop("Kali to bogini"));
    }

    #[test]
    fn test_remove_simple_repetitions() {
        // Basic word repetitions
        assert_eq!(
            remove_simple_repetitions("Wielki Wielki Wielki problem"),
            "Wielki problem"
        );
        assert_eq!(
            remove_simple_repetitions("Kali Kali Kali Kali bogini"),
            "Kali bogini"
        );
        assert_eq!(remove_simple_repetitions("test test test"), "test");

        // Comma-separated repetitions (real-world case)
        assert_eq!(
            remove_simple_repetitions(
                "W tym momencie, w tym roku, w tym roku, w tym roku, w tym roku"
            ),
            "W tym momencie, w tym roku"
        );

        // Period-separated repetitions
        assert_eq!(
            remove_simple_repetitions("To jest. To jest. To jest."),
            "To jest."
        );

        // Multi-word phrase repetitions
        assert_eq!(
            remove_simple_repetitions("to jest to jest to jest test"),
            "to jest test"
        );

        // Should preserve normal text
        assert_eq!(
            remove_simple_repetitions("normalny tekst bez powtórzeń"),
            "normalny tekst bez powtórzeń"
        );
    }
}
