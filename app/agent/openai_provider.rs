//! Responses-API agent provider (`POST /v1/responses`, streaming SSE).
//!
//! Serves every Responses-family vendor, not just OpenAI — the target is
//! whatever the assistive lane resolved, and [`ProviderKind`] is carried so the
//! account-auth path asks that vendor for tokens instead of reaching for
//! OpenAI's Keychain slot by reflex.
//!
//! Unlike [`super::AnthropicProvider`], which replays full history every turn,
//! this API can chain turns server-side via `previous_response_id`. That chain
//! is the delicate part of the file: it may only advance on a clean terminal the
//! consumer actually received, so [`forward_events_and_track_chain`] delivers
//! the event first and mutates the chain second, and a dirty terminal resets it
//! rather than resuming from a poisoned response.

use std::env;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use async_trait::async_trait;
use base64::{Engine, engine::general_purpose::STANDARD as BASE64};
use reqwest::Client;
use serde::Serialize;
use serde_json::{Value, json};
use tokio::sync::{Mutex, mpsc};
use tracing::{info, warn};

use codescribe_core::agent::{
    AgentEvent, AgentProvider, ContentBlock, ImageAsset, Message, Role, StreamOptions,
    ToolDefinition,
};
use codescribe_core::config::RuntimeLlmLane;
use codescribe_core::llm::account_auth;
use codescribe_core::llm::provider::ProviderKind;
use codescribe_core::llm::responses_streaming_manager::{
    AuthHeaderMode, ResponsesStreamingManager, StreamCallbacks,
};

/// How long to wait for the first byte of the response before giving up.
const DEFAULT_INITIAL_RESPONSE_TIMEOUT_MS: u64 = 90_000;
/// How long a started stream may stall between chunks before giving up.
const DEFAULT_INTER_CHUNK_TIMEOUT_MS: u64 = 90_000;

/// Agent provider speaking the Responses API over SSE.
#[derive(Clone)]
pub struct OpenAiProvider {
    /// Shared HTTP client; its own timeout is the outer streaming ceiling.
    client: Client,
    /// Full Responses endpoint URL for the resolved lane.
    endpoint: String,
    /// Bearer/API key; empty means an intentionally unauthenticated endpoint.
    api_key: String,
    /// Model used when the caller leaves `StreamOptions::model` blank.
    default_model: String,
    /// Whether server-side chaining is enabled at all
    /// (`CODESCRIBE_AGENT_USE_PREVIOUS_RESPONSE_ID`). When off, every turn
    /// replays the full message list.
    use_previous_response_id: bool,
    /// Single source of truth for the AGENT path's response chain
    /// (`previous_response_id`).
    ///
    /// Single source of truth for the Agent path's server-side response chain.
    /// Advanced/reset only by this provider's terminal handling.
    previous_response_id: Arc<Mutex<Option<String>>>,
    /// Deadline for the first byte of a response.
    initial_response_timeout: Duration,
    /// Deadline between chunks of an already-started stream.
    inter_chunk_timeout: Duration,
    /// Lane resolved to provider-account auth (no API key, official endpoint,
    /// stored tokens). Each request fetches a FRESH access token via
    /// `account_auth` so the auto-refresh path keeps long sessions alive.
    use_account_auth: bool,
    /// Which Responses vendor this lane targets. Carried so the account-auth
    /// path asks for THAT provider's tokens: this provider serves every
    /// Responses-family vendor, and reaching for OpenAI's Keychain slot by
    /// reflex would send an OpenAI token to `api.x.ai`.
    provider: ProviderKind,
}

impl OpenAiProvider {
    /// Build from the resolved assistive lane (fresh settings → env →
    /// Keychain) instead of the frozen bootstrap process env. `api_key: None`
    /// becomes an empty key, which the streaming manager translates into a
    /// clean unauthenticated request — key-optional local endpoints are a
    /// first-class configuration, not an error.
    pub fn from_lane(lane: &RuntimeLlmLane) -> Result<Self> {
        let endpoint = lane.endpoint().to_string();
        let default_model = lane.model().to_string();
        let api_key = lane.credential().api_key().unwrap_or_default().to_string();
        let use_account_auth = lane.credential().account_auth();
        let provider = lane.provider();

        let use_previous_response_id =
            parse_env_bool("CODESCRIBE_AGENT_USE_PREVIOUS_RESPONSE_ID", true);
        let initial_response_timeout = Duration::from_millis(parse_env_u64(
            "CODESCRIBE_AI_ATTEMPT_TIMEOUT_MS",
            DEFAULT_INITIAL_RESPONSE_TIMEOUT_MS,
        ));
        let inter_chunk_timeout = Duration::from_millis(parse_env_u64(
            "CODESCRIBE_AI_INTER_CHUNK_TIMEOUT_MS",
            DEFAULT_INTER_CHUNK_TIMEOUT_MS,
        ));

        let client = Client::builder()
            .timeout(Duration::from_secs(3600))
            .build()
            .context("Failed to create OpenAI agent HTTP client")?;

        info!(
            "OpenAI agent provider configured (model={}, account_auth={}, has_api_key={}, initial_timeout={}s, inter_chunk_timeout={}s, previous_response_id={})",
            default_model,
            use_account_auth,
            !api_key.is_empty(),
            initial_response_timeout.as_secs(),
            inter_chunk_timeout.as_secs(),
            use_previous_response_id
        );

        Ok(Self {
            client,
            endpoint,
            api_key,
            default_model,
            use_previous_response_id,
            previous_response_id: Arc::new(Mutex::new(None)),
            initial_response_timeout,
            inter_chunk_timeout,
            use_account_auth,
            provider,
        })
    }
}

