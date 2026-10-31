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

use crate::config::{FormattingPolicy, RuntimeLlmLane, RuntimeSettingsSnapshot};

use super::account_auth;
use super::provider::{ProviderKind, ReasoningEffort, WireFamily, capability_policy};
use super::responses_output::{ResponsesOutputItem, extract_output_channels};
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

/// Host-owned Max execution capability. Core formatting never constructs
/// platform tools or discovers a conversation through process-global state.
#[async_trait::async_trait]
pub trait FormattingAgent: Send + Sync {
    async fn execute(
        &self,
        turn_id: &str,
        text: &str,
        settings: &RuntimeSettingsSnapshot,
    ) -> Result<String>;

    /// Tool-free assessment; neither silence nor a seal alone establishes a
    /// complete instruction. Unavailable assessment cannot authorize execution.
    async fn assess_group(
        &self,
        _input: crate::agent::consultation::SealedConsultationInput,
        _settings: &RuntimeSettingsSnapshot,
    ) -> Result<crate::agent::consultation::ConsultationReadiness> {
        anyhow::bail!("consultation readiness assessment unavailable")
    }

    /// Blocking durable preparation without execution permission. The live
    /// host runs this off capture, then revalidates before authorizing the handle.
    fn prepare_group(
        &self,
        _input: crate::agent::consultation::SealedConsultationInput,
        _settings: &RuntimeSettingsSnapshot,
    ) -> Result<crate::agent::consultation::PreparedConsultationGroup> {
        anyhow::bail!("grouped consultation execution unavailable")
    }
}

/// Explicit consultation and turn selected by the presentation/capture host.
pub struct FormattingConsultation<'a> {
    pub agent: &'a dyn FormattingAgent,
    pub turn_id: &'a str,
}

