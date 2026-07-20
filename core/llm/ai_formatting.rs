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
use serde_json::Value;
use std::env;
use std::sync::{Arc, OnceLock, RwLock};
use std::time::Duration;
use tracing::{debug, info, trace, warn};

use crate::config::{Config, FormattingPolicy};

use super::lane_truth;
use super::provider::{LlmMode, ProviderKind, capability_policy};
use super::responses_streaming_manager::{ResponsesStreamingManager, StreamCallbacks};

/// HTTP client for AI providers
static AI_CLIENT: OnceLock<Client> = OnceLock::new();

/// Non-assistive formatting skips only extremely short transcripts.
/// Short-but-real utterances still flow through AI formatting; the controller
/// owns the separate quality-gate logic for that 10-23 char window.
const NON_ASSISTIVE_AI_SKIP_CHARS: usize = 10;

fn should_skip_ai_formatting(text: &str, assistive: bool) -> bool {
    !assistive && text.chars().count() < NON_ASSISTIVE_AI_SKIP_CHARS
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AiFormatStatus {
    Applied,
    Failed,
    Skipped,
    AiNoop,
}

pub type AiStreamCallback = Arc<dyn Fn(&str) + Send + Sync>;
pub type AiReasoningCallback = Arc<dyn Fn(&str) + Send + Sync>;

#[derive(Debug, Clone)]
pub struct AiFormatResult {
    pub text: String,
    pub reasoning_text: Option<String>,
    pub status: AiFormatStatus,
}

#[derive(Debug, Clone)]
struct ProviderOutput {
    assistant_text: String,
    reasoning_text: Option<String>,
}

#[derive(Clone)]
struct StreamRequestContext {
    callbacks: StreamCallbacks,
    initial_response_timeout: Duration,
    inter_chunk_timeout: Duration,
}

#[derive(Clone)]
struct MemoryMessage {
    role: String,
    content: String,
}

static OLLAMA_MEMORY: OnceLock<RwLock<Vec<MemoryMessage>>> = OnceLock::new();
const MAX_OLLAMA_MEMORY_CHARS: usize = 4000;

// Retry count is "extra attempts after the first request". Default 0 keeps
// daily-driver formatting fail-fast instead of multiplying deterministic
// provider/parser failures into long cascades.
const DEFAULT_AI_MAX_RETRIES: u32 = 0;
const DEFAULT_AI_RETRY_DELAY_MS: u64 = 500;
// Bumped from 30s → 90s (2026-05-13). Operator observed
// "Agent SSE inter-chunk timeout after 30s" mid-stream from chat overlay
// during longer responses (multi-paragraph PL text with code blocks).
// LLM backends ('programmer' model on api.libraxis.cloud) emit tokens
// in bursts with 5-15s pauses for reasoning/tool-call hops; 30s budget
// was too tight and triggered "Agent runtime unavailable. Using legacy
// formatter" fallback mid-response, breaking the assistant UX. 90s
// keeps streams alive across realistic backend hiccups without making
// stalled requests linger forever. Env override `CODESCRIBE_AI_*_MS`
// still wins for power users (operator can lower for fast models).
const DEFAULT_AI_ATTEMPT_TIMEOUT_MS: u64 = 90_000;
const DEFAULT_AI_OLLAMA_ATTEMPT_TIMEOUT_MS: u64 = 5_000;
const DEFAULT_AI_INTER_CHUNK_TIMEOUT_MS: u64 = 90_000;
const DEFAULT_AI_CLIENT_TIMEOUT_MS: u64 = 90_000;
const DEFAULT_AI_CONNECT_TIMEOUT_MS: u64 = 5_000;
const DEFAULT_AI_POOL_IDLE_TIMEOUT_MS: u64 = 90_000;
const DEFAULT_AI_TCP_KEEPALIVE_MS: u64 = 30_000;
const ANTHROPIC_VERSION: &str = "2023-06-01";
const DEFAULT_ANTHROPIC_MAX_TOKENS: u32 = 8192;
const THREAD_TITLE_TIMEOUT: Duration = Duration::from_secs(8);
const THREAD_TITLE_MAX_TOKENS: u32 = 24;
const THREAD_TITLE_MAX_CHARS: usize = 72;
const THREAD_TITLE_PROMPT: &str = "Create a concise 3-6 word title for this conversation. \
Use the user's language and a descriptive noun phrase. Return only the title on one line, \
with no quotes, bullet, label, or decorative punctuation.";

#[derive(Debug, Clone, Copy)]
struct RetryPolicy {
    max_retries: u32,
    retry_delay: Duration,
    attempt_timeout: Duration,
    ollama_attempt_timeout: Duration,
    inter_chunk_timeout: Duration,
}

impl RetryPolicy {
    fn from_env() -> Self {
        Self {
            max_retries: env_u32("CODESCRIBE_AI_MAX_RETRIES", DEFAULT_AI_MAX_RETRIES),
            retry_delay: duration_from_env_ms(
                "CODESCRIBE_AI_RETRY_DELAY_MS",
                DEFAULT_AI_RETRY_DELAY_MS,
            ),
            attempt_timeout: duration_from_env_ms(
                "CODESCRIBE_AI_ATTEMPT_TIMEOUT_MS",
                DEFAULT_AI_ATTEMPT_TIMEOUT_MS,
            ),
            ollama_attempt_timeout: duration_from_env_ms(
                "CODESCRIBE_AI_OLLAMA_ATTEMPT_TIMEOUT_MS",
                DEFAULT_AI_OLLAMA_ATTEMPT_TIMEOUT_MS,
            ),
            inter_chunk_timeout: duration_from_env_ms(
                "CODESCRIBE_AI_INTER_CHUNK_TIMEOUT_MS",
                DEFAULT_AI_INTER_CHUNK_TIMEOUT_MS,
            ),
        }
    }
}

fn env_u32(key: &str, default: u32) -> u32 {
    env::var(key)
        .ok()
        .and_then(|v| v.parse::<u32>().ok())
        .unwrap_or(default)
}

fn env_u64(key: &str, default: u64) -> u64 {
    env::var(key)
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(default)
}

fn duration_from_env_ms(key: &str, default_ms: u64) -> Duration {
    Duration::from_millis(env_u64(key, default_ms))
}

fn should_retry_provider_error(error: &anyhow::Error) -> bool {
    let message = error.to_string();
    !(message.contains("No text content in SSE stream")
        || message.contains("No text content in response")
        || message.contains("No text content in Anthropic response")
        || message.contains("Anthropic refusal stop")
        || message.contains("Anthropic response truncated")
        || message.contains("SSE error internal_error")
        || message.contains("SSE error bad_request"))
}

fn ollama_memory() -> &'static RwLock<Vec<MemoryMessage>> {
    OLLAMA_MEMORY.get_or_init(|| RwLock::new(Vec::new()))
}

fn get_client() -> &'static Client {
    AI_CLIENT.get_or_init(|| {
        let timeout = duration_from_env_ms(
            "CODESCRIBE_AI_CLIENT_TIMEOUT_MS",
            DEFAULT_AI_CLIENT_TIMEOUT_MS,
        );
        let connect_timeout = duration_from_env_ms(
            "CODESCRIBE_AI_CONNECT_TIMEOUT_MS",
            DEFAULT_AI_CONNECT_TIMEOUT_MS,
        );
        let pool_idle_timeout = duration_from_env_ms(
            "CODESCRIBE_AI_POOL_IDLE_TIMEOUT_MS",
            DEFAULT_AI_POOL_IDLE_TIMEOUT_MS,
        );
        let tcp_keepalive = duration_from_env_ms(
            "CODESCRIBE_AI_TCP_KEEPALIVE_MS",
            DEFAULT_AI_TCP_KEEPALIVE_MS,
        );

        Client::builder()
            .timeout(timeout)
            .connect_timeout(connect_timeout)
            .pool_idle_timeout(pool_idle_timeout)
            .tcp_keepalive(tcp_keepalive)
            .build()
            .expect("Failed to create AI HTTP client")
    })
}