#[async_trait]
impl AgentProvider for OpenAiProvider {
    /// Open a Responses API SSE stream, tracking `previous_response_id` when enabled.
    async fn stream(
        &self,
        messages: &[Message],
        tools: &[ToolDefinition],
        options: &StreamOptions,
    ) -> Result<mpsc::Receiver<AgentEvent>> {
        let model = if options.model.trim().is_empty() {
            self.default_model.clone()
        } else {
            options.model.clone()
        };

        // Operator's spec 2026-05-26 (4th iteration): retry must NOT resend prior
        // chain. Caller (session retry path) signals via `options.reset_chain`.
        self.apply_chain_reset(options).await;

        let previous_response_id = if self.use_previous_response_id {
            self.previous_response_id.lock().await.clone()
        } else {
            None
        };
        let previous_response_state = if previous_response_id
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .is_some()
        {
            "present"
        } else {
            "absent"
        };

        info!(
            "Agent provider request (model={}, messages={}, tools={}, previous_response_id={}, timeout={}s, inter_chunk_timeout={}s)",
            model,
            messages.len(),
            tools.len(),
            previous_response_state,
            self.initial_response_timeout.as_secs(),
            self.inter_chunk_timeout.as_secs()
        );

        let request = OpenAiResponsesRequest {
            reasoning: reasoning_summary_request(&model),
            model,
            input: build_request_input(
                &options.system_prompt,
                messages,
                previous_response_id.as_deref(),
            )?,
            // Param on the first turn only; chained turns re-carry the prompt
            // as a developer input item (the chain does not preserve it).
            instructions: chained_instructions(
                &options.system_prompt,
                previous_response_id.as_deref(),
            ),
            previous_response_id,
            max_output_tokens: options.max_tokens,
            temperature: options.temperature,
            tools: build_tool_payload(tools),
            stream: true,
        };

        // Account-auth lanes fetch a fresh access token per request (60s-skew
        // auto-refresh) — never a token frozen at provider construction. The
        // manager formats the `Bearer` header itself, so this is the raw token.
        let account_token = if self.use_account_auth {
            Some(
                account_auth::access_token(self.provider)
                    .await
                    .map_err(|error| {
                        anyhow::anyhow!(
                            "{} account authentication failed: {error}",
                            self.provider.display_name()
                        )
                    })?,
            )
        } else {
            None
        };
        let auth_secret = account_token.as_deref().unwrap_or(&self.api_key);

        let auth_header_mode = if self.use_account_auth {
            AuthHeaderMode::BearerOnly
        } else {
            AuthHeaderMode::BearerAndApiKey
        };
        let manager = ResponsesStreamingManager::new(
            &self.client,
            &self.endpoint,
            auth_secret,
            StreamCallbacks {
                assistant: None,
                reasoning: None,
            },
            self.initial_response_timeout,
            self.inter_chunk_timeout,
        )
        .with_auth_header_mode(auth_header_mode);

        let provider_rx = manager.stream_agent(&request).await?;

        if !self.use_previous_response_id {
            return Ok(provider_rx);
        }

        let (tx, rx) = mpsc::channel(256);
        let previous_response_id = Arc::clone(&self.previous_response_id);

        tokio::spawn(forward_events_and_track_chain(
            provider_rx,
            tx,
            previous_response_id,
        ));

        Ok(rx)
    }

    /// Wrap tool output as a user `ToolResult` message for the next model turn.
    fn build_tool_result(
        &self,
        call_id: &str,
        content: Vec<ContentBlock>,
        is_error: bool,
    ) -> Message {
        Message::new(
            Role::User,
            vec![ContentBlock::ToolResult {
                tool_use_id: call_id.to_string(),
                content,
                is_error,
            }],
        )
    }

    /// Build an inline image content block from raw bytes and media type.
    fn build_image_block(&self, data: &[u8], media_type: &str) -> ContentBlock {
        ContentBlock::Image {
            data: data.to_vec(),
            media_type: media_type.to_string(),
        }
    }

    /// Initial-response and inter-chunk timeouts for the streaming manager.
    fn stream_timeouts(&self) -> Option<(Duration, Duration)> {
        Some((self.initial_response_timeout, self.inter_chunk_timeout))
    }

    /// Stable provider id used in logs and session diagnostics.
    fn name(&self) -> &str {
        "openai-responses"
    }

    /// Current stored Responses chain id, if chain tracking is active.
    async fn response_chain_id(&self) -> Option<String> {
        self.previous_response_id.lock().await.clone()
    }

    /// Reinstate a pre-turn chain id after user Stop so follow-ups keep continuity.
    async fn restore_response_chain(&self, id: Option<String>) {
        let mut lock = self.previous_response_id.lock().await;
        if *lock != id {
            info!(
                "Agent provider chain restored after user stop (provider=openai-responses, previous_response_id={})",
                if id
                    .as_deref()
                    .map(str::trim)
                    .filter(|v| !v.is_empty())
                    .is_some()
                {
                    "present"
                } else {
                    "absent"
                }
            );
            *lock = id;
        }
    }
}

impl OpenAiProvider {
    /// Operator's spec 2026-05-26 (4th iteration): when caller requests chain
    /// reset (typically session retry path after a failed attempt), clear the
    /// stored `previous_response_id` BEFORE building the next request — fresh
    /// start, no context bloat from the prior failed attempt's chain.
    ///
    /// Extracted as a standalone helper so the behavior is unit-testable
    /// without needing a full mock SSE round-trip.
    pub async fn apply_chain_reset(&self, options: &StreamOptions) {
        if !options.reset_chain {
            return;
        }
        let mut lock = self.previous_response_id.lock().await;
        if lock.is_some() {
            info!(
                "Agent provider chain reset requested (provider=openai-responses); clearing stored previous_response_id before request"
            );
            *lock = None;
        }
    }
}

/// Outcome of inspecting a `ResponseDone` for its effect on the chain.
enum ChainEffect {
    /// Clean terminal with a usable id: advance the chain to this id.
    Advance(String),
    /// Dirty terminal (EOF/timeout, failed/incomplete): reset the chain so the
    /// next turn replays from local history instead of resuming a poisoned one.
    Reset,
    /// Not a terminal event: leave the chain untouched.
    None,
}

/// Forward provider events to the consumer while advancing the chain id.
///
/// The chain (`previous_response_id`) must only advance for turns that ended on
/// a CLEAN terminal AND that the consumer actually received. We compute the
/// chain effect from `ResponseDone { clean }`, deliver the event FIRST, and
/// mutate the chain ONLY on a successful send:
/// - clean terminal with id  -> advance the chain (P1.6 happy path);
/// - dirty terminal          -> reset the chain to `None` so the next turn does
///   a full replay (P1.6 chain-poisoning fix);
/// - non-terminal events     -> untouched.
///
/// If the consumer's `rx` was dropped (session gone), `tx.send` returns `Err`,
/// we break without mutating the chain, and a stale id cannot outlive the
/// session (P3.7).
async fn forward_events_and_track_chain(
    mut provider_rx: mpsc::Receiver<AgentEvent>,
    tx: mpsc::Sender<AgentEvent>,
    previous_response_id: Arc<Mutex<Option<String>>>,
) {
    while let Some(event) = provider_rx.recv().await {
        let chain_effect = match &event {
            AgentEvent::ResponseDone {
                response_id: Some(response_id),
                clean: true,
            } if !response_id.is_empty() => ChainEffect::Advance(response_id.clone()),
            AgentEvent::ResponseDone { clean: false, .. } => ChainEffect::Reset,
            _ => ChainEffect::None,
        };

        if tx.send(event).await.is_err() {
            break;
        }

        match chain_effect {
            ChainEffect::Advance(response_id) => {
                let mut lock = previous_response_id.lock().await;
                *lock = Some(response_id);
            }
            ChainEffect::Reset => {
                let mut lock = previous_response_id.lock().await;
                if lock.is_some() {
                    info!(
                        "Agent provider chain reset after dirty terminal (provider=openai-responses); next turn will full-replay"
                    );
                    *lock = None;
                }
            }
            ChainEffect::None => {}
        }
    }
}