/// Result of a formatting request: the text to use plus how it was obtained.
///
/// `text` carries the provider's formatting — on failure it carries the input
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
    let msg = error.to_string();
    msg.contains("previous_response_not_found")
        || (msg.contains("response_not_found") && msg.contains("previous_response_id"))
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
    /// Google resolves API-key/ADC credentials at the request boundary.
    credential_account: Option<String>,
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
    let api_key = lane.credential().request_api_key();
    if let Some(reason) = lane.request_unavailable_reason() {
        anyhow::bail!("{reason}");
    }
    // Titles are a one-shot JSON call with key headers. A signed-in account
    // lane without a stored key is not wired for this call; say so instead
    // of sending an unauthenticated request.
    if api_key.is_none()
        && lane.credential().account_auth()
        && lane.wire_family() != WireFamily::GoogleGenerateContent
    {
        anyhow::bail!(
            "Thread titles need a stored API key on the formatting lane; the signed-in {} account serves formatting only",
            lane.provider_display_name()
        );
    }

    Ok(ThreadTitleProvider {
        wire_family: lane.wire_family(),
        endpoint: lane.endpoint().to_string(),
        model: lane.model().to_string(),
        api_key,
        credential_account: Some(lane.credential().key_account().to_string()),
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
        WireFamily::GoogleGenerateContent => request_google_thread_title(text, provider).await,
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
        store: None,
        reasoning: None,
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

/// Title requests use the native Google protocol without joining a tool or
/// response chain. ADC is refreshed at send time rather than frozen in settings.
async fn request_google_thread_title(text: &str, provider: &ThreadTitleProvider) -> Result<String> {
    let credential = match provider.api_key.as_ref() {
        Some(key) => account_auth::GoogleCredential::ApiKey(key.clone()),
        None => {
            account_auth::resolve_google_credential(
                provider
                    .credential_account
                    .as_deref()
                    .unwrap_or("LLM_GOOGLE_API_KEY"),
            )
            .await?
        }
    };
    let output = call_google_generate_content(
        &provider.endpoint,
        &provider.model,
        text,
        THREAD_TITLE_PROMPT,
        credential,
        Some(THREAD_TITLE_MAX_TOKENS),
        None,
    )
    .await?;
    Ok(output.assistant_text)
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
    /// `Some(true)` on a key route, `Some(false)` on an account backend that
    /// keeps no server chain, omitted only by the stateless title request.
    #[serde(skip_serializing_if = "Option::is_none")]
    store: Option<bool>,
    /// Formatting only; see [`reasoning_ladder`].
    #[serde(skip_serializing_if = "Option::is_none")]
    reasoning: Option<ReasoningParam>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    stream: bool,
}

/// The Responses `reasoning` object; only `effort` is sent.
#[derive(Debug, Clone, Copy, Serialize)]
struct ReasoningParam {
    effort: &'static str,
}

/// Reasoning efforts a formatting request asks for, the selected level first,
/// ending in no field at all. Formatting transforms text it already has: the
/// local Qwen 3.5 9B on MTPLX, left at its server's `auto`, thought for
/// 14 190 tokens (180 s) over one 30-word sentence without answering, and
/// answered in 1.4 s with effort `none` (2026-10-22). Maciej the same day: ask
/// with none or low, higher levels opt-in. A provider that refuses a rung by
/// name gets the next one. A lane without a level (the Agent) sends no field.
fn reasoning_ladder(lane: &RuntimeLlmLane) -> &'static [Option<&'static str>] {
    match lane.reasoning() {
        None => &[None],
        Some(ReasoningEffort::None) => &[Some("none"), Some("low"), None],
        Some(ReasoningEffort::Minimal) => &[Some("minimal"), None],
        Some(ReasoningEffort::Low) => &[Some("low"), None],
        Some(ReasoningEffort::Medium) => &[Some("medium"), None],
        Some(ReasoningEffort::High) => &[Some("high"), None],
        Some(ReasoningEffort::XHigh) => &[Some("xhigh"), None],
    }
}

/// The rung each formatting endpoint, model and level reached in this
/// process, so a refusal is paid once and not on every live formatter job.
static FORMATTING_REASONING_RUNG: OnceLock<
    std::sync::Mutex<std::collections::HashMap<String, usize>>,
> = OnceLock::new();

fn reasoning_rung_key(lane: &RuntimeLlmLane) -> String {
    let level = lane.reasoning().map_or("-", ReasoningEffort::as_str);
    format!("{}\n{}\n{level}", lane.endpoint(), lane.model())
}

fn reasoning_rung(lane: &RuntimeLlmLane) -> usize {
    FORMATTING_REASONING_RUNG
        .get_or_init(Default::default)
        .lock()
        .map(|rungs| rungs.get(&reasoning_rung_key(lane)).copied().unwrap_or(0))
        .unwrap_or(0)
}

fn remember_reasoning_rung(lane: &RuntimeLlmLane, rung: usize) {
    if let Ok(mut rungs) = FORMATTING_REASONING_RUNG
        .get_or_init(Default::default)
        .lock()
    {
        rungs.insert(reasoning_rung_key(lane), rung);
    }
}

/// A request refused for its `reasoning` field: buddy's MTPLX build wants
/// `auto|low|…` and refuses `none`; OpenAI refuses `reasoning.effort` on a
/// model that does not reason. Both come back as a 400 naming the field
/// before any stream opens, so the same attempt can be sent again.
fn is_reasoning_param_rejected(error: &anyhow::Error) -> bool {
    super::provider::is_reasoning_param_rejection(&error.to_string())
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

/// Earlier turns a take replays when no stored predecessor carries them. A
/// bound, not a measured optimum: enough for a sentence spread over several
/// silence-delimited fragments, without resending a long take on every one.
const TAKE_CHAIN_REPLAY_TURNS: usize = 8;

/// One take's formatter conversation (repair points 3–4 of Olga's 2026-10-22
/// report, `2026_1022/olga-responses-chain`): fragments go one after another,
/// each with what came before in the same take, and nothing crosses into
/// another take or the Formatting Test.
///
/// A key route chains to this take's own completed predecessor
/// (`previous_response_id`, `store: true`). The signed-in account backend keeps
/// no responses, so it replays this take's earlier turns in `input`; a key route
/// whose predecessor is refused or missing reseeds the same way instead of
/// sending the fragment bare.
#[derive(Debug, Default)]
pub struct TakeFormatterChain {
    turn: tokio::sync::Mutex<()>,
    state: std::sync::Mutex<TakeChainState>,
}

#[derive(Debug, Default)]
struct TakeChainState {
    previous_response_id: Option<String>,
    turns: std::collections::VecDeque<(String, String)>,
}

tokio::task_local! {
    static TAKE_CHAIN: Arc<TakeFormatterChain>;
}

impl TakeFormatterChain {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// Run one formatter call as this take's next turn: it starts after the
    /// previous turn has finished, so it reads that turn's reply.
    pub async fn next_turn<F: std::future::Future>(self: &Arc<Self>, call: F) -> F::Output {
        let _turn = self.turn.lock().await;
        TAKE_CHAIN.scope(Arc::clone(self), call).await
    }

    fn state(&self) -> std::sync::MutexGuard<'_, TakeChainState> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn previous_response_id(&self) -> Option<String> {
        self.state().previous_response_id.clone()
    }

    fn replay_turns(&self) -> Vec<(String, String)> {
        self.state().turns.iter().cloned().collect()
    }

    /// A completed turn joins the replay; its id becomes the predecessor, and a
    /// turn without one (account backend, or a reply that named none) makes the
    /// next turn replay instead of chaining past it.
    fn record(&self, user_message: &str, assistant_text: &str, response_id: Option<String>) {
        let mut state = self.state();
        state
            .turns
            .push_back((user_message.to_string(), assistant_text.to_string()));
        while state.turns.len() > TAKE_CHAIN_REPLAY_TURNS {
            state.turns.pop_front();
        }
        state.previous_response_id = response_id.filter(|id| !id.is_empty());
    }

    fn forget_predecessor(&self) {
        self.state().previous_response_id = None;
    }
}

/// The take that owns this formatting call, if any. Only the live formatter
/// lane runs inside one; every other caller keeps the process-wide chain.
fn take_chain(ai_mode: crate::state::conversation::AiMode) -> Option<Arc<TakeFormatterChain>> {
    if ai_mode != crate::state::conversation::AiMode::Formatting {
        return None;
    }
    TAKE_CHAIN.try_with(Arc::clone).ok()
}

/// Record a completed reply where this call's chain lives.
fn remember_completed_turn(
    ai_mode: crate::state::conversation::AiMode,
    user_message: &str,
    assistant_text: &str,
    response_id: Option<String>,
) {
    if let Some(take) = take_chain(ai_mode) {
        take.record(user_message, assistant_text, response_id);
    } else if let Some(response_id) = response_id {
        crate::state::conversation::set_response_id_for_mode(ai_mode, response_id);
    }
}

/// Drop a predecessor the server refused, so the retry reseeds from the take's
/// own turns (or, outside a take, starts unchained as before).
fn forget_rejected_predecessor(ai_mode: crate::state::conversation::AiMode) {
    if let Some(take) = take_chain(ai_mode) {
        take.forget_predecessor();
    } else {
        crate::state::conversation::reset_conversation_for_mode(ai_mode);
    }
}

/// Build the `input` items for a Responses request. On chained turns the
/// system prompt rides as a leading `developer` item, because the
/// `instructions` param is absent there (see [`chained_instructions`]) and the
/// chain does not carry it server-side. First turns carry the prompt via
/// `instructions` only — no duplicate developer item. Replayed turns of the
/// same take go before the new fragment as the user/assistant pairs they were.
fn build_responses_input(
    system_prompt: &str,
    previous_response_id: Option<&str>,
    replay: &[(String, String)],
    user_content: Vec<InputContent>,
) -> Vec<InputItem> {
    let mut input = Vec::with_capacity(2 + replay.len() * 2);
    if previous_response_id.is_some() {
        input.push(InputItem {
            role: "developer",
            content: vec![InputContent::Text {
                text: system_prompt.to_string(),
            }],
        });
    }
    for (user, assistant) in replay {
        input.push(InputItem {
            role: "user",
            content: vec![InputContent::Text { text: user.clone() }],
        });
        input.push(InputItem {
            role: "assistant",
            content: vec![InputContent::Output {
                text: assistant.clone(),
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
    /// An earlier assistant reply replayed into `input`.
    #[serde(rename = "output_text")]
    Output { text: String },
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

/// A single content block of an Anthropic turn.
///
/// Text only: the formatter transforms a transcript, and a transcript never
/// selects a file, so no image block is built on this path.
#[derive(Debug, Serialize)]
#[serde(tag = "type")]
enum AnthropicContentBlock {
    #[serde(rename = "text")]
    Text { text: String },
}

/// Responses API response format.
#[derive(Debug, Deserialize)]
struct ResponsesResponse {
    id: String,
    output: Vec<ResponsesOutputItem>,
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
    format_text_with_status_channels(text, language, assistive, runtime_settings, on_delta, None)
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
        None,
    )
    .await
    .0
}

/// Format through the normalized policy sealed in the selected generation.
/// Deliberate one-shot callers choose a generation at their outer boundary;
/// this request path cannot reconstruct policy or prompt independently.
pub async fn format_text_with_status_for_policy(
    text: &str,
    language: Option<&str>,
    runtime_settings: &RuntimeSettingsSnapshot,
    consultation: Option<FormattingConsultation<'_>>,
) -> AiFormatResult {
    format_text_with_status_channels_for_policy(
        text,
        language,
        false,
        runtime_settings,
        None,
        None,
        consultation,
    )
    .await
    .0
}

/// Settings › Formatting › Test sample: a raw transcript as the engines hand
/// it to the formatter, lowercase and unpunctuated (take 6493bab0, 2026-10-22).
pub const FORMATTING_PROBE_SAMPLE: &str = "tak jeśli nie stosuje się zaleceń dokumentacji \
     i nie czyta się specyfikacji to instrukcja się przenosi w polu które jest niedozwolone";

/// How one Settings formatting test ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FormattingProbeStatus {
    /// The provider answered with formatted text.
    Applied,
    /// The provider answered with the input unchanged.
    Unchanged,
    /// The level is off or Max, or the text is below the formatting floor.
    Skipped,
    /// Every attempt failed; the reason is in the log.
    Failed,
    /// No answer within a live formatter job's deadline.
    TimedOut,
}

/// Bounded diagnostics from the existing formatter request. Never carries a
/// bearer, provider response body, URL query, prompt or transcript.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FormattingFailure {
    Unavailable,
    AccountAuthentication,
    Http(u16),
    Timeout,
    Transport,
    ProviderResponse,
}

impl FormattingFailure {
    pub fn code(self) -> &'static str {
        match self {
            Self::Unavailable => "unavailable",
            Self::AccountAuthentication => "account_authentication",
            Self::Http(_) => "http",
            Self::Timeout => "timeout",
            Self::Transport => "transport",
            Self::ProviderResponse => "provider_response",
        }
    }

    pub fn http_status(self) -> Option<u16> {
        match self {
            Self::Http(status) => Some(status),
            _ => None,
        }
    }

    fn from_error(error: &anyhow::Error) -> Self {
        for cause in error.chain() {
            if let Some(transport) = cause.downcast_ref::<reqwest::Error>() {
                if transport.is_timeout() {
                    return Self::Timeout;
                }
                if let Some(status) = transport.status() {
                    return Self::Http(status.as_u16());
                }
                return Self::Transport;
            }
            let message = cause.to_string();
            // Only the request owner's fixed HTTP prefix is inspected. The
            // provider body may echo secrets or source text: never retain it.
            if let Some(status) = message
                .strip_prefix("HTTP ")
                .or_else(|| message.strip_prefix("Anthropic HTTP "))
                .and_then(|rest| rest.split_whitespace().next())
                .and_then(|code| code.parse::<u16>().ok())
                .filter(|code| (100..=599).contains(code))
            {
                return Self::Http(status);
            }
            if message.contains("account authentication failed:") {
                return Self::AccountAuthentication;
            }
            if message.starts_with("SSE initial response timeout")
                || message.starts_with("SSE inter-chunk timeout")
                || message.starts_with("SSE stream inter-chunk timeout")
                || message.starts_with("SSE stream deadline")
            {
                return Self::Timeout;
            }
        }
        Self::ProviderResponse
    }
}

/// One formatting request sent from Settings, measured.
#[derive(Debug, Clone)]
pub struct FormattingProbe {
    pub status: FormattingProbeStatus,
    pub input: String,
    pub output: String,
    pub elapsed: Duration,
    /// The live channel's deadline the request had to meet.
    pub deadline: Duration,
    pub provider: String,
    pub model: String,
    pub reasoning: Option<&'static str>,
    pub failure: Option<FormattingFailure>,
    /// Configured cloud route, not a claim that an on-device success used it.
    pub endpoint_host: Option<String>,
    pub credential_source: &'static str,
    pub checked_at_unix_ms: u64,
}

/// Format `sample` (or [`FORMATTING_PROBE_SAMPLE`]) once, exactly as a live
/// take's formatter job does: same generation, prompt, lane, reasoning level
/// and deadline (Maciej, 2026-10-22: the provider screen needs a one-request
/// test, and the test must hold for the live channel, not post-processing).
pub async fn probe_formatting(
    sample: Option<&str>,
    language: Option<&str>,
    runtime_settings: &RuntimeSettingsSnapshot,
) -> FormattingProbe {
    let input = sample
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .unwrap_or(FORMATTING_PROBE_SAMPLE)
        .to_string();
    let lane = runtime_settings.llm_lanes().formatting();
    let deadline = runtime_settings
        .ai_execution()
        .request_timing()
        .attempt_timeout();
    let ladder = reasoning_ladder(lane);
    let checked_at_unix_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| u64::try_from(duration.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0);
    let account = lane.credential().account_auth();
    let endpoint = account
        .then(|| {
            lane.vendor()
                .and_then(account_auth::account_responses_route)
        })
        .flatten()
        .map(|route| route.endpoint)
        .unwrap_or_else(|| lane.endpoint().to_string());
    let endpoint_host = reqwest::Url::parse(&endpoint)
        .ok()
        .and_then(|url| url.host_str().map(str::to_string));
    let measured = |status, output: String, elapsed, failure| FormattingProbe {
        status,
        input: input.clone(),
        output,
        elapsed,
        deadline,
        provider: lane.provider_display_name().to_string(),
        model: lane.model().to_string(),
        reasoning: ladder[reasoning_rung(lane).min(ladder.len() - 1)],
        failure,
        endpoint_host: endpoint_host.clone(),
        credential_source: if account {
            "account"
        } else {
            "api_key_or_key_optional"
        },
        checked_at_unix_ms,
    };
    // Off sends nothing and Max runs on the Agent lane: neither is this lane.
    if matches!(
        runtime_settings.formatting_policy(),
        FormattingPolicy::Off | FormattingPolicy::Max
    ) {
        return measured(
            FormattingProbeStatus::Skipped,
            input.clone(),
            Duration::ZERO,
            None,
        );
    }
    let started = std::time::Instant::now();
    let reply = tokio::time::timeout(
        deadline,
        format_text_with_status_channels_for_policy(
            &input,
            language,
            false,
            runtime_settings,
            None,
            None,
            None,
        ),
    )
    .await;
    let elapsed = started.elapsed();
    let (status, output, failure) = match reply {
        Err(_) => (
            FormattingProbeStatus::TimedOut,
            String::new(),
            Some(FormattingFailure::Timeout),
        ),
        Ok((result, failure)) => (
            match result.status {
                AiFormatStatus::Applied => FormattingProbeStatus::Applied,
                AiFormatStatus::AiNoop => FormattingProbeStatus::Unchanged,
                AiFormatStatus::Skipped => FormattingProbeStatus::Skipped,
                AiFormatStatus::Failed => FormattingProbeStatus::Failed,
            },
            result.text,
            failure,
        ),
    };
    measured(status, output, elapsed, failure)
}

/// The single implementation every public formatting entry point funnels into.
///
/// Order of the pipeline matters and is load-bearing:
/// 1. bail out for `Off` policy and sub-floor text (never reaches a provider);
/// 2. send the source text under the selected policy's sealed prompt;
/// 3. attempt the provider up to the retry budget, giving up early on errors
///    that are deterministic;
/// 4. publish formatting as returned: lexical edits and structure belong to
///    the prompt, not word-count or word-order vetoes;
/// 5. classify an exact unchanged formatting echo as `AiNoop`.
///
/// Never returns an error: an exhausted budget yields the original input with a
/// `Failed` status, so the caller always has text to deliver.
async fn format_text_with_status_channels_for_policy(
    text: &str,
    language: Option<&str>,
    assistive: bool,
    runtime_settings: &RuntimeSettingsSnapshot,
    on_assistant_delta: Option<AiStreamCallback>,
    on_reasoning_delta: Option<AiReasoningCallback>,
    consultation: Option<FormattingConsultation<'_>>,
) -> (AiFormatResult, Option<FormattingFailure>) {
    let policy = runtime_settings.formatting_policy();
    if !assistive && policy == FormattingPolicy::Max {
        // Commands and corrections must not pass through the text-only floor,
        // repetition cleanup, expansion retries or refusal/echo filters.
        let result = match consultation {
            Some(context) => {
                context
                    .agent
                    .execute(context.turn_id, text, runtime_settings)
                    .await
            }
            None => Err(anyhow::anyhow!("Max consultation executor unavailable")),
        };
        return match result {
            Ok(text) => (
                AiFormatResult {
                    text,
                    reasoning_text: None,
                    status: AiFormatStatus::Applied,
                },
                None,
            ),
            Err(error) => {
                warn!(%error, "Max consultation did not produce an admitted answer");
                (
                    AiFormatResult {
                        text: text.to_string(),
                        reasoning_text: None,
                        status: AiFormatStatus::Failed,
                    },
                    Some(FormattingFailure::ProviderResponse),
                )
            }
        };
    }
    if !assistive && policy == FormattingPolicy::Off {
        return (
            AiFormatResult {
                text: text.to_string(),
                reasoning_text: None,
                status: AiFormatStatus::Skipped,
            },
            None,
        );
    }

    // Skip short non-assistive texts. The controller quality gate starts at 24 chars,
    // so formatting anything shorter would create an unguarded rewrite zone.
    if should_skip_ai_formatting(text, assistive) {
        return (
            AiFormatResult {
                text: text.to_string(),
                reasoning_text: None,
                status: AiFormatStatus::Skipped,
            },
            None,
        );
    }

    // Give the model the actual source. Whether speech repetition is an
    // intentional emphasis or a false start is governed by its sealed prompt.
    let cleaned = text.to_string();

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
    let mut last_failure = FormattingFailure::ProviderResponse;
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
                info!(
                    model = lane.model(),
                    prompt_source = sealed_prompt.source().as_str(),
                    prompt_sha256 = sealed_prompt.composed_sha256(),
                    base_prompt_sha256 = sealed_prompt.base_sha256(),
                    settings_sha256 = runtime_settings.digest().as_str(),
                    "Using assistive mode (model: {})",
                    lane.model()
                );
            }
        } else if attempt == 0 {
            if runtime_settings.values().format_on_device {
                info!(
                    policy = policy.as_str(),
                    cloud_model = lane.model(),
                    prompt_source = sealed_prompt.source().as_str(),
                    prompt_sha256 = sealed_prompt.composed_sha256(),
                    base_prompt_sha256 = sealed_prompt.base_sha256(),
                    settings_sha256 = runtime_settings.digest().as_str(),
                    "Using formatting mode (on-device: Apple FoundationModels, cloud fallback: {})",
                    lane.model()
                );
            } else {
                info!(
                    policy = policy.as_str(),
                    model = lane.model(),
                    prompt_source = sealed_prompt.source().as_str(),
                    prompt_sha256 = sealed_prompt.composed_sha256(),
                    base_prompt_sha256 = sealed_prompt.base_sha256(),
                    settings_sha256 = runtime_settings.digest().as_str(),
                    "Using formatting mode (model: {})",
                    lane.model()
                );
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

            if assistive {
                system_prompt.push_str(
                    "\n\nCRITICAL: You MUST format/enhance the text. Do NOT return raw input.",
                );
            }
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
            (WireFamily::GoogleGenerateContent, _) => "google-generate-content-json",
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
        // On-device lane (opt-in knob, W6): one host attempt first. Any
        // failure falls back to the cloud wire below inside the same retry
        // budget, so refusal detection, `AiNoop` handling and the retry
        // policy apply to both engines identically.
        let on_device_output = if !assistive && runtime_settings.values().format_on_device {
            match crate::llm::on_device::on_device_formatter() {
                Some(formatter) => match formatter.format(&system_prompt, &user_message).await {
                    Ok(assistant_text) => {
                        if let Some(callback) = stream_context.callbacks.assistant.as_ref() {
                            callback(&assistant_text);
                        }
                        info!(
                            "Formatted on-device ({} -> {} chars)",
                            user_message.len(),
                            assistant_text.len()
                        );
                        Some(ProviderOutput {
                            assistant_text,
                            reasoning_text: None,
                        })
                    }
                    Err(error) => {
                        warn!(%error, "on-device formatter failed; falling back to the cloud lane");
                        None
                    }
                },
                None => {
                    warn!(
                        "on-device formatting selected but no host formatter is registered; \
                         using the cloud lane"
                    );
                    None
                }
            }
        } else {
            None
        };
        if on_device_output.is_none()
            && let Some(reason) = lane.request_unavailable_reason()
        {
            // No engine produced text and no cloud lane can: every further
            // attempt would fail the same way. The caller keeps the text.
            warn!(%reason, "no formatting engine available; preserving the source text");
            last_failure = FormattingFailure::Unavailable;
            break;
        }
        let used_on_device = on_device_output.is_some();
        let result_opt = if on_device_output.is_some() {
            on_device_output
        } else if should_stream {
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
                    last_failure = FormattingFailure::from_error(&e);
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
                    last_failure = FormattingFailure::from_error(&e);
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
                    last_failure = FormattingFailure::Timeout;
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

            if assistive && is_refusal {
                warn!("AI returned refusal response, returning raw input instead");
                return (
                    AiFormatResult {
                        text: cleaned,
                        reasoning_text: None,
                        status: AiFormatStatus::Failed,
                    },
                    Some(FormattingFailure::ProviderResponse),
                );
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
                // Paragraphs and spacing are formatting too. Only an exact
                // echo has no returned change to publish.
                if formatted == cleaned {
                    warn!("Formatting mode returned AI No-op (raw echo)");
                    return (
                        AiFormatResult {
                            text: cleaned,
                            reasoning_text,
                            status: AiFormatStatus::AiNoop,
                        },
                        None,
                    );
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
                    return (
                        AiFormatResult {
                            text: formatted,
                            reasoning_text,
                            status,
                        },
                        (status == AiFormatStatus::Failed)
                            .then_some(FormattingFailure::ProviderResponse),
                    );
                }
            }

            let engine = if used_on_device {
                "apple-on-device"
            } else {
                lane.model()
            };
            info!(
                engine,
                policy = policy.as_str(),
                prompt_source = sealed_prompt.source().as_str(),
                prompt_sha256 = sealed_prompt.composed_sha256(),
                "Formatted via AI ({} -> {} chars, assistive={}, content_match={}, raw_like={})",
                text.len(),
                formatted.len(),
                assistive,
                content_match,
                raw_like
            );
            return (
                AiFormatResult {
                    text: formatted,
                    reasoning_text,
                    status: AiFormatStatus::Applied,
                },
                None,
            );
        } else if !retryable_error {
            warn!("Provider returned deterministic empty-content error; skipping retries");
            break;
        }
        attempt += 1;
    }

    // All providers failed
    warn!("All AI providers/retries failed, returning cleaned text");
    (
        AiFormatResult {
            text: cleaned,
            reasoning_text: None,
            status: AiFormatStatus::Failed,
        },
        Some(last_failure),
    )
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
        WireFamily::GoogleGenerateContent => {
            let credential =
                account_auth::resolve_google_credential(lane.credential().key_account()).await?;
            call_google_generate_content(
                lane.endpoint(),
                lane.model(),
                user_message,
                system_prompt,
                credential,
                None,
                get_temperature(assistive),
            )
            .await
        }
        WireFamily::AnthropicMessages => {
            call_anthropic_messages(user_message, system_prompt, assistive, lane).await
        }
        WireFamily::OpenAiResponses => {
            let ladder = if assistive {
                &[None][..]
            } else {
                reasoning_ladder(lane)
            };
            let mut rung = reasoning_rung(lane).min(ladder.len() - 1);
            loop {
                let reasoning = ladder[rung];
                let result = call_responses_once(
                    user_message,
                    system_prompt,
                    assistive,
                    lane,
                    streaming_enabled,
                    stream_context.clone(),
                    reasoning,
                )
                .await;
                match result {
                    Err(error)
                        if rung + 1 < ladder.len() && is_reasoning_param_rejected(&error) =>
                    {
                        rung += 1;
                        warn!(
                            model = lane.model(),
                            refused = reasoning.unwrap_or("-"),
                            next = ladder[rung].unwrap_or("no field"),
                            "Formatting provider refused its reasoning effort; asking again"
                        );
                        remember_reasoning_rung(lane, rung);
                    }
                    result => return result,
                }
            }
        }
    }
}

/// One native JSON GenerateContent request for a text-only lane. The caller
/// owns attempt timeouts/retries; this transport never fabricates a response
/// chain or borrows the Responses API's body shape.
async fn call_google_generate_content(
    configured_endpoint: &str,
    model: &str,
    user_message: &str,
    system_prompt: &str,
    credential: account_auth::GoogleCredential,
    max_tokens: Option<u32>,
    temperature: Option<f32>,
) -> Result<ProviderOutput> {
    let mut body = serde_json::json!({
        "contents": [{"role": "user", "parts": [{"text": user_message}]}],
    });
    if !system_prompt.trim().is_empty() {
        body["systemInstruction"] = serde_json::json!({"parts": [{"text": system_prompt}]});
    }
    let mut generation = serde_json::Map::new();
    if let Some(tokens) = max_tokens.filter(|tokens| *tokens > 0) {
        generation.insert("maxOutputTokens".to_string(), tokens.into());
    }
    if let Some(temperature) = temperature.filter(|temperature| temperature.is_finite()) {
        generation.insert("temperature".to_string(), temperature.into());
    }
    if !generation.is_empty() {
        body["generationConfig"] = Value::Object(generation);
    }
    let (endpoint, body) =
        super::vendors::google::prepare_request(configured_endpoint, model, body, false)
            .map_err(anyhow::Error::msg)?;
    let mut request = get_client().post(endpoint).json(&body);
    request = match &credential {
        account_auth::GoogleCredential::ApiKey(key) => request.header("x-goog-api-key", key),
        account_auth::GoogleCredential::Bearer(token) => request.bearer_auth(token),
    };
    let response = request.send().await.map_err(|error| {
        anyhow::anyhow!(
            "Google request failed: {}",
            account_auth::redact_google_secrets(&error.without_url().to_string())
        )
    })?;
    let status = response.status();
    let bytes = response.bytes().await.map_err(|error| {
        anyhow::anyhow!(
            "Google response download failed: {}",
            account_auth::redact_google_secrets(&error.without_url().to_string())
        )
    })?;
    if !status.is_success() {
        anyhow::bail!("Google HTTP {status}");
    }
    let body: Value = serde_json::from_slice(&bytes).context("Failed to parse Google response")?;
    extract_google_output(body.get("response").unwrap_or(&body))
}

/// A successful HTTP status alone cannot turn a truncated or refused
/// completion into a formatted transcript. Thoughts stay on their own channel.
fn extract_google_output(body: &Value) -> Result<ProviderOutput> {
    if body
        .get("promptFeedback")
        .and_then(|feedback| feedback.get("blockReason"))
        .is_some()
    {
        anyhow::bail!("Google refused the prompt");
    }
    let candidate = body
        .get("candidates")
        .and_then(Value::as_array)
        .and_then(|candidates| candidates.first())
        .ok_or_else(|| anyhow::anyhow!("No candidate in Google response"))?;
    let finish = candidate
        .get("finishReason")
        .and_then(Value::as_str)
        .unwrap_or("missing");
    anyhow::ensure!(
        finish == "STOP",
        "Google response did not finish cleanly ({})",
        account_auth::redact_google_secrets(finish)
    );
    let mut assistant_text = String::new();
    let mut reasoning_text = String::new();
    for part in candidate
        .get("content")
        .and_then(|content| content.get("parts"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        if let Some(text) = part.get("text").and_then(Value::as_str) {
            if part.get("thought").and_then(Value::as_bool) == Some(true) {
                reasoning_text.push_str(text);
            } else {
                assistant_text.push_str(text);
            }
        }
    }
    anyhow::ensure!(
        !assistant_text.trim().is_empty(),
        "No text content in Google response"
    );
    Ok(ProviderOutput {
        assistant_text,
        reasoning_text: (!reasoning_text.is_empty()).then_some(reasoning_text),
    })
}

/// One Responses request, streamed or JSON, at one reasoning effort.
async fn call_responses_once(
    user_message: &str,
    system_prompt: &str,
    assistive: bool,
    lane: &RuntimeLlmLane,
    streaming_enabled: bool,
    stream_context: StreamRequestContext,
    reasoning: Option<&'static str>,
) -> Result<ProviderOutput> {
    if streaming_enabled {
        call_llm_endpoint_streaming(
            user_message,
            system_prompt,
            assistive,
            lane,
            stream_context,
            reasoning,
        )
        .await
    } else {
        call_llm_endpoint(user_message, system_prompt, assistive, lane, reasoning).await
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
    let api_key = lane_request_credential(lane, true)
        .await?
        .unwrap_or_default();
    call_anthropic_messages_resolved(
        user_message,
        system_prompt,
        assistive,
        lane.endpoint(),
        lane.model(),
        &api_key,
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
            content: vec![AnthropicContentBlock::Text {
                text: user_message.to_string(),
            }],
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

/// Chain state shared by the JSON and SSE formatting calls.
///
/// Thread titles do not use this: a title is one shot and must not join the
/// conversation chain.
fn responses_chain_request(
    user_message: &str,
    system_prompt: &str,
    assistive: bool,
    model: String,
    stream: bool,
    account_backend: bool,
    reasoning: Option<&'static str>,
) -> (
    crate::state::conversation::AiMode,
    Option<String>,
    ResponsesRequest,
) {
    let ai_mode = if assistive {
        crate::state::conversation::AiMode::Assistive
    } else {
        crate::state::conversation::AiMode::Formatting
    };
    // An account backend stores nothing: each request carries its own prompt
    // and no chain id, mirroring the Agent provider's body on that route. A
    // take's own turns ride in `input` whenever no predecessor carries them.
    let take = take_chain(ai_mode);
    let previous_response_id = if account_backend {
        None
    } else if let Some(take) = &take {
        take.previous_response_id()
    } else {
        crate::state::conversation::get_previous_response_id_for_mode(ai_mode)
    };
    let replay = match &take {
        Some(take) if previous_response_id.is_none() => take.replay_turns(),
        _ => Vec::new(),
    };
    let temperature = if account_backend {
        None
    } else {
        get_temperature(assistive)
    };
    let request = ResponsesRequest {
        model,
        input: build_responses_input(
            system_prompt,
            previous_response_id.as_deref(),
            &replay,
            vec![InputContent::Text {
                text: user_message.to_string(),
            }],
        ),
        instructions: chained_instructions(system_prompt, previous_response_id.as_deref()),
        previous_response_id: previous_response_id.clone(),
        max_output_tokens: None,
        temperature,
        // A key route asks the server to keep the response explicitly: an
        // omitted `store` is `true` on OpenAI but `false` on mtplx, where every
        // chained request then came back 404 and fell back to a bare fragment.
        store: Some(!account_backend),
        reasoning: reasoning.map(|effort| ReasoningParam { effort }),
        stream,
    };
    (ai_mode, previous_response_id, request)
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
    reasoning: Option<&'static str>,
) -> Result<ProviderOutput> {
    let model = lane.model().to_string();
    let (api_key, bearer_only) = resolve_lane_auth(lane).await?;
    // The Codex backend only streams; JSON is never sent there.
    if bearer_only
        && lane
            .vendor()
            .and_then(account_auth::account_responses_route)
            .is_some()
    {
        anyhow::bail!(
            "The signed-in {} account accepts only streamed Responses requests",
            lane.provider_display_name()
        );
    }
    let endpoint = lane.endpoint().to_string();
    let temperature = get_temperature(assistive);

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

    let (ai_mode, previous_response_id, request) = responses_chain_request(
        user_message,
        system_prompt,
        assistive,
        model.clone(),
        false,
        false,
        reasoning,
    );

    let send_once = |req: &ResponsesRequest| {
        let mut builder = get_client()
            .post(&endpoint)
            .header("Content-Type", "application/json")
            .json(req);
        if !api_key.trim().is_empty() {
            builder = builder.header("Authorization", format!("Bearer {}", api_key));
            if !bearer_only {
                builder = builder.header("x-api-key", &api_key);
            }
        }
        builder
    };
    let mut response = send_once(&request).send().await.context("Request failed")?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        if previous_response_id.is_some() && is_stale_chain_error(&anyhow::anyhow!("{body}")) {
            warn!(
                "Clearing rejected previous_response_id for {:?} and retrying unchained",
                ai_mode
            );
            forget_rejected_predecessor(ai_mode);
            let (_, _, unchained) = responses_chain_request(
                user_message,
                system_prompt,
                assistive,
                model,
                false,
                false,
                reasoning,
            );
            response = send_once(&unchained)
                .send()
                .await
                .context("Request failed")?;
            if !response.status().is_success() {
                let retry_status = response.status();
                let retry_body = response.text().await.unwrap_or_default();
                anyhow::bail!("HTTP {} - {}", retry_status, retry_body);
            }
        } else {
            anyhow::bail!("HTTP {} - {}", status, body);
        }
    }

    let responses_result: ResponsesResponse =
        response.json().await.context("Failed to parse response")?;

    let channels = extract_output_channels(&responses_result.output);
    let output = ProviderOutput {
        assistant_text: channels.assistant_text,
        reasoning_text: channels.reasoning_text,
    };

    if output.assistant_text.is_empty() {
        anyhow::bail!("No text content in response (id: {})", responses_result.id);
    }

    // Store response_id for this mode's conversation chain (separate streams)
    remember_completed_turn(
        ai_mode,
        user_message,
        &output.assistant_text,
        Some(responses_result.id.clone()),
    );
    debug!(
        "Response id ({}): {}",
        if assistive { "assistive" } else { "formatting" },
        responses_result.id
    );
    Ok(output)
}

/// Resolve one lane's auth: a signed-in provider account wins over a stored
/// API key (same order as the Agent provider). The token is fetched, and
/// refreshed when near expiry, for this request only. Returns
/// `(secret, bearer_only)` — account tokens must not also go out as `x-api-key`.
async fn resolve_lane_auth(lane: &RuntimeLlmLane) -> Result<(String, bool)> {
    let account_vendor = lane
        .credential()
        .account_auth()
        .then(|| lane.vendor())
        .flatten();
    let api_key = lane_request_credential(lane, account_vendor.is_none()).await?;
    if let Some(vendor) = account_vendor {
        let token = account_auth::access_token(vendor).await.map_err(|error| {
            anyhow::anyhow!(
                "{} account authentication failed: {error}",
                vendor.display_name()
            )
        })?;
        return Ok((token, true));
    }
    Ok((api_key.unwrap_or_default(), false))
}

/// The lane's request-time credential verdict, taken on the blocking pool:
/// refusal when no credential truth exists now, else the stored API key when
/// `read_api_key`. Both read the Keychain on a cold cache, and a Keychain call
/// can wait on a macOS authorization dialog for as long as it stays open. A
/// live take polls its formatter jobs inline, so a read on the polling thread
/// stopped the whole session and every job deadline with it (replay cbf54ca9,
/// 2026-10-21).
async fn lane_request_credential(
    lane: &RuntimeLlmLane,
    read_api_key: bool,
) -> Result<Option<String>> {
    let lane = lane.clone();
    tokio::task::spawn_blocking(move || {
        if let Some(reason) = lane.request_unavailable_reason() {
            anyhow::bail!("{reason}");
        }
        Ok(read_api_key
            .then(|| lane.credential().request_api_key())
            .flatten())
    })
    .await
    .map_err(|error| anyhow::anyhow!("credential storage task ended: {error}"))?
}

/// Call LLM endpoint with SSE streaming (Responses API) through the sealed lane.
async fn call_llm_endpoint_streaming(
    user_message: &str,
    system_prompt: &str,
    assistive: bool,
    lane: &RuntimeLlmLane,
    stream_context: StreamRequestContext,
    reasoning: Option<&'static str>,
) -> Result<ProviderOutput> {
    let model = lane.model().to_string();
    let (api_key, bearer_only) = resolve_lane_auth(lane).await?;
    let auth_header_mode = if bearer_only {
        AuthHeaderMode::BearerOnly
    } else {
        AuthHeaderMode::BearerAndApiKey
    };
    // Same account backend and headers as the Agent provider; a key-auth lane
    // keeps its own endpoint and chain.
    let account_route = bearer_only
        .then(|| {
            lane.vendor()
                .and_then(account_auth::account_responses_route)
        })
        .flatten();
    let account_backend = account_route.is_some();
    let (endpoint, extra_headers) = match account_route {
        Some(route) => (route.endpoint, route.headers),
        None => (lane.endpoint().to_string(), Vec::new()),
    };
    let temperature = if account_backend {
        None
    } else {
        get_temperature(assistive)
    };

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

    let (ai_mode, previous_response_id, request) = responses_chain_request(
        user_message,
        system_prompt,
        assistive,
        model.clone(),
        true,
        account_backend,
        reasoning,
    );

    let StreamRequestContext {
        callbacks,
        initial_response_timeout,
        inter_chunk_timeout,
    } = stream_context;
    let manager = ResponsesStreamingManager::new(
        get_client(),
        &endpoint,
        &api_key,
        callbacks.clone(),
        initial_response_timeout,
        inter_chunk_timeout,
    )
    .with_auth_header_mode(auth_header_mode)
    .with_extra_headers(extra_headers.clone());
    let streamed = match manager.stream(&request).await {
        Ok(streamed) => streamed,
        Err(err) if previous_response_id.is_some() && is_stale_chain_error(&err) => {
            warn!(
                "Clearing rejected previous_response_id for {:?} and retrying SSE unchained",
                ai_mode
            );
            forget_rejected_predecessor(ai_mode);
            let (_, _, unchained) = responses_chain_request(
                user_message,
                system_prompt,
                assistive,
                model,
                true,
                account_backend,
                reasoning,
            );
            let retry_manager = ResponsesStreamingManager::new(
                get_client(),
                &endpoint,
                &api_key,
                callbacks,
                initial_response_timeout,
                inter_chunk_timeout,
            )
            .with_auth_header_mode(auth_header_mode)
            .with_extra_headers(extra_headers);
            retry_manager.stream(&unchained).await?
        }
        Err(err) => return Err(err),
    };
    let output = ProviderOutput {
        assistant_text: streamed.assistant_text,
        reasoning_text: streamed.reasoning_text,
    };
    if account_backend {
        // `store: false`: the id names nothing a later request may chain to;
        // a take keeps the turn itself and replays it.
        remember_completed_turn(ai_mode, user_message, &output.assistant_text, None);
        debug!(
            "SSE complete on the account backend for {}; no chain id retained",
            if assistive { "assistive" } else { "formatting" }
        );
    } else if let Some(response_id) = streamed.response_id.filter(|id| !id.is_empty()) {
        remember_completed_turn(
            ai_mode,
            user_message,
            &output.assistant_text,
            Some(response_id.clone()),
        );
        debug!(
            "SSE complete, response_id ({}): {}",
            if assistive { "assistive" } else { "formatting" },
            response_id
        );
    } else {
        // A take keeps this turn and replays it next time, since no id chains
        // past it; outside a take the previous id is kept as before.
        remember_completed_turn(ai_mode, user_message, &output.assistant_text, None);
        if let Some(prev_id) = previous_response_id.as_deref()
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
    }
    Ok(output)
}

/// Check if the cloud formatting lane can send a request now.
pub fn is_formatting_available(lane: &RuntimeLlmLane) -> bool {
    lane.request_unavailable_reason().is_none()
}

/// Whether a Smart/Corrections text pass has an engine under this generation:
/// the selected Apple on-device formatter (host-registered, no credential),
/// or the configured cloud formatting lane. Max is not a text pass; its
/// availability is the Agent lane's (see the app's Max consultation).
/// `None` means a request may be attempted; the text always comes from the
/// caller, never from audio or a seal.
pub fn text_formatting_unavailable_reason(settings: &RuntimeSettingsSnapshot) -> Option<String> {
    if settings.values().format_on_device && crate::llm::on_device::on_device_formatter().is_some()
    {
        return None;
    }
    settings
        .llm_lanes()
        .formatting()
        .request_unavailable_reason()
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
    use crate::config::Config;
    use mockito::Matcher;
    use serde_json::json;
    use serial_test::serial;

    /// Env keys cleared/pinned around Anthropic formatting unit tests.
    const ANTHROPIC_TEST_ENV_KEYS: &[&str] = &[
        "LLM_FORMATTING_TEMPERATURE",
        "LLM_TEMPERATURE",
        "CODESCRIBE_ANTHROPIC_MAX_TOKENS",
    ];

    /// The stale-chain classifier keys on the provider's error code alone:
    /// `previous_response_not_found` (id minted under a rotated-away key) is
    /// self-healable; everything else is not a chain problem.
    #[test]
    fn stale_chain_classifier_matches_only_the_not_found_code() {
        let stale = anyhow::anyhow!(
            "HTTP 400 Bad Request - {{\"error\":{{\"code\":\"previous_response_not_found\"}}}}"
        );
        assert!(is_stale_chain_error(&stale));
        let buddy_404 = anyhow::anyhow!(
            "HTTP 404 Not Found - {{\"error\":{{\"message\":\"response 'resp-1' was not found\",\"type\":\"invalid_request_error\",\"param\":\"previous_response_id\",\"code\":\"response_not_found\"}}}}"
        );
        assert!(is_stale_chain_error(&buddy_404));
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
            store: None,
            max_output_tokens: None,
            temperature: None,
            reasoning: None,
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
        let chained = build_responses_input("SYS", Some("resp_123"), &[], user);
        assert_eq!(chained.len(), 2);
        assert_eq!(chained[0].role, "developer");
        let InputContent::Text { text } = &chained[0].content[0] else {
            panic!("the re-carried prompt is input text");
        };
        assert_eq!(text, "SYS");
        assert_eq!(chained[1].role, "user");

        let first = build_responses_input(
            "SYS",
            None,
            &[],
            vec![InputContent::Text { text: "RAW".into() }],
        );
        assert_eq!(
            first.len(),
            1,
            "first turn carries the prompt via the instructions param only"
        );
        assert_eq!(first[0].role, "user");
    }

    use crate::test_isolation::EnvGuard;

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
            credential_account: None,
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
    /// only to the header phase would let this call run past its budget. The
    /// budget leaves a loaded host time to connect: at 100 ms, half the runs on
    /// a build machine timed out before the request arrived.
    async fn thread_title_timeout_covers_response_body() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("POST", "/v1/responses")
            .match_body(Matcher::Any)
            .expect(1)
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_chunked_body(|writer| {
                // Beats, not one sleep: see `trickle_until_the_client_leaves`.
                writer.write_all(br#"{"id":"late","#)?;
                trickle_until_the_client_leaves(writer, b" ", Duration::from_millis(1500))?;
                writer.write_all(br#""output":[]}"#)
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
            generate_thread_title_with_provider("Raw input", &provider, Duration::from_millis(500))
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
    ///
    /// Statically proves that all public formatting entry points require the immutable
    /// runtime settings generation (`&RuntimeSettingsSnapshot`) without polling futures,
    /// guaranteeing zero network activity.
    ///
    /// `#[serial]`: the snapshot load resolves the process-global
    /// `CODESCRIBE_DATA_DIR`; running off the serial lane let this load consume a
    /// serial settings test's one-time V1 migration and rewrite its private
    /// fixture (traced 2026-10-01, X-hermetic-test-config load loop).
    #[test]
    #[serial]
    fn formatter_entry_requires_runtime_settings_snapshot() {
        let runtime_settings = Config::load_runtime_snapshot().expect("seal runtime settings");
        let _f1 = format_text("test", None, false, &runtime_settings);
        let _f2 = format_text_with_status("test", None, false, &runtime_settings, None);
        let _f3 =
            format_text_with_status_channels("test", None, false, &runtime_settings, None, None);
        let _f4 = format_text_with_status_for_policy("test", None, &runtime_settings, None);
    }

    /// Saved settings win on the next seal — a settings change takes effect on
    /// the next request, not the next restart. Provider and model come from
    /// `settings.json` through the one resolution path in the loader.
    #[test]
    #[serial]
    fn runtime_llm_lanes_read_fresh_settings_after_save() {
        let data_dir = tempfile::TempDir::new().expect("isolated data dir");
        let mut env = TestEnv::clean();
        env.set(
            "CODESCRIBE_DATA_DIR",
            data_dir.path().to_str().expect("utf-8 test data path"),
        );
        env.set("CODESCRIBE_DISABLE_KEYCHAIN", "1");
        for key in LANE_SELECTOR_ENV_KEYS {
            env.guards.push(EnvGuard::remove(key));
        }
        let mut settings = crate::config::UserSettings {
            llm_formatting_provider: Some("xai-responses".to_string()),
            llm_formatting_model: Some("grok-fresh".to_string()),
            llm_assistive_provider: Some("anthropic-messages".to_string()),
            llm_assistive_model: Some("claude-fresh".to_string()),
            ..Default::default()
        };
        settings.save().expect("persist lane settings");

        let snapshot = Config::load_runtime_snapshot().expect("seal runtime settings");
        assert_eq!(
            snapshot.llm_lanes().formatting().endpoint(),
            "https://api.x.ai/v1/responses"
        );
        assert_eq!(snapshot.llm_lanes().formatting().model(), "grok-fresh");
        assert_eq!(
            snapshot.llm_lanes().assistive().endpoint(),
            "https://api.anthropic.com/v1/messages"
        );
        assert_eq!(snapshot.llm_lanes().assistive().model(), "claude-fresh");

        settings.llm_assistive_model = Some("claude-fresher".to_string());
        settings.save().expect("persist changed model");
        let snapshot = Config::load_runtime_snapshot().expect("re-seal runtime settings");
        assert_eq!(snapshot.llm_lanes().assistive().model(), "claude-fresher");
    }

    /// Every env selector that could hand a lane a provider or a model.
    const LANE_SELECTOR_ENV_KEYS: [&str; 4] = [
        "LLM_FORMATTING_PROVIDER",
        "LLM_FORMATTING_MODEL",
        "LLM_ASSISTIVE_PROVIDER",
        "LLM_ASSISTIVE_MODEL",
    ];

    /// The formatter lane's credential verdict and key read happen off the
    /// polling thread: a live take polls formatter jobs inline, and a cold
    /// Keychain read can wait on an authorization dialog indefinitely.
    #[tokio::test(flavor = "current_thread")]
    #[serial]
    async fn formatter_lane_credentials_are_read_off_the_polling_thread() {
        let data_dir = tempfile::TempDir::new().expect("isolated data dir");
        let mut env = TestEnv::clean();
        env.set(
            "CODESCRIBE_DATA_DIR",
            data_dir.path().to_str().expect("utf-8 test data path"),
        );
        for key in LANE_SELECTOR_ENV_KEYS {
            env.guards.push(EnvGuard::remove(key));
        }
        env.guards.push(EnvGuard::remove("LLM_OPENAI_API_KEY"));
        let snapshot = Config::load_runtime_snapshot().expect("seal lane topology");
        let lane = snapshot.llm_lanes().formatting().clone();
        let _bundle = crate::config::keychain::test_support::install_bundle(&[]);
        let probe = crate::config::keychain::CredentialAcquisitionProbe::forbid();
        let _ = lane_request_credential(&lane, true).await;
        assert!(probe.attempts().is_empty(), "{:?}", probe.attempts());
    }

    /// Both lanes on one Custom row at the mock server, sealed before any key
    /// exists (the production controller built through `new_without_keychain`);
    /// the key that appears in the Keychain bundle afterwards is what every
    /// request authenticates with — Custom keys never pass through env.
    #[tokio::test]
    #[serial]
    async fn every_runtime_llm_lane_authenticates_from_the_key_present_at_request_time() {
        use crate::config::RuntimeLlmLaneKind;
        use crate::config::keychain::test_support::install_bundle;
        use crate::llm::provider::{CustomProvider, WireFamily};
        use crate::state::conversation::{AiMode, reset_conversation_for_mode};

        let data_dir = tempfile::TempDir::new().expect("isolated data dir");
        let mut server = mockito::Server::new_async().await;
        let mut env = TestEnv::clean();
        env.set(
            "CODESCRIBE_DATA_DIR",
            data_dir.path().to_str().expect("utf-8 test data path"),
        );
        env.set("CODESCRIBE_DISABLE_KEYCHAIN", "1");
        for key in LANE_SELECTOR_ENV_KEYS {
            env.guards.push(EnvGuard::remove(key));
        }
        let row = CustomProvider::new("Witness Box", WireFamily::OpenAiResponses, &server.url())
            .expect("valid custom row");
        let account = row.key_account();
        let mut settings = crate::config::UserSettings::default();
        settings.add_custom_provider(row).expect("add custom row");
        settings.llm_formatting_provider = Some("custom:witness-box".to_string());
        settings.llm_formatting_model = Some("gpt-test-formatting".to_string());
        settings.llm_assistive_provider = Some("custom:witness-box".to_string());
        settings.llm_assistive_model = Some("gpt-test-assistive".to_string());
        settings.save().expect("persist lane settings");

        // Seal the lane topology before any credential exists.
        let _empty_bundle = install_bundle(&[]);
        let snapshot = Config::load_runtime_snapshot().expect("seal credential-free topology");
        for lane in [
            RuntimeLlmLaneKind::Formatting,
            RuntimeLlmLaneKind::Assistive,
        ] {
            let lane = snapshot.llm_lanes().lane(lane);
            assert_eq!(lane.credential().key_account(), account);
            assert!(
                lane.credential().api_key().is_none(),
                "baseline snapshot must not already contain the test credential",
            );
        }

        let cases = [
            (
                RuntimeLlmLaneKind::Formatting,
                "synthetic-formatting-key",
                false,
            ),
            (
                RuntimeLlmLaneKind::Assistive,
                "synthetic-assistive-key",
                true,
            ),
        ];
        for (kind, synthetic_key, assistive) in cases {
            let _bundle = install_bundle(&[(account.as_str(), synthetic_key)]);
            let expected_authorization = format!("Bearer {synthetic_key}");
            let mock = server
                .mock("POST", "/v1/responses")
                .match_header("authorization", expected_authorization.as_str())
                .match_header("x-api-key", synthetic_key)
                .match_body(Matcher::Any)
                .expect(1)
                .with_status(200)
                .with_header("content-type", "application/json")
                .with_body(
                    json!({
                        "id": format!("resp-{}", kind.as_str()),
                        "output": [{
                            "type": "message",
                            "content": [{"type": "output_text", "text": "authenticated"}]
                        }]
                    })
                    .to_string(),
                )
                .create_async()
                .await;

            reset_conversation_for_mode(if assistive {
                AiMode::Assistive
            } else {
                AiMode::Formatting
            });
            let output = call_llm_endpoint(
                "request-time credential witness",
                "test system prompt",
                assistive,
                snapshot.llm_lanes().lane(kind),
                None,
            )
            .await
            .unwrap_or_else(|error| panic!("{} lane request failed: {error}", kind.as_str()));
            assert_eq!(output.assistant_text, "authenticated");
            mock.assert_async().await;
        }
        reset_conversation_for_mode(AiMode::Formatting);
        reset_conversation_for_mode(AiMode::Assistive);
    }

    #[tokio::test]
    #[serial]
    async fn formatter_oauth_uses_account_backend_without_key_or_stored_chain() {
        use crate::config::keychain::test_support::install_bundle;
        use crate::llm::account_auth::{AccountTokens, OPENAI_ACCOUNT_TOKENS_ACCOUNT};
        use crate::llm::provider::ProviderKind;
        use crate::state::conversation::{
            AiMode, reset_conversation_for_mode, set_response_id_for_mode,
        };
        let root = tempfile::tempdir().unwrap();
        let mut server = mockito::Server::new_async().await;
        let mut env = TestEnv::clean();
        env.set("CODESCRIBE_DATA_DIR", root.path().to_str().unwrap());
        env.set("CODESCRIBE_DISABLE_KEYCHAIN", "1");
        env.set(
            account_auth::CODEX_BACKEND_ENDPOINT_ENV,
            &format!("{}/account/responses", server.url()),
        );
        for key in LANE_SELECTOR_ENV_KEYS {
            env.guards.push(EnvGuard::remove(key));
        }
        env.guards.push(EnvGuard::remove("LLM_OPENAI_API_KEY"));
        env.guards.push(EnvGuard::remove("OPENAI_API_KEY"));
        let tokens = AccountTokens::new(
            ProviderKind::OpenAiResponses,
            "synthetic-account-access".into(),
            None,
            None,
            None,
            Some(3600),
        );
        let serialized = serde_json::to_string(&tokens).unwrap();
        let _bundle = install_bundle(&[(OPENAI_ACCOUNT_TOKENS_ACCOUNT, &serialized)]);
        let snapshot = Config::load_runtime_snapshot().unwrap();
        let lane = snapshot.llm_lanes().formatting();
        assert!(lane.credential().account_auth());
        assert!(lane.credential().api_key().is_none());
        let mock = server.mock("POST", "/account/responses")
            .match_header("authorization", "Bearer synthetic-account-access")
            .match_header("x-api-key", Matcher::Missing)
            .match_header("originator", "codescribe")
            .match_request(|request| {
                let body: serde_json::Value = serde_json::from_slice(request.body().unwrap()).unwrap();
                body["stream"] == true && body["store"] == false
                    && body.get("previous_response_id").is_none()
                    && body.get("temperature").is_none()
                    && body.get("max_output_tokens").is_none()
                    && body["instructions"] == "format only"
            })
            .with_status(200).with_header("content-type", "text/event-stream")
            .with_body(concat!(
                "data: {\"type\":\"response.created\",\"response\":{\"id\":\"ephemeral\"}}\n\n",
                "data: {\"type\":\"response.output_text.delta\",\"delta\":\"Formatted.\"}\n\n",
                "data: {\"type\":\"response.output_text.done\",\"text\":\"Formatted.\"}\n\n",
                "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"ephemeral\",\"status\":\"completed\"}}\n\n",
                "data: [DONE]\n\n"))
            .expect(1).create_async().await;
        reset_conversation_for_mode(AiMode::Formatting);
        set_response_id_for_mode(AiMode::Formatting, "must-not-be-sent".into());
        let output = call_llm_endpoint_streaming(
            "raw words",
            "format only",
            false,
            lane,
            StreamRequestContext {
                callbacks: StreamCallbacks {
                    assistant: None,
                    reasoning: None,
                },
                initial_response_timeout: Duration::from_secs(2),
                inter_chunk_timeout: Duration::from_secs(2),
            },
            None,
        )
        .await
        .unwrap();
        assert_eq!(output.assistant_text, "Formatted.");
        assert_eq!(
            crate::state::conversation::get_previous_response_id_for_mode(AiMode::Formatting)
                .as_deref(),
            Some("must-not-be-sent")
        );
        mock.assert_async().await;
        reset_conversation_for_mode(AiMode::Formatting);
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

    #[tokio::test]
    #[serial]
    async fn stale_previous_response_id_resets_chain_and_retries_unchained() {
        let temp = tempfile::tempdir().expect("temp data dir");
        let _data_dir = EnvGuard::set("CODESCRIBE_DATA_DIR", temp.path());
        let mut server = mockito::Server::new_async().await;
        crate::state::conversation::set_response_id_for_mode(
            crate::state::conversation::AiMode::Assistive,
            "resp-expired-123".to_string(),
        );

        let stale_mock = server
            .mock("POST", "/v1/responses")
            .match_body(Matcher::PartialJson(json!({
                "previous_response_id": "resp-expired-123"
            })))
            .with_status(404)
            .with_header("content-type", "application/json")
            .with_body(
                json!({
                    "error": {
                        "message": "response 'resp-expired-123' was not found",
                        "type": "invalid_request_error",
                        "param": "previous_response_id",
                        "code": "response_not_found"
                    }
                })
                .to_string(),
            )
            .expect(1)
            .create_async()
            .await;

        let unchained_mock = server
            .mock("POST", "/v1/responses")
            .match_body(Matcher::Regex(r#""instructions":"system prompt""#.to_string()))
            .with_status(200)
            .with_header("content-type", "text/event-stream")
            .with_body(
                "data: {\"type\":\"response.created\",\"response\":{\"id\":\"resp-fresh-456\"}}\n\n\
                 data: {\"type\":\"response.output_text.delta\",\"delta\":\"Fresh answer.\"}\n\n\
                 data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp-fresh-456\"}}\n\n",
            )
            .expect(1)
            .create_async()
            .await;

        let endpoint = format!("{}/v1/responses", server.url());
        let provider = crate::llm::provider::CustomProvider::new(
            "test-buddy",
            crate::llm::provider::WireFamily::OpenAiResponses,
            &endpoint,
        )
        .unwrap();
        let user_settings = crate::config::UserSettings {
            llm_assistive_provider: Some(format!("custom:{}", provider.id)),
            llm_assistive_model: Some("buddy".into()),
            llm_custom_providers: vec![provider],
            ..Default::default()
        };
        user_settings.save().unwrap();
        let settings = Config::load_runtime_snapshot_without_keychain().unwrap();
        let output = call_llm_endpoint_streaming(
            "pytanie",
            "system prompt",
            true,
            settings.llm_lanes().assistive(),
            StreamRequestContext {
                callbacks: StreamCallbacks {
                    assistant: None,
                    reasoning: None,
                },
                initial_response_timeout: Duration::from_secs(5),
                inter_chunk_timeout: Duration::from_secs(5),
            },
            None,
        )
        .await
        .expect("stale previous_response_id must recover via unchained retry");

        assert_eq!(output.assistant_text, "Fresh answer.");
        assert_eq!(
            crate::state::conversation::get_previous_response_id_for_mode(
                crate::state::conversation::AiMode::Assistive
            )
            .as_deref(),
            Some("resp-fresh-456")
        );
        stale_mock.assert_async().await;
        unchained_mock.assert_async().await;
    }

    fn sse_answer(text: &str) -> String {
        format!(
            "data: {{\"type\":\"response.created\",\"response\":{{\"id\":\"r\"}}}}\n\n\
             data: {{\"type\":\"response.output_text.delta\",\"delta\":\"{text}\"}}\n\n\
             data: {{\"type\":\"response.completed\",\"response\":{{\"id\":\"r\",\"status\":\"completed\"}}}}\n\n"
        )
    }

    fn quick_stream() -> StreamRequestContext {
        StreamRequestContext {
            callbacks: StreamCallbacks {
                assistant: None,
                reasoning: None,
            },
            initial_response_timeout: Duration::from_secs(5),
            inter_chunk_timeout: Duration::from_secs(5),
        }
    }

    /// A slow mock body: writes `beat` every 50 ms for `span`. Dropping a
    /// mockito chunked stream joins its writer thread on the server's own
    /// runtime, so one long sleep after the client gave up froze that pooled
    /// server for the rest of the sleep, and the next test handed it saw no
    /// response at all. A beat fails as soon as the client is gone.
    fn trickle_until_the_client_leaves(
        writer: &mut dyn std::io::Write,
        beat: &[u8],
        span: Duration,
    ) -> std::io::Result<()> {
        let started = std::time::Instant::now();
        while started.elapsed() < span {
            std::thread::sleep(Duration::from_millis(50));
            writer.write_all(beat)?;
        }
        Ok(())
    }

    /// Both lanes on one custom Responses provider at `endpoint`.
    fn custom_lanes(endpoint: &str) -> RuntimeSettingsSnapshot {
        custom_lanes_reasoning(endpoint, None)
    }

    fn custom_lanes_reasoning(endpoint: &str, reasoning: Option<&str>) -> RuntimeSettingsSnapshot {
        custom_lanes_with(endpoint, "mtplx-9b", reasoning)
    }

    /// The refused rung is remembered per endpoint, model and level for the
    /// whole process, and mockito hands a later test the same port, so a test
    /// that teaches a refusal names its own model.
    fn custom_lanes_with(
        endpoint: &str,
        model: &str,
        reasoning: Option<&str>,
    ) -> RuntimeSettingsSnapshot {
        let provider = crate::llm::provider::CustomProvider::new(
            "test-mtplx",
            crate::llm::provider::WireFamily::OpenAiResponses,
            endpoint,
        )
        .unwrap();
        crate::config::UserSettings {
            llm_formatting_provider: Some(format!("custom:{}", provider.id)),
            llm_formatting_model: Some(model.into()),
            llm_formatting_reasoning: reasoning.map(str::to_string),
            formatting_level: Some("smart".into()),
            llm_assistive_provider: Some(format!("custom:{}", provider.id)),
            llm_assistive_model: Some(model.into()),
            llm_custom_providers: vec![provider],
            ..Default::default()
        }
        .save()
        .unwrap();
        Config::load_runtime_snapshot_without_keychain().unwrap()
    }

    /// A server that refuses effort `none` and accepts `low`. The formatter
    /// asks again one rung lower, and the next request starts there.
    #[tokio::test]
    #[serial]
    async fn formatting_steps_down_the_reasoning_ladder_once_per_provider() {
        let temp = tempfile::tempdir().expect("temp data dir");
        let _data_dir = EnvGuard::set("CODESCRIBE_DATA_DIR", temp.path());
        crate::state::conversation::reset_conversation_for_mode(
            crate::state::conversation::AiMode::Formatting,
        );
        let mut server = mockito::Server::new_async().await;
        let refused = server
            .mock("POST", "/v1/responses")
            .match_body(Matcher::PartialJson(
                json!({"reasoning": {"effort": "none"}}),
            ))
            .with_status(400)
            .with_body(
                json!({"error": {
                    "message": "reasoning_effort must be one of: auto, low, medium, high, xhigh",
                    "type": "invalid_request_error"
                }})
                .to_string(),
            )
            .expect(1)
            .create_async()
            .await;
        let accepted = server
            .mock("POST", "/v1/responses")
            .match_body(Matcher::PartialJson(
                json!({"reasoning": {"effort": "low"}}),
            ))
            .with_status(200)
            .with_header("content-type", "text/event-stream")
            .with_body(sse_answer("Sformatowane."))
            .expect(2)
            .create_async()
            .await;
        let settings =
            custom_lanes_with(&format!("{}/v1/responses", server.url()), "ladder-9b", None);
        for _ in 0..2 {
            crate::state::conversation::reset_conversation_for_mode(
                crate::state::conversation::AiMode::Formatting,
            );
            let output = call_provider_once(
                WireFamily::OpenAiResponses,
                "surowy tekst",
                "format only",
                false,
                settings.llm_lanes().formatting(),
                true,
                quick_stream(),
            )
            .await
            .expect("the low rung answers");
            assert_eq!(output.assistant_text, "Sformatowane.");
        }
        refused.assert_async().await;
        accepted.assert_async().await;
    }

    /// The Agent lane keeps its model's own reasoning: no field is sent.
    #[tokio::test]
    #[serial]
    async fn assistive_requests_carry_no_reasoning_field() {
        let temp = tempfile::tempdir().expect("temp data dir");
        let _data_dir = EnvGuard::set("CODESCRIBE_DATA_DIR", temp.path());
        crate::state::conversation::reset_conversation_for_mode(
            crate::state::conversation::AiMode::Assistive,
        );
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("POST", "/v1/responses")
            .match_request(|request| {
                let body: serde_json::Value =
                    serde_json::from_slice(request.body().unwrap()).unwrap();
                body.get("reasoning").is_none()
            })
            .with_status(200)
            .with_header("content-type", "text/event-stream")
            .with_body(sse_answer("Odpowiedź."))
            .expect(1)
            .create_async()
            .await;
        let settings = custom_lanes(&format!("{}/v1/responses", server.url()));
        let output = call_provider_once(
            WireFamily::OpenAiResponses,
            "pytanie",
            "system prompt",
            true,
            settings.llm_lanes().assistive(),
            true,
            quick_stream(),
        )
        .await
        .expect("assistive answers");
        assert_eq!(output.assistant_text, "Odpowiedź.");
        mock.assert_async().await;
        crate::state::conversation::reset_conversation_for_mode(
            crate::state::conversation::AiMode::Assistive,
        );
    }

    /// A level the user opted into goes out as chosen.
    #[tokio::test]
    #[serial]
    async fn formatting_sends_the_opted_in_reasoning_level() {
        let temp = tempfile::tempdir().expect("temp data dir");
        let _data_dir = EnvGuard::set("CODESCRIBE_DATA_DIR", temp.path());
        crate::state::conversation::reset_conversation_for_mode(
            crate::state::conversation::AiMode::Formatting,
        );
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("POST", "/v1/responses")
            .match_body(Matcher::PartialJson(
                json!({"reasoning": {"effort": "medium"}}),
            ))
            .with_status(200)
            .with_header("content-type", "text/event-stream")
            .with_body(sse_answer("Gotowe."))
            .expect(1)
            .create_async()
            .await;
        let settings =
            custom_lanes_reasoning(&format!("{}/v1/responses", server.url()), Some("medium"));
        let output = call_provider_once(
            WireFamily::OpenAiResponses,
            "surowy tekst",
            "format only",
            false,
            settings.llm_lanes().formatting(),
            true,
            quick_stream(),
        )
        .await
        .expect("medium answers");
        assert_eq!(output.assistant_text, "Gotowe.");
        mock.assert_async().await;
    }

    /// The Settings test meets the live deadline on a provider that answers,
    /// and says so plainly when one only thinks.
    #[tokio::test]
    #[serial]
    async fn formatting_probe_reports_an_answer_and_a_silent_thinker() {
        let temp = tempfile::tempdir().expect("temp data dir");
        let _data_dir = EnvGuard::set("CODESCRIBE_DATA_DIR", temp.path());
        let _deadline = EnvGuard::set("CODESCRIBE_AI_ATTEMPT_TIMEOUT_MS", "1500");
        crate::state::conversation::reset_conversation_for_mode(
            crate::state::conversation::AiMode::Formatting,
        );
        let mut server = mockito::Server::new_async().await;
        let answers = server
            .mock("POST", "/v1/responses")
            .match_body(Matcher::PartialJson(
                json!({"reasoning": {"effort": "none"}}),
            ))
            .with_status(200)
            .with_header("content-type", "text/event-stream")
            .with_body(sse_answer(
                "Tak, jeśli nie stosuje się zaleceń dokumentacji.",
            ))
            .expect(1)
            .create_async()
            .await;
        let settings = custom_lanes(&format!("{}/v1/responses", server.url()));
        let probe = probe_formatting(None, Some("pl"), &settings).await;
        answers.assert_async().await;
        assert_eq!(probe.status, FormattingProbeStatus::Applied, "{probe:?}");
        assert_eq!(probe.input, FORMATTING_PROBE_SAMPLE);
        assert!(probe.output.starts_with("Tak, jeśli"));
        assert!(probe.elapsed < probe.deadline);
        assert_eq!(probe.reasoning, Some("none"));
        assert_eq!(probe.model, "mtplx-9b");
        assert_eq!(probe.failure, None);

        let mut silent = mockito::Server::new_async().await;
        let _thinks = silent
            .mock("POST", "/v1/responses")
            .with_status(200)
            .with_header("content-type", "text/event-stream")
            .with_chunked_body(|writer| {
                writer.write_all(
                    b"data: {\"type\":\"response.created\",\"response\":{\"id\":\"t\"}}\n\n",
                )?;
                trickle_until_the_client_leaves(
                    writer,
                    b"data: {\"type\":\"response.in_progress\",\"response\":{\"id\":\"t\"}}\n\n",
                    Duration::from_secs(4),
                )
            })
            .create_async()
            .await;
        let settings = custom_lanes(&format!("{}/v1/responses", silent.url()));
        let probe = probe_formatting(
            Some("własny tekst do sformatowania przez model"),
            None,
            &settings,
        )
        .await;
        assert_eq!(probe.status, FormattingProbeStatus::TimedOut, "{probe:?}");
        assert_eq!(probe.input, "własny tekst do sformatowania przez model");
        assert!(probe.elapsed >= Duration::from_millis(1500));
        assert_eq!(probe.failure, Some(FormattingFailure::Timeout));
    }

    /// Diagnostics expose a fixed classification even when an upstream error
    /// includes credential or transcript material in its response body.
    #[test]
    fn formatting_failure_receipts_never_retain_provider_payloads() {
        for status in [400, 401, 403, 404, 402, 429, 500, 503] {
            for prefix in ["HTTP", "Anthropic HTTP"] {
                let error = anyhow::anyhow!(
                    "{prefix} {status} fixture bearer-secret transcript-secret?token=secret"
                )
                .context("selected formatting request failed");
                let failure = FormattingFailure::from_error(&error);
                assert_eq!(failure, FormattingFailure::Http(status));
                assert_eq!(failure.http_status(), Some(status));
                assert_eq!(failure.code(), "http");
                assert!(!format!("{failure:?}").contains("secret"));
            }
        }
        for (message, expected) in [
            (
                "account authentication failed: bearer-secret",
                FormattingFailure::AccountAuthentication,
            ),
            (
                "SSE initial response timeout: transcript-secret",
                FormattingFailure::Timeout,
            ),
            ("SSE inter-chunk timeout", FormattingFailure::Timeout),
            (
                "SSE stream inter-chunk timeout after 30s (no data received)",
                FormattingFailure::Timeout,
            ),
            ("SSE stream deadline exceeded", FormattingFailure::Timeout),
            (
                "unknown provider text bearer-secret",
                FormattingFailure::ProviderResponse,
            ),
        ] {
            let failure = FormattingFailure::from_error(&anyhow::anyhow!(message));
            assert_eq!(failure, expected);
            assert_eq!(failure.http_status(), None);
            assert!(!format!("{failure:?}").contains("secret"));
        }
    }

    /// Exercise the actual Settings request through the normal formatter,
    /// rather than constructing a diagnostic result in the test.
    #[tokio::test]
    #[serial]
    async fn formatting_probe_keeps_http_failure_then_clears_it_on_success() {
        let temp = tempfile::tempdir().expect("temp data dir");
        let _data_dir = EnvGuard::set("CODESCRIBE_DATA_DIR", temp.path());
        let _deadline = EnvGuard::set("CODESCRIBE_AI_ATTEMPT_TIMEOUT_MS", "3000");
        let _retries = EnvGuard::set("CODESCRIBE_AI_MAX_RETRIES", "0");
        let _selectors: Vec<_> = LANE_SELECTOR_ENV_KEYS
            .iter()
            .map(|key| EnvGuard::remove(key))
            .collect();
        let mut server = mockito::Server::new_async().await;
        let rejected = server
            .mock("POST", "/v1/responses")
            .with_status(403)
            .with_body(r#"{"error":{"message":"fixture bearer-secret transcript-secret"}}"#)
            .expect(1)
            .create_async()
            .await;
        let settings = custom_lanes_with(
            &format!("{}/v1/responses", server.url()),
            "receipt-fixture",
            None,
        );
        crate::state::conversation::reset_conversation_for_mode(
            crate::state::conversation::AiMode::Formatting,
        );
        let failed = probe_formatting(None, Some("pl"), &settings).await;
        assert_eq!(failed.status, FormattingProbeStatus::Failed, "{failed:?}");
        assert_eq!(failed.failure, Some(FormattingFailure::Http(403)));
        assert_eq!(failed.endpoint_host.as_deref(), Some("127.0.0.1"));
        assert_eq!(failed.credential_source, "api_key_or_key_optional");
        assert!(failed.checked_at_unix_ms > 0);
        assert!(!format!("{failed:?}").contains("bearer-secret"));
        assert!(!format!("{failed:?}").contains("transcript-secret"));
        rejected.assert_async().await;
        rejected.remove_async().await;
        let answered = server
            .mock("POST", "/v1/responses")
            .with_status(200)
            .with_header("content-type", "text/event-stream")
            .with_body(sse_answer(
                "Tak, jeśli nie stosuje się zaleceń dokumentacji.",
            ))
            .expect(1)
            .create_async()
            .await;
        crate::state::conversation::reset_conversation_for_mode(
            crate::state::conversation::AiMode::Formatting,
        );
        let success = probe_formatting(None, Some("pl"), &settings).await;
        assert_eq!(
            success.status,
            FormattingProbeStatus::Applied,
            "{success:?}"
        );
        assert_eq!(success.failure, None);
        assert!(success.checked_at_unix_ms >= failed.checked_at_unix_ms);
        answered.assert_async().await;
    }

    /// Skipping a request must never manufacture cloud acceptance or a failure.
    #[tokio::test]
    #[serial]
    async fn formatting_probe_skipped_paths_send_nothing_and_have_no_failure() {
        let temp = tempfile::tempdir().expect("temp data dir");
        let _data_dir = EnvGuard::set("CODESCRIBE_DATA_DIR", temp.path());
        let _selectors: Vec<_> = LANE_SELECTOR_ENV_KEYS
            .iter()
            .map(|key| EnvGuard::remove(key))
            .collect();
        let mut server = mockito::Server::new_async().await;
        let no_request = server
            .mock("POST", "/v1/responses")
            .expect(0)
            .create_async()
            .await;
        let settings = custom_lanes_with(
            &format!("{}/v1/responses", server.url()),
            "skip-receipt-fixture",
            None,
        );
        for policy in [FormattingPolicy::Off, FormattingPolicy::Max] {
            let snapshot = settings.with_formatting_level(policy);
            let probe = probe_formatting(None, None, &snapshot).await;
            assert_eq!(probe.status, FormattingProbeStatus::Skipped);
            assert_eq!(probe.failure, None);
            assert_eq!(probe.elapsed, Duration::ZERO);
            assert_eq!(probe.output, probe.input);
        }
        let short = probe_formatting(Some("test"), None, &settings).await;
        assert_eq!(short.status, FormattingProbeStatus::Skipped);
        assert_eq!(short.failure, None);
        assert_eq!(short.output, "test");
        no_request.assert_async().await;
    }

    fn sse_answer_with_id(id: &str, text: &str) -> String {
        format!(
            "data: {{\"type\":\"response.created\",\"response\":{{\"id\":\"{id}\"}}}}\n\n\
             data: {{\"type\":\"response.output_text.delta\",\"delta\":\"{text}\"}}\n\n\
             data: {{\"type\":\"response.completed\",\"response\":{{\"id\":\"{id}\",\"status\":\"completed\"}}}}\n\n"
        )
    }

    fn request_json(request: &mockito::Request) -> serde_json::Value {
        serde_json::from_slice(request.body().expect("request body")).expect("JSON body")
    }

    fn input_texts(body: &serde_json::Value) -> Vec<(String, String, String)> {
        body["input"]
            .as_array()
            .expect("input items")
            .iter()
            .map(|item| {
                let part = &item["content"][0];
                (
                    item["role"].as_str().unwrap_or_default().to_string(),
                    part["type"].as_str().unwrap_or_default().to_string(),
                    part["text"].as_str().unwrap_or_default().to_string(),
                )
            })
            .collect()
    }

    /// A key route asks the server to keep its reply: mtplx keeps nothing when
    /// `store` is omitted, so every chained fragment came back 404 (2026-10-23).
    /// The account backend still sends `store: false` and no predecessor.
    #[test]
    #[serial]
    fn a_key_route_asks_the_server_to_keep_the_reply_it_chains_to() {
        use crate::state::conversation::{
            AiMode, reset_conversation_for_mode, set_response_id_for_mode,
        };
        reset_conversation_for_mode(AiMode::Formatting);
        set_response_id_for_mode(AiMode::Formatting, "resp_prev".into());
        let (_, previous, key) =
            responses_chain_request("kawałek", "SYS", false, "m".into(), true, false, None);
        let key = serde_json::to_value(&key).unwrap();
        assert_eq!(previous.as_deref(), Some("resp_prev"));
        assert_eq!(key["store"], json!(true));
        assert_eq!(key["previous_response_id"], json!("resp_prev"));
        let (_, previous, account) =
            responses_chain_request("kawałek", "SYS", false, "m".into(), true, true, None);
        let account = serde_json::to_value(&account).unwrap();
        assert_eq!(previous, None);
        assert_eq!(account["store"], json!(false));
        assert!(account.get("previous_response_id").is_none());
        reset_conversation_for_mode(AiMode::Formatting);
    }

    /// The account backend keeps no responses, so a take replays its own
    /// earlier turns in `input`: user fragments and the assistant's replies.
    #[tokio::test]
    #[serial]
    async fn an_account_backend_take_replays_its_own_turns() {
        use crate::state::conversation::AiMode;
        let take = TakeFormatterChain::new();
        take.next_turn(async {
            remember_completed_turn(AiMode::Formatting, "żyje i się", "Żyje i się", None);
        })
        .await;
        let body = take
            .next_turn(async {
                let (_, previous, request) =
                    responses_chain_request("obudziła", "SYS", false, "m".into(), true, true, None);
                assert_eq!(previous, None);
                serde_json::to_value(&request).unwrap()
            })
            .await;
        assert_eq!(body["store"], json!(false));
        assert_eq!(body["instructions"], json!("SYS"));
        assert_eq!(
            input_texts(&body),
            vec![
                ("user".into(), "input_text".into(), "żyje i się".into()),
                (
                    "assistant".into(),
                    "output_text".into(),
                    "Żyje i się".into()
                ),
                ("user".into(), "input_text".into(), "obudziła".into()),
            ]
        );
    }

    /// One take over the real Responses path: the second fragment chains to the
    /// first one's reply, a refused predecessor reseeds from the take's own
    /// turns instead of going bare, and a new take starts clean even though
    /// the process-wide chain holds an id from elsewhere.
    #[tokio::test]
    #[serial]
    async fn a_take_chains_its_own_fragments_and_never_another_take() {
        use crate::state::conversation::{
            AiMode, get_previous_response_id_for_mode, reset_conversation_for_mode,
            set_response_id_for_mode,
        };
        let temp = tempfile::tempdir().expect("temp data dir");
        let _data_dir = EnvGuard::set("CODESCRIBE_DATA_DIR", temp.path());
        reset_conversation_for_mode(AiMode::Formatting);
        set_response_id_for_mode(AiMode::Formatting, "resp_earlier_take".into());
        let mut server = mockito::Server::new_async().await;
        let sse = |id: &str, text: &str| sse_answer_with_id(id, text);
        let first = server
            .mock("POST", "/v1/responses")
            .match_request(|request| {
                let body = request_json(request);
                body["store"] == json!(true)
                    && body.get("previous_response_id").is_none()
                    && input_texts(&body)
                        == vec![(
                            "user".into(),
                            "input_text".into(),
                            "pierwszy kawałek".into(),
                        )]
            })
            .with_header("content-type", "text/event-stream")
            .with_body(sse("resp_take_1", "Pierwszy kawałek"))
            .expect(1)
            .create_async()
            .await;
        let second = server
            .mock("POST", "/v1/responses")
            .match_request(|request| {
                let body = request_json(request);
                body["store"] == json!(true)
                    && body["previous_response_id"] == json!("resp_take_1")
                    && input_texts(&body).last().map(|item| item.2.as_str())
                        == Some("drugi kawałek")
            })
            .with_header("content-type", "text/event-stream")
            .with_body(sse("resp_take_2", "drugi kawałek."))
            .expect(1)
            .create_async()
            .await;
        let refused = server
            .mock("POST", "/v1/responses")
            .match_request(|request| {
                request_json(request)["previous_response_id"] == json!("resp_take_2")
            })
            .with_status(404)
            .with_body(
                json!({"error": {
                    "message": "Previous response with id 'resp_take_2' not found.",
                    "param": "previous_response_id",
                    "code": "previous_response_not_found"
                }})
                .to_string(),
            )
            .expect(1)
            .create_async()
            .await;
        let reseeded = server
            .mock("POST", "/v1/responses")
            .match_request(|request| {
                let body = request_json(request);
                body.get("previous_response_id").is_none()
                    && input_texts(&body)
                        == vec![
                            (
                                "user".into(),
                                "input_text".into(),
                                "pierwszy kawałek".into(),
                            ),
                            (
                                "assistant".into(),
                                "output_text".into(),
                                "Pierwszy kawałek".into(),
                            ),
                            ("user".into(), "input_text".into(), "drugi kawałek".into()),
                            (
                                "assistant".into(),
                                "output_text".into(),
                                "drugi kawałek.".into(),
                            ),
                            ("user".into(), "input_text".into(), "trzeci kawałek".into()),
                        ]
            })
            .with_header("content-type", "text/event-stream")
            .with_body(sse("resp_take_3", "Trzeci kawałek."))
            .expect(1)
            .create_async()
            .await;
        let fresh = server
            .mock("POST", "/v1/responses")
            .match_request(|request| {
                let body = request_json(request);
                body.get("previous_response_id").is_none()
                    && input_texts(&body)
                        == vec![("user".into(), "input_text".into(), "nowe nagranie".into())]
            })
            .with_header("content-type", "text/event-stream")
            .with_body(sse("resp_next_take", "Nowe nagranie."))
            .expect(1)
            .create_async()
            .await;
        let settings = custom_lanes_with(
            &format!("{}/v1/responses", server.url()),
            "take-chain-9b",
            None,
        );
        let lane = settings.llm_lanes().formatting();
        let call = |message: &'static str| {
            call_provider_once(
                WireFamily::OpenAiResponses,
                message,
                "format only",
                false,
                lane,
                true,
                quick_stream(),
            )
        };

        let take = TakeFormatterChain::new();
        for message in ["pierwszy kawałek", "drugi kawałek", "trzeci kawałek"] {
            take.next_turn(call(message))
                .await
                .unwrap_or_else(|error| panic!("{message}: {error}"));
        }
        let next_take = TakeFormatterChain::new();
        let output = next_take
            .next_turn(call("nowe nagranie"))
            .await
            .expect("a new take formats");
        assert_eq!(output.assistant_text, "Nowe nagranie.");

        for mock in [first, second, refused, reseeded, fresh] {
            mock.assert_async().await;
        }
        assert_eq!(
            get_previous_response_id_for_mode(AiMode::Formatting).as_deref(),
            Some("resp_earlier_take"),
            "a take's replies never move the process-wide chain"
        );
        reset_conversation_for_mode(AiMode::Formatting);
    }

    /// The next fragment of a take starts only after the previous one has
    /// finished, so it can read that reply.
    #[tokio::test]
    async fn a_take_runs_its_fragments_one_after_another() {
        let take = TakeFormatterChain::new();
        let (release, held) = tokio::sync::oneshot::channel::<()>();
        let order = Arc::new(std::sync::Mutex::new(Vec::new()));
        let first = {
            let (take, order) = (Arc::clone(&take), Arc::clone(&order));
            tokio::spawn(async move {
                take.next_turn(async {
                    order.lock().unwrap().push("first started");
                    held.await.unwrap();
                    order.lock().unwrap().push("first finished");
                })
                .await
            })
        };
        tokio::task::yield_now().await;
        let second = {
            let (take, order) = (Arc::clone(&take), Arc::clone(&order));
            tokio::spawn(async move {
                take.next_turn(async { order.lock().unwrap().push("second started") })
                    .await
            })
        };
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(*order.lock().unwrap(), vec!["first started"]);
        release.send(()).unwrap();
        first.await.unwrap();
        second.await.unwrap();
        assert_eq!(
            *order.lock().unwrap(),
            vec!["first started", "first finished", "second started"]
        );
    }
}