// ============================================================================
// LLM Configuration - Separate providers for Formatting vs Assistive
// ============================================================================
//
// Contract: LLM_{FORMATTING,ASSISTIVE}_{ENDPOINT,MODEL,API_KEY}
//
// FORMATTING mode (cheap, fast): punctuation, structure, cleanup
// ASSISTIVE mode (smart): Voice Chat, AI assistant
//
// NO legacy variables. Clean contract only.

// ---- FORMATTING mode config ----

fn get_formatting_endpoint() -> Result<String> {
    Ok(lane_truth::endpoint(LlmMode::Formatting, &Config::load()))
}

fn get_formatting_model() -> Result<String> {
    Ok(lane_truth::formatting_identity(&Config::load()).1)
}

fn get_formatting_api_key() -> Result<String> {
    lane_truth::secret("LLM_FORMATTING_API_KEY")
        .context("LLM API key (formatting) is required. Set LLM_FORMATTING_API_KEY.")
}

// ---- ASSISTIVE mode config ----

fn get_assistive_endpoint() -> Result<String> {
    Ok(lane_truth::assistive_snapshot(&Config::load()).endpoint)
}

fn get_assistive_model() -> Result<String> {
    Ok(lane_truth::assistive_snapshot(&Config::load()).model)
}

fn get_anthropic_api_key() -> Result<String> {
    let account = ProviderKind::AnthropicMessages.api_key_env_key();
    lane_truth::secret(account)
        .with_context(|| format!("Anthropic API key is required. Set {account}."))
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
    /// Anthropic Messages API (/v1/messages)
    AnthropicMessages,
}

#[derive(Debug, Clone)]
struct ThreadTitleProvider {
    format: EndpointFormat,
    endpoint: String,
    model: String,
    api_key: Option<String>,
}

/// Resolve request format from explicit provider, preserving path-based Ollama
/// compatibility only for the protected OpenAI/default lane.
fn detect_format(endpoint: &str, provider: ProviderKind) -> EndpointFormat {
    match provider {
        ProviderKind::AnthropicMessages => EndpointFormat::AnthropicMessages,
        ProviderKind::OpenAiResponses if endpoint.contains("/api/chat") => {
            EndpointFormat::OllamaChat
        }
        ProviderKind::OpenAiResponses => EndpointFormat::ResponsesApi,
    }
}

/// Generate one isolated title through the currently selected formatting lane.
///
/// This path deliberately does not call any formatting/assistive request helper:
/// it sends exactly one bounded JSON request, passes only `text` as user input,
/// and never reads or writes response-chain or Ollama memory state.
pub async fn generate_thread_title(text: &str) -> Result<Option<String>> {
    if text.trim().is_empty() {
        return Ok(None);
    }
    let provider = resolve_thread_title_provider()?;
    generate_thread_title_with_provider(text, &provider, THREAD_TITLE_TIMEOUT).await
}

fn resolve_thread_title_provider() -> Result<ThreadTitleProvider> {
    let config = Config::load();
    let provider = lane_truth::provider(LlmMode::Formatting);
    let model = lane_truth::model_for_provider(LlmMode::Formatting, provider, &config);
    let endpoint = match provider {
        ProviderKind::OpenAiResponses => lane_truth::endpoint(LlmMode::Formatting, &config),
        ProviderKind::AnthropicMessages => lane_truth::anthropic_messages_endpoint(),
    };
    let format = detect_format(&endpoint, provider);
    let api_key = match format {
        EndpointFormat::ResponsesApi => Some(get_formatting_api_key()?),
        EndpointFormat::AnthropicMessages => Some(get_anthropic_api_key()?),
        EndpointFormat::OllamaChat => None,
    };

    Ok(ThreadTitleProvider {
        format,
        endpoint,
        model,
        api_key,
    })
}

async fn generate_thread_title_with_provider(
    text: &str,
    provider: &ThreadTitleProvider,
    timeout: Duration,
) -> Result<Option<String>> {
    let raw = tokio::time::timeout(timeout, request_thread_title(text, provider))
        .await
        .context("Thread title request timed out after 8 seconds")??;
    Ok(sanitize_thread_title(&raw))
}

async fn request_thread_title(text: &str, provider: &ThreadTitleProvider) -> Result<String> {
    match provider.format {
        EndpointFormat::ResponsesApi => request_responses_thread_title(text, provider).await,
        EndpointFormat::AnthropicMessages => request_anthropic_thread_title(text, provider).await,
        EndpointFormat::OllamaChat => request_ollama_thread_title(text, provider).await,
    }
}

async fn request_responses_thread_title(
    text: &str,
    provider: &ThreadTitleProvider,
) -> Result<String> {
    let api_key = provider
        .api_key
        .as_deref()
        .context("Formatting API key is required for thread titles")?;
    let request = ResponsesRequest {
        model: provider.model.clone(),
        input: vec![InputItem {
            role: "user",
            content: vec![InputContent::Text {
                text: text.to_string(),
            }],
        }],
        previous_response_id: None,
        instructions: Some(THREAD_TITLE_PROMPT.to_string()),
        max_output_tokens: Some(THREAD_TITLE_MAX_TOKENS),
        temperature: None,
        stream: false,
    };

    let response = get_client()
        .post(&provider.endpoint)
        .header("Authorization", format!("Bearer {api_key}"))
        .header("x-api-key", api_key)
        .header("Content-Type", "application/json")
        .json(&request)
        .send()
        .await
        .context("Thread title Responses request failed")?;
    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        anyhow::bail!("Thread title HTTP {status} - {body}");
    }

    let response: ResponsesResponse = response
        .json()
        .await
        .context("Failed to parse thread title Responses response")?;
    Ok(extract_output_channels(&response.output).assistant_text)
}

async fn request_anthropic_thread_title(
    text: &str,
    provider: &ThreadTitleProvider,
) -> Result<String> {
    let api_key = provider
        .api_key
        .as_deref()
        .context("Anthropic API key is required for thread titles")?;
    let endpoint = lane_truth::normalize_anthropic_messages_endpoint(&provider.endpoint);
    let request = AnthropicMessagesRequest {
        model: provider.model.clone(),
        system: Some(THREAD_TITLE_PROMPT.to_string()),
        messages: vec![AnthropicMessage {
            role: "user",
            content: vec![AnthropicContentBlock::Text {
                text: text.to_string(),
            }],
        }],
        max_tokens: THREAD_TITLE_MAX_TOKENS,
        temperature: None,
    };

    let response = get_client()
        .post(&endpoint)
        .header("x-api-key", api_key)
        .header("anthropic-version", ANTHROPIC_VERSION)
        .header("Content-Type", "application/json")
        .json(&request)
        .send()
        .await
        .context("Thread title Anthropic request failed")?;
    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        anyhow::bail!("Thread title Anthropic HTTP {status} - {body}");
    }

    let response: AnthropicMessagesResponse = response
        .json()
        .await
        .context("Failed to parse thread title Anthropic response")?;
    if matches!(response.stop_reason.as_deref(), Some("refusal")) {
        anyhow::bail!(
            "Anthropic refusal stop (id: {}): {}",
            anthropic_response_id(&response),
            anthropic_stop_detail(&response)
        );
    }
    Ok(extract_anthropic_text(&response))
}