/// Wire body for `POST /v1/responses`.
///
/// Optional fields are skipped when unset rather than sent as null: omitting a
/// parameter is always accepted, while sending one a model rejects is a 400.
#[derive(Debug, Serialize)]
struct OpenAiResponsesRequest {
    /// Target model id.
    model: String,
    /// Ask reasoning-capable Responses models for the safe public summary
    /// stream. Without this field the API may reason internally but emits no
    /// `response.reasoning_summary_text.*` events, leaving the macOS bubble on
    /// an opaque "thinking…" placeholder for the whole tool turn.
    #[serde(skip_serializing_if = "Option::is_none")]
    reasoning: Option<OpenAiReasoningRequest>,
    /// Conversation items for this turn — the tail only when chaining.
    input: Vec<Value>,
    /// Server-side chain anchor, absent on a fresh or reset chain.
    #[serde(skip_serializing_if = "Option::is_none")]
    previous_response_id: Option<String>,
    /// System prompt; top-level here, not a message role.
    #[serde(skip_serializing_if = "Option::is_none")]
    instructions: Option<String>,
    /// Output ceiling when the caller sets one.
    #[serde(skip_serializing_if = "Option::is_none")]
    max_output_tokens: Option<u32>,
    /// Sampling temperature when the caller sets one.
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    /// Callable tool definitions; omitted entirely when empty.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tools: Vec<OpenAiToolDefinition>,
    /// Always `true` — this provider only ever streams.
    stream: bool,
}

/// Opt-in request for the public reasoning-summary stream.
#[derive(Debug, Serialize)]
struct OpenAiReasoningRequest {
    /// Summary mode; `"auto"` is the only value sent.
    summary: &'static str,
}

/// Ask for reasoning summaries, but only from models that emit them.
///
/// Without this the API may reason internally while emitting no
/// `response.reasoning_summary_text.*` events, leaving the UI stuck on an
/// opaque "thinking…" placeholder for the whole turn.
fn reasoning_summary_request(model: &str) -> Option<OpenAiReasoningRequest> {
    let model = model.trim().to_ascii_lowercase();
    let supports_reasoning = model.starts_with("gpt-5")
        || model.starts_with("o1")
        || model.starts_with("o3")
        || model.starts_with("o4");
    supports_reasoning.then_some(OpenAiReasoningRequest { summary: "auto" })
}

/// Wire form of one callable tool.
#[derive(Debug, Serialize)]
struct OpenAiToolDefinition {
    /// Always `"function"`.
    #[serde(rename = "type")]
    tool_type: &'static str,
    /// Tool name the model calls.
    name: String,
    /// Natural-language description shown to the model.
    description: String,
    /// JSON Schema for the tool's arguments.
    parameters: Value,
}

/// Project the registry's tool definitions onto the Responses wire shape.
fn build_tool_payload(tools: &[ToolDefinition]) -> Vec<OpenAiToolDefinition> {
    tools
        .iter()
        .map(|tool| OpenAiToolDefinition {
            tool_type: "function",
            name: tool.name.clone(),
            description: tool.description.clone(),
            parameters: tool.input_schema.clone(),
        })
        .collect()
}

/// Instructions for a Responses request: the `instructions` PARAM goes only
/// on the first turn of a chain — endpoints reject the pair with
/// `previous_response_id` (HTTP 400 "instructions and previous_response_id
/// together").
///
/// But instructions are NOT preserved server-side across chained turns
/// (OpenAI Responses contract), so a chained turn MUST re-carry the system
/// prompt inside `input` — see [`build_request_input`]. A promptless chained
/// turn is how the formatting lane leaked a chat-assistant reply as product
/// output (2026-08-14, build 661); the agent shares the wire contract.
fn chained_instructions(
    system_prompt: &Option<String>,
    previous_response_id: Option<&str>,
) -> Option<String> {
    if previous_response_id.is_some() {
        None
    } else {
        system_prompt.clone()
    }
}

/// Build the full `input` array for a request. On chained turns the system
/// prompt rides as a leading `developer` message item, because the
/// `instructions` param is absent there (see [`chained_instructions`]) and
/// the chain does not carry it server-side. First turns carry the prompt via
/// `instructions` only — no duplicate developer item.
fn build_request_input(
    system_prompt: &Option<String>,
    messages: &[Message],
    previous_response_id: Option<&str>,
) -> Result<Vec<Value>> {
    let mut items = Vec::new();
    if previous_response_id.is_some()
        && let Some(prompt) = system_prompt.as_deref().filter(|p| !p.trim().is_empty())
    {
        items.push(json!({
            "type": "message",
            "role": "developer",
            "content": [{"type": "input_text", "text": prompt}]
        }));
    }
    items.extend(build_request_input_items(messages, previous_response_id)?);
    Ok(items)
}

/// Build the `input` array: select the messages to send, then encode them.
fn build_request_input_items(
    messages: &[Message],
    previous_response_id: Option<&str>,
) -> Result<Vec<Value>> {
    build_input_items(request_messages(messages, previous_response_id))
}

/// Choose which messages a request carries.
///
/// Without a chain, everything. With one, only the trailing run of user
/// messages — the server already holds the rest, so resending it would both
/// duplicate context and inflate the bill.
fn request_messages<'a>(
    messages: &'a [Message],
    previous_response_id: Option<&str>,
) -> &'a [Message] {
    if previous_response_id.is_none() {
        return messages;
    }

    let mut start = messages.len();
    while start > 0 && messages[start - 1].role == Role::User {
        start -= 1;
    }

    &messages[start..]
}

/// Encode messages into Responses input items.
///
/// Tool calls and tool results become their own top-level items rather than
/// message content, which is what the API expects. Empty content produces no
/// item at all, since a message with nothing in it is rejected.
///
/// # Errors
/// Returns an error if tool arguments fail to serialize or an image asset
/// cannot be read.
fn build_input_items(messages: &[Message]) -> Result<Vec<Value>> {
    let mut items = Vec::new();

    for message in messages {
        let mut content = Vec::new();
        for block in &message.content {
            match block {
                ContentBlock::Text(text) => {
                    if !text.is_empty() {
                        content.push(json!({
                            "type": text_content_type(message.role),
                            "text": text
                        }));
                    }
                }
                ContentBlock::Image { data, media_type } => {
                    // Images restored from the thread store carry no bytes
                    // (persisted with `data_omitted`). Emitting an empty data URL
                    // makes the provider reject the whole request
                    // ("empty base64-encoded bytes"), so skip empty images.
                    if data.is_empty() {
                        warn!(
                            "Skipping image content block with no bytes (likely restored from history)"
                        );
                        continue;
                    }
                    content.push(json!({
                        "type": "input_image",
                        "image_url": to_data_uri(data, media_type)
                    }));
                }
                ContentBlock::ImageAsset(asset) => {
                    content.push(image_asset_input_content(asset)?);
                }
                ContentBlock::ToolUse { id, name, input } => {
                    let arguments = serde_json::to_string(input).with_context(|| {
                        format!("Failed to serialize arguments for tool '{name}'")
                    })?;
                    items.push(json!({
                        "type": "function_call",
                        "call_id": id,
                        "name": name,
                        "arguments": arguments
                    }));
                }
                ContentBlock::ToolResult {
                    tool_use_id,
                    content: tool_content,
                    is_error,
                } => {
                    items.push(json!({
                        "type": "function_call_output",
                        "call_id": tool_use_id,
                        "output": format_tool_output(tool_content, *is_error)?
                    }));
                    let image_content = tool_result_image_content(tool_content)?;
                    if !image_content.is_empty() {
                        items.push(json!({
                            "type": "message",
                            "role": "user",
                            "content": image_content
                        }));
                    }
                }
            }
        }

        if !content.is_empty() {
            items.push(json!({
                "type": "message",
                "role": role_to_str(message.role),
                "content": content
            }));
        }
    }

    Ok(items)
}

