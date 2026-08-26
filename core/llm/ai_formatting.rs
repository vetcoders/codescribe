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
use std::sync::{Arc, OnceLock};
use std::time::Duration;
use tracing::{debug, info, trace, warn};

use crate::config::{Config, FormattingPolicy, RuntimeLlmLane, RuntimeSettingsSnapshot};

use super::account_auth;
use super::provider::{ProviderKind, WireFamily, capability_policy};
use super::responses_streaming_manager::{
    AuthHeaderMode, ResponsesStreamingManager, StreamCallbacks,
};

/// HTTP client for AI providers
static AI_CLIENT: OnceLock<Client> = OnceLock::new();

/// Non-assistive formatting skips only extremely short transcripts.
/// Short-but-real utterances still flow through AI formatting; the controller
/// owns the separate quality-gate logic for that 10-23 char window.
const NON_ASSISTIVE_AI_SKIP_CHARS: usize = 10;

/// Whether a transcript is too short to be worth an LLM round-trip.
///
/// Assistive requests are never skipped — a two-word command is still a real
/// instruction. Non-assistive formatting is skipped below the char floor.
fn should_skip_ai_formatting(text: &str, assistive: bool) -> bool {
    !assistive && text.chars().count() < NON_ASSISTIVE_AI_SKIP_CHARS
}

/// How one formatting request ended, from the caller's point of view.
///
/// The three non-success arms are deliberately distinct: `Skipped` means the
/// request was never sent (policy off, or text below the floor), `Failed` means
/// every provider attempt was exhausted, and `AiNoop` means the provider
/// answered but echoed the input back unchanged. Callers downstream treat these
/// differently — only `AiNoop` implies a healthy provider that had nothing to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AiFormatStatus {
    Applied,
    Failed,
    Skipped,
    AiNoop,
}

/// Streaming assistant-token callback delivered as each chunk arrives.
pub type AiStreamCallback = Arc<dyn Fn(&str) + Send + Sync>;
/// Streaming reasoning-token callback, kept separate from assistant text.
pub type AiReasoningCallback = Arc<dyn Fn(&str) + Send + Sync>;

/// Result of a formatting request: the text to use plus how it was obtained.
///
/// `text` is always safe to deliver — on failure it carries the cleaned input
/// rather than an error, so the caller never has to invent a fallback.
#[derive(Debug, Clone)]
pub struct AiFormatResult {
    pub text: String,
    pub reasoning_text: Option<String>,
    pub status: AiFormatStatus,
}

/// One provider reply split into its two channels.
///
/// Reasoning text is kept separate from assistant text so a reasoning model's
/// visible scratchpad never leaks into the delivered transcript.
#[derive(Debug, Clone)]
struct ProviderOutput {
    assistant_text: String,
    reasoning_text: Option<String>,
}

/// Per-attempt streaming state that is not part of the wire request.
///
/// The two timeouts guard different failure shapes: one bounds how long the
/// provider may take to start answering, the other bounds silence mid-stream.
#[derive(Clone)]
struct StreamRequestContext {
    callbacks: StreamCallbacks,
    initial_response_timeout: Duration,
    inter_chunk_timeout: Duration,
}

/// reqwest overall request timeout for the shared AI HTTP client.
const DEFAULT_AI_CLIENT_TIMEOUT_MS: u64 = 90_000;
/// TCP connect deadline for the shared AI HTTP client.
const DEFAULT_AI_CONNECT_TIMEOUT_MS: u64 = 5_000;
/// Idle keep-alive for pooled connections on the shared AI HTTP client.
const DEFAULT_AI_POOL_IDLE_TIMEOUT_MS: u64 = 90_000;
/// TCP keepalive probe interval for the shared AI HTTP client.
const DEFAULT_AI_TCP_KEEPALIVE_MS: u64 = 30_000;
/// Anthropic Messages API version header value.
const ANTHROPIC_VERSION: &str = "2023-06-01";
/// Default `max_tokens` for Anthropic formatting/chat when env is unset.
const DEFAULT_ANTHROPIC_MAX_TOKENS: u32 = 8192;
/// Wall budget for auto-generating a thread title after the first turn.
const THREAD_TITLE_TIMEOUT: Duration = Duration::from_secs(8);
/// Completion token budget for the thread-title request.
const THREAD_TITLE_MAX_TOKENS: u32 = 24;
/// Hard clip length for a generated title before it is stored on the thread.
const THREAD_TITLE_MAX_CHARS: usize = 72;
/// System prompt for the thread-title generator; asks for a short noun phrase.
const THREAD_TITLE_PROMPT: &str = "Create a concise 3-6 word title for this conversation. \
Use the user's language and a descriptive noun phrase. Return only the title on one line, \
with no quotes, bullet, label, or decorative punctuation.";

/// Read a `u32` env override, falling back to `default` on unset or garbage.
///
/// A malformed value is treated as absent rather than fatal: a typo in a power
/// user's dotenv must not take the formatting lane down.
fn env_u32(key: &str, default: u32) -> u32 {
    env::var(key)
        .ok()
        .and_then(|v| v.parse::<u32>().ok())
        .unwrap_or(default)
}

/// Read a `u64` env override, falling back to `default` on unset or garbage.
fn env_u64(key: &str, default: u64) -> u64 {
    env::var(key)
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(default)
}

/// Read a millisecond env override as a [`Duration`].
fn duration_from_env_ms(key: &str, default_ms: u64) -> Duration {
    Duration::from_millis(env_u64(key, default_ms))
}