async fn request_ollama_thread_title(text: &str, provider: &ThreadTitleProvider) -> Result<String> {
    let request = OllamaRequest {
        model: provider.model.clone(),
        messages: vec![
            ChatMessage {
                role: "system".to_string(),
                content: THREAD_TITLE_PROMPT.to_string(),
            },
            ChatMessage {
                role: "user".to_string(),
                content: text.to_string(),
            },
        ],
        stream: false,
        options: OllamaOptions {
            temperature: 0.1,
            num_predict: THREAD_TITLE_MAX_TOKENS,
        },
    };

    let endpoint = normalize_ollama_chat_endpoint(&provider.endpoint);
    let response = get_client()
        .post(&endpoint)
        .header("Content-Type", "application/json")
        .json(&request)
        .send()
        .await
        .context("Thread title Ollama request failed")?;
    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        anyhow::bail!("Thread title Ollama HTTP {status} - {body}");
    }

    let response: OllamaResponse = response
        .json()
        .await
        .context("Failed to parse thread title Ollama response")?;
    Ok(response
        .message
        .map(|message| message.content)
        .or(response.response)
        .unwrap_or_default())
}

fn normalize_ollama_chat_endpoint(endpoint: &str) -> String {
    let mut base = endpoint.trim().trim_end_matches('/').to_string();
    loop {
        let previous_len = base.len();
        for suffix in ["/v1/responses", "/api/chat", "/v1"] {
            if base.ends_with(suffix) {
                base.truncate(base.len() - suffix.len());
                break;
            }
        }
        if base.len() == previous_len {
            break;
        }
    }
    format!("{base}/api/chat")
}

fn sanitize_thread_title(raw: &str) -> Option<String> {
    let mut title = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    if title.is_empty() {
        return None;
    }

    title = strip_title_bullet(&title).to_string();
    title = strip_title_wrapping(&title).to_string();
    title = title.split_whitespace().collect::<Vec<_>>().join(" ");
    if title.is_empty() {
        return None;
    }

    let clipped = title
        .chars()
        .take(THREAD_TITLE_MAX_CHARS)
        .collect::<String>();
    (!clipped.trim().is_empty()).then_some(clipped)
}

fn strip_title_bullet(title: &str) -> &str {
    let trimmed = title.trim();
    for prefix in ["- ", "* ", "• ", "– ", "— "] {
        if let Some(rest) = trimmed.strip_prefix(prefix) {
            return rest.trim();
        }
    }

    let digit_count = trimmed.chars().take_while(char::is_ascii_digit).count();
    if digit_count > 0 {
        let rest = &trimmed[digit_count..];
        if let Some(rest) = rest.strip_prefix(". ").or_else(|| rest.strip_prefix(") ")) {
            return rest.trim();
        }
    }
    trimmed
}

fn strip_title_wrapping(title: &str) -> &str {
    let trimmed = title.trim();
    for (open, close) in [
        ("**", "**"),
        ("__", "__"),
        ("\"", "\""),
        ("'", "'"),
        ("`", "`"),
        ("“", "”"),
        ("„", "”"),
    ] {
        if let Some(inner) = trimmed
            .strip_prefix(open)
            .and_then(|value| value.strip_suffix(close))
        {
            return inner.trim();
        }
    }
    trimmed
}

/// Streaming is mandatory for chat/assistant UX consistency.
/// `LLM_USE_STREAMING` is intentionally ignored.
fn use_streaming() -> bool {
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

/// Anthropic Messages request format (/v1/messages)
#[derive(Debug, Serialize)]
struct AnthropicMessagesRequest {
    model: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    system: Option<String>,
    messages: Vec<AnthropicMessage>,
    max_tokens: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
}

#[derive(Debug, Serialize)]
struct AnthropicMessage {
    role: &'static str,
    content: Vec<AnthropicContentBlock>,
}

#[derive(Debug, Serialize)]
#[serde(tag = "type")]
enum AnthropicContentBlock {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "image")]
    Image { source: AnthropicImageSource },
}

#[derive(Debug, Serialize)]
struct AnthropicImageSource {
    #[serde(rename = "type")]
    source_type: &'static str,
    media_type: String,
    data: String,
}

fn encode_image_as_data_url(path: &std::path::Path) -> Option<String> {
    use base64::{Engine, engine::general_purpose::STANDARD as BASE64};

    // Marker parsing, MIME mapping and the size cap are shared with the agent
    // send path via `crate::attachment` so both routes honor one contract.
    let (bytes, mime) =
        crate::attachment::load_image_for_vision(path, crate::attachment::MAX_VISION_IMAGE_BYTES)?;
    let b64 = BASE64.encode(bytes);
    Some(format!("data:{mime};base64,{b64}"))
}