/// Text block type for a role: the API distinguishes model output from input.
fn text_content_type(role: Role) -> &'static str {
    match role {
        Role::Assistant => "output_text",
        Role::User | Role::System => "input_text",
    }
}

/// Render tool result blocks into the string `function_call_output` expects.
///
/// A lone text block is returned bare (prefixed `ERROR: ` on failure) so the
/// common case stays readable to the model; anything richer is serialized as
/// JSON. Empty results still produce a sentence, because a silent tool result
/// reads to the model as the tool having done nothing.
///
/// Image bytes are described here, not embedded — they ride as separate input
/// items so the model can actually see them.
fn format_tool_output(content: &[ContentBlock], is_error: bool) -> Result<String> {
    let mut parts = Vec::new();
    for block in content {
        match block {
            ContentBlock::Text(text) => {
                if !text.trim().is_empty() {
                    parts.push(json!({
                        "type": "text",
                        "text": text.trim()
                    }));
                }
            }
            ContentBlock::Image { data, media_type } => {
                if data.is_empty() {
                    warn!(
                        "Skipping tool_result image reference with no bytes (likely restored from history)"
                    );
                    continue;
                }
                parts.push(json!({
                    "type": "image_reference",
                    "media_type": media_type,
                    "size_bytes": data.len(),
                    "data_omitted": true
                }));
            }
            ContentBlock::ImageAsset(asset) => {
                parts.push(json!({
                    "type": "image_asset",
                    "asset_id": asset.asset_id,
                    "media_type": asset.media_type,
                    "size_bytes": asset.size_bytes,
                    "path": asset.path
                }));
            }
            ContentBlock::ToolUse { .. } | ContentBlock::ToolResult { .. } => {}
        }
    }

    if parts.is_empty() {
        let fallback = if is_error {
            "Tool execution failed"
        } else {
            "Tool executed successfully"
        };
        return Ok(fallback.to_string());
    }

    if parts.len() == 1
        && let Some(text) = parts[0].get("text").and_then(Value::as_str)
    {
        return Ok(if is_error {
            format!("ERROR: {text}")
        } else {
            text.to_string()
        });
    }

    let payload = json!({
        "is_error": is_error,
        "content": parts
    });
    serde_json::to_string(&payload).context("Failed to serialize tool output payload")
}

/// Collect viewable images from a tool result as `input_image` content.
///
/// Emitted as a follow-up user message, since `function_call_output` carries
/// only text. Byte-less images restored from history are skipped loudly — an
/// empty data URL makes the provider reject the entire request.
fn tool_result_image_content(content: &[ContentBlock]) -> Result<Vec<Value>> {
    let mut image_content = Vec::new();
    for block in content {
        match block {
            ContentBlock::Image { data, media_type } => {
                if data.is_empty() {
                    warn!(
                        "Skipping tool_result image content block with no bytes (likely restored from history)"
                    );
                    continue;
                }
                image_content.push(json!({
                    "type": "input_image",
                    "image_url": to_data_uri(data, media_type)
                }));
            }
            ContentBlock::ImageAsset(asset) => {
                image_content.push(image_asset_input_content(asset)?);
            }
            ContentBlock::Text(_)
            | ContentBlock::ToolUse { .. }
            | ContentBlock::ToolResult { .. } => {}
        }
    }
    Ok(image_content)
}

/// Load a stored image asset and inline it as an `input_image` data URI.
///
/// # Errors
/// Returns an error if the asset cannot be read from the store.
fn image_asset_input_content(asset: &ImageAsset) -> Result<Value> {
    // Tainted-path guard: asset paths ride through conversation state, so the
    // read goes through the store, which honors only the file name re-rooted
    // under the canonical assets dir.
    let data = codescribe_core::agent::AgentAssetStore::read_image(&asset.path)?;
    Ok(json!({
        "type": "input_image",
        "image_url": to_data_uri(&data, &asset.media_type)
    }))
}

/// Wire spelling of a message role.
fn role_to_str(role: Role) -> &'static str {
    match role {
        Role::User => "user",
        Role::Assistant => "assistant",
        Role::System => "system",
    }
}

/// Base64 `data:` URI for image bytes, defaulting a blank media type to PNG.
fn to_data_uri(data: &[u8], media_type: &str) -> String {
    let media_type = {
        let normalized = media_type.trim();
        if normalized.is_empty() {
            "image/png"
        } else {
            normalized
        }
    };
    format!("data:{media_type};base64,{}", BASE64.encode(data))
}

/// Read a `u64` env override, falling back on absent or unparseable values.
fn parse_env_u64(key: &str, default: u64) -> u64 {
    env::var(key)
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(default)
}

/// Read a boolean env override, accepting `1/true/yes/on` and their negatives.
///
/// An unrecognized value keeps the default rather than reading as `false`, so a
/// typo cannot silently disable a feature.
fn parse_env_bool(key: &str, default: bool) -> bool {
    match env::var(key) {
        Ok(value) => match value.trim().to_ascii_lowercase().as_str() {
            "1" | "true" | "yes" | "on" => true,
            "0" | "false" | "no" | "off" => false,
            _ => default,
        },
        Err(_) => default,
    }
}

/// Responses request shape, image payload, and chain-reset unit tests.
#[cfg(test)]
mod tests {
    use super::{
        OpenAiProvider, ProviderKind, build_request_input, build_request_input_items,
        chained_instructions, format_tool_output, forward_events_and_track_chain,
        reasoning_summary_request, request_messages, to_data_uri,
    };
    use std::sync::Arc;
    use std::time::Duration;

    use codescribe_core::agent::{
        AgentAssetStore, AgentEvent, AgentProvider, ContentBlock, Message, Role, StreamOptions,
    };
    use reqwest::Client;
    use serde_json::json;
    use tokio::sync::{Mutex, mpsc};

    /// Reasoning summary requests apply only to reasoning-capable model families.
    #[test]
    fn requests_public_reasoning_summaries_only_for_reasoning_models() {
        let gpt5 = serde_json::to_value(reasoning_summary_request("gpt-5.6").unwrap())
            .expect("serialize reasoning request");
        assert_eq!(gpt5, json!({ "summary": "auto" }));
        assert!(reasoning_summary_request("o3-mini").is_some());
        assert!(reasoning_summary_request("gpt-4o-mini").is_none());
        assert!(reasoning_summary_request("llama3.3").is_none());
    }

    /// Without a chain id the request replays the full conversation history.
    #[test]
    fn request_messages_replays_full_history_without_previous_response_id() {
        let messages = vec![
            Message::new(Role::User, vec![ContentBlock::Text("first".to_string())]),
            Message::new(
                Role::Assistant,
                vec![ContentBlock::Text("second".to_string())],
            ),
        ];

        let selected = request_messages(&messages, None);
        assert_eq!(selected, messages.as_slice());
    }