/// Whether a provider error is worth another attempt.
///
/// Errors are matched by message because they arrive as opaque `anyhow` chains
/// from several transports. The listed shapes are deterministic — an empty
/// completion, a refusal, or a rejected request will reproduce identically on
/// retry, so retrying only multiplies latency.
/// A chain id the requesting key cannot see (`previous_response_not_found`).
/// Measured mechanism (2026-08-12 22:31→23:02): the id was minted under the
/// OLD key, the operator swapped Keychain keys at 22:47–22:51, and the new
/// key's org cannot read the old org's response — three identical formatting
/// failures, transcript delivered raw. NOT retention: the same-key chain was
/// proven alive hours later (2026-08-14, full recall of the 10:38 take). The
/// stored id is poison for THIS key, so drop it and go unchained.
pub(crate) fn is_stale_chain_error(error: &anyhow::Error) -> bool {
    error.to_string().contains("previous_response_not_found")
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

/// The shared HTTP client, built once on first use.
///
/// One pooled client for every provider: rebuilding per request would discard
/// warm TCP/TLS connections and add a handshake to every formatting round-trip.
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

/// A fully resolved target for one thread-title request.
///
/// Resolution is deliberately separated from sending so tests can drive every
/// wire format against a mock server without touching real config or secrets.
#[derive(Debug, Clone)]
struct ThreadTitleProvider {
    wire_family: WireFamily,
    endpoint: String,
    model: String,
    api_key: Option<String>,
}

/// Generate one isolated title through the currently selected formatting lane.
///
/// This path deliberately does not call any formatting/assistive request helper:
/// it sends exactly one bounded JSON request, passes only `text` as user input,
/// and never reads or writes response-chain or Ollama memory state.
pub async fn generate_thread_title(
    text: &str,
    formatting_lane: &RuntimeLlmLane,
) -> Result<Option<String>> {
    if text.trim().is_empty() {
        return Ok(None);
    }
    let provider = resolve_thread_title_provider(formatting_lane)?;
    generate_thread_title_with_provider(text, &provider, THREAD_TITLE_TIMEOUT).await
}

/// Resolve the formatting lane into a concrete title target, including which
/// credential applies.
///
/// Key selection follows the lane rules rather than one global secret. A local
/// endpoint may legitimately have no credential; cloud availability was sealed
/// by the loader and fails closed before a request is attempted.
fn resolve_thread_title_provider(lane: &RuntimeLlmLane) -> Result<ThreadTitleProvider> {
    if !lane.available() {
        anyhow::bail!(
            "{}",
            lane.unavailable_reason()
                .unwrap_or("Formatting lane is unavailable")
        );
    }

    Ok(ThreadTitleProvider {
        wire_family: lane.wire_family(),
        endpoint: lane.endpoint().to_string(),
        model: lane.model().to_string(),
        api_key: lane.credential().api_key().map(str::to_string),
    })
}

/// Send one title request against an already-resolved provider and sanitize it.
///
/// The timeout wraps the whole call including body download, so a provider that
/// answers headers fast and then trickles the body cannot outlive the budget.
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

/// Dispatch the title request to the wire format the provider speaks.
async fn request_thread_title(text: &str, provider: &ThreadTitleProvider) -> Result<String> {
    match provider.wire_family {
        WireFamily::OpenAiResponses => request_responses_thread_title(text, provider).await,
        WireFamily::AnthropicMessages => request_anthropic_thread_title(text, provider).await,
    }
}

/// One-shot title request over the Responses API.
///
/// `previous_response_id` is deliberately `None`: titling must not join the
/// user's conversation chain, or the title prompt would pollute later turns.
async fn request_responses_thread_title(
    text: &str,
    provider: &ThreadTitleProvider,
) -> Result<String> {
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

    let mut request_builder = get_client()
        .post(&provider.endpoint)
        .header("Content-Type", "application/json")
        .json(&request);
    if let Some(api_key) = provider.api_key.as_deref() {
        request_builder = request_builder
            .header("Authorization", format!("Bearer {api_key}"))
            .header("x-api-key", api_key);
    }
    let response = request_builder
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

/// One-shot title request over the Anthropic Messages API.
///
/// A `refusal` stop reason is raised as an error rather than returned as an
/// empty title, so the caller can tell "model declined" from "model had nothing".
async fn request_anthropic_thread_title(
    text: &str,
    provider: &ThreadTitleProvider,
) -> Result<String> {
    let endpoint = provider.endpoint.clone();
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

    let mut request_builder = get_client()
        .post(&endpoint)
        .header("anthropic-version", ANTHROPIC_VERSION)
        .header("Content-Type", "application/json")
        .json(&request);
    if let Some(api_key) = provider.api_key.as_deref() {
        request_builder = request_builder.header("x-api-key", api_key);
    }
    let response = request_builder
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

/// Turn a raw model reply into a usable one-line title, or `None` if nothing
/// survives.
///
/// Models decorate titles despite instructions — bullets, numbering, bold, and
/// quotes all get stripped. The length cap counts characters, not bytes, so a
/// Polish or CJK title is clipped where a reader would expect.
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

/// Drop a leading list marker — `-`, `*`, a bullet glyph, or `1.` / `1)`.
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

/// Unwrap one layer of matched emphasis or quoting around a title.
///
/// Covers typographic and Polish-style quote pairs too, since the title is
/// generated in the user's own language.
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

/// Instructions for a Responses request: the `instructions` PARAM goes only on
/// the first turn of a chain — endpoints reject the pair with
/// `previous_response_id` (HTTP 400 "instructions and previous_response_id
/// together").
///
/// But instructions are NOT preserved server-side across chained turns
/// (OpenAI Responses: "instructions … not carried over to the next response
/// when using previous_response_id"), so a chained turn MUST re-carry the
/// system prompt inside `input` — see [`build_responses_input`]. Dropping it
/// entirely left the formatter promptless mid-chain and the model answered as
/// a chat assistant instead of transforming (2026-08-14 leak: "Jasne — oto to
/// samo, przepisane czytelnie…" delivered as the formatted transcript).
fn chained_instructions(system_prompt: &str, previous_response_id: Option<&str>) -> Option<String> {
    if previous_response_id.is_some() {
        None
    } else {
        Some(system_prompt.to_string())
    }
}

/// Build the `input` items for a Responses request. On chained turns the
/// system prompt rides as a leading `developer` item, because the
/// `instructions` param is absent there (see [`chained_instructions`]) and the
/// chain does not carry it server-side. First turns carry the prompt via
/// `instructions` only — no duplicate developer item.
fn build_responses_input(
    system_prompt: &str,
    previous_response_id: Option<&str>,
    user_content: Vec<InputContent>,
) -> Vec<InputItem> {
    let mut input = Vec::with_capacity(2);
    if previous_response_id.is_some() {
        input.push(InputItem {
            role: "developer",
            content: vec![InputContent::Text {
                text: system_prompt.to_string(),
            }],
        });
    }
    input.push(InputItem {
        role: "user",
        content: user_content,
    });
    input
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

/// One turn in an Anthropic Messages request.
///
/// The system prompt is a top-level field in this API, not a message, so `role`
/// is always `"user"` on the outbound path.
#[derive(Debug, Serialize)]
struct AnthropicMessage {
    role: &'static str,
    content: Vec<AnthropicContentBlock>,
}

/// A single content block of an Anthropic turn — text or an inline image.
///
/// Anthropic takes base64 images as a structured `source` object rather than the
/// data URL the Responses API accepts, which is why the two paths diverge here.
#[derive(Debug, Serialize)]
#[serde(tag = "type")]
enum AnthropicContentBlock {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "image")]
    Image { source: AnthropicImageSource },
}

/// Base64 image payload in the shape Anthropic expects.
#[derive(Debug, Serialize)]
struct AnthropicImageSource {
    #[serde(rename = "type")]
    source_type: &'static str,
    media_type: String,
    data: String,
}

/// Load an image from disk as a `data:` URL, or `None` if it cannot be used.
///
/// Returns `None` on anything unreadable, unsupported, or over the size cap —
/// an attachment problem degrades the request instead of failing it.
fn encode_image_as_data_url(path: &std::path::Path) -> Option<String> {
    use base64::{Engine, engine::general_purpose::STANDARD as BASE64};

    // Marker parsing, MIME mapping and the size cap are shared with the agent
    // send path via `crate::attachment` so both routes honor one contract.
    let (bytes, mime) =
        crate::attachment::load_image_for_vision(path, crate::attachment::MAX_VISION_IMAGE_BYTES)?;
    let b64 = BASE64.encode(bytes);
    Some(format!("data:{mime};base64,{b64}"))
}

/// Split a user message into Responses API content parts, lifting any attached
/// images out of the marker block.
///
/// The image filenames are appended to the text as well, so a model that drops
/// or cannot see an attachment still knows something was sent.
fn build_responses_user_content(user_message: &str) -> Vec<InputContent> {
    // Kept in sync with `MAX_AGENT_VISION_IMAGES` in the agent send path.
    /// Cap on image parts per Responses request; surplus paths are dropped.
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

/// The Anthropic counterpart of [`build_responses_user_content`].
///
/// Differs in one respect: an empty text block is omitted when images are
/// present, because Anthropic rejects a blank text block alongside content.
fn build_anthropic_user_content(user_message: &str) -> Vec<AnthropicContentBlock> {
    // Kept in sync with `MAX_AGENT_VISION_IMAGES` in the agent send path.
    /// Cap on image parts per Anthropic request; surplus paths are dropped.
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

/// Re-shape a `data:<mime>;base64,<payload>` URL into Anthropic's source object.
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

/// One item of a Responses API output array.
///
/// `item_type` distinguishes a `message` from a `reasoning` item; the split
/// matters because only the former may reach the user's transcript.
#[derive(Debug, Deserialize)]
struct OutputItem {
    #[serde(rename = "type")]
    item_type: String,
    #[serde(default)]
    content: Option<Vec<ContentPart>>,
}

/// A content part inside an output item.
///
/// Text arrives under `text` for output parts and under `summary` for reasoning
/// summaries, so both are accepted and normalised downstream.
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

/// One content block of an Anthropic response; only `"text"` blocks are read.
#[derive(Debug, Deserialize)]
struct AnthropicResponseContent {
    #[serde(rename = "type")]
    part_type: String,
    #[serde(default)]
    text: Option<String>,
}

/// The meaningful text of a content part, preferring `text` over `summary`.
///
/// Whitespace-only parts collapse to `None` so they cannot pad the joined output.
fn part_text(part: &ContentPart) -> Option<&str> {
    part.text
        .as_deref()
        .or(part.summary.as_deref())
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

/// Fold a Responses output array into the assistant and reasoning channels.
///
/// The item type gates the part type: `output_text` counts only inside a
/// `message`. That guard is what keeps a reasoning model's internal narration
/// out of the text that gets pasted into the user's document.
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

/// Concatenate the text blocks of an Anthropic response.
///
/// Joined without a separator: the API splits one continuous answer across
/// blocks, so any inserted glue would corrupt the text.
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

/// The response id for log and error correlation, or `"unknown"` if absent.
fn anthropic_response_id(response: &AnthropicMessagesResponse) -> &str {
    response.id.as_deref().unwrap_or("unknown")
}

/// The most specific available explanation of why generation stopped.
///
/// Prefers the structured `stop_details` over the bare `stop_reason`, so a
/// refusal error carries the actual cause instead of just the word "refusal".
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
pub async fn format_text(
    text: &str,
    language: Option<&str>,
    assistive: bool,
    runtime_settings: &RuntimeSettingsSnapshot,
) -> String {
    format_text_with_status(text, language, assistive, runtime_settings, None)
        .await
        .text
}

/// Format text using AI provider with fallback chain, returning status.
///
/// Callers supply the selected runtime generation. This entry never reloads
/// settings, prompt files, or process env.
pub async fn format_text_with_status(
    text: &str,
    language: Option<&str>,
    assistive: bool,
    runtime_settings: &RuntimeSettingsSnapshot,
    on_delta: Option<AiStreamCallback>,
) -> AiFormatResult {
    format_text_with_status_channels(
        text,
        language,
        assistive,
        runtime_settings,
        on_delta,
        None,
    )
    .await
}

/// Format text using AI provider with explicit channel callbacks.
///
/// Contract:
/// - `on_assistant_delta`: receives only `response.output_text.*` deltas.
/// - `on_reasoning_delta`: receives only `response.reasoning_summary_text.*` deltas.
/// - policy, lane, prompt, retry, and request timing all come from one immutable
///   settings generation; assistive still selects the assistive lane/prompt.
pub async fn format_text_with_status_channels(
    text: &str,
    language: Option<&str>,
    assistive: bool,
    runtime_settings: &RuntimeSettingsSnapshot,
    on_assistant_delta: Option<AiStreamCallback>,
    on_reasoning_delta: Option<AiReasoningCallback>,
) -> AiFormatResult {
    format_text_with_status_channels_for_policy(
        text,
        language,
        assistive,
        runtime_settings,
        on_assistant_delta,
        on_reasoning_delta,
    )
    .await
}

/// Format through the normalized policy sealed in the selected generation.
/// Deliberate one-shot callers choose a generation at their outer boundary;
/// this request path cannot reconstruct policy or prompt independently.
pub async fn format_text_with_status_for_policy(
    text: &str,
    language: Option<&str>,
    runtime_settings: &RuntimeSettingsSnapshot,
) -> AiFormatResult {
    format_text_with_status_channels_for_policy(
        text,
        language,
        false,
        runtime_settings,
        None,
        None,
    )
    .await
}

/// The single implementation every public formatting entry point funnels into.
///
/// Order of the pipeline matters and is load-bearing:
/// 1. bail out for `Off` policy and sub-floor text (never reaches a provider);
/// 2. strip Whisper repetition loops locally, before spending a request;
/// 3. attempt the provider up to the retry budget, giving up early on errors
///    that are deterministic;
/// 4. re-apply the protected lexicon to the reply, since a model can silently
///    corrupt proper nouns while rewriting prose;
/// 5. reject refusal text and classify an unchanged echo as `AiNoop`.
///
/// Never returns an error: an exhausted budget yields the cleaned input with a
/// `Failed` status, so the caller always has text to deliver.
async fn format_text_with_status_channels_for_policy(
    text: &str,
    language: Option<&str>,
    assistive: bool,
    runtime_settings: &RuntimeSettingsSnapshot,
    on_assistant_delta: Option<AiStreamCallback>,
    on_reasoning_delta: Option<AiReasoningCallback>,
) -> AiFormatResult {
    let policy = runtime_settings.formatting_policy();
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

    let ai_execution = runtime_settings.ai_execution();
    let formatter_execution = ai_execution.formatter();
    let request_timing = ai_execution.request_timing();
    let lane = if assistive {
        runtime_settings.llm_lanes().assistive()
    } else {
        runtime_settings.llm_lanes().formatting()
    };
    let sealed_prompt = if assistive {
        formatter_execution.assistive_prompt()
    } else {
        formatter_execution
            .formatting_prompt()
            .expect("Off policy bypasses before provider prompt selection")
    };
    let max_retries = formatter_execution.max_retries();
    debug!(
        "AI retry policy: max_retries={}, retry_delay={:?}, attempt_timeout={:?}, \
         inter_chunk_timeout={:?}",
        max_retries,
        formatter_execution.retry_delay(),
        request_timing.attempt_timeout(),
        request_timing.inter_chunk_timeout()
    );

    // Mode key for the conversation chain this call rides on — needed by the
    // stale-chain self-heal below to reset the RIGHT stream (modes have
    // separate chains and separate key slots).
    let ai_mode = if assistive {
        crate::state::conversation::AiMode::Assistive
    } else {
        crate::state::conversation::AiMode::Formatting
    };
    // One-shot chain self-heal: a stale stored response_id re-runs the SAME
    // attempt unchained instead of consuming the selected retry budget; a
    // poisoned chain would otherwise burn attempts without changing inputs.
    let mut stale_chain_retry_used = false;
    let mut attempt = 0;
    while attempt <= max_retries {
        info!(
            "AI formatting attempt {} (assistive={}, input_len={})",
            attempt + 1,
            assistive,
            cleaned.len()
        );
        let mut system_prompt = sealed_prompt.composed_content().to_string();
        if assistive {
            if attempt == 0 {
                info!("Using assistive mode (model: {})", lane.model());
            }
        } else {
            if attempt == 0 {
                info!("Using formatting mode (model: {})", lane.model());
            }
        }

        // If retrying, wait and strengthen instructions
        if attempt > 0 {
            info!(
                "Retry attempt {}/{} (waiting {:?})",
                attempt,
                max_retries,
                formatter_execution.retry_delay()
            );
            tokio::time::sleep(formatter_execution.retry_delay()).await;

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

        // The loader already sealed the protocol family. Consumers must not
        // infer a second route from endpoint spelling.
        let wire_family = lane.wire_family();
        // Streaming is always enabled. Callbacks only decide whether UI receives live chunks.
        let streaming_enabled = use_streaming();
        let should_stream = streaming_enabled && wire_family == WireFamily::OpenAiResponses;
        let route = match (wire_family, should_stream) {
            (WireFamily::AnthropicMessages, _) => "anthropic-messages-json",
            (WireFamily::OpenAiResponses, true) => "responses-sse",
            (WireFamily::OpenAiResponses, false) => "responses-json",
        };
        // Streaming calls:
        // - attempt_timeout guards initial response latency (request -> first response readiness)
        // - inter_chunk_timeout guards stalled streams after they start
        // We intentionally do not cap total stream duration here.
        //
        // Non-streaming calls: attempt_timeout caps the total wait for a single
        // JSON response.
        let stream_context = StreamRequestContext {
            callbacks: StreamCallbacks {
                assistant: on_assistant_delta.clone(),
                reasoning: on_reasoning_delta.clone(),
            },
            initial_response_timeout: request_timing.attempt_timeout(),
            inter_chunk_timeout: request_timing.inter_chunk_timeout(),
        };
        let mut retryable_error = true;
        let result_opt = if should_stream {
            match call_provider_once(
                wire_family,
                &user_message,
                &system_prompt,
                assistive,
                lane,
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
                    if !stale_chain_retry_used && is_stale_chain_error(&e) {
                        warn!(
                            "stale conversation chain: dropping stored response_id and retrying unchained"
                        );
                        crate::state::conversation::reset_conversation_for_mode(ai_mode);
                        stale_chain_retry_used = true;
                        continue;
                    }
                    None
                }
            }
        } else {
            let attempt_timeout = request_timing.attempt_timeout();
            match tokio::time::timeout(
                attempt_timeout,
                call_provider_once(
                    wire_family,
                    &user_message,
                    &system_prompt,
                    assistive,
                    lane,
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
                    if !stale_chain_retry_used && is_stale_chain_error(&e) {
                        warn!(
                            "stale conversation chain: dropping stored response_id and retrying unchained"
                        );
                        crate::state::conversation::reset_conversation_for_mode(ai_mode);
                        stale_chain_retry_used = true;
                        continue;
                    }
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
            let formatted = output.assistant_text;
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
                    attempt += 1;
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
        attempt += 1;
    }

    // All providers failed
    warn!("All AI providers/retries failed, returning cleaned text");
    AiFormatResult {
        text: cleaned,
        reasoning_text: None,
        status: AiFormatStatus::Failed,
    }
}

/// Perform exactly one provider attempt over the selected wire format.
///
/// Carries no retry logic of its own — the caller owns the budget, so this stays
/// the single place where "one attempt" is defined.
async fn call_provider_once(
    wire_family: WireFamily,
    user_message: &str,
    system_prompt: &str,
    assistive: bool,
    lane: &RuntimeLlmLane,
    streaming_enabled: bool,
    stream_context: StreamRequestContext,
) -> Result<ProviderOutput> {
    match wire_family {
        WireFamily::AnthropicMessages => {
            call_anthropic_messages(user_message, system_prompt, assistive, lane).await
        }
        WireFamily::OpenAiResponses => {
            if streaming_enabled {
                call_llm_endpoint_streaming(
                    user_message,
                    system_prompt,
                    assistive,
                    lane,
                    stream_context,
                )
                .await
            } else {
                call_llm_endpoint(user_message, system_prompt, assistive, lane).await
            }
        }
    }
}

/// Resolve endpoint, model and credential for the active lane, then send an
/// Anthropic Messages request.
///
/// Resolution is split from [`call_anthropic_messages_resolved`] so tests can
/// exercise the wire contract against a mock without any config in play.
async fn call_anthropic_messages(
    user_message: &str,
    system_prompt: &str,
    assistive: bool,
    lane: &RuntimeLlmLane,
) -> Result<ProviderOutput> {
    if !lane.available() {
        anyhow::bail!(
            "{}",
            lane.unavailable_reason()
                .unwrap_or("Anthropic lane is unavailable")
        );
    }
    let api_key = lane.credential().api_key().unwrap_or_default();

    call_anthropic_messages_resolved(
        user_message,
        system_prompt,
        assistive,
        lane.endpoint(),
        lane.model(),
        api_key,
    )
    .await
}

/// Send an Anthropic Messages request against explicitly supplied wire values.
///
/// Three response shapes are rejected rather than passed on as success: a
/// `refusal` stop (when the model's capability policy reports one), an empty
/// text body, and a `max_tokens` truncation. Each would otherwise surface as a
/// silently short or blank transcript. Temperature is filtered through the
/// model's capability policy, because some Anthropic models reject the field
/// outright rather than ignoring it.
async fn call_anthropic_messages_resolved(
    user_message: &str,
    system_prompt: &str,
    assistive: bool,
    configured_endpoint: &str,
    model: &str,
    api_key: &str,
) -> Result<ProviderOutput> {
    let endpoint = configured_endpoint.to_string();
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

    let mut request_builder = get_client()
        .post(&endpoint)
        .header("anthropic-version", ANTHROPIC_VERSION)
        .header("Content-Type", "application/json")
        .json(&request);
    if !api_key.trim().is_empty() {
        request_builder = request_builder.header("x-api-key", api_key);
    }
    let response = request_builder
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
    lane: &RuntimeLlmLane,
) -> Result<ProviderOutput> {
    let endpoint = lane.endpoint().to_string();
    let model = lane.model().to_string();
    let (api_key, bearer_only) = resolve_lane_auth(lane).await?;

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
        input: build_responses_input(
            system_prompt,
            previous_response_id.as_deref(),
            build_responses_user_content(user_message),
        ),
        // Param on the first turn only; chained turns carry the prompt in input.
        instructions: chained_instructions(system_prompt, previous_response_id.as_deref()),
        previous_response_id: previous_response_id.clone(),
        max_output_tokens: None,
        temperature,
        stream: false,
    };

    // API keys use dual-header (Bearer + x-api-key). OAuth access tokens are
    // Bearer-only — OpenAI rejects account tokens posted as x-api-key.
    let mut request_builder = get_client()
        .post(&endpoint)
        .header("Content-Type", "application/json")
        .json(&request);
    if !api_key.trim().is_empty() {
        request_builder = request_builder.header("Authorization", format!("Bearer {}", api_key));
        if !bearer_only {
            request_builder = request_builder.header("x-api-key", &api_key);
        }
    }
    let response = request_builder.send().await.context("Request failed")?;

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

fn ensure_inline_responses_wire(lane: &RuntimeLlmLane) -> Result<()> {
    if lane.wire_family() != WireFamily::OpenAiResponses {
        anyhow::bail!(
            "Inline formatting requires a Responses API lane; configured {} uses {:?}",
            lane.provider().display_name(),
            lane.wire_family()
        );
    }
    Ok(())
}

/// One chained Responses request over a pinned Formatting lane, chain owned by
/// the caller (W13-1 inline-format buffer).
///
/// Deliberately does NOT touch [`crate::state::conversation`]: the inline
/// buffer keeps its own `previous_response_id` per dictation session, so chunk
/// chaining can reset per session without disturbing the persistent
/// formatting-mode conversation. Returns `(assistant_text, response_id)`.
pub(crate) async fn format_inline_chunk(
    chunk_text: &str,
    language: Option<&str>,
    previous_response_id: Option<String>,
    system_prompt: &str,
    lane: &RuntimeLlmLane,
) -> Result<(String, Option<String>)> {
    ensure_inline_responses_wire(lane)?;
    let api_key = lane
        .credential()
        .api_key()
        .context("Inline formatting requires the sealed formatting credential")?;
    format_inline_chunk_resolved(
        chunk_text,
        language,
        previous_response_id,
        system_prompt,
        lane.endpoint(),
        lane.model(),
        api_key,
    )
    .await
}

/// Send one inline-chunk Responses request against explicitly supplied wire
/// values. Resolution is split out (mirroring
/// [`call_anthropic_messages_resolved`]) so the delivery harness can exercise
/// the wire contract against a mock without config in play.
pub(crate) async fn format_inline_chunk_resolved(
    chunk_text: &str,
    language: Option<&str>,
    previous_response_id: Option<String>,
    system_prompt: &str,
    endpoint: &str,
    model: &str,
    api_key: &str,
) -> Result<(String, Option<String>)> {
    let user_message = match language {
        Some(lang) => format!("[Language: {lang}]\n\n{chunk_text}"),
        None => chunk_text.to_string(),
    };
    let request = ResponsesRequest {
        model: model.to_string(),
        input: build_responses_input(
            system_prompt,
            previous_response_id.as_deref(),
            vec![InputContent::Text { text: user_message }],
        ),
        // Param on the first turn only; chained turns carry the prompt in input.
        instructions: chained_instructions(system_prompt, previous_response_id.as_deref()),
        previous_response_id,
        max_output_tokens: None,
        temperature: get_temperature(false),
        stream: false,
    };

    let response = get_client()
        .post(endpoint)
        .header("Authorization", format!("Bearer {api_key}"))
        .header("x-api-key", api_key)
        .header("Content-Type", "application/json")
        .json(&request)
        .send()
        .await
        .context("Inline chunk request failed")?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        anyhow::bail!("Inline chunk HTTP {} - {}", status, body);
    }

    let responses_result: ResponsesResponse = response
        .json()
        .await
        .context("Failed to parse inline chunk response")?;
    let output = extract_output_channels(&responses_result.output);
    if output.assistant_text.is_empty() {
        anyhow::bail!(
            "No text content in inline chunk response (id: {})",
            responses_result.id
        );
    }
    Ok((output.assistant_text, Some(responses_result.id)))
}

/// Resolve assistive-lane auth: signed-in ChatGPT OAuth wins over a stored API key.
/// Returns `(secret, bearer_only)` — OAuth tokens must not also go out as `x-api-key`.
async fn resolve_lane_auth(lane: &RuntimeLlmLane) -> Result<(String, bool)> {
    if lane.credential().account_auth() {
        let token = account_auth::access_token(lane.provider())
            .await
            .map_err(|error| anyhow::anyhow!("Provider account authentication failed: {error}"))?;
        return Ok((token, true));
    }
    if !lane.available() {
        anyhow::bail!(
            "{}",
            lane.unavailable_reason().unwrap_or("LLM lane is unavailable")
        );
    }
    Ok((
        lane.credential().api_key().unwrap_or_default().to_string(),
        false,
    ))
}

/// Call LLM endpoint with SSE streaming (Responses API)
///
/// Uses mode-aware config: LLM_{FORMATTING,ASSISTIVE}_{ENDPOINT,MODEL,API_KEY}
async fn call_llm_endpoint_streaming(
    user_message: &str,
    system_prompt: &str,
    assistive: bool,
    lane: &RuntimeLlmLane,
    stream_context: StreamRequestContext,
) -> Result<ProviderOutput> {
    let endpoint = lane.endpoint().to_string();
    let model = lane.model().to_string();
    let (api_key, bearer_only) = resolve_lane_auth(lane).await?;
    let auth_header_mode = if bearer_only {
        AuthHeaderMode::BearerOnly
    } else {
        AuthHeaderMode::BearerAndApiKey
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
        input: build_responses_input(
            system_prompt,
            previous_response_id.as_deref(),
            build_responses_user_content(user_message),
        ),
        // Param on the first turn only; chained turns carry the prompt in input.
        instructions: chained_instructions(system_prompt, previous_response_id.as_deref()),
        previous_response_id: previous_response_id.clone(),
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
    )
    .with_auth_header_mode(auth_header_mode);
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

/// Check if AI formatting is available for report/test flows.
pub fn is_formatting_available(lane: &RuntimeLlmLane) -> bool {
    lane.available()
}

/// Wire-contract and text-hygiene tests for the formatting module.
///
/// Provider tests run against `mockito` and assert the exact JSON body, so a
/// drifted request shape fails here rather than at a live endpoint. Anything
/// touching process env is `#[serial]` — env is global, and parallel tests
/// would otherwise read each other's overrides.
#[cfg(test)]
mod tests {
    use super::*;
    use mockito::Matcher;
    use serde_json::json;
    use serial_test::serial;

    /// Env keys cleared/pinned around Anthropic formatting unit tests.
    const ANTHROPIC_TEST_ENV_KEYS: &[&str] = &[
        "LLM_FORMATTING_TEMPERATURE",
        "LLM_TEMPERATURE",
        "CODESCRIBE_ANTHROPIC_MAX_TOKENS",
    ];
    /// Env flag set in the runtime-lanes child process so nested tests skip re-spawn.
    const RUNTIME_LLM_LANES_TEST_CHILD: &str = "CODESCRIBE_RUNTIME_LLM_LANES_TEST_CHILD";

    /// Inline L3 carries a Responses chain id, so it must not send that wire
    /// shape to Anthropic Messages.
    #[test]
    #[serial]
    fn inline_formatter_accepts_only_sealed_responses_wire() {
        let _provider = EnvGuard::set("LLM_FORMATTING_PROVIDER", "openai-responses");
        let runtime_settings = Config::load_runtime_snapshot().expect("seal Responses lane");
        assert!(ensure_inline_responses_wire(runtime_settings.llm_lanes().formatting()).is_ok());

        let _provider = EnvGuard::set("LLM_FORMATTING_PROVIDER", "anthropic-messages");
        let _endpoint = EnvGuard::set(
            "LLM_ANTHROPIC_ENDPOINT",
            "https://api.anthropic.com/v1/messages",
        );
        let runtime_settings = Config::load_runtime_snapshot().expect("seal Anthropic lane");
        assert!(ensure_inline_responses_wire(runtime_settings.llm_lanes().formatting()).is_err());
    }

    /// The stale-chain classifier keys on the provider's error code alone:
    /// `previous_response_not_found` (id minted under a rotated-away key) is
    /// self-healable; everything else is not a chain problem.
    #[test]
    fn stale_chain_classifier_matches_only_the_not_found_code() {
        let stale = anyhow::anyhow!(
            "HTTP 400 Bad Request - {{\"error\":{{\"code\":\"previous_response_not_found\"}}}}"
        );
        assert!(is_stale_chain_error(&stale));
        let pair = anyhow::anyhow!("HTTP 400 - instructions and previous_response_id together");
        assert!(!is_stale_chain_error(&pair));
        let auth = anyhow::anyhow!("HTTP 401 Unauthorized - missing scopes");
        assert!(!is_stale_chain_error(&auth));
    }

    /// Regression for the field HTTP 400 ("instructions and
    /// previous_response_id together", 2026-08-14): every chained Responses
    /// request must drop the `instructions` PARAM. First turn keeps it.
    #[test]
    fn responses_chain_never_carries_instructions_with_previous_id() {
        assert_eq!(chained_instructions("SYS", None).as_deref(), Some("SYS"));
        assert_eq!(chained_instructions("SYS", Some("resp_123")), None);

        // Wire proof: the chained request serializes without an
        // `instructions` key at all (serde skips the None).
        let request = ResponsesRequest {
            model: "m".into(),
            input: vec![],
            instructions: chained_instructions("SYS", Some("resp_123")),
            previous_response_id: Some("resp_123".into()),
            max_output_tokens: None,
            temperature: None,
            stream: false,
        };
        let wire = serde_json::to_value(&request).expect("serialize");
        assert!(wire.get("instructions").is_none());
        assert_eq!(wire["previous_response_id"], "resp_123");
    }

    /// Regression for the promptless-chain leak (2026-08-14, build 661):
    /// dropping `instructions` on a chained turn left the formatter with NO
    /// system prompt — the chain does NOT preserve instructions server-side —
    /// and the model replied as a chat assistant ("Jasne — oto to samo,
    /// przepisane czytelnie…") which was delivered as the formatted
    /// transcript. A chained turn must re-carry the prompt as a leading
    /// developer input item; a first turn must NOT duplicate it there.
    #[test]
    fn chained_turn_recarries_system_prompt_as_developer_input() {
        let user = vec![InputContent::Text { text: "RAW".into() }];
        let chained = build_responses_input("SYS", Some("resp_123"), user);
        assert_eq!(chained.len(), 2);
        assert_eq!(chained[0].role, "developer");
        match &chained[0].content[0] {
            InputContent::Text { text } => assert_eq!(text, "SYS"),
            other => panic!("developer item must be text, got {other:?}"),
        }
        assert_eq!(chained[1].role, "user");

        let first =
            build_responses_input("SYS", None, vec![InputContent::Text { text: "RAW".into() }]);
        assert_eq!(
            first.len(),
            1,
            "first turn carries the prompt via the instructions param only"
        );
        assert_eq!(first[0].role, "user");
    }

    /// RAII holder that restores one env var to its prior value on drop.
    ///
    /// Captures the previous value rather than assuming the variable was unset,
    /// so a test run under an operator dotenv leaves the environment as it found it.
    struct EnvGuard {
        key: &'static str,
        prev: Option<String>,
    }

    impl EnvGuard {
        /// Set the variable, remembering what was there before.
        fn set(key: &'static str, value: &str) -> Self {
            let prev = std::env::var(key).ok();
            unsafe { std::env::set_var(key, value) };
            Self { key, prev }
        }

        /// Unset the variable, remembering what was there before.
        fn remove(key: &'static str) -> Self {
            let prev = std::env::var(key).ok();
            unsafe { std::env::remove_var(key) };
            Self { key, prev }
        }
    }

    impl Drop for EnvGuard {
        /// Restore the captured value, or unset again if there was none.
        fn drop(&mut self) {
            match self.prev.as_deref() {
                Some(value) => unsafe { std::env::set_var(self.key, value) },
                None => unsafe { std::env::remove_var(self.key) },
            }
        }
    }

    /// A bundle of [`EnvGuard`]s that unwinds in one go at end of scope.
    struct TestEnv {
        guards: Vec<EnvGuard>,
    }

    impl TestEnv {
        /// Clear every temperature and token-cap override so the test starts
        /// from provider defaults rather than the operator's dotenv.
        fn clean() -> Self {
            Self {
                guards: ANTHROPIC_TEST_ENV_KEYS
                    .iter()
                    .map(|key| EnvGuard::remove(key))
                    .collect(),
            }
        }

        /// Add one more override under the same unwind.
        fn set(&mut self, key: &'static str, value: &str) {
            self.guards.push(EnvGuard::set(key, value));
        }
    }

    /// Build a title target directly, bypassing config resolution entirely.
    fn title_provider(
        wire_family: WireFamily,
        endpoint: String,
        model: &str,
        api_key: Option<&str>,
    ) -> ThreadTitleProvider {
        ThreadTitleProvider {
            wire_family,
            endpoint,
            model: model.to_string(),
            api_key: api_key.map(ToOwned::to_owned),
        }
    }

    /// Titles survive the decorations models add despite instructions, and the
    /// length cap clips by character so Polish diacritics are not cut mid-glyph.
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

    /// The title budget is pinned, not env-tunable: titling is a background
    /// nicety and must never be able to stall a session the user is waiting on.
    #[test]
    fn thread_title_contract_has_fixed_timeout_and_token_cap() {
        assert_eq!(THREAD_TITLE_TIMEOUT, Duration::from_secs(8));
        assert_eq!(THREAD_TITLE_MAX_TOKENS, 24);
    }

    #[tokio::test]
    #[serial]
    /// Titling must not join the user's response chain. Asserts the request
    /// carries no `previous_response_id` and that the stored chain id is
    /// byte-identical before and after — a stored title id would leak the title
    /// prompt into every later turn of the real conversation.
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
            WireFamily::OpenAiResponses,
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
    /// The Anthropic title path keeps the same prompt and token cap as the
    /// Responses path, and forwards the user's text unmodified — the two wire
    /// formats must not drift into producing different titles for one input.
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
            WireFamily::AnthropicMessages,
            format!("{}/v1/messages", server.url()),
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
    /// The timeout wraps body download, not just the response headers. The mock
    /// answers `200` immediately and then trickles the body; a timeout applied
    /// only to the header phase would let this call run past its budget.
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
            WireFamily::OpenAiResponses,
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

    /// Detection fires on genuine Whisper loops — both single words and
    /// multi-word phrases — while ordinary Polish prose that merely reuses a
    /// word stays untouched. Over-eager detection would delete real speech.
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

    /// Collapsing a loop keeps the punctuation a reader expects: a comma that
    /// only separated repeats is dropped, a sentence-ending period is kept.
    /// Covers the real-world comma-chained Polish case, not just word doubling.
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

    /// Sub-floor dictations never reach a provider — below the floor there is
    /// no structure to add, and a rewrite would be pure risk.
    #[test]
    fn test_short_non_assistive_text_is_skipped() {
        assert!(should_skip_ai_formatting("krótki", false));
        assert!(should_skip_ai_formatting("123456789", false));
    }

    /// The floor is formatting-only: a two-word assistive request is a real
    /// instruction and must always be sent.
    #[test]
    fn test_assistive_short_text_is_not_skipped() {
        assert!(!should_skip_ai_formatting("Pomóż mi", true));
    }

    /// Pins the boundary as exclusive: text of exactly the floor length is
    /// formatted. Guards the off-by-one that would silently widen the skip zone.
    #[test]
    fn test_non_assistive_text_at_threshold_is_not_skipped() {
        let text = "1234567890";
        assert_eq!(text.chars().count(), NON_ASSISTIVE_AI_SKIP_CHARS);
        assert!(!should_skip_ai_formatting(text, false));
    }

    /// Whitespace-only differences count as an echo — re-spacing is not work.
    #[test]
    fn test_effectively_same_ignores_whitespace_only() {
        assert!(is_effectively_same("raw   one two", "raw one two"));
        assert!(is_effectively_same("raw one two\n", "raw one two"));
    }

    /// The other half of the echo contract: punctuation and capitalization
    /// changes are real formatting work and must not collapse into `AiNoop` —
    /// for dictation they are usually the *only* thing the pass was asked to do.
    #[test]
    fn test_effectively_same_preserves_formatting_changes() {
        assert!(!is_effectively_same("raw one two", "RAW ONE TWO."));
        assert!(!is_effectively_same("to jest test", "To jest test"));
    }

    /// C15D falsifier: public formatter entries require one selected runtime
    /// generation rather than separate policy, lane, prompt, or timing facts.
    #[test]
    fn formatter_entry_requires_runtime_settings_snapshot() {
        let _: fn(
            &str,
            Option<&str>,
            bool,
            &RuntimeSettingsSnapshot,
            Option<AiStreamCallback>,
        ) -> _ = format_text_with_status;
    }

    /// Saved settings win over stale env — a settings change takes effect on the
    /// next request, not the next restart.
    ///
    /// Re-executes itself as a child process with an isolated data dir and
    /// deliberately stale `LLM_*` env. That indirection is required: the lane
    /// resolvers read process-global state, so the contract cannot be observed
    /// honestly from inside a shared test process.
    #[test]
    fn runtime_llm_lanes_read_fresh_settings_after_save() {
        if std::env::var_os(RUNTIME_LLM_LANES_TEST_CHILD).is_none() {
            let data_dir = tempfile::TempDir::new().expect("isolated data dir");
            let executable = std::env::current_exe().expect("current core test executable");
            let status = std::process::Command::new(executable)
                .arg("--exact")
                .arg("llm::ai_formatting::tests::runtime_llm_lanes_read_fresh_settings_after_save")
                .arg("--nocapture")
                .env(RUNTIME_LLM_LANES_TEST_CHILD, "1")
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
                .expect("run isolated runtime LLM lanes test");
            assert!(
                status.success(),
                "isolated runtime LLM lanes test failed: {status}"
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

        let snapshot = Config::load_runtime_snapshot().expect("seal runtime settings");
        assert_eq!(
            snapshot.llm_lanes().formatting().endpoint(),
            "https://fresh-formatting.example/v1/responses"
        );
        assert_eq!(snapshot.llm_lanes().formatting().model(), "fresh-formatting-model");
        assert_eq!(
            snapshot.llm_lanes().assistive().endpoint(),
            "https://fresh-assistive.example/v1/responses"
        );
        assert_eq!(snapshot.llm_lanes().assistive().model(), "fresh-assistive-model");
    }

    #[tokio::test]
    #[serial]
    /// A model that accepts `temperature` gets it: the configured value appears
    /// verbatim in the request body.
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
            &format!("{}/v1/messages", server.url()),
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
    /// The mirror case: with the same env override set, a model whose
    /// capability policy rejects `temperature` must have the field omitted
    /// entirely. Sending it anyway is a hard API error, not a soft ignore.
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
            &format!("{}/v1/messages", server.url()),
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
    /// A refusal arrives as HTTP 200 with empty content. It must surface as an
    /// error naming the cause, not as a successful empty transcript — otherwise
    /// the user silently loses their dictation with no explanation.
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
            &format!("{}/v1/messages", server.url()),
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
    /// Multiple text blocks join with no separator: Anthropic splits one
    /// continuous answer across blocks, so any inserted glue would corrupt it.
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
            &format!("{}/v1/messages", server.url()),
            "claude-sonnet-4-6",
            "anthropic-test-key",
        )
        .await
        .expect("text content blocks should parse");

        assert_eq!(output.assistant_text, "Hello world.");
        mock.assert_async().await;
    }

    /// Pins the retry classifier against the exact error strings it matches on.
    /// Deterministic failures (empty completion, refusal, rejected request) are
    /// not retried; a transport stall is. Since the classifier reads opaque
    /// messages, this test is the only thing anchoring those strings.
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