fn build_responses_user_content(user_message: &str) -> Vec<InputContent> {
    // Kept in sync with `MAX_AGENT_VISION_IMAGES` in the agent send path.
    const MAX_IMAGES: usize = 16;

    let (mut cleaned, mut image_paths) =
        crate::attachment::parse_image_attachment_block(user_message);
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

fn build_anthropic_user_content(user_message: &str) -> Vec<AnthropicContentBlock> {
    // Kept in sync with `MAX_AGENT_VISION_IMAGES` in the agent send path.
    const MAX_IMAGES: usize = 16;

    let (mut cleaned, mut image_paths) =
        crate::attachment::parse_image_attachment_block(user_message);
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

    let mut content = Vec::new();
    if !cleaned.is_empty() || image_paths.is_empty() {
        content.push(AnthropicContentBlock::Text { text: cleaned });
    }

    for p in image_paths {
        let Some(url) = encode_image_as_data_url(&p) else {
            warn!("Failed to encode image attachment: {}", p.display());
            continue;
        };
        let Some(source) = anthropic_image_source_from_data_url(&url) else {
            warn!(
                "Failed to convert image attachment for Anthropic: {}",
                p.display()
            );
            continue;
        };
        content.push(AnthropicContentBlock::Image { source });
    }
    content
}

fn anthropic_image_source_from_data_url(url: &str) -> Option<AnthropicImageSource> {
    let payload = url.strip_prefix("data:")?;
    let (media_type, data) = payload.split_once(";base64,")?;
    Some(AnthropicImageSource {
        source_type: "base64",
        media_type: media_type.to_string(),
        data: data.to_string(),
    })
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
    #[serde(default)]
    summary: Option<String>,
}

/// Anthropic Messages response format
#[derive(Debug, Deserialize)]
struct AnthropicMessagesResponse {
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    content: Vec<AnthropicResponseContent>,
    #[serde(default)]
    stop_reason: Option<String>,
    #[serde(default)]
    stop_details: Option<Value>,
}

#[derive(Debug, Deserialize)]
struct AnthropicResponseContent {
    #[serde(rename = "type")]
    part_type: String,
    #[serde(default)]
    text: Option<String>,
}

/// Legacy chat message (for Ollama compatibility)
#[derive(Debug, Serialize, Clone)]
struct ChatMessage {
    role: String,
    content: String,
}

fn part_text(part: &ContentPart) -> Option<&str> {
    part.text
        .as_deref()
        .or(part.summary.as_deref())
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

fn extract_output_channels(output: &[OutputItem]) -> ProviderOutput {
    let mut assistant_parts = Vec::new();
    let mut reasoning_parts = Vec::new();

    for item in output {
        let Some(parts) = item.content.as_ref() else {
            continue;
        };
        let is_message = item.item_type == "message";
        let is_reasoning = item.item_type == "reasoning";

        for part in parts {
            match part.part_type.as_str() {
                "output_text" | "text" if is_message => {
                    if let Some(text) = part_text(part) {
                        assistant_parts.push(text.to_string());
                    }
                }
                "reasoning_summary_text" if is_message || is_reasoning => {
                    if let Some(text) = part_text(part) {
                        reasoning_parts.push(text.to_string());
                    }
                }
                _ => {}
            }
        }
    }

    let assistant_text = assistant_parts.join("").trim().to_string();
    let reasoning_text = reasoning_parts.join("").trim().to_string();

    ProviderOutput {
        assistant_text,
        reasoning_text: if reasoning_text.is_empty() {
            None
        } else {
            Some(reasoning_text)
        },
    }
}

fn extract_anthropic_text(response: &AnthropicMessagesResponse) -> String {
    response
        .content
        .iter()
        .filter(|part| part.part_type == "text")
        .filter_map(|part| part.text.as_deref())
        .filter(|text| !text.trim().is_empty())
        .collect::<Vec<_>>()
        .join("")
        .trim()
        .to_string()
}

fn anthropic_response_id(response: &AnthropicMessagesResponse) -> &str {
    response.id.as_deref().unwrap_or("unknown")
}

fn anthropic_stop_detail(response: &AnthropicMessagesResponse) -> String {
    response
        .stop_details
        .as_ref()
        .map(Value::to_string)
        .or_else(|| response.stop_reason.clone())
        .unwrap_or_else(|| "unknown stop reason".to_string())
}

// No token limits - let the API decide. Tokens are cheap, lost notes are not.

/// Check if output is effectively the same as input (raw-like)
/// Returns true only for whitespace-only echoes. Punctuation and capitalization
/// changes are meaningful formatting work and must not be collapsed into AiNoop.
fn is_effectively_same(input: &str, output: &str) -> bool {
    let normalize = |s: &str| -> String { s.split_whitespace().collect::<Vec<_>>().join(" ") };
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
    format_text_with_status_channels(text, language, assistive, on_delta, None).await
}

/// Format text using AI provider with explicit channel callbacks.
///
/// Contract:
/// - `on_assistant_delta`: receives only `response.output_text.*` deltas.
/// - `on_reasoning_delta`: receives only `response.reasoning_summary_text.*` deltas.
pub async fn format_text_with_status_channels(
    text: &str,
    language: Option<&str>,
    assistive: bool,
    on_assistant_delta: Option<AiStreamCallback>,
    on_reasoning_delta: Option<AiReasoningCallback>,
) -> AiFormatResult {
    let policy = if assistive {
        FormattingPolicy::Correction
    } else {
        match Config::formatting_policy() {
            Ok(policy) => policy,
            Err(error) => {
                warn!("Rejected invalid formatting policy: {error}");
                return AiFormatResult {
                    text: text.to_string(),
                    reasoning_text: None,
                    status: AiFormatStatus::Failed,
                };
            }
        }
    };
    format_text_with_status_channels_for_policy(
        text,
        language,
        assistive,
        policy,
        on_assistant_delta,
        on_reasoning_delta,
    )
    .await
}

/// Format through an explicitly selected normalized policy. This is the seam
/// used by deliberate one-shot formatting actions; it never changes persisted
/// Auto Format state.
pub async fn format_text_with_status_for_policy(
    text: &str,
    language: Option<&str>,
    policy: FormattingPolicy,
) -> AiFormatResult {
    format_text_with_status_channels_for_policy(text, language, false, policy, None, None).await
}

async fn format_text_with_status_channels_for_policy(
    text: &str,
    language: Option<&str>,
    assistive: bool,
    policy: FormattingPolicy,
    on_assistant_delta: Option<AiStreamCallback>,
    on_reasoning_delta: Option<AiReasoningCallback>,
) -> AiFormatResult {
    if !assistive && policy == FormattingPolicy::Off {
        return AiFormatResult {
            text: text.to_string(),
            reasoning_text: None,
            status: AiFormatStatus::Skipped,
        };
    }

    // Skip short non-assistive texts. The controller quality gate starts at 24 chars,
    // so formatting anything shorter would create an unguarded rewrite zone.
    if should_skip_ai_formatting(text, assistive) {
        return AiFormatResult {
            text: text.to_string(),
            reasoning_text: None,
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

    let retry_policy = RetryPolicy::from_env();
    let max_retries = retry_policy.max_retries;
    debug!(
        "AI retry policy: max_retries={}, retry_delay={:?}, attempt_timeout={:?}, \
         ollama_attempt_timeout={:?}, inter_chunk_timeout={:?}",
        retry_policy.max_retries,
        retry_policy.retry_delay,
        retry_policy.attempt_timeout,
        retry_policy.ollama_attempt_timeout,
        retry_policy.inter_chunk_timeout
    );

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
            formatting_provider_system_prompt(true, policy)
                .expect("assistive mode always owns a provider prompt")
        } else {
            if attempt == 0 {
                let model = get_formatting_model().unwrap_or_else(|_| "unknown".into());
                info!("Using formatting mode (model: {})", model);
            }
            formatting_provider_system_prompt(false, policy)
                .expect("Off policy bypasses before provider prompt selection")
        };

        // If retrying, wait and strengthen instructions
        if attempt > 0 {
            info!(
                "Retry attempt {}/{} (waiting {:?})",
                attempt, max_retries, retry_policy.retry_delay
            );
            tokio::time::sleep(retry_policy.retry_delay).await;

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

        // Route from explicit provider selection, retaining endpoint-path Ollama
        // compatibility only for the default OpenAI Responses lane.
        let endpoint = if assistive {
            get_assistive_endpoint().unwrap_or_default()
        } else {
            get_formatting_endpoint().unwrap_or_default()
        };
        let provider = lane_truth::provider(if assistive {
            LlmMode::Assistive
        } else {
            LlmMode::Formatting
        });
        let endpoint_format = detect_format(&endpoint, provider);
        // Streaming is always enabled. Callbacks only decide whether UI receives live chunks.
        let streaming_enabled = use_streaming();
        let should_stream =
            streaming_enabled && matches!(endpoint_format, EndpointFormat::ResponsesApi);
        let route = match (endpoint_format, should_stream) {
            (EndpointFormat::OllamaChat, _) => "ollama",
            (EndpointFormat::AnthropicMessages, _) => "anthropic-messages-json",
            (EndpointFormat::ResponsesApi, true) => "responses-sse",
            (EndpointFormat::ResponsesApi, false) => "responses-json",
        };
        // Streaming calls:
        // - attempt_timeout guards initial response latency (request -> first response readiness)
        // - inter_chunk_timeout guards stalled streams after they start
        // We intentionally do not cap total stream duration here.
        //
        // Non-streaming / Ollama calls: attempt_timeout caps the total wait for a
        // single JSON response.
        let stream_context = StreamRequestContext {
            callbacks: StreamCallbacks {
                assistant: on_assistant_delta.clone(),
                reasoning: on_reasoning_delta.clone(),
            },
            initial_response_timeout: retry_policy.attempt_timeout,
            inter_chunk_timeout: retry_policy.inter_chunk_timeout,
        };
        let mut retryable_error = true;
        let result_opt = if should_stream {
            match call_provider_once(
                endpoint_format,
                &user_message,
                &system_prompt,
                assistive,
                should_stream,
                stream_context.clone(),
            )
            .await
            {
                Ok(output) => Some(output),
                Err(e) => {
                    retryable_error = should_retry_provider_error(&e);
                    warn!(
                        "LLM {} attempt {}/{} failed: {}",
                        route,
                        attempt + 1,
                        max_retries + 1,
                        e
                    );
                    None
                }
            }
        } else {
            let attempt_timeout = if endpoint_format == EndpointFormat::OllamaChat {
                retry_policy.ollama_attempt_timeout
            } else {
                retry_policy.attempt_timeout
            };
            match tokio::time::timeout(
                attempt_timeout,
                call_provider_once(
                    endpoint_format,
                    &user_message,
                    &system_prompt,
                    assistive,
                    should_stream,
                    stream_context.clone(),
                ),
            )
            .await
            {
                Ok(Ok(output)) => Some(output),
                Ok(Err(e)) => {
                    retryable_error = should_retry_provider_error(&e);
                    warn!(
                        "LLM {} attempt {}/{} failed: {}",
                        route,
                        attempt + 1,
                        max_retries + 1,
                        e
                    );
                    None
                }
                Err(_) => {
                    warn!(
                        "LLM {} attempt {}/{} timed out after {:?}",
                        route,
                        attempt + 1,
                        max_retries + 1,
                        attempt_timeout
                    );
                    None
                }
            }
        };

        if let Some(output) = result_opt {
            // Deterministic protected-vocabulary pass AFTER the LLM. The model can
            // silently corrupt proper nouns ("Loctree" -> "Luxury") or drop
            // operator/tool/agent names while rewriting prose; re-applying the
            // lexicon restores any registered mispronunciation to its canonical
            // form. Safe + idempotent: it only rewrites known variants, never
            // ordinary language. Applies to both formatting and assistive modes.
            let formatted = crate::stream_postprocess::apply_lexicon(&output.assistant_text);
            let reasoning_text = output.reasoning_text;

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
                    reasoning_text: None,
                    status: AiFormatStatus::Failed,
                };
            }

            // Analyze result quality
            let content_match = is_effectively_same(&cleaned, &formatted);

            let mut should_retry = false;
            let raw_like = content_match;

            if assistive {
                // Assistive should change/expand content
                // If it matches normalized content, it likely failed to enhance
                if content_match {
                    warn!("Assistive mode returned content-matching output (not expanded)");
                    should_retry = true;
                }
            } else {
                // Formatting should preserve content but add structure
                // If output matches input (effectively same), it's a no-op
                if content_match {
                    warn!("Formatting mode returned AI No-op (raw echo)");
                    return AiFormatResult {
                        text: cleaned,
                        reasoning_text,
                        status: AiFormatStatus::AiNoop,
                    };
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
                        reasoning_text,
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
                reasoning_text,
                status: AiFormatStatus::Applied,
            };
        } else if !retryable_error {
            warn!("Provider returned deterministic empty-content error; skipping retries");
            break;
        }
    }

    // All providers failed
    warn!("All AI providers/retries failed, returning cleaned text");
    AiFormatResult {
        text: cleaned,
        reasoning_text: None,
        status: AiFormatStatus::Failed,
    }
}

/// Exact system prompt handed to the provider for a normalized request.
/// Exposed so delivery tests can observe the provider seam rather than merely
/// asserting enum-to-enum mappings.
pub fn formatting_provider_system_prompt(
    assistive: bool,
    policy: FormattingPolicy,
) -> Option<String> {
    if assistive {
        Some(crate::config::prompts::get_assistive_prompt())
    } else {
        crate::config::prompts::get_formatting_prompt_for_policy(policy)
    }
}

async fn call_provider_once(
    endpoint_format: EndpointFormat,
    user_message: &str,
    system_prompt: &str,
    assistive: bool,
    streaming_enabled: bool,
    stream_context: StreamRequestContext,
) -> Result<ProviderOutput> {
    match endpoint_format {
        EndpointFormat::OllamaChat => call_ollama(user_message, system_prompt, assistive).await,
        EndpointFormat::AnthropicMessages => {
            call_anthropic_messages(user_message, system_prompt, assistive).await
        }
        EndpointFormat::ResponsesApi => {
            if streaming_enabled {
                call_llm_endpoint_streaming(user_message, system_prompt, assistive, stream_context)
                    .await
            } else {
                call_llm_endpoint(user_message, system_prompt, assistive).await
            }
        }
    }
}

async fn call_anthropic_messages(
    user_message: &str,
    system_prompt: &str,
    assistive: bool,
) -> Result<ProviderOutput> {
    let mode = if assistive {
        LlmMode::Assistive
    } else {
        LlmMode::Formatting
    };
    let configured_endpoint = if assistive {
        get_assistive_endpoint()?
    } else {
        get_formatting_endpoint()?
    };
    let model =
        lane_truth::model_for_provider(mode, ProviderKind::AnthropicMessages, &Config::load());
    let api_key = get_anthropic_api_key()?;

    call_anthropic_messages_resolved(
        user_message,
        system_prompt,
        assistive,
        &configured_endpoint,
        &model,
        &api_key,
    )
    .await
}

async fn call_anthropic_messages_resolved(
    user_message: &str,
    system_prompt: &str,
    assistive: bool,
    configured_endpoint: &str,
    model: &str,
    api_key: &str,
) -> Result<ProviderOutput> {
    let endpoint = lane_truth::normalize_anthropic_messages_endpoint(configured_endpoint);
    let policy = capability_policy(ProviderKind::AnthropicMessages, model);
    let temperature = policy.sanitize_temperature(get_temperature(assistive));
    let max_tokens = env_u32(
        "CODESCRIBE_ANTHROPIC_MAX_TOKENS",
        DEFAULT_ANTHROPIC_MAX_TOKENS,
    );

    trace!(
        "Anthropic Messages request: endpoint={}, model={}, mode={}, temp={:?}, max_tokens={}",
        endpoint,
        model,
        if assistive { "assistive" } else { "formatting" },
        temperature,
        max_tokens
    );

    let request = AnthropicMessagesRequest {
        model: model.to_string(),
        system: Some(system_prompt.to_string()).filter(|value| !value.trim().is_empty()),
        messages: vec![AnthropicMessage {
            role: "user",
            content: build_anthropic_user_content(user_message),
        }],
        max_tokens,
        temperature,
    };

    let response = get_client()
        .post(&endpoint)
        .header("x-api-key", api_key)
        .header("anthropic-version", ANTHROPIC_VERSION)
        .header("Content-Type", "application/json")
        .json(&request)
        .send()
        .await
        .context("Anthropic request failed")?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        anyhow::bail!("Anthropic HTTP {} - {}", status, body);
    }

    let anthropic_response: AnthropicMessagesResponse = response
        .json()
        .await
        .context("Failed to parse Anthropic response")?;

    if policy.refusal_stop_reason
        && matches!(anthropic_response.stop_reason.as_deref(), Some("refusal"))
    {
        anyhow::bail!(
            "Anthropic refusal stop (id: {}): {}",
            anthropic_response_id(&anthropic_response),
            anthropic_stop_detail(&anthropic_response)
        );
    }

    let assistant_text = extract_anthropic_text(&anthropic_response);
    if assistant_text.is_empty() {
        anyhow::bail!(
            "No text content in Anthropic response (id: {}, stop_reason: {})",
            anthropic_response_id(&anthropic_response),
            anthropic_stop_detail(&anthropic_response)
        );
    }

    if matches!(
        anthropic_response.stop_reason.as_deref(),
        Some("max_tokens")
    ) {
        anyhow::bail!(
            "Anthropic response truncated by max_tokens (id: {})",
            anthropic_response_id(&anthropic_response)
        );
    }

    Ok(ProviderOutput {
        assistant_text,
        reasoning_text: None,
    })
}

/// Call LLM endpoint using /v1/responses API
///
/// Uses mode-aware config: LLM_{FORMATTING,ASSISTIVE}_{ENDPOINT,MODEL,API_KEY}
/// Falls back to LLM_{ENDPOINT,MODEL,API_KEY} if specific vars not set.
async fn call_llm_endpoint(
    user_message: &str,
    system_prompt: &str,
    assistive: bool,
) -> Result<ProviderOutput> {
    // Mode-aware config: formatting vs assistive use different providers
    let (endpoint, model, api_key) = if assistive {
        let lane = lane_truth::assistive_snapshot(&Config::load());
        let account = lane.provider.api_key_env_key();
        let api_key = lane
            .api_key
            .with_context(|| format!("LLM API key (assistive) is required. Set {account}."))?;
        (lane.endpoint, lane.model, api_key)
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
        "LLM request chain: endpoint={}, model={}, mode={}, temp={:?}",
        endpoint,
        model,
        if assistive { "assistive" } else { "formatting" },
        temperature
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

    let output = extract_output_channels(&responses_result.output);

    if output.assistant_text.is_empty() {
        anyhow::bail!("No text content in response (id: {})", responses_result.id);
    }

    // Store response_id for this mode's conversation chain (separate streams)
    crate::state::conversation::set_response_id_for_mode(ai_mode, responses_result.id.clone());
    debug!(
        "Response id ({}): {}",
        if assistive { "assistive" } else { "formatting" },
        responses_result.id
    );
    Ok(output)
}

/// Call LLM endpoint with SSE streaming (Responses API)
///
/// Uses mode-aware config: LLM_{FORMATTING,ASSISTIVE}_{ENDPOINT,MODEL,API_KEY}
async fn call_llm_endpoint_streaming(
    user_message: &str,
    system_prompt: &str,
    assistive: bool,
    stream_context: StreamRequestContext,
) -> Result<ProviderOutput> {
    // Mode-aware config: formatting vs assistive use different providers
    let (endpoint, model, api_key) = if assistive {
        let lane = lane_truth::assistive_snapshot(&Config::load());
        let account = lane.provider.api_key_env_key();
        let api_key = lane
            .api_key
            .with_context(|| format!("LLM API key (assistive) is required. Set {account}."))?;
        (lane.endpoint, lane.model, api_key)
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
        "SSE request chain: endpoint={}, model={}, mode={}, temp={:?}",
        endpoint,
        model,
        if assistive { "assistive" } else { "formatting" },
        temperature
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

    let StreamRequestContext {
        callbacks,
        initial_response_timeout,
        inter_chunk_timeout,
    } = stream_context;
    let manager = ResponsesStreamingManager::new(
        get_client(),
        &endpoint,
        &api_key,
        callbacks,
        initial_response_timeout,
        inter_chunk_timeout,
    );
    let streamed = manager.stream(&request).await?;
    let output = ProviderOutput {
        assistant_text: streamed.assistant_text,
        reasoning_text: streamed.reasoning_text,
    };
    if let Some(response_id) = streamed.response_id.filter(|id| !id.is_empty()) {
        crate::state::conversation::set_response_id_for_mode(ai_mode, response_id.clone());
        debug!(
            "SSE complete, response_id ({}): {}",
            if assistive { "assistive" } else { "formatting" },
            response_id
        );
    } else if let Some(prev_id) = previous_response_id.as_deref()
        && !prev_id.is_empty()
    {
        warn!(
            "SSE complete without response_id for {}; keeping previous_response_id={}",
            if assistive { "assistive" } else { "formatting" },
            prev_id
        );
    } else {
        warn!(
            "SSE complete without response_id for {}; no previous_response_id to keep",
            if assistive { "assistive" } else { "formatting" }
        );
    }
    Ok(output)
}

/// Call Ollama/local LLM for text formatting/assistive mode
///
/// Uses mode-aware config. Ollama native API uses /api/chat endpoint format.
async fn call_ollama(
    user_message: &str,
    system_prompt: &str,
    assistive: bool,
) -> Result<ProviderOutput> {
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

    Ok(ProviderOutput {
        assistant_text: formatted,
        reasoning_text: None,
    })
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

    let provider = lane_truth::provider(LlmMode::Formatting);
    let endpoint_format = detect_format(&endpoint, provider);

    // OllamaChat doesn't need API key
    if matches!(endpoint_format, EndpointFormat::OllamaChat) {
        return true;
    }

    if matches!(endpoint_format, EndpointFormat::AnthropicMessages) {
        return get_anthropic_api_key().is_ok();
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
    use mockito::Matcher;
    use serde_json::json;
    use serial_test::serial;

    const ANTHROPIC_TEST_ENV_KEYS: &[&str] = &[
        "LLM_FORMATTING_TEMPERATURE",
        "LLM_TEMPERATURE",
        "CODESCRIBE_ANTHROPIC_MAX_TOKENS",
    ];
    const LANE_TRUTH_TEST_CHILD: &str = "CODESCRIBE_LANE_TRUTH_TEST_CHILD";

    struct EnvGuard {
        key: &'static str,
        prev: Option<String>,
    }

    impl EnvGuard {
        fn set(key: &'static str, value: &str) -> Self {
            let prev = std::env::var(key).ok();
            unsafe { std::env::set_var(key, value) };
            Self { key, prev }
        }

        fn remove(key: &'static str) -> Self {
            let prev = std::env::var(key).ok();
            unsafe { std::env::remove_var(key) };
            Self { key, prev }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            match self.prev.as_deref() {
                Some(value) => unsafe { std::env::set_var(self.key, value) },
                None => unsafe { std::env::remove_var(self.key) },
            }
        }
    }

    struct TestEnv {
        guards: Vec<EnvGuard>,
    }

    impl TestEnv {
        fn clean() -> Self {
            Self {
                guards: ANTHROPIC_TEST_ENV_KEYS
                    .iter()
                    .map(|key| EnvGuard::remove(key))
                    .collect(),
            }
        }

        fn set(&mut self, key: &'static str, value: &str) {
            self.guards.push(EnvGuard::set(key, value));
        }
    }

    fn title_provider(
        format: EndpointFormat,
        endpoint: String,
        model: &str,
        api_key: Option<&str>,
    ) -> ThreadTitleProvider {
        ThreadTitleProvider {
            format,
            endpoint,
            model: model.to_string(),
            api_key: api_key.map(ToOwned::to_owned),
        }
    }

    #[test]
    fn thread_title_sanitizer_normalizes_noise_and_unicode_length() {
        let cases = [
            ("  **Plan   leczenia Łatki**\n", Some("Plan leczenia Łatki")),
            ("•  Kontrola po zabiegu", Some("Kontrola po zabiegu")),
            ("1. \"Wyniki badań krwi\"", Some("Wyniki badań krwi")),
            ("\n\t ", None),
            ("- ** **", None),
        ];
        for (raw, expected) in cases {
            assert_eq!(sanitize_thread_title(raw).as_deref(), expected, "{raw:?}");
        }

        let long = "ą".repeat(80);
        let clipped = sanitize_thread_title(&long).expect("non-empty title");
        assert_eq!(clipped.chars().count(), THREAD_TITLE_MAX_CHARS);
        assert_eq!(clipped, "ą".repeat(THREAD_TITLE_MAX_CHARS));
    }

    #[test]
    fn thread_title_contract_has_fixed_timeout_and_token_cap() {
        assert_eq!(THREAD_TITLE_TIMEOUT, Duration::from_secs(8));
        assert_eq!(THREAD_TITLE_MAX_TOKENS, 24);
    }

    #[tokio::test]
    #[serial]
    async fn responses_thread_title_is_one_shot_and_chain_stateless() {
        use crate::state::conversation::{
            AiMode, get_previous_response_id_for_mode, set_response_id_for_mode,
        };

        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("POST", "/v1/responses")
            .match_header("authorization", "Bearer title-key")
            .match_header("x-api-key", "title-key")
            .match_body(Matcher::Json(json!({
                "model": "title-model",
                "input": [{
                    "role": "user",
                    "content": [{"type": "input_text", "text": "Surowy\ntekst użytkownika"}]
                }],
                "instructions": THREAD_TITLE_PROMPT,
                "max_output_tokens": THREAD_TITLE_MAX_TOKENS
            })))
            .expect(1)
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                json!({
                    "id": "resp_title_should_not_be_stored",
                    "output": [{
                        "type": "message",
                        "content": [{"type": "output_text", "text": "Plan leczenia Łatki"}]
                    }]
                })
                .to_string(),
            )
            .create_async()
            .await;

        set_response_id_for_mode(AiMode::Formatting, "resp_existing_chain".to_string());
        let before = get_previous_response_id_for_mode(AiMode::Formatting);
        let provider = title_provider(
            EndpointFormat::ResponsesApi,
            format!("{}/v1/responses", server.url()),
            "title-model",
            Some("title-key"),
        );
        let title = generate_thread_title_with_provider(
            "Surowy\ntekst użytkownika",
            &provider,
            THREAD_TITLE_TIMEOUT,
        )
        .await
        .expect("Responses title request should succeed");

        assert_eq!(title.as_deref(), Some("Plan leczenia Łatki"));
        assert_eq!(
            get_previous_response_id_for_mode(AiMode::Formatting),
            before
        );
        mock.assert_async().await;
        crate::state::conversation::reset_conversation_for_mode(AiMode::Formatting);
    }

    #[tokio::test]
    #[serial]
    async fn anthropic_thread_title_uses_same_prompt_cap_and_raw_text() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("POST", "/v1/messages")
            .match_header("x-api-key", "anthropic-title-key")
            .match_header("anthropic-version", ANTHROPIC_VERSION)
            .match_body(Matcher::Json(json!({
                "model": "claude-sonnet-4-6",
                "system": THREAD_TITLE_PROMPT,
                "messages": [{
                    "role": "user",
                    "content": [{"type": "text", "text": "Raw\nAnthropic input"}]
                }],
                "max_tokens": THREAD_TITLE_MAX_TOKENS
            })))
            .expect(1)
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                json!({
                    "id": "msg_title",
                    "content": [{"type": "text", "text": "Anthropic title"}],
                    "stop_reason": "end_turn"
                })
                .to_string(),
            )
            .create_async()
            .await;
        let provider = title_provider(
            EndpointFormat::AnthropicMessages,
            server.url(),
            "claude-sonnet-4-6",
            Some("anthropic-title-key"),
        );

        let title = generate_thread_title_with_provider(
            "Raw\nAnthropic input",
            &provider,
            THREAD_TITLE_TIMEOUT,
        )
        .await
        .expect("Anthropic title request should succeed");
        assert_eq!(title.as_deref(), Some("Anthropic title"));
        mock.assert_async().await;
    }

    #[tokio::test]
    #[serial]
    async fn ollama_thread_title_uses_same_prompt_cap_without_memory() {
        reset_ollama_memory();
        push_memory("user", "stale conversation memory");
        push_memory("assistant", "stale answer");

        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("POST", "/api/chat")
            .match_body(Matcher::Json(json!({
                "model": "qwen-title",
                "messages": [
                    {"role": "system", "content": THREAD_TITLE_PROMPT},
                    {"role": "user", "content": "Raw\nOllama input"}
                ],
                "stream": false,
                "options": {
                    "temperature": 0.1,
                    "num_predict": THREAD_TITLE_MAX_TOKENS
                }
            })))
            .expect(1)
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(json!({"message": {"content": "Ollama title"}}).to_string())
            .create_async()
            .await;
        let provider = title_provider(
            EndpointFormat::OllamaChat,
            format!("{}/api/chat/v1/responses", server.url()),
            "qwen-title",
            None,
        );

        let title = generate_thread_title_with_provider(
            "Raw\nOllama input",
            &provider,
            THREAD_TITLE_TIMEOUT,
        )
        .await
        .expect("Ollama title request should succeed");
        assert_eq!(title.as_deref(), Some("Ollama title"));
        assert_eq!(
            snapshot_memory().len(),
            2,
            "title path must not mutate memory"
        );
        mock.assert_async().await;
        reset_ollama_memory();
    }

    #[tokio::test]
    #[serial]
    async fn thread_title_timeout_covers_response_body() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("POST", "/v1/responses")
            .match_body(Matcher::Any)
            .expect(1)
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_chunked_body(|writer| {
                std::thread::sleep(Duration::from_millis(300));
                writer.write_all(br#"{"id":"late","output":[]}"#)
            })
            .create_async()
            .await;
        let provider = title_provider(
            EndpointFormat::ResponsesApi,
            format!("{}/v1/responses", server.url()),
            "title-model",
            Some("title-key"),
        );

        let error =
            generate_thread_title_with_provider("Raw input", &provider, Duration::from_millis(100))
                .await
                .expect_err("slow response body must be covered by the whole-call timeout");
        assert!(error.to_string().contains("timed out"));
        mock.assert_async().await;
    }

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

    #[test]
    fn test_short_non_assistive_text_is_skipped() {
        assert!(should_skip_ai_formatting("krótki", false));
        assert!(should_skip_ai_formatting("123456789", false));
    }

    #[test]
    fn test_assistive_short_text_is_not_skipped() {
        assert!(!should_skip_ai_formatting("Pomóż mi", true));
    }

    #[test]
    fn test_non_assistive_text_at_threshold_is_not_skipped() {
        let text = "1234567890";
        assert_eq!(text.chars().count(), NON_ASSISTIVE_AI_SKIP_CHARS);
        assert!(!should_skip_ai_formatting(text, false));
    }

    #[test]
    fn test_effectively_same_ignores_whitespace_only() {
        assert!(is_effectively_same("raw   one two", "raw one two"));
        assert!(is_effectively_same("raw one two\n", "raw one two"));
    }

    #[test]
    fn test_effectively_same_preserves_formatting_changes() {
        assert!(!is_effectively_same("raw one two", "RAW ONE TWO."));
        assert!(!is_effectively_same("to jest test", "To jest test"));
    }

    #[test]
    fn default_retry_policy_is_single_attempt() {
        assert_eq!(DEFAULT_AI_MAX_RETRIES, 0);
        assert_eq!(DEFAULT_AI_RETRY_DELAY_MS, 500);
    }

    #[test]
    fn lane_configs_read_fresh_truth_after_settings_save() {
        if std::env::var_os(LANE_TRUTH_TEST_CHILD).is_none() {
            let data_dir = tempfile::TempDir::new().expect("isolated data dir");
            let executable = std::env::current_exe().expect("current core test executable");
            let status = std::process::Command::new(executable)
                .arg("--exact")
                .arg("llm::ai_formatting::tests::lane_configs_read_fresh_truth_after_settings_save")
                .arg("--nocapture")
                .env(LANE_TRUTH_TEST_CHILD, "1")
                .env("CODESCRIBE_DATA_DIR", data_dir.path())
                .env("CODESCRIBE_DISABLE_KEYCHAIN", "1")
                .envs([
                    ("LLM_FORMATTING_PROVIDER", "openai-responses"),
                    (
                        "LLM_FORMATTING_ENDPOINT",
                        "https://stale-formatting.example/v1",
                    ),
                    ("LLM_FORMATTING_MODEL", "stale-formatting-model"),
                    ("LLM_ASSISTIVE_PROVIDER", "openai-responses"),
                    (
                        "LLM_ASSISTIVE_ENDPOINT",
                        "https://stale-assistive.example/v1",
                    ),
                    ("LLM_ASSISTIVE_MODEL", "stale-assistive-model"),
                ])
                .status()
                .expect("run isolated lane-truth test");
            assert!(
                status.success(),
                "isolated lane-truth test failed: {status}"
            );
            return;
        }

        crate::config::UserSettings {
            llm_formatting_endpoint: Some("https://fresh-formatting.example/v1".to_string()),
            llm_formatting_model: Some("fresh-formatting-model".to_string()),
            llm_assistive_provider: Some("openai-responses".to_string()),
            llm_assistive_endpoint: Some("https://fresh-assistive.example/v1".to_string()),
            llm_assistive_model: Some("fresh-assistive-model".to_string()),
            ..Default::default()
        }
        .save()
        .expect("persist lane settings");

        assert_eq!(
            get_formatting_endpoint().expect("formatting endpoint"),
            "https://fresh-formatting.example/v1/responses"
        );
        assert_eq!(
            get_formatting_model().expect("formatting model"),
            "fresh-formatting-model"
        );
        assert_eq!(
            get_assistive_endpoint().expect("assistive endpoint"),
            "https://fresh-assistive.example/v1/responses"
        );
        assert_eq!(
            get_assistive_model().expect("assistive model"),
            "fresh-assistive-model"
        );
    }

    #[tokio::test]
    #[serial]
    async fn anthropic_sonnet_request_keeps_temperature() {
        let mut env = TestEnv::clean();
        let mut server = mockito::Server::new_async().await;
        env.set("LLM_FORMATTING_TEMPERATURE", "0.5");

        let mock = server
            .mock("POST", "/v1/messages")
            .match_header("x-api-key", "anthropic-test-key")
            .match_header("anthropic-version", ANTHROPIC_VERSION)
            .match_body(Matcher::Json(json!({
                "model": "claude-sonnet-4-6",
                "system": "format carefully",
                "messages": [{
                    "role": "user",
                    "content": [{"type": "text", "text": "hello world"}]
                }],
                "max_tokens": DEFAULT_ANTHROPIC_MAX_TOKENS,
                "temperature": 0.5
            })))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                json!({
                    "id": "msg_sonnet",
                    "type": "message",
                    "role": "assistant",
                    "content": [{"type": "text", "text": "Hello world."}],
                    "stop_reason": "end_turn"
                })
                .to_string(),
            )
            .create_async()
            .await;

        let output = call_anthropic_messages_resolved(
            "hello world",
            "format carefully",
            false,
            &server.url(),
            "claude-sonnet-4-6",
            "anthropic-test-key",
        )
        .await
        .expect("sonnet formatting request should succeed");

        assert_eq!(output.assistant_text, "Hello world.");
        mock.assert_async().await;
    }

    #[tokio::test]
    #[serial]
    async fn anthropic_opus_request_strips_temperature() {
        let mut env = TestEnv::clean();
        let mut server = mockito::Server::new_async().await;
        env.set("LLM_FORMATTING_TEMPERATURE", "0.5");

        let mock = server
            .mock("POST", "/v1/messages")
            .match_body(Matcher::Json(json!({
                "model": "claude-opus-4-8",
                "system": "format carefully",
                "messages": [{
                    "role": "user",
                    "content": [{"type": "text", "text": "hello world"}]
                }],
                "max_tokens": DEFAULT_ANTHROPIC_MAX_TOKENS
            })))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                json!({
                    "id": "msg_opus",
                    "type": "message",
                    "role": "assistant",
                    "content": [{"type": "text", "text": "Hello world."}],
                    "stop_reason": "end_turn"
                })
                .to_string(),
            )
            .create_async()
            .await;

        let output = call_anthropic_messages_resolved(
            "hello world",
            "format carefully",
            false,
            &server.url(),
            "claude-opus-4-8",
            "anthropic-test-key",
        )
        .await
        .expect("opus formatting request should succeed without temperature");

        assert_eq!(output.assistant_text, "Hello world.");
        mock.assert_async().await;
    }

    #[tokio::test]
    #[serial]
    async fn anthropic_refusal_stop_reason_is_readable_error() {
        let _env = TestEnv::clean();
        let mut server = mockito::Server::new_async().await;

        let mock = server
            .mock("POST", "/v1/messages")
            .match_body(Matcher::Any)
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                json!({
                    "id": "msg_refusal",
                    "type": "message",
                    "role": "assistant",
                    "content": [],
                    "stop_reason": "refusal",
                    "stop_details": {"reason": "safety"}
                })
                .to_string(),
            )
            .create_async()
            .await;

        let err = call_anthropic_messages_resolved(
            "hello world",
            "format carefully",
            false,
            &server.url(),
            "claude-sonnet-4-6",
            "anthropic-test-key",
        )
        .await
        .expect_err("refusal stop_reason should not parse as empty success");

        let message = err.to_string();
        assert!(message.contains("Anthropic refusal stop"));
        assert!(message.contains("safety"));
        mock.assert_async().await;
    }

    #[tokio::test]
    #[serial]
    async fn anthropic_happy_path_joins_text_content_blocks() {
        let _env = TestEnv::clean();
        let mut server = mockito::Server::new_async().await;

        let mock = server
            .mock("POST", "/v1/messages")
            .match_body(Matcher::Any)
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                json!({
                    "id": "msg_joined",
                    "type": "message",
                    "role": "assistant",
                    "content": [
                        {"type": "text", "text": "Hello"},
                        {"type": "text", "text": " world."}
                    ],
                    "stop_reason": "end_turn"
                })
                .to_string(),
            )
            .create_async()
            .await;

        let output = call_anthropic_messages_resolved(
            "hello world",
            "format carefully",
            false,
            &server.url(),
            "claude-sonnet-4-6",
            "anthropic-test-key",
        )
        .await
        .expect("text content blocks should parse");

        assert_eq!(output.assistant_text, "Hello world.");
        mock.assert_async().await;
    }

    #[test]
    fn empty_content_provider_errors_are_not_retryable() {
        assert!(!should_retry_provider_error(&anyhow::anyhow!(
            "No text content in SSE stream"
        )));
        assert!(!should_retry_provider_error(&anyhow::anyhow!(
            "No text content in response (id: resp_1)"
        )));
        assert!(!should_retry_provider_error(&anyhow::anyhow!(
            "SSE error internal_error: backend failed"
        )));
        assert!(!should_retry_provider_error(&anyhow::anyhow!(
            "SSE error bad_request: invalid input"
        )));
        assert!(!should_retry_provider_error(&anyhow::anyhow!(
            "Anthropic refusal stop (id: msg_1): safety"
        )));
        assert!(should_retry_provider_error(&anyhow::anyhow!(
            "SSE stream inter-chunk timeout"
        )));
    }
}