    /// With a chain id only trailing user messages are re-sent as input.
    #[test]
    fn request_messages_uses_only_trailing_user_messages_with_previous_response_id() {
        let messages = vec![
            Message::new(
                Role::User,
                vec![ContentBlock::Text("earlier turn".to_string())],
            ),
            Message::new(
                Role::Assistant,
                vec![ContentBlock::ToolUse {
                    id: "call_1".to_string(),
                    name: "read_file".to_string(),
                    input: json!({"path": "/tmp/ignored.txt"}),
                }],
            ),
            Message::new(
                Role::User,
                vec![ContentBlock::ToolResult {
                    tool_use_id: "call_1".to_string(),
                    content: vec![ContentBlock::Text("tool output".to_string())],
                    is_error: false,
                }],
            ),
            Message::new(
                Role::User,
                vec![ContentBlock::Text("follow-up".to_string())],
            ),
        ];

        let selected = request_messages(&messages, Some("resp_prev"));
        assert_eq!(selected.len(), 2);
        assert!(selected.iter().all(|message| message.role == Role::User));
    }

    /// Chained turns must NOT resend the `instructions` PARAM (endpoints
    /// reject the pair with HTTP 400, which froze the Agent UI on every
    /// second turn — repro 2026-08-10). The prompt itself still travels: as a
    /// developer input item, because the chain does NOT preserve instructions
    /// server-side (see `chained_turn_recarries_prompt_as_developer_item`).
    #[test]
    fn chained_turn_omits_instructions() {
        let system = Some("system prompt".to_string());
        assert_eq!(
            chained_instructions(&system, None),
            Some("system prompt".to_string()),
            "first turn of a chain must carry instructions"
        );
        assert_eq!(
            chained_instructions(&system, Some("resp_prev")),
            None,
            "chained turn must not resend instructions"
        );
        assert_eq!(chained_instructions(&None, None), None);
    }

    /// The 2026-08-14 promptless-chain leak, agent side: a chained turn must
    /// re-carry the system prompt as a leading developer input item (the
    /// chain does not preserve `instructions` server-side), while the first
    /// turn carries it via the param only — no duplicate developer item.
    #[test]
    fn chained_turn_recarries_prompt_as_developer_item() {
        let system = Some("system prompt".to_string());
        let messages = vec![Message::new(
            Role::User,
            vec![ContentBlock::Text("hello".to_string())],
        )];

        let chained = build_request_input(&system, &messages, Some("resp_prev"))
            .expect("chained input should build");
        assert_eq!(chained[0]["role"], "developer");
        assert_eq!(chained[0]["content"][0]["text"], "system prompt");
        assert_eq!(chained[1]["role"], "user");

        let first = build_request_input(&system, &messages, None).expect("first input builds");
        assert!(
            first.iter().all(|item| item["role"] != "developer"),
            "first turn must not duplicate the prompt as a developer item"
        );

        let promptless = build_request_input(&None, &messages, Some("resp_prev"))
            .expect("promptless chained input builds");
        assert!(
            promptless.iter().all(|item| item["role"] != "developer"),
            "no prompt configured ⇒ no developer item"
        );
    }

    /// Resuming a chain omits prior turns already stored server-side.
    #[test]
    fn build_request_input_items_skips_prior_history_when_resuming_chain() {
        let messages = vec![
            Message::new(
                Role::User,
                vec![ContentBlock::Text("earlier turn".to_string())],
            ),
            Message::new(
                Role::Assistant,
                vec![ContentBlock::ToolUse {
                    id: "call_1".to_string(),
                    name: "read_file".to_string(),
                    input: json!({"path": "/tmp/ignored.txt"}),
                }],
            ),
            Message::new(
                Role::User,
                vec![ContentBlock::ToolResult {
                    tool_use_id: "call_1".to_string(),
                    content: vec![ContentBlock::Text("tool output".to_string())],
                    is_error: false,
                }],
            ),
        ];

        let items = build_request_input_items(&messages, Some("resp_prev"))
            .expect("request input items should build");

        assert_eq!(items.len(), 1);
        assert_eq!(items[0]["type"], "function_call_output");
        assert_eq!(items[0]["call_id"], "call_1");
    }

    /// Large tool output is sent as a stored-path reference, not inline bytes.
    #[test]
    fn stored_tool_output_reference_is_the_only_body_sent_to_openai() {
        let reference = "[tool output stored: /tmp/tool-output-deadbeef.txt (90000 bytes)]";
        let messages = vec![Message::new(
            Role::User,
            vec![ContentBlock::ToolResult {
                tool_use_id: "call_large".to_string(),
                content: vec![ContentBlock::Text(reference.to_string())],
                is_error: false,
            }],
        )];

        let items = build_request_input_items(&messages, None)
            .expect("stored tool reference should serialize");
        let payload = serde_json::to_string(&items).expect("OpenAI payload JSON");

        assert!(payload.contains(reference));
        assert!(!payload.contains("monster inline body"));
    }

    /// Assistant history serializes as `output_text`, user as `input_text`.
    #[test]
    fn build_request_input_items_uses_output_text_for_assistant_history() {
        let messages = vec![
            Message::new(Role::User, vec![ContentBlock::Text("question".to_string())]),
            Message::new(
                Role::Assistant,
                vec![ContentBlock::Text("answer".to_string())],
            ),
            Message::new(
                Role::User,
                vec![ContentBlock::Text("follow-up".to_string())],
            ),
        ];

        let items =
            build_request_input_items(&messages, None).expect("request input items should build");

        assert_eq!(items[0]["content"][0]["type"], "input_text");
        assert_eq!(items[1]["role"], "assistant");
        assert_eq!(items[1]["content"][0]["type"], "output_text");
        assert_eq!(items[2]["content"][0]["type"], "input_text");
    }

    /// Tool-result images become references; raw base64 must not hit the wire.
    #[test]
    fn format_tool_output_omits_raw_image_base64() {
        let output = format_tool_output(
            &[ContentBlock::Image {
                data: b"not really a png".to_vec(),
                media_type: "image/png".to_string(),
            }],
            false,
        )
        .expect("tool output should serialize");

        assert!(output.contains("image_reference"));
        assert!(output.contains("data_omitted"));
        assert!(!output.contains("bm90IHJlYWxseSBhIHBuZw"));
    }

    /// Restored thread images still serialize as native input_image data URIs.
    #[test]
    fn restored_thread_inline_image_reaches_prompt_on_next_turn() {
        let _env_serial = crate::test_env::data_dir_env_serial();
        // Turn 2 on a restored thread: an inline composer image persisted via
        // the thread store must come back as a disk-backed asset and still
        // reach the request payload instead of being skipped as byteless.
        let image_bytes = b"w5a-openai-turn2".to_vec();
        let original = Message::new(
            Role::User,
            vec![ContentBlock::Image {
                data: image_bytes.clone(),
                media_type: "image/png".to_string(),
            }],
        );
        let restored = codescribe_core::agent::ThreadMessage::from(&original).to_message();

        let items = build_request_input_items(std::slice::from_ref(&restored), None)
            .expect("restored image should serialize");
        assert_eq!(items.len(), 1, "restored image must not be skipped");
        assert_eq!(items[0]["type"], "message");
        assert_eq!(items[0]["content"][0]["type"], "input_image");
        let image_url = items[0]["content"][0]["image_url"]
            .as_str()
            .expect("image_url should be a string");
        assert_eq!(image_url, to_data_uri(&image_bytes, "image/png"));

        if let ContentBlock::ImageAsset(asset) = &restored.content[0] {
            std::fs::remove_file(&asset.path).ok();
        }
    }

