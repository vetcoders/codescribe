//! AI-powered text formatting service
//!
//! Uses OpenAI (primary) and Libraxis (fallback) for:
//! - Text formatting and grammar correction
//! - Punctuation and capitalization
//! - Anti-repetition filtering (fixes Whisper loops like "Wielki, Wielki...")
//! - Language-specific formatting

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

/// AI Provider configuration
#[derive(Debug, Clone)]
pub struct ProviderConfig {
    pub name: &'static str,
    pub endpoint: &'static str,
    pub api_key_env: &'static str,
    pub model: &'static str,
}

/// OpenAI provider (primary)
const OPENAI: ProviderConfig = ProviderConfig {
    name: "OpenAI",
    endpoint: "https://api.openai.com/v1/chat/completions",
    api_key_env: "OPENAI_API_KEY",
    model: "gpt-4o-mini",
};

/// Libraxis provider (fallback)
const LIBRAXIS: ProviderConfig = ProviderConfig {
    name: "Libraxis",
    endpoint: "https://api.libraxis.cloud/v1/chat/completions",
    api_key_env: "LIBRAXIS_API_KEY",
    model: "chat",
};

/// Fallback chain: OpenAI -> Libraxis
const PROVIDER_CHAIN: &[ProviderConfig] = &[OPENAI, LIBRAXIS];

/// Chat completion request (OpenAI-compatible)
#[derive(Debug, Serialize)]
struct ChatRequest {
    model: String,
    messages: Vec<ChatMessage>,
    max_tokens: u32,
    temperature: f32,
}

#[derive(Debug, Serialize)]
struct ChatMessage {
    role: &'static str,
    content: String,
}

/// Chat completion response (OpenAI-compatible)
#[derive(Debug, Deserialize)]
struct ChatResponse {
    choices: Vec<ChatChoice>,
}

#[derive(Debug, Deserialize)]
struct ChatChoice {
    message: ResponseMessage,
}

#[derive(Debug, Deserialize)]
struct ResponseMessage {
    content: String,
}

/// System prompt for text formatting
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

/// Simple local repetition cleanup (no AI needed)
pub fn remove_simple_repetitions(text: &str) -> String {
    let words: Vec<&str> = text.split_whitespace().collect();
    if words.is_empty() {
        return text.to_string();
    }

    let mut result: Vec<&str> = Vec::new();
    let mut i = 0;

    while i < words.len() {
        // Check for consecutive same words
        let mut repeat_count = 1;
        while i + repeat_count < words.len()
            && words[i + repeat_count].to_lowercase() == words[i].to_lowercase()
        {
            repeat_count += 1;
        }

        // Keep only one occurrence
        result.push(words[i]);
        i += repeat_count;
    }

    result.join(" ")
}

/// Format text using AI provider with fallback chain
///
/// # Arguments
/// * `text` - Raw text from transcription
/// * `language` - Optional language hint (e.g., "pl", "en")
///
/// # Returns
/// Formatted text or original if all providers fail
pub async fn format_text(text: &str, language: Option<&str>) -> String {
    // Skip very short texts
    if text.len() < 10 {
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

    // Try each provider in chain
    for provider in PROVIDER_CHAIN {
        match call_provider(provider, &user_message).await {
            Ok(formatted) => {
                info!(
                    "Formatted via {} ({} -> {} chars)",
                    provider.name,
                    text.len(),
                    formatted.len()
                );
                return formatted;
            }
            Err(e) => {
                warn!("Provider {} failed: {}", provider.name, e);
                continue;
            }
        }
    }

    // All providers failed - return cleaned text
    warn!("All AI providers failed, returning cleaned text");
    cleaned
}

/// Call a single AI provider
async fn call_provider(provider: &ProviderConfig, user_message: &str) -> Result<String> {
    let api_key = env::var(provider.api_key_env)
        .context(format!("{} not set", provider.api_key_env))?;

    if api_key.is_empty() {
        anyhow::bail!("{} is empty", provider.api_key_env);
    }

    let request = ChatRequest {
        model: provider.model.to_string(),
        messages: vec![
            ChatMessage {
                role: "system",
                content: FORMATTING_SYSTEM_PROMPT.to_string(),
            },
            ChatMessage {
                role: "user",
                content: user_message.to_string(),
            },
        ],
        max_tokens: 2048,
        temperature: 0.1, // Low temperature for consistent formatting
    };

    debug!("Calling {} for formatting", provider.name);

    let response = get_client()
        .post(provider.endpoint)
        .header("Authorization", format!("Bearer {}", api_key))
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

    let chat_response: ChatResponse = response
        .json()
        .await
        .context("Failed to parse response")?;

    let formatted = chat_response
        .choices
        .first()
        .map(|c| c.message.content.trim().to_string())
        .ok_or_else(|| anyhow::anyhow!("No response content"))?;

    // Sanity check - formatted shouldn't be empty or much longer than original
    if formatted.is_empty() || formatted.len() > user_message.len() * 2 {
        anyhow::bail!("Invalid response length");
    }

    Ok(formatted)
}

/// Check if any AI provider is configured
pub fn has_api_key() -> bool {
    for provider in PROVIDER_CHAIN {
        if env::var(provider.api_key_env)
            .map(|k| !k.is_empty())
            .unwrap_or(false)
        {
            return true;
        }
    }
    false
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
        assert_eq!(
            remove_simple_repetitions("Wielki Wielki Wielki problem"),
            "Wielki problem"
        );
        assert_eq!(
            remove_simple_repetitions("Kali Kali Kali Kali bogini"),
            "Kali bogini"
        );
        assert_eq!(
            remove_simple_repetitions("test test test"),
            "test"
        );
        // Should preserve normal text
        assert_eq!(
            remove_simple_repetitions("normalny tekst bez powtórzeń"),
            "normalny tekst bez powtórzeń"
        );
    }
}
