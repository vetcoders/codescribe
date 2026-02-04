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
//! Configuration contract:
//! - LLM_{FORMATTING,ASSISTIVE}_{ENDPOINT,MODEL,API_KEY} - mode-specific config
//! - LLM_{ENDPOINT,MODEL,API_KEY} - shared fallback defaults
//!
//! Authentication: `Authorization: Bearer <key>` + `x-api-key: <key>` (dual-header)

use anyhow::{Context, Result};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::env;
use std::path::PathBuf;
use std::sync::{Arc, OnceLock, RwLock};
use std::time::Duration;
use tracing::{debug, info, trace, warn};

/// HTTP client for AI providers
static AI_CLIENT: OnceLock<Client> = OnceLock::new();

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AiFormatStatus {
    Applied,
    Failed,
    Skipped,
}

pub type AiStreamCallback = Arc<dyn Fn(&str) + Send + Sync>;

#[derive(Debug, Clone)]
pub struct AiFormatResult {
    pub text: String,
    pub status: AiFormatStatus,
}

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
            .timeout(Duration::from_secs(90)) // Longer timeout for GPT-5.x with long inputs
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

// ============================================================================
// LLM Configuration - Separate providers for Formatting vs Assistive
// ============================================================================
//
// Contract: LLM_{FORMATTING,ASSISTIVE}_{ENDPOINT,MODEL,API_KEY}
// Fallback: LLM_{ENDPOINT,MODEL,API_KEY} (shared defaults)
//
// FORMATTING mode (cheap, fast): punctuation, structure, cleanup
// ASSISTIVE mode (smart): Voice Chat, AI assistant
//
// NO legacy variables. Clean contract only.

/// Helper: get specific var or fall back to shared default
fn get_mode_config(specific_key: &str, default_key: &str, what: &str) -> Result<String> {
    // Try specific first
    if let Ok(val) = env::var(specific_key) {
        let val = val.trim();
        if !val.is_empty() {
            return Ok(val.to_string());
        }
    }
    // Fall back to shared default
    get_env_non_empty(&[default_key], what)
}

// ---- FORMATTING mode config ----

fn get_formatting_endpoint() -> Result<String> {
    get_mode_config(
        "LLM_FORMATTING_ENDPOINT",
        "LLM_ENDPOINT",
        "LLM endpoint (formatting)",
    )
}

fn get_formatting_model() -> Result<String> {
    get_mode_config(
        "LLM_FORMATTING_MODEL",
        "LLM_MODEL",
        "LLM model (formatting)",
    )
}

fn get_formatting_api_key() -> Result<String> {
    get_mode_config(
        "LLM_FORMATTING_API_KEY",
        "LLM_API_KEY",
        "LLM API key (formatting)",
    )
}

// ---- ASSISTIVE mode config ----

fn get_assistive_endpoint() -> Result<String> {
    get_mode_config(
        "LLM_ASSISTIVE_ENDPOINT",
        "LLM_ENDPOINT",
        "LLM endpoint (assistive)",
    )
}

fn get_assistive_model() -> Result<String> {
    get_mode_config("LLM_ASSISTIVE_MODEL", "LLM_MODEL", "LLM model (assistive)")
}

fn get_assistive_api_key() -> Result<String> {
    get_mode_config(
        "LLM_ASSISTIVE_API_KEY",
        "LLM_API_KEY",
        "LLM API key (assistive)",
    )
}

/// Get temperature from env var. Returns None if empty/unset (skip parameter).
/// Supports mode-specific: LLM_FORMATTING_TEMPERATURE, LLM_ASSISTIVE_TEMPERATURE
/// Falls back to LLM_TEMPERATURE, then to default (0.1 formatting, 0.3 assistive)
fn get_temperature(assistive: bool) -> Option<f32> {
    let specific_key = if assistive {
        "LLM_ASSISTIVE_TEMPERATURE"
    } else {
        "LLM_FORMATTING_TEMPERATURE"
    };

    // Try specific first, then fallback
    for key in [specific_key, "LLM_TEMPERATURE"] {
        if let Ok(val) = env::var(key) {
            let val = val.trim();
            if val.is_empty() {
                // Explicitly empty = skip temperature
                return None;
            }
            if let Ok(temp) = val.parse::<f32>() {
                return Some(temp);
            }
        }
    }

    // No default — user sets if they want, model decides otherwise
    None
}