    /// Disk-backed tool image assets add a native input_image item beside output.
    #[test]
    fn tool_result_image_asset_adds_native_input_image_item() {
        let _env_serial = crate::test_env::data_dir_env_serial();
        let asset = AgentAssetStore::save_image(b"png bytes", "image/png")
            .expect("image asset should save");
        let asset_id = asset.asset_id.clone();
        let asset_path = asset.path.clone();
        let messages = vec![Message::new(
            Role::User,
            vec![ContentBlock::ToolResult {
                tool_use_id: "call_screenshot".to_string(),
                content: vec![ContentBlock::ImageAsset(asset)],
                is_error: false,
            }],
        )];

        let items = build_request_input_items(&messages, None)
            .expect("request input items should include image asset");

        assert_eq!(items.len(), 2);
        assert_eq!(items[0]["type"], "function_call_output");
        assert!(
            items[0]["output"]
                .as_str()
                .expect("tool output should be a string")
                .contains(&asset_id)
        );
        assert_eq!(items[1]["type"], "message");
        assert_eq!(items[1]["content"][0]["type"], "input_image");
        assert!(
            items[1]["content"][0]["image_url"]
                .as_str()
                .expect("image_url should be a string")
                .starts_with("data:image/png;base64,")
        );
        std::fs::remove_file(asset_path).ok();
    }

    /// Empty restored tool images are dropped, never empty data URIs.
    #[test]
    fn tool_result_data_omitted_image_is_skipped_not_sent_as_empty_data_uri() {
        // D8 parity: a tool-result image restored from history (`data_omitted`)
        // has no bytes. It must be dropped from the native image message — never
        // serialized as an empty data URI — while the function output remains
        // valid via the text fallback.
        let messages = vec![Message::new(
            Role::User,
            vec![ContentBlock::ToolResult {
                tool_use_id: "call_restored".to_string(),
                content: vec![ContentBlock::Image {
                    data: vec![],
                    media_type: "image/png".to_string(),
                }],
                is_error: false,
            }],
        )];

        let items =
            build_request_input_items(&messages, None).expect("request input items should build");

        assert_eq!(items.len(), 1, "empty tool-result image is not sent");
        assert_eq!(items[0]["type"], "function_call_output");
        assert_eq!(items[0]["call_id"], "call_restored");
        assert_eq!(items[0]["output"], "Tool executed successfully");
    }

    /// Composer inline images serialize as input_image next to caption text.
    #[test]
    fn user_message_inline_image_serializes_as_input_image() {
        // Composer 📎 path parity with Anthropic: `AgentSession::send` builds a
        // [Text, Image{bytes}] user turn via `build_image_block`. The request
        // must carry the image as a native input_image data URI alongside the
        // caption — a regression here silently drops user attachments.
        let messages = vec![Message::new(
            Role::User,
            vec![
                ContentBlock::Text("what is in this image?".to_string()),
                ContentBlock::Image {
                    data: b"png bytes".to_vec(),
                    media_type: "image/png".to_string(),
                },
            ],
        )];

        let items =
            build_request_input_items(&messages, None).expect("request input items should build");

        assert_eq!(items.len(), 1);
        assert_eq!(items[0]["role"], "user");
        let content = items[0]["content"].as_array().expect("content array");
        assert_eq!(content.len(), 2, "caption + image both survive");
        assert_eq!(content[0]["type"], "input_text");
        assert_eq!(content[1]["type"], "input_image");
        assert!(
            content[1]["image_url"]
                .as_str()
                .expect("image_url string")
                .starts_with("data:image/png;base64,")
        );
    }

    /// SSE `error` events surface as specific AgentEvent::Error, not session noise.
    #[tokio::test]
    async fn stream_surfaces_sse_error_event_as_specific_agent_error() {
        let mut server = mockito::Server::new_async().await;
        let body = [
            "event: error",
            r#"data: {"error":{"message":"'list' object has no attribute 'uid'","code":"internal_error"}}"#,
            "",
            "data: [DONE]",
            "",
        ]
            .join("\n");
        let mock = server
            .mock("POST", "/v1/responses")
            .with_status(200)
            .with_header("content-type", "text/event-stream")
            .with_body(body)
            .create_async()
            .await;
        let provider = OpenAiProvider {
            client: Client::new(),
            endpoint: format!("{}/v1/responses", server.url()),
            api_key: "test-key".to_string(),
            default_model: "gpt-5.5".to_string(),
            use_previous_response_id: false,
            previous_response_id: Arc::new(Mutex::new(None)),
            initial_response_timeout: Duration::from_secs(1),
            inter_chunk_timeout: Duration::from_secs(1),
            use_account_auth: false,
            provider: ProviderKind::OpenAiResponses,
        };
        let messages = vec![Message::new(
            Role::User,
            vec![ContentBlock::Text("hello".to_string())],
        )];

        let mut rx = provider
            .stream(&messages, &[], &StreamOptions::default())
            .await
            .expect("agent provider stream should start");
        let event = tokio::time::timeout(Duration::from_secs(2), rx.recv())
            .await
            .expect("agent provider should emit an error event before timeout")
            .expect("agent provider should emit one event");

        match event {
            AgentEvent::Error(message) => {
                assert!(message.contains("Agent SSE error internal_error"));
                assert!(message.contains("'list' object has no attribute 'uid'"));
                assert!(!message.contains("AgentSession send failed"));
            }
            other => panic!("expected AgentEvent::Error, got {other:?}"),
        }
        mock.assert_async().await;
    }

    /// Operator's spec 2026-05-26 (4th iteration): retry attempts must NOT
    /// resend prior chain via stored previous_response_id. `apply_chain_reset`
    /// is the focused helper — when `options.reset_chain == true`, it clears
    /// any stored chain BEFORE the request is built.
    #[tokio::test]
    async fn apply_chain_reset_clears_stored_previous_response_id_when_requested() {
        let stored_chain = Arc::new(Mutex::new(Some("resp_prev_failed".to_string())));
        let provider = OpenAiProvider {
            client: Client::new(),
            endpoint: "http://unused.invalid/v1/responses".to_string(),
            api_key: "test-key".to_string(),
            default_model: "gpt-5.5".to_string(),
            use_previous_response_id: true,
            previous_response_id: Arc::clone(&stored_chain),
            initial_response_timeout: Duration::from_secs(1),
            inter_chunk_timeout: Duration::from_secs(1),
            use_account_auth: false,
            provider: ProviderKind::OpenAiResponses,
        };

        // Pre-condition: stored chain holds prior failed attempt's response id.
        assert_eq!(
            stored_chain.lock().await.as_deref(),
            Some("resp_prev_failed")
        );

        let options = StreamOptions {
            reset_chain: true,
            ..StreamOptions::default()
        };
        provider.apply_chain_reset(&options).await;

        // Post-condition: stored chain is cleared.
        assert!(
            stored_chain.lock().await.is_none(),
            "reset_chain=true must clear stored previous_response_id"
        );
    }

