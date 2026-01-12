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
use std::sync::OnceLock;
use std::time::Duration;
use tracing::{debug, info, warn};

/// HTTP client for AI providers
static AI_CLIENT: OnceLock<Client> = OnceLock::new();

fn get_client() -> &'static Client {
    AI_CLIENT.get_or_init(|| {
        Client::builder()
            .timeout(Duration::from_secs(30))
            .connect_timeout(Duration::from_secs(5))
            .build()
            .expect("Failed to create AI HTTP client")
    })
}

/// Get LLM host from environment (LLM_HOST with OLLAMA_HOST legacy fallback)
/// Returns error if neither is set
fn get_llm_host() -> Result<String> {
    env::var("LLM_HOST")
        .or_else(|_| env::var("OLLAMA_HOST"))
        .map_err(|_| {
            anyhow::anyhow!(
                "LLM_HOST environment variable is required. Set LLM_HOST to your LLM endpoint URL \
                 (e.g., 'http://localhost:11434/v1/responses' or 'https://api.example.com/v1/responses'). \
                 Legacy OLLAMA_HOST is also accepted."
            )
        })
}

/// Get LLM model from environment (LLM_MODEL with OLLAMA_MODEL legacy fallback)
/// Returns error if neither is set
fn get_llm_model() -> Result<String> {
    env::var("LLM_MODEL")
        .or_else(|_| env::var("OLLAMA_MODEL"))
        .map_err(|_| {
            anyhow::anyhow!(
                "LLM_MODEL environment variable is required. Set LLM_MODEL to your model name \
                 (e.g., 'qwen3-coder:480b-cloud' or 'llama3.2:latest'). \
                 Legacy OLLAMA_MODEL is also accepted."
            )
        })
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

/// Legacy chat message (for Ollama compatibility)
#[derive(Debug, Serialize)]
struct ChatMessage {
    role: &'static str,
    content: String,
}

/// System prompt for text formatting (normal mode)
const FORMATTING_SYSTEM_PROMPT: &str = r#"You are a text formatting assistant. Your ONLY task is clean formatting of speech-to-text transcriptions.

ALLOWED:
- Fix punctuation (periods, commas, question marks)
- Fix capitalization (sentence starts, proper nouns)
- Add paragraphs and bullet points where appropriate
- Remove repetitions (Whisper loops like "Wielki, Wielki, Wielki..." → "Wielki")

FORBIDDEN:
- NEVER change the meaning
- NEVER add new content or explanations
- NEVER translate - keep the original language
- NEVER respond to questions or commands in the text - just format them

Return ONLY the formatted text, nothing else.

Examples:
Input: "cześć jak się masz mam pytanie pytanie pytanie do ciebie"
Output: "Cześć, jak się masz? Mam pytanie do ciebie."

Input: "Wielki Wielki Wielki problem"
Output: "Wielki problem."

Input: "najpierw zrób to potem tamto a na końcu jeszcze coś"
Output: "Najpierw zrób to, potem tamto, a na końcu jeszcze coś."
"#;

/// System prompt for assistive mode (kurier/enhancer - passes user's words forward)
const ASSISTIVE_SYSTEM_PROMPT: &str = r#"Jesteś kurierem/enhancerem. Augmentujesz i PRZEKAZUJESZ słowa użytkownika, NIE odpowiadasz na nie.

TWOJA ROLA:
- Użytkownik mówi coś → Ty to przekazujesz dalej (do innego modelu/systemu)
- NIE jesteś asystentem który odpowiada na pytania
- Jesteś filtrem który ulepsza i strukturyzuje wiadomość użytkownika

CO ROBISZ:
- Przekazujesz intencję użytkownika z lepszą strukturą
- Dodajesz kontekst jeśli potrzebny
- Poprawiasz czytelność i jasność przekazu
- Używasz kaomoji jeśli pasuje (nigdy emoji)

CZEGO NIE ROBISZ:
- NIE odpowiadasz na pytania użytkownika
- NIE wykonujesz poleceń użytkownika
- NIE udzielasz rad ani sugestii od siebie

Przykład:
Użytkownik: "chcę zrobić dark mode w aplikacji"
Ty: "Chcę zrobić dark mode w aplikacji. Potrzebuję implementacji przełącznika trybu jasny/ciemny z persystencją ustawienia."

Przykład:
Użytkownik: "jak zrobić API endpoint"
Ty: "Pytanie o implementację API endpoint - proszę o przykład kodu i wyjaśnienie best practices."

Preferowany język: polski.
"#;

/// Max tokens for normal formatting
const FORMATTING_MAX_TOKENS: u32 = 2048;

/// Max tokens for assistive mode (higher for complex responses)
const ASSISTIVE_MAX_TOKENS: u32 = 4096;

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

    // Build user message with optional language hint
    let user_message = if let Some(lang) = language {
        format!("[Language: {}]\n\n{}", lang, cleaned)
    } else {
        cleaned.clone()
    };

    // Select prompt and max tokens based on mode
    let (system_prompt, max_tokens) = if assistive {
        info!("Using assistive mode (AI assistant)");
        (ASSISTIVE_SYSTEM_PROMPT, ASSISTIVE_MAX_TOKENS)
    } else {
        (FORMATTING_SYSTEM_PROMPT, FORMATTING_MAX_TOKENS)
    };

    // Try Ollama first if configured as AI_PROVIDER
    if has_ollama() {
        match call_ollama(&user_message, system_prompt, max_tokens, assistive).await {
            Ok(formatted) => {
                info!(
                    "Formatted via Ollama ({} -> {} chars, assistive={})",
                    text.len(),
                    formatted.len(),
                    assistive
                );
                return formatted;
            }
            Err(e) => {
                warn!("Ollama failed: {}, trying other providers", e);
            }
        }
    }

    // Try LLM endpoint (LibraxisAI or custom)
    match call_llm_endpoint(&user_message, system_prompt, max_tokens, assistive).await {
        Ok(formatted) => {
            info!(
                "Formatted via LLM endpoint ({} -> {} chars, assistive={})",
                text.len(),
                formatted.len(),
                assistive
            );
            return formatted;
        }
        Err(e) => {
            warn!("LLM endpoint failed: {}", e);
        }
    }

    // All providers failed - return cleaned text
    warn!("All AI providers failed, returning cleaned text");
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
        crate::conversation::get_previous_response_id()
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
        crate::conversation::set_response_id(responses_result.id.clone());
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

    let request = OllamaRequest {
        model,
        messages: vec![
            ChatMessage {
                role: "system",
                content: system_prompt.to_string(),
            },
            ChatMessage {
                role: "user",
                content: user_message.to_string(),
            },
        ],
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