// ============================================================================
// Endpoint routing — path-based, no domain heuristics
// ============================================================================

/// Endpoint format detected from URL path
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EndpointFormat {
    /// Responses API (/v1/responses or anything else) — default
    ResponsesApi,
    /// Ollama native chat (/api/chat) — legacy compatibility
    OllamaChat,
}

/// Detect format from endpoint path. No domain checks, no guessing.
fn detect_format(endpoint: &str) -> EndpointFormat {
    if endpoint.contains("/api/chat") {
        EndpointFormat::OllamaChat
    } else {
        EndpointFormat::ResponsesApi
    }
}

/// Streaming = default. Opt-out via LLM_USE_STREAMING=0|false.
fn use_streaming() -> bool {
    if let Ok(val) = env::var("LLM_USE_STREAMING") {
        return val != "0" && val.to_lowercase() != "false";
    }
    true
}

fn prune_memory(memory: &mut Vec<MemoryMessage>) {
    while memory.iter().map(|m| m.content.len()).sum::<usize>() > MAX_OLLAMA_MEMORY_CHARS {
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
#[serde(tag = "type")]
enum InputContent {
    #[serde(rename = "input_text")]
    Text { text: String },
    #[serde(rename = "input_image")]
    Image { image_url: String },
}

fn image_mime_from_path(path: &std::path::Path) -> Option<&'static str> {
    let ext = path
        .extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    match ext.as_str() {
        "png" => Some("image/png"),
        "jpg" | "jpeg" => Some("image/jpeg"),
        "webp" => Some("image/webp"),
        "gif" => Some("image/gif"),
        "bmp" => Some("image/bmp"),
        "tif" | "tiff" => Some("image/tiff"),
        _ => None,
    }
}

fn strip_image_attachments(user_message: &str) -> (String, Vec<PathBuf>) {
    let mut out_lines: Vec<String> = Vec::new();
    let mut image_paths: Vec<PathBuf> = Vec::new();
    let mut in_block = false;

    for line in user_message.lines() {
        let trimmed = line.trim();

        if trimmed == "ATTACHMENTS (image paths)" {
            // Drop a preceding separator if present to avoid leaving a dangling "---".
            if out_lines
                .last()
                .is_some_and(|l| l.trim() == "---" || l.trim() == "—")
            {
                out_lines.pop();
            }
            in_block = true;
            continue;
        }

        if in_block {
            if trimmed.is_empty() {
                in_block = false;
                continue;
            }
            if let Some(rest) = trimmed.strip_prefix("- ") {
                let p = rest.trim();
                if !p.is_empty() {
                    image_paths.push(PathBuf::from(p));
                }
                continue;
            }
            // Unexpected line → end block, keep the line.
            in_block = false;
            out_lines.push(line.to_string());
            continue;
        }

        out_lines.push(line.to_string());
    }

    (out_lines.join("\n"), image_paths)
}

fn encode_image_as_data_url(path: &PathBuf) -> Option<String> {
    use base64::{Engine, engine::general_purpose::STANDARD as BASE64};

    const MAX_IMAGE_BYTES: u64 = 8 * 1024 * 1024; // 8MB per image

    let mime = image_mime_from_path(path)?;

    let meta = std::fs::metadata(path).ok()?;
    if meta.len() > MAX_IMAGE_BYTES {
        warn!(
            "Skipping image attachment (too large, {} bytes): {}",
            meta.len(),
            path.display()
        );
        return None;
    }

    let bytes = std::fs::read(path).ok()?;
    let b64 = BASE64.encode(bytes);
    Some(format!("data:{mime};base64,{b64}"))
}

fn build_responses_user_content(user_message: &str) -> Vec<InputContent> {
    const MAX_IMAGES: usize = 4;

    let (mut cleaned, mut image_paths) = strip_image_attachments(user_message);
    if image_paths.len() > MAX_IMAGES {
        warn!(
            "Too many image attachments ({}); keeping first {}",
            image_paths.len(),
            MAX_IMAGES
        );
        image_paths.truncate(MAX_IMAGES);
    }

    if !image_paths.is_empty() {
        let names = image_paths
            .iter()
            .filter_map(|p| p.file_name().map(|n| n.to_string_lossy().to_string()))
            .collect::<Vec<_>>()
            .join(", ");
        cleaned.push_str("\n\n[Attached images: ");
        cleaned.push_str(&names);
        cleaned.push_str("]\n");
    }

    let mut content = vec![InputContent::Text { text: cleaned }];
    for p in image_paths {
        let Some(url) = encode_image_as_data_url(&p) else {
            warn!("Failed to encode image attachment: {}", p.display());
            continue;
        };
        content.push(InputContent::Image { image_url: url });
    }
    content
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

// No token limits - let the API decide. Tokens are cheap, lost notes are not.

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
    format_text_with_status(text, language, assistive, None)
        .await
        .text
}

/// Format text using AI provider with fallback chain, returning status
pub async fn format_text_with_status(
    text: &str,
    language: Option<&str>,
    assistive: bool,
    on_delta: Option<AiStreamCallback>,
) -> AiFormatResult {
    // Skip very short texts (but not in assistive mode - user might say "help")
    if text.len() < 10 && !assistive {
        return AiFormatResult {
            text: text.to_string(),
            status: AiFormatStatus::Skipped,
        };
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
        // Select prompt based on mode
        let mut system_prompt = if assistive {
            if attempt == 0 {
                let model = get_assistive_model().unwrap_or_else(|_| "unknown".into());
                info!("Using assistive mode (model: {})", model);
            }
            crate::config::prompts::get_assistive_prompt()
        } else {
            if attempt == 0 {
                let model = get_formatting_model().unwrap_or_else(|_| "unknown".into());
                info!("Using formatting mode (model: {})", model);
            }
            crate::config::prompts::get_formatting_prompt()
        };

        // If retrying, wait and strengthen instructions
        if attempt > 0 {
            info!(
                "Retry attempt {}/{} (waiting {:?})",
                attempt, max_retries, retry_delay
            );
            tokio::time::sleep(retry_delay).await;

            // Append critical instruction
            system_prompt.push_str(
                "\n\nCRITICAL: You MUST format/enhance the text. Do NOT return raw input.",
            );
        }

        // Build user message with optional language hint
        let user_message = if let Some(lang) = language {
            format!("[Language: {}]\n\n{}", lang, cleaned)
        } else {
            cleaned.clone()
        };

        // Route based on endpoint path — no domain heuristics
        let endpoint = if assistive {
            get_assistive_endpoint().unwrap_or_default()
        } else {
            get_formatting_endpoint().unwrap_or_default()
        };

        let llm_timeout = Duration::from_secs(90);

        let result_opt = match detect_format(&endpoint) {
            EndpointFormat::OllamaChat => {
                match tokio::time::timeout(
                    attempt_timeout,
                    call_ollama(&user_message, &system_prompt, assistive),
                )
                .await
                {
                    Ok(Ok(formatted)) => Some(formatted),
                    Ok(Err(e)) => {
                        warn!("Ollama failed (attempt {}): {}", attempt, e);
                        None
                    }
                    Err(_) => {
                        warn!(
                            "Ollama timed out after {:?} (attempt {})",
                            attempt_timeout, attempt
                        );
                        None
                    }
                }
            }
            EndpointFormat::ResponsesApi => {
                if use_streaming() {
                    match tokio::time::timeout(
                        llm_timeout,
                        call_llm_endpoint_streaming(
                            &user_message,
                            &system_prompt,
                            assistive,
                            on_delta.clone(),
                        ),
                    )
                    .await
                    {
                        Ok(Ok(formatted)) => Some(formatted),
                        Ok(Err(e)) => {
                            warn!("LLM streaming failed (attempt {}): {}", attempt, e);
                            None
                        }
                        Err(_) => {
                            warn!(
                                "LLM streaming timed out after {:?} (attempt {})",
                                llm_timeout, attempt
                            );
                            None
                        }
                    }
                } else {
                    match tokio::time::timeout(
                        llm_timeout,
                        call_llm_endpoint(&user_message, &system_prompt, assistive),
                    )
                    .await
                    {
                        Ok(Ok(formatted)) => Some(formatted),
                        Ok(Err(e)) => {
                            warn!("LLM endpoint failed (attempt {}): {}", attempt, e);
                            None
                        }
                        Err(_) => {
                            warn!(
                                "LLM endpoint timed out after {:?} (attempt {})",
                                llm_timeout, attempt
                            );
                            None
                        }
                    }
                }
            }
        };

        if let Some(formatted) = result_opt {
            // Detect AI refusal responses (OpenAI content policy)
            let formatted_lower = formatted.to_lowercase();
            let is_refusal = formatted_lower.contains("i'm sorry")
                || formatted_lower.contains("i cannot")
                || formatted_lower.contains("i can't assist")
                || formatted_lower.contains("i can't help")
                || formatted_lower.contains("i'm not able")
                || formatted_lower.contains("as an ai");

            if is_refusal {
                warn!("AI returned refusal response, returning raw input instead");
                return AiFormatResult {
                    text: cleaned,
                    status: AiFormatStatus::Failed,
                };
            }

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
                    let input_has_punct = cleaned_trim.ends_with('.')
                        || cleaned_trim.ends_with('?')
                        || cleaned_trim.ends_with('!');
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
                    let status = if raw_like {
                        AiFormatStatus::Failed
                    } else {
                        AiFormatStatus::Applied
                    };
                    return AiFormatResult {
                        text: formatted,
                        status,
                    };
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
            return AiFormatResult {
                text: formatted,
                status: AiFormatStatus::Applied,
            };
        }
    }

    // All providers failed
    warn!("All AI providers/retries failed, returning cleaned text");
    AiFormatResult {
        text: cleaned,
        status: AiFormatStatus::Failed,
    }
}

/// Call LLM endpoint using /v1/responses API
///
/// Uses mode-aware config: LLM_{FORMATTING,ASSISTIVE}_{ENDPOINT,MODEL,API_KEY}
/// Falls back to LLM_{ENDPOINT,MODEL,API_KEY} if specific vars not set.
async fn call_llm_endpoint(
    user_message: &str,
    system_prompt: &str,
    assistive: bool,
) -> Result<String> {
    // Mode-aware config: formatting vs assistive use different providers
    let (endpoint, model, api_key) = if assistive {
        (
            get_assistive_endpoint()?,
            get_assistive_model()?,
            get_assistive_api_key()?,
        )
    } else {
        (
            get_formatting_endpoint()?,
            get_formatting_model()?,
            get_formatting_api_key()?,
        )
    };

    // Temperature from env (None = skip parameter for models that don't support it)
    let temperature = get_temperature(assistive);

    // Determine AI mode for conversation tracking (separate streams per mode)
    let ai_mode = if assistive {
        crate::state::conversation::AiMode::Assistive
    } else {
        crate::state::conversation::AiMode::Formatting
    };

    // Get previous_response_id for this mode's conversation chain
    let previous_response_id =
        crate::state::conversation::get_previous_response_id_for_mode(ai_mode);

    // TRACE: full chain details for debugging (before model is moved)
    trace!(
        "LLM request chain: endpoint={}, model={}, mode={}, temp={:?}, api_key={}...{}",
        endpoint,
        model,
        if assistive { "assistive" } else { "formatting" },
        temperature,
        &api_key[..8.min(api_key.len())],
        &api_key[api_key.len().saturating_sub(4)..]
    );
    debug!(
        "Calling LLM endpoint {} for {} (temp={:?})",
        endpoint,
        if assistive { "assistive" } else { "formatting" },
        temperature
    );

    // Build Responses API request (no token limit - let API decide)
    let request = ResponsesRequest {
        model,
        input: vec![InputItem {
            role: "user",
            content: build_responses_user_content(user_message),
        }],
        previous_response_id: previous_response_id.clone(),
        // Only send instructions on first request - Responses API preserves them via previous_response_id
        instructions: Some(system_prompt.to_string()),
        max_output_tokens: None,
        temperature,
        stream: false,
    };

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

    // Store response_id for this mode's conversation chain (separate streams)
    crate::state::conversation::set_response_id_for_mode(ai_mode, responses_result.id.clone());
    debug!(
        "Response id ({}): {}",
        if assistive { "assistive" } else { "formatting" },
        responses_result.id
    );
    Ok(formatted)
}

/// Call LLM endpoint with SSE streaming (Responses API)
///
/// Uses mode-aware config: LLM_{FORMATTING,ASSISTIVE}_{ENDPOINT,MODEL,API_KEY}
async fn call_llm_endpoint_streaming(
    user_message: &str,
    system_prompt: &str,
    assistive: bool,
    on_delta: Option<AiStreamCallback>,
) -> Result<String> {
    use futures_util::StreamExt;

    // Mode-aware config: formatting vs assistive use different providers
    let (endpoint, model, api_key) = if assistive {
        (
            get_assistive_endpoint()?,
            get_assistive_model()?,
            get_assistive_api_key()?,
        )
    } else {
        (
            get_formatting_endpoint()?,
            get_formatting_model()?,
            get_formatting_api_key()?,
        )
    };

    // Temperature from env (None = skip parameter for models that don't support it)
    let temperature = get_temperature(assistive);

    // Determine AI mode for conversation tracking (separate streams per mode)
    let ai_mode = if assistive {
        crate::state::conversation::AiMode::Assistive
    } else {
        crate::state::conversation::AiMode::Formatting
    };

    // Get previous_response_id for this mode's conversation chain
    let previous_response_id =
        crate::state::conversation::get_previous_response_id_for_mode(ai_mode);

    // TRACE: full chain details for debugging (before model is moved)
    trace!(
        "SSE request chain: endpoint={}, model={}, mode={}, temp={:?}, api_key={}...{}",
        endpoint,
        model,
        if assistive { "assistive" } else { "formatting" },
        temperature,
        &api_key[..8.min(api_key.len())],
        &api_key[api_key.len().saturating_sub(4)..]
    );
    debug!(
        "SSE streaming to {} for {} (temp={:?})",
        endpoint,
        if assistive { "assistive" } else { "formatting" },
        temperature
    );

    // No token limit - let API decide
    let request = ResponsesRequest {
        model,
        input: vec![InputItem {
            role: "user",
            content: build_responses_user_content(user_message),
        }],
        previous_response_id: previous_response_id.clone(),
        // Only send instructions on first request - Responses API preserves them via previous_response_id
        instructions: Some(system_prompt.to_string()),
        max_output_tokens: None,
        temperature,
        stream: true,
    };

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
                                if let Some(cb) = &on_delta {
                                    cb(&delta);
                                }
                                collected_text.push_str(&delta);
                            }
                        }
                        "response.output_text.done" => {
                            // Full text available - use it if we missed deltas
                            if let Some(text) = chunk.text
                                && collected_text.is_empty()
                            {
                                collected_text = text;
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

    // Store response_id for this mode's conversation chain (separate streams)
    if !response_id.is_empty() {
        crate::state::conversation::set_response_id_for_mode(ai_mode, response_id.clone());
    }

    debug!(
        "SSE complete, response_id ({}): {}",
        if assistive { "assistive" } else { "formatting" },
        response_id
    );
    Ok(formatted)
}

/// Call Ollama/local LLM for text formatting/assistive mode
///
/// Uses mode-aware config. Ollama native API uses /api/chat endpoint format.
async fn call_ollama(user_message: &str, system_prompt: &str, assistive: bool) -> Result<String> {
    // Mode-aware config
    let (host, model) = if assistive {
        (get_assistive_endpoint()?, get_assistive_model()?)
    } else {
        (get_formatting_endpoint()?, get_formatting_model()?)
    };

    // Normalize: strip known path suffixes, then always use /api/chat
    let base_host = host
        .trim_end_matches('/')
        .trim_end_matches("/api/chat")
        .trim_end_matches("/v1/responses")
        .trim_end_matches("/v1");
    let endpoint = format!("{}/api/chat", base_host);

    // Use higher temperature for assistive mode
    let temperature = if assistive { 0.3 } else { 0.1 };

    let messages = build_ollama_messages(system_prompt, user_message, assistive);

    // No token limit - let Ollama decide
    let request = OllamaRequest {
        model,
        messages,
        stream: false,
        options: OllamaOptions {
            temperature,
            num_predict: 0, // 0 = no limit in Ollama
        },
    };

    debug!(
        "Calling Ollama for {} (temp={})",
        if assistive { "assistive" } else { "formatting" },
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

/// Check if AI formatting is available
/// Returns true if at least formatting mode is configured
pub fn has_api_key() -> bool {
    let endpoint = match get_formatting_endpoint() {
        Ok(e) => e,
        Err(_) => return false,
    };

    if get_formatting_model().is_err() {
        return false;
    }

    // OllamaChat doesn't need API key
    if matches!(detect_format(&endpoint), EndpointFormat::OllamaChat) {
        return true;
    }

    // Responses API requires API key
    get_formatting_api_key().is_ok()
}

/// Check if AI formatting is available for report/test flows.
pub fn is_formatting_available() -> bool {
    has_api_key()
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