    /// Operator 2026-08-05: user Stop reinstates the pre-turn chain id instead
    /// of wiping previous_response_id so a queued follow-up keeps continuity.
    #[tokio::test]
    async fn restore_response_chain_reinstates_pre_turn_id_after_user_stop() {
        let stored_chain = Arc::new(Mutex::new(Some("resp_pre_turn".to_string())));
        let provider = OpenAiProvider {
            client: Client::new(),
            endpoint: "http://unused.invalid/v1/responses".to_string(),
            api_key: "test-key".to_string(),
            default_model: "gpt-5.5".to_string(),
            use_previous_response_id: true,
            previous_response_id: Arc::clone(&stored_chain),
            initial_response_timeout: Duration::from_secs(1),
            inter_chunk_timeout: Duration::from_secs(1),
            use_account_auth: false,
            provider: ProviderKind::OpenAiResponses,
        };

        // Mid-turn advance (tool round) or dirty cancel would move the live id.
        *stored_chain.lock().await = Some("resp_mid_turn_cancelled".to_string());
        assert_eq!(
            provider.response_chain_id().await.as_deref(),
            Some("resp_mid_turn_cancelled")
        );

        provider
            .restore_response_chain(Some("resp_pre_turn".to_string()))
            .await;

        assert_eq!(
            stored_chain.lock().await.as_deref(),
            Some("resp_pre_turn"),
            "user Stop must reinstate the pre-turn previous_response_id"
        );
    }

    /// Default stream options must keep the stored previous_response_id chain.
    #[tokio::test]
    async fn apply_chain_reset_preserves_stored_chain_when_not_requested() {
        let stored_chain = Arc::new(Mutex::new(Some("resp_keep_me".to_string())));
        let provider = OpenAiProvider {
            client: Client::new(),
            endpoint: "http://unused.invalid/v1/responses".to_string(),
            api_key: "test-key".to_string(),
            default_model: "gpt-5.5".to_string(),
            use_previous_response_id: true,
            previous_response_id: Arc::clone(&stored_chain),
            initial_response_timeout: Duration::from_secs(1),
            inter_chunk_timeout: Duration::from_secs(1),
            use_account_auth: false,
            provider: ProviderKind::OpenAiResponses,
        };

        let options = StreamOptions::default();
        assert!(!options.reset_chain, "default must NOT reset chain");

        provider.apply_chain_reset(&options).await;

        assert_eq!(
            stored_chain.lock().await.as_deref(),
            Some("resp_keep_me"),
            "default options must preserve conversational chain"
        );
    }

    /// P3.8: exercise the implicit chain invariant end-to-end across the
    /// sequence `send -> ResponseDone(id) -> next send (trailing-user only) ->
    /// error -> retry(reset_chain) -> success -> next (full replay)`.
    ///
    /// The non-fakeable proof is the number of input items handed to the
    /// provider per phase: a present chain id sends only the trailing user
    /// turn, while a None id (after reset) replays the full history. If a future
    /// change makes `request_messages` truncate history at id=None, the
    /// full-replay assertions fail.
    #[tokio::test]
    async fn chain_reset_then_full_replay() {
        // Conversation history: user turn, assistant reply, follow-up user turn.
        let history = vec![
            Message::new(
                Role::User,
                vec![ContentBlock::Text("first question".to_string())],
            ),
            Message::new(
                Role::Assistant,
                vec![ContentBlock::Text("first answer".to_string())],
            ),
            Message::new(
                Role::User,
                vec![ContentBlock::Text("follow-up question".to_string())],
            ),
        ];

        // Phase 1 — first send, no chain yet (id=None): full replay of history.
        let phase1 =
            build_request_input_items(&history, None).expect("phase 1 input items should build");
        assert_eq!(
            phase1.len(),
            3,
            "id=None must replay the full history (3 items)"
        );

        // Phase 2 — provider returned ResponseDone(id); next send carries the
        // chain id, so only the trailing user turn is sent.
        let chain_id = "resp_phase1";
        let phase2 = build_request_input_items(&history, Some(chain_id))
            .expect("phase 2 input items should build");
        assert_eq!(
            phase2.len(),
            1,
            "a present chain id must send only the trailing user turn"
        );
        assert_eq!(
            phase2[0]["role"], "user",
            "trailing item must be the user turn"
        );

        // Phase 3 — that turn errored; the session retry path requests a chain
        // reset. apply_chain_reset must zero the stored chain so the rebuild
        // sees id=None.
        let stored_chain = Arc::new(Mutex::new(Some(chain_id.to_string())));
        let provider = OpenAiProvider {
            client: Client::new(),
            endpoint: "http://unused.invalid/v1/responses".to_string(),
            api_key: "test-key".to_string(),
            default_model: "gpt-5.5".to_string(),
            use_previous_response_id: true,
            previous_response_id: Arc::clone(&stored_chain),
            initial_response_timeout: Duration::from_secs(1),
            inter_chunk_timeout: Duration::from_secs(1),
            use_account_auth: false,
            provider: ProviderKind::OpenAiResponses,
        };
        let reset_options = StreamOptions {
            reset_chain: true,
            ..StreamOptions::default()
        };
        provider.apply_chain_reset(&reset_options).await;
        let chain_after_reset = stored_chain.lock().await.clone();
        assert!(
            chain_after_reset.is_none(),
            "apply_chain_reset must clear the stored chain before the retry"
        );

        // Phase 4 — retry success with id=None: full replay again, proving the
        // invariant "id None => full replay" holds after a reset.
        let phase4 = build_request_input_items(&history, chain_after_reset.as_deref())
            .expect("phase 4 input items should build");
        assert_eq!(
            phase4.len(),
            3,
            "after reset (id=None) the retry must replay the full history"
        );
    }

