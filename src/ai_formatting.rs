//! AI-powered text formatting service
//!
//! Uses LibraxisAI Responses API (/v1/responses) for:
//! - Text formatting and grammar correction
//! - Punctuation and capitalization
//! - Anti-repetition filtering (fixes Whisper loops like "Wielki, Wielki...")
//! - Language-specific formatting
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

/// Default LLM endpoint URL (used if LLM_ENDPOINT env var is not set)
const DEFAULT_LLM_ENDPOINT: &str = "https://api.libraxis.cloud/v1/responses";

/// Default LLM model name
const DEFAULT_LLM_MODEL: &str = "chat";

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
    #[serde(rename = "type")]
    item_type: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    role: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    content: Option<String>,
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
const FORMATTING_SYSTEM_PROMPT: &str = r#"You are a text formatting assistant. Your task is to clean up speech-to-text transcriptions.

Rules:
1. Fix punctuation (add periods, commas, question marks where appropriate)
2. Fix capitalization (start sentences with capitals, proper nouns)
3. IMPORTANT: Remove repetitions - if a word/phrase repeats multiple times (like "Wielki, Wielki, Wielki..."), keep only ONE occurrence
4. Do NOT change the meaning or add new content
5. Do NOT translate - keep the original language
6. Return ONLY the corrected text, nothing else

Example input: "cześć jak się masz mam pytanie pytanie pytanie do ciebie"
Example output: "Cześć, jak się masz? Mam pytanie do ciebie."

Example input: "Wielki Wielki Wielki problem"
Example output: "Wielki problem."

Example input: "Kali Kali Kali Kali bogini"
Example output: "Kali, bogini."
"#;

/// System prompt for assistive mode (contextual AI assistant)
const ASSISTIVE_SYSTEM_PROMPT: &str = r#"Jesteś asystentem kontekstowym dla programisty i weterynarza. Pomagasz przy transkrypcjach i zadaniach.

Twoje zadania:
1. Rozumiesz kontekst i intencję użytkownika
2. Odpowiadasz konkretnie i pomocnie
3. Formatujesz odpowiedzi czytelnie
4. Możesz planować, sugerować, wyjaśniać
5. Używaj kaomoji jeśli pasuje, ale nigdy emoji

Zachowuj się jak kolega-programista który rozumie co użytkownik chce osiągnąć.
Odpowiadaj w tym samym języku co użytkownik (zwykle polski).
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
/// Reads endpoint URL from LLM_ENDPOINT env var (falls back to DEFAULT_LLM_ENDPOINT).
/// Reads model from LLM_MODEL env var (falls back to DEFAULT_LLM_MODEL).
/// Reads API key from LLM_API_KEY env var.
async fn call_llm_endpoint(
    user_message: &str,
    system_prompt: &str,
    max_tokens: u32,
    assistive: bool,
) -> Result<String> {
    let endpoint = env::var("LLM_ENDPOINT").unwrap_or_else(|_| DEFAULT_LLM_ENDPOINT.to_string());
    let model = env::var("LLM_MODEL").unwrap_or_else(|_| DEFAULT_LLM_MODEL.to_string());
    let api_key = env::var("LLM_API_KEY").context("LLM_API_KEY not set")?;

    if api_key.is_empty() {
        anyhow::bail!("LLM_API_KEY is empty");
    }

    // Use higher temperature for assistive mode (more creative responses)
    let temperature = if assistive { 0.3 } else { 0.1 };

    // Get previous_response_id for conversation continuity (only in assistive mode)
    let previous_response_id = if assistive {
        codescribe::conversation::get_previous_response_id()
    } else {
        None
    };

    // Build Responses API request
    let request = ResponsesRequest {
        model,
        input: vec![InputItem {
            item_type: "message",
            role: Some("user"),
            content: Some(user_message.to_string()),
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
        codescribe::conversation::set_response_id(responses_result.id.clone());
    }

    // Sanity check - in assistive mode, allow longer responses
    let max_len_multiplier = if assistive { 5 } else { 2 };
    if formatted.len() > user_message.len() * max_len_multiplier {
        anyhow::bail!("Response too long");
    }

    debug!("Response id: {}", responses_result.id);
    Ok(formatted)
}

/// Call Ollama/local LLM for text formatting/assistive mode
async fn call_ollama(
    user_message: &str,
    system_prompt: &str,
    max_tokens: u32,
    assistive: bool,
) -> Result<String> {
    // Unified naming: LLM_HOST, LLM_MODEL (with legacy OLLAMA_* fallback)
    let host = env::var("LLM_HOST")
        .or_else(|_| env::var("OLLAMA_HOST"))
        .unwrap_or_else(|_| "http://127.0.0.1:11434".to_string());
    let model = env::var("LLM_MODEL")
        .or_else(|_| env::var("OLLAMA_MODEL"))
        .unwrap_or_else(|_| "qwen3:8b".to_string());
    let endpoint = format!("{}/api/chat", host.trim_end_matches('/'));

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

/// Check if local LLM (Ollama) is configured
fn has_ollama() -> bool {
    // Check if LLM_HOST points to localhost (Ollama)
    let host = env::var("LLM_HOST")
        .or_else(|_| env::var("OLLAMA_HOST"))
        .unwrap_or_default();

    host.contains("127.0.0.1") || host.contains("localhost")
}

/// Check if any AI provider is configured
pub fn has_api_key() -> bool {
    // Ollama doesn't need an API key
    if has_ollama() {
        return true;
    }

    // Check for LLM_API_KEY
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