    /// P3.7: the detached forwarder must not advance `previous_response_id` once
    /// the consumer has dropped its receiver. Otherwise a chain id from a turn
    /// nobody received outlives the session and poisons the next request.
    #[tokio::test]
    async fn forwarder_does_not_update_chain_after_drop() {
        let stored_chain: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));

        let (provider_tx, provider_rx) = mpsc::channel::<AgentEvent>(8);
        let (consumer_tx, consumer_rx) = mpsc::channel::<AgentEvent>(8);

        // Consumer is gone before any event flows through the forwarder.
        drop(consumer_rx);

        let forwarder = tokio::spawn(forward_events_and_track_chain(
            provider_rx,
            consumer_tx,
            Arc::clone(&stored_chain),
        ));

        // Emit a clean ResponseDone with a real id — under a live consumer this
        // would advance the chain.
        provider_tx
            .send(AgentEvent::ResponseDone {
                response_id: Some("resp_after_drop".to_string()),
                clean: true,
            })
            .await
            .expect("provider channel should accept the event");
        drop(provider_tx);

        forwarder.await.expect("forwarder task should finish");

        assert!(
            stored_chain.lock().await.is_none(),
            "chain must stay None when the consumer dropped before delivery"
        );
    }

    /// Counterpart to the drop case: with a live consumer, a clean ResponseDone
    /// advances the chain exactly once.
    #[tokio::test]
    async fn forwarder_updates_chain_when_delivered() {
        let stored_chain: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));

        let (provider_tx, provider_rx) = mpsc::channel::<AgentEvent>(8);
        let (consumer_tx, mut consumer_rx) = mpsc::channel::<AgentEvent>(8);

        let forwarder = tokio::spawn(forward_events_and_track_chain(
            provider_rx,
            consumer_tx,
            Arc::clone(&stored_chain),
        ));

        provider_tx
            .send(AgentEvent::ResponseDone {
                response_id: Some("resp_delivered".to_string()),
                clean: true,
            })
            .await
            .expect("provider channel should accept the event");

        // Drain delivery so the forwarder commits the chain.
        let received = consumer_rx.recv().await.expect("event should be delivered");
        assert!(matches!(received, AgentEvent::ResponseDone { .. }));

        drop(provider_tx);
        forwarder.await.expect("forwarder task should finish");

        assert_eq!(
            stored_chain.lock().await.as_deref(),
            Some("resp_delivered"),
            "delivered clean ResponseDone must advance the chain"
        );
    }

    /// P1.6: a DIRTY terminal (`clean=false`, e.g. EOF/timeout or a
    /// failed/incomplete response) must RESET the chain so the next turn does a
    /// full replay instead of resuming a poisoned `previous_response_id`.
    #[tokio::test]
    async fn dirty_terminal_resets_chain() {
        // Pre-existing chain from a prior clean turn.
        let stored_chain: Arc<Mutex<Option<String>>> =
            Arc::new(Mutex::new(Some("resp_prev_clean".to_string())));

        let (provider_tx, provider_rx) = mpsc::channel::<AgentEvent>(8);
        let (consumer_tx, mut consumer_rx) = mpsc::channel::<AgentEvent>(8);

        let forwarder = tokio::spawn(forward_events_and_track_chain(
            provider_rx,
            consumer_tx,
            Arc::clone(&stored_chain),
        ));

        // Synthetic dirty terminal: an id may still be present, but clean=false.
        provider_tx
            .send(AgentEvent::ResponseDone {
                response_id: Some("resp_dirty".to_string()),
                clean: false,
            })
            .await
            .expect("provider channel should accept the event");

        let received = consumer_rx.recv().await.expect("event should be delivered");
        assert!(matches!(
            received,
            AgentEvent::ResponseDone { clean: false, .. }
        ));

        drop(provider_tx);
        forwarder.await.expect("forwarder task should finish");

        assert!(
            stored_chain.lock().await.is_none(),
            "dirty terminal must reset the chain to None for full replay"
        );
    }

    /// P2.13 end-to-end (provider): a `response.failed` terminal arriving over
    /// the real `stream()` path must reset the provider's stored
    /// `previous_response_id`. The parser emits a dirty `ResponseDone` ahead of
    /// the error, the forwarder consumes it, and the chain returns to None so the
    /// next turn full-replays instead of resuming a poisoned chain.
    #[tokio::test]
    async fn failed_terminal_resets_provider_chain_end_to_end() {
        let mut server = mockito::Server::new_async().await;
        let body = [
            r#"data: {"type":"response.created","sequence_number":0,"response":{"id":"resp_e2e_fail"}}"#,
            "",
            r#"data: {"type":"response.failed","sequence_number":1,"response":{"id":"resp_e2e_fail","status":"failed","error":{"code":"server_error","message":"boom"}}}"#,
            "",
            "data: [DONE]",
            "",
        ]
            .join("\n");
        let mock = server
            .mock("POST", "/v1/responses")
            .with_status(200)
            .with_header("content-type", "text/event-stream")
            .with_body(body)
            .create_async()
            .await;

        // Pre-existing chain from a prior clean turn — this is the poisoned id.
        let stored_chain = Arc::new(Mutex::new(Some("resp_prev_clean".to_string())));
        let provider = OpenAiProvider {
            client: Client::new(),
            endpoint: format!("{}/v1/responses", server.url()),
            api_key: "test-key".to_string(),
            default_model: "gpt-5.5".to_string(),
            use_previous_response_id: true,
            previous_response_id: Arc::clone(&stored_chain),
            initial_response_timeout: Duration::from_secs(2),
            inter_chunk_timeout: Duration::from_secs(2),
            use_account_auth: false,
            provider: ProviderKind::OpenAiResponses,
        };
        let messages = vec![Message::new(
            Role::User,
            vec![ContentBlock::Text("hello".to_string())],
        )];

        let mut rx = provider
            .stream(&messages, &[], &StreamOptions::default())
            .await
            .expect("agent provider stream should start");

        // Drain the dirty ResponseDone (resets the chain) and the Error.
        let first = tokio::time::timeout(Duration::from_secs(2), rx.recv())
            .await
            .expect("first event should arrive")
            .expect("first event present");
        assert!(
            matches!(first, AgentEvent::ResponseDone { clean: false, .. }),
            "expected dirty ResponseDone first, got {first:?}"
        );
        let second = tokio::time::timeout(Duration::from_secs(2), rx.recv())
            .await
            .expect("second event should arrive")
            .expect("second event present");
        assert!(
            matches!(second, AgentEvent::Error(_)),
            "expected Error after dirty terminal, got {second:?}"
        );
        // Drain any trailing events until the channel closes so the forwarder
        // has committed the reset.
        while tokio::time::timeout(Duration::from_secs(2), rx.recv())
            .await
            .expect("recv should not time out while draining")
            .is_some()
        {}

        assert!(
            stored_chain.lock().await.is_none(),
            "failed terminal must reset the provider chain to None for full replay"
        );
        mock.assert_async().await;
    }

    /// P1.6 counterpart: a clean terminal must NOT be downgraded — the chain
    /// advances even when a prior chain id was present.
    #[tokio::test]
    async fn clean_terminal_keeps_chain() {
        let stored_chain: Arc<Mutex<Option<String>>> =
            Arc::new(Mutex::new(Some("resp_prev".to_string())));

        let (provider_tx, provider_rx) = mpsc::channel::<AgentEvent>(8);
        let (consumer_tx, mut consumer_rx) = mpsc::channel::<AgentEvent>(8);

        let forwarder = tokio::spawn(forward_events_and_track_chain(
            provider_rx,
            consumer_tx,
            Arc::clone(&stored_chain),
        ));

        provider_tx
            .send(AgentEvent::ResponseDone {
                response_id: Some("resp_next_clean".to_string()),
                clean: true,
            })
            .await
            .expect("provider channel should accept the event");

        let _ = consumer_rx.recv().await.expect("event should be delivered");
        drop(provider_tx);
        forwarder.await.expect("forwarder task should finish");

        assert_eq!(
            stored_chain.lock().await.as_deref(),
            Some("resp_next_clean"),
            "clean terminal must advance the chain"
        );
    }
}
