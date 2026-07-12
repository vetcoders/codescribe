//! Anthropic Messages API agent provider.
//!
//! Second concrete [`AgentProvider`] alongside [`super::OpenAiProvider`]. It
//! targets the Anthropic Messages API (`POST /v1/messages`, streaming SSE) and
//! reuses the canonical provider-identity layer in
//! [`codescribe_core::llm::provider`] to gate request parameters per model —
//! `claude-opus-4-8` rejects sampling params with HTTP 400, `claude-sonnet-*`
//! tolerates `temperature` — so the request builder never sends a parameter the
//! target will reject.
//!
//! Wire spec (Anthropic Messages API, `anthropic-version: 2023-06-01`):
//! - auth via the `x-api-key` header (NOT a bearer token);
//! - `system` is a top-level field, not a message role;
//! - `tool_use` blocks live in assistant turns, `tool_result` blocks in user
//!   turns — which is exactly how [`super::super`]'s `AgentSession` already
//!   structures history, so the mapping is close to 1:1;
//! - images ride as `{"type":"image","source":{"type":"base64", ...}}`;
//! - streaming emits `message_start` / `content_block_start` /
//!   `content_block_delta` / `content_block_stop` / `message_delta` /
//!   `message_stop` SSE events (plus `ping` and `error`).
//!
//! Anthropic replays full history every turn (no `previous_response_id`
//! chaining), so this provider holds no per-turn chain state — the session
//! always sends the complete message list.

use std::time::Duration;

use anyhow::{Context, Result};
use async_trait::async_trait;
use base64::{Engine, engine::general_purpose::STANDARD as BASE64};
use futures_util::StreamExt;
use reqwest::Client;
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::HashMap;
use tokio::sync::mpsc;
use tracing::{debug, info, warn};

use codescribe_core::agent::{
    AgentEvent, AgentProvider, ContentBlock, ImageAsset, Message, Role, StreamOptions,
    ToolDefinition,
};
use codescribe_core::llm::provider::{ProviderKind, capability_policy};

const ANTHROPIC_VERSION: &str = "2023-06-01";
/// Anthropic requires `max_tokens` on every request; used only when the caller
/// (assistive lane) leaves `options.max_tokens` unset.
const DEFAULT_MAX_TOKENS: u32 = 8192;

const DEFAULT_INITIAL_RESPONSE_TIMEOUT_MS: u64 = 90_000;
const DEFAULT_INTER_CHUNK_TIMEOUT_MS: u64 = 90_000;
const STREAM_REQUEST_TIMEOUT: Duration = Duration::from_secs(3600);
const STREAM_DEADLINE: Duration = Duration::from_secs(10 * 60);

#[derive(Clone)]
pub struct AnthropicProvider {
    client: Client,
    endpoint: String,
    api_key: String,
    anthropic_version: String,
    default_model: String,
    default_max_tokens: u32,
    initial_response_timeout: Duration,
    inter_chunk_timeout: Duration,
}

impl AnthropicProvider {
    /// Build from the resolved assistive lane (fresh settings → env →
    /// Keychain). Anthropic always authenticates, so a missing key is a
    /// readable error naming the exact account — the availability gate
    /// reports the same reason before a send is ever attempted.
    pub fn from_lane(
        lane: codescribe_core::llm::lane_truth::AssistiveLaneSnapshot,
    ) -> Result<Self> {
        let api_key = lane
            .api_key
            .context("Anthropic API key (assistive) is required. Set LLM_ANTHROPIC_API_KEY.")?;
        let endpoint = lane.endpoint;
        // Model comes from the shared assistive-lane setting; Settings supplies a
        // Claude model when the assistive provider is Anthropic.
        let default_model = lane.model;

        let initial_response_timeout = Duration::from_millis(parse_env_u64(
            "CODESCRIBE_AI_ATTEMPT_TIMEOUT_MS",
            DEFAULT_INITIAL_RESPONSE_TIMEOUT_MS,
        ));
        let inter_chunk_timeout = Duration::from_millis(parse_env_u64(
            "CODESCRIBE_AI_INTER_CHUNK_TIMEOUT_MS",
            DEFAULT_INTER_CHUNK_TIMEOUT_MS,
        ));

        let client = Client::builder()
            .timeout(STREAM_REQUEST_TIMEOUT)
            .build()
            .context("Failed to create Anthropic agent HTTP client")?;

        info!(
            "Anthropic agent provider configured (model={}, initial_timeout={}s, inter_chunk_timeout={}s)",
            default_model,
            initial_response_timeout.as_secs(),
            inter_chunk_timeout.as_secs()
        );

        Ok(Self {
            client,
            endpoint,
            api_key,
            anthropic_version: ANTHROPIC_VERSION.to_string(),
            default_model,
            default_max_tokens: DEFAULT_MAX_TOKENS,
            initial_response_timeout,
            inter_chunk_timeout,
        })
    }
}

#[async_trait]
impl AgentProvider for AnthropicProvider {
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

        // Per-model capability gate (CORRECTION.md matrix): Opus-4.8 rejects
        // sampling params with a 400, Sonnet tolerates temperature. `sanitize_
        // temperature` returns None when sampling is disallowed so the param is
        // simply omitted (omitting can never 400; sending it can).
        let policy = capability_policy(ProviderKind::AnthropicMessages, &model);
        let temperature = policy.sanitize_temperature(options.temperature);
        let max_tokens = options
            .max_tokens
            .filter(|tokens| *tokens > 0)
            .unwrap_or(self.default_max_tokens);

        let system = build_system(options.system_prompt.as_deref(), messages);
        let body = build_request_body(&model, system, messages, tools, max_tokens, temperature)
            .context("Failed to build Anthropic Messages request")?;

        info!(
            "Anthropic agent request (model={}, messages={}, tools={}, max_tokens={}, temperature={}, timeout={}s)",
            model,
            messages.len(),
            tools.len(),
            max_tokens,
            temperature.map(|_| "present").unwrap_or("absent"),
            self.initial_response_timeout.as_secs()
        );

        let (tx, rx) = mpsc::channel(256);
        let client = self.client.clone();
        let endpoint = self.endpoint.clone();
        let api_key = self.api_key.clone();
        let anthropic_version = self.anthropic_version.clone();
        let initial_response_timeout = self.initial_response_timeout;
        let inter_chunk_timeout = self.inter_chunk_timeout;

        tokio::spawn(async move {
            if let Err(error) = run_anthropic_stream(
                client,
                endpoint,
                api_key,
                anthropic_version,
                initial_response_timeout,
                inter_chunk_timeout,
                body,
                tx.clone(),
            )
            .await
            {
                let _ = tx.send(AgentEvent::Error(error.to_string())).await;
            }
        });

        Ok(rx)
    }

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

    fn build_image_block(&self, data: &[u8], media_type: &str) -> ContentBlock {
        ContentBlock::Image {
            data: data.to_vec(),
            media_type: media_type.to_string(),
        }
    }

    fn stream_timeouts(&self) -> Option<(Duration, Duration)> {
        Some((self.initial_response_timeout, self.inter_chunk_timeout))
    }

    fn name(&self) -> &str {
        "anthropic-messages"
    }
}

// ── Request building ─────────────────────────────────────────────────────────

/// Compose the top-level `system` string from the caller's system prompt plus
/// any `System`-role history messages (folded in — Anthropic has no `system`
/// message role). `None` when there is nothing to say.
fn build_system(prompt: Option<&str>, messages: &[Message]) -> Option<String> {
    let mut parts = Vec::new();
    if let Some(prompt) = prompt
        && !prompt.trim().is_empty()
    {
        parts.push(prompt.trim().to_string());
    }
    for message in messages {
        if message.role != Role::System {
            continue;
        }
        for block in &message.content {
            if let ContentBlock::Text(text) = block
                && !text.trim().is_empty()
            {
                parts.push(text.trim().to_string());
            }
        }
    }
    if parts.is_empty() {
        None
    } else {
        Some(parts.join("\n\n"))
    }
}

fn build_request_body(
    model: &str,
    system: Option<String>,
    messages: &[Message],
    tools: &[ToolDefinition],
    max_tokens: u32,
    temperature: Option<f32>,
) -> Result<Value> {
    let mut body = serde_json::Map::new();
    body.insert("model".to_string(), json!(model));
    body.insert("max_tokens".to_string(), json!(max_tokens));
    body.insert("stream".to_string(), json!(true));
    if let Some(system) = system.filter(|value| !value.trim().is_empty()) {
        body.insert("system".to_string(), json!(system));
    }
    body.insert(
        "messages".to_string(),
        json!(build_anthropic_messages(messages)?),
    );
    let tool_payload = build_tool_payload(tools);
    if !tool_payload.is_empty() {
        body.insert("tools".to_string(), json!(tool_payload));
    }
    if let Some(temperature) = temperature {
        body.insert("temperature".to_string(), json!(temperature));
    }
    Ok(Value::Object(body))
}

fn build_tool_payload(tools: &[ToolDefinition]) -> Vec<Value> {
    tools
        .iter()
        .map(|tool| {
            json!({
                "name": tool.name,
                "description": tool.description,
                "input_schema": tool.input_schema,
            })
        })
        .collect()
}

fn build_anthropic_messages(messages: &[Message]) -> Result<Vec<Value>> {
    let mut out = Vec::new();
    for message in messages {
        // System-role content is hoisted into the top-level `system` field.
        if message.role == Role::System {
            continue;
        }
        let content = message_content_blocks(message)?;
        // Anthropic rejects a message with empty content — skip it entirely.
        if content.is_empty() {
            continue;
        }
        out.push(json!({
            "role": role_str(message.role),
            "content": content,
        }));
    }
    Ok(out)
}

fn message_content_blocks(message: &Message) -> Result<Vec<Value>> {
    let mut blocks = Vec::new();
    for block in &message.content {
        match block {
            ContentBlock::Text(text) => {
                if !text.is_empty() {
                    blocks.push(json!({ "type": "text", "text": text }));
                }
            }
            ContentBlock::Image { data, media_type } => {
                // Images restored from the thread store carry no bytes
                // (persisted with `data_omitted`). An empty base64 payload makes
                // Anthropic reject the whole request, so skip empty images.
                if data.is_empty() {
                    warn!(
                        "Skipping image content block with no bytes (likely restored from history)"
                    );
                    continue;
                }
                blocks.push(image_block(data, media_type));
            }
            ContentBlock::ImageAsset(asset) => {
                blocks.push(image_asset_block(asset)?);
            }
            ContentBlock::ToolUse { id, name, input } => {
                blocks.push(json!({
                    "type": "tool_use",
                    "id": id,
                    "name": name,
                    "input": input,
                }));
            }
            ContentBlock::ToolResult {
                tool_use_id,
                content,
                is_error,
            } => {
                blocks.push(tool_result_block(tool_use_id, content, *is_error)?);
            }
        }
    }
    Ok(blocks)
}

fn tool_result_block(tool_use_id: &str, content: &[ContentBlock], is_error: bool) -> Result<Value> {
    let mut inner = Vec::new();
    for block in content {
        match block {
            ContentBlock::Text(text) => {
                if !text.trim().is_empty() {
                    inner.push(json!({ "type": "text", "text": text }));
                }
            }
            ContentBlock::Image { data, media_type } => {
                if !data.is_empty() {
                    inner.push(image_block(data, media_type));
                }
            }
            ContentBlock::ImageAsset(asset) => {
                inner.push(image_asset_block(asset)?);
            }
            ContentBlock::ToolUse { .. } | ContentBlock::ToolResult { .. } => {}
        }
    }
    if inner.is_empty() {
        let fallback = if is_error {
            "Tool execution failed"
        } else {
            "Tool executed successfully"
        };
        inner.push(json!({ "type": "text", "text": fallback }));
    }
    let mut result = serde_json::Map::new();
    result.insert("type".to_string(), json!("tool_result"));
    result.insert("tool_use_id".to_string(), json!(tool_use_id));
    result.insert("content".to_string(), json!(inner));
    if is_error {
        result.insert("is_error".to_string(), json!(true));
    }
    Ok(Value::Object(result))
}

fn image_asset_block(asset: &ImageAsset) -> Result<Value> {
    // Tainted-path guard: asset paths ride through conversation state, so the
    // read goes through the store, which honors only the file name re-rooted
    // under the canonical assets dir.
    let data = codescribe_core::agent::AgentAssetStore::read_image(&asset.path)?;
    Ok(image_block(&data, &asset.media_type))
}

fn image_block(data: &[u8], media_type: &str) -> Value {
    let media_type = {
        let normalized = media_type.trim();
        if normalized.is_empty() {
            "image/png"
        } else {
            normalized
        }
    };
    json!({
        "type": "image",
        "source": {
            "type": "base64",
            "media_type": media_type,
            "data": BASE64.encode(data),
        }
    })
}

fn role_str(role: Role) -> &'static str {
    match role {
        Role::Assistant => "assistant",
        // System is hoisted out earlier; map defensively to user.
        Role::User | Role::System => "user",
    }
}

// ── SSE streaming ────────────────────────────────────────────────────────────

// allow(too_many_arguments): task entry point for one Anthropic SSE stream; all
// values are owned moves into the spawned task.
#[allow(clippy::too_many_arguments)]
async fn run_anthropic_stream(
    client: Client,
    endpoint: String,
    api_key: String,
    anthropic_version: String,
    initial_response_timeout: Duration,
    inter_chunk_timeout: Duration,
    request_body: Value,
    tx: mpsc::Sender<AgentEvent>,
) -> Result<()> {
    let endpoint_url =
        validate_anthropic_endpoint(&endpoint).context("Invalid Anthropic endpoint URL")?;
    let request_builder = client
        // nosemgrep: rust.actix.ssrf.reqwest-taint.reqwest-taint -- URL is validated by `validate_anthropic_endpoint`.
        .post(endpoint_url)
        .header("x-api-key", &api_key)
        .header("anthropic-version", &anthropic_version)
        .header("Content-Type", "application/json")
        .header("Accept", "text/event-stream")
        .timeout(STREAM_REQUEST_TIMEOUT)
        .json(&request_body);

    let response =
        match tokio::time::timeout(initial_response_timeout, request_builder.send()).await {
            Ok(Ok(resp)) => resp,
            Ok(Err(error)) => return Err(error).context("Anthropic SSE request failed"),
            Err(_) => anyhow::bail!(
                "Anthropic SSE initial response timeout after {:?}",
                initial_response_timeout
            ),
        };

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        let detail = parse_http_error_body(&body).unwrap_or(body);
        anyhow::bail!("Anthropic HTTP {} - {}", status, detail);
    }

    let mut state = StreamState::default();
    let mut stream = response.bytes_stream();
    let mut buffer = String::new();
    let stream_deadline = tokio::time::Instant::now() + STREAM_DEADLINE;

    'outer: loop {
        if tokio::time::Instant::now() > stream_deadline {
            anyhow::bail!(
                "Anthropic SSE global safety timeout after {:?}",
                STREAM_DEADLINE
            );
        }

        let next_chunk = match tokio::time::timeout(inter_chunk_timeout, stream.next()).await {
            Ok(chunk) => chunk,
            Err(_) => {
                anyhow::bail!(
                    "Anthropic SSE inter-chunk timeout after {:?}",
                    inter_chunk_timeout
                );
            }
        };

        let Some(chunk_result) = next_chunk else {
            break;
        };
        let chunk = chunk_result.context("Anthropic SSE stream read error")?;
        buffer.push_str(&String::from_utf8_lossy(&chunk));

        while let Some(newline_pos) = buffer.find('\n') {
            let line = buffer[..newline_pos].trim().to_string();
            buffer = buffer[newline_pos + 1..].to_string();

            // Anthropic emits both `event:` and `data:` lines; every `data:`
            // payload also carries its own `type`, so we key off the JSON and
            // ignore the `event:` line (and SSE comments beginning with `:`).
            if line.is_empty() || line.starts_with(':') || line.starts_with("event:") {
                continue;
            }

            let Some(data) = line
                .strip_prefix("data: ")
                .or_else(|| line.strip_prefix("data:"))
            else {
                continue;
            };
            let data = data.trim();
            if data.is_empty() {
                continue;
            }

            let event = match serde_json::from_str::<AnthropicChunk>(data) {
                Ok(parsed) => parsed,
                Err(error) => {
                    warn!("Skipping malformed Anthropic SSE chunk: {}", error);
                    continue;
                }
            };

            if handle_chunk(&event, &mut state, &tx).await? {
                break 'outer;
            }
        }
    }

    // EOF (or `message_stop` already handled). If no terminal was emitted, the
    // stream ended early — surface a dirty terminal so the session resets.
    if !state.terminal_emitted
        && tx
            .send(AgentEvent::ResponseDone {
                response_id: state.message_id.clone(),
                clean: false,
            })
            .await
            .is_err()
    {
        return Ok(());
    }

    Ok(())
}

#[derive(Default)]
struct StreamState {
    message_id: Option<String>,
    assistant_text: String,
    stop_reason: Option<String>,
    /// index -> in-flight tool_use block (id, name, accumulated JSON).
    tool_blocks: HashMap<u64, ToolBlock>,
    terminal_emitted: bool,
}

struct ToolBlock {
    id: String,
    name: String,
    json_buffer: String,
}

/// Dispatch one parsed SSE chunk. Returns `Ok(true)` when a terminal was
/// emitted and the caller should stop reading; `Ok(false)` to keep going. An
/// `Err` propagates a fatal parse/consumer condition to the task wrapper.
async fn handle_chunk(
    chunk: &AnthropicChunk,
    state: &mut StreamState,
    tx: &mpsc::Sender<AgentEvent>,
) -> Result<bool> {
    match chunk.chunk_type.as_str() {
        "message_start" => {
            if let Some(message) = &chunk.message
                && !message.id.is_empty()
            {
                state.message_id = Some(message.id.clone());
            }
            Ok(false)
        }
        "content_block_start" => {
            let (Some(index), Some(block)) = (chunk.index, chunk.content_block.as_ref()) else {
                return Ok(false);
            };
            if block.block_type == "tool_use" {
                let id = block.id.clone().unwrap_or_default();
                let name = block
                    .name
                    .clone()
                    .filter(|value| !value.is_empty())
                    .unwrap_or_else(|| "unknown_tool".to_string());
                state.tool_blocks.insert(
                    index,
                    ToolBlock {
                        id: id.clone(),
                        name: name.clone(),
                        json_buffer: String::new(),
                    },
                );
                if send(tx, AgentEvent::ToolCallStart { id, name })
                    .await
                    .is_err()
                {
                    return Ok(true);
                }
            }
            Ok(false)
        }
        "content_block_delta" => {
            let Some(delta) = chunk.delta.as_ref() else {
                return Ok(false);
            };
            match delta.delta_type.as_deref() {
                Some("text_delta") => {
                    if let Some(text) = &delta.text {
                        state.assistant_text.push_str(text);
                        if send(tx, AgentEvent::TextDelta(text.clone())).await.is_err() {
                            return Ok(true);
                        }
                    }
                }
                Some("input_json_delta") => {
                    if let (Some(index), Some(partial)) = (chunk.index, delta.partial_json.as_ref())
                        && let Some(tool) = state.tool_blocks.get_mut(&index)
                    {
                        tool.json_buffer.push_str(partial);
                        if send(
                            tx,
                            AgentEvent::ToolCallArgsDelta {
                                id: tool.id.clone(),
                                delta: partial.clone(),
                            },
                        )
                        .await
                        .is_err()
                        {
                            return Ok(true);
                        }
                    }
                }
                Some("thinking_delta") => {
                    if let Some(thinking) = &delta.thinking
                        && send(tx, AgentEvent::ReasoningDelta(thinking.clone()))
                            .await
                            .is_err()
                    {
                        return Ok(true);
                    }
                }
                _ => {}
            }
            Ok(false)
        }
        "content_block_stop" => {
            let Some(index) = chunk.index else {
                return Ok(false);
            };
            if let Some(tool) = state.tool_blocks.remove(&index) {
                let raw = tool.json_buffer.trim();
                let raw = if raw.is_empty() { "{}" } else { raw };
                match serde_json::from_str::<Value>(raw) {
                    Ok(arguments) => {
                        if send(
                            tx,
                            AgentEvent::ToolCallReady {
                                id: tool.id,
                                name: tool.name,
                                arguments,
                            },
                        )
                        .await
                        .is_err()
                        {
                            return Ok(true);
                        }
                    }
                    Err(error) => {
                        if send(
                            tx,
                            AgentEvent::Error(format!(
                                "Failed to parse arguments for tool '{}': {}",
                                tool.name, error
                            )),
                        )
                        .await
                        .is_err()
                        {
                            return Ok(true);
                        }
                    }
                }
            }
            Ok(false)
        }
        "message_delta" => {
            if let Some(delta) = chunk.delta.as_ref()
                && let Some(stop_reason) = delta.stop_reason.as_ref()
            {
                state.stop_reason = Some(stop_reason.clone());
            }
            Ok(false)
        }
        "message_stop" => {
            state.terminal_emitted = true;
            if state.stop_reason.as_deref() == Some("refusal") {
                let _ = send(
                    tx,
                    AgentEvent::Error(
                        "Anthropic declined the request (stop_reason: refusal)".to_string(),
                    ),
                )
                .await;
                return Ok(true);
            }
            let text = state.assistant_text.trim();
            if !text.is_empty()
                && send(tx, AgentEvent::TextDone(text.to_string()))
                    .await
                    .is_err()
            {
                return Ok(true);
            }
            let _ = send(
                tx,
                AgentEvent::ResponseDone {
                    response_id: state.message_id.clone(),
                    clean: true,
                },
            )
            .await;
            Ok(true)
        }
        "error" => {
            state.terminal_emitted = true;
            let detail = chunk
                .error
                .as_ref()
                .map(format_stream_error)
                .unwrap_or_else(|| "unknown error".to_string());
            let _ = send(
                tx,
                AgentEvent::Error(format!("Anthropic SSE error: {detail}")),
            )
            .await;
            Ok(true)
        }
        // `ping`, `message_delta` usage-only, and unknown lifecycle events.
        other => {
            debug!("Anthropic SSE ignoring event type={}", other);
            Ok(false)
        }
    }
}

async fn send(tx: &mpsc::Sender<AgentEvent>, event: AgentEvent) -> Result<(), ()> {
    tx.send(event).await.map_err(|_| ())
}

fn format_stream_error(error: &AnthropicError) -> String {
    match (error.error_type.as_deref(), error.message.as_deref()) {
        (Some(kind), Some(message)) => format!("{kind}: {message}"),
        (Some(kind), None) => kind.to_string(),
        (None, Some(message)) => message.to_string(),
        (None, None) => "unknown error".to_string(),
    }
}

/// Extract `error.message` (with optional `error.type`) from an HTTP error body,
/// falling back to `None` when the body is not the standard Anthropic error
/// envelope.
fn parse_http_error_body(body: &str) -> Option<String> {
    #[derive(Deserialize)]
    struct Envelope {
        error: Option<AnthropicError>,
    }
    let envelope = serde_json::from_str::<Envelope>(body).ok()?;
    envelope.error.map(|error| format_stream_error(&error))
}

fn validate_anthropic_endpoint(endpoint: &str) -> Result<reqwest::Url> {
    let endpoint = endpoint.trim();
    if endpoint.is_empty() {
        anyhow::bail!("Endpoint URL is empty");
    }
    let url = reqwest::Url::parse(endpoint).context("Endpoint is not a valid URL")?;
    let host = url.host_str().context("Endpoint URL is missing a host")?;
    let is_loopback = matches!(
        host.trim_matches(['[', ']']),
        "localhost" | "127.0.0.1" | "::1"
    );
    match url.scheme() {
        "https" => {}
        "http" if is_loopback => {}
        "http" => anyhow::bail!("Plain HTTP is only allowed for localhost loopback endpoints"),
        other => anyhow::bail!("Unsupported endpoint URL scheme: {}", other),
    }
    Ok(url)
}

fn parse_env_u64(key: &str, default: u64) -> u64 {
    std::env::var(key)
        .ok()
        .and_then(|value| value.trim().parse::<u64>().ok())
        .unwrap_or(default)
}

// ── SSE wire types ───────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct AnthropicChunk {
    #[serde(rename = "type")]
    chunk_type: String,
    #[serde(default)]
    index: Option<u64>,
    #[serde(default)]
    message: Option<AnthropicMessageMeta>,
    #[serde(default)]
    content_block: Option<AnthropicContentBlockMeta>,
    #[serde(default)]
    delta: Option<AnthropicDelta>,
    #[serde(default)]
    error: Option<AnthropicError>,
}

#[derive(Debug, Deserialize)]
struct AnthropicMessageMeta {
    #[serde(default)]
    id: String,
}

#[derive(Debug, Deserialize)]
struct AnthropicContentBlockMeta {
    #[serde(rename = "type")]
    block_type: String,
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    name: Option<String>,
}

#[derive(Debug, Deserialize)]
struct AnthropicDelta {
    #[serde(rename = "type", default)]
    delta_type: Option<String>,
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    partial_json: Option<String>,
    #[serde(default)]
    thinking: Option<String>,
    #[serde(default)]
    stop_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
struct AnthropicError {
    #[serde(rename = "type", default)]
    error_type: Option<String>,
    #[serde(default)]
    message: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use codescribe_core::agent::AgentAssetStore;
    use serde_json::json;
    use std::time::Duration;

    fn text_message(role: Role, text: &str) -> Message {
        Message::new(role, vec![ContentBlock::Text(text.to_string())])
    }

    #[test]
    fn build_system_folds_prompt_and_system_messages() {
        let messages = vec![
            text_message(Role::System, "history system note"),
            text_message(Role::User, "hi"),
        ];
        let system = build_system(Some("primary prompt"), &messages)
            .expect("system should combine prompt and system-role text");
        assert!(system.contains("primary prompt"));
        assert!(system.contains("history system note"));

        assert_eq!(build_system(None, &[text_message(Role::User, "hi")]), None);
    }

    #[test]
    fn build_request_body_hoists_system_and_serializes_user_message() {
        let messages = vec![text_message(Role::User, "hello world")];
        let body = build_request_body(
            "claude-opus-4-8",
            Some("be terse".to_string()),
            &messages,
            &[],
            4096,
            None,
        )
        .expect("request body should build");

        assert_eq!(body["model"], "claude-opus-4-8");
        assert_eq!(body["max_tokens"], 4096);
        assert_eq!(body["stream"], true);
        assert_eq!(body["system"], "be terse");
        assert_eq!(body["messages"][0]["role"], "user");
        assert_eq!(body["messages"][0]["content"][0]["type"], "text");
        assert_eq!(body["messages"][0]["content"][0]["text"], "hello world");
        // No tools, no temperature when omitted.
        assert!(body.get("tools").is_none());
        assert!(body.get("temperature").is_none());
    }

    #[test]
    fn opus_strips_temperature_sonnet_keeps_it() {
        let messages = vec![text_message(Role::User, "hi")];

        let opus = capability_policy(ProviderKind::AnthropicMessages, "claude-opus-4-8");
        let opus_temp = opus.sanitize_temperature(Some(0.7));
        let opus_body =
            build_request_body("claude-opus-4-8", None, &messages, &[], 1024, opus_temp).unwrap();
        assert!(
            opus_body.get("temperature").is_none(),
            "Opus-4.8 rejects sampling params; temperature must be omitted"
        );

        let sonnet = capability_policy(ProviderKind::AnthropicMessages, "claude-sonnet-4-6");
        let sonnet_temp = sonnet.sanitize_temperature(Some(0.3));
        let sonnet_body =
            build_request_body("claude-sonnet-4-6", None, &messages, &[], 1024, sonnet_temp)
                .unwrap();
        let sent = sonnet_body["temperature"]
            .as_f64()
            .expect("Sonnet keeps temperature as a number");
        assert!(
            (sent - 0.3).abs() < 1e-6,
            "Sonnet-4.6 tolerates temperature; expected ~0.3, got {sent}"
        );
    }

    #[test]
    fn tools_serialize_to_anthropic_input_schema() {
        let tools = vec![ToolDefinition {
            name: "read_file".to_string(),
            description: "Read a file".to_string(),
            input_schema: json!({"type": "object", "properties": {"path": {"type": "string"}}}),
        }];
        let payload = build_tool_payload(&tools);
        assert_eq!(payload[0]["name"], "read_file");
        assert_eq!(payload[0]["description"], "Read a file");
        assert_eq!(payload[0]["input_schema"]["type"], "object");
    }

    #[test]
    fn tool_use_and_tool_result_map_to_anthropic_blocks() {
        let assistant = Message::new(
            Role::Assistant,
            vec![ContentBlock::ToolUse {
                id: "toolu_1".to_string(),
                name: "read_file".to_string(),
                input: json!({"path": "/tmp/x"}),
            }],
        );
        let blocks = message_content_blocks(&assistant).unwrap();
        assert_eq!(blocks[0]["type"], "tool_use");
        assert_eq!(blocks[0]["id"], "toolu_1");
        assert_eq!(blocks[0]["name"], "read_file");
        assert_eq!(blocks[0]["input"]["path"], "/tmp/x");

        let user = Message::new(
            Role::User,
            vec![ContentBlock::ToolResult {
                tool_use_id: "toolu_1".to_string(),
                content: vec![ContentBlock::Text("file body".to_string())],
                is_error: false,
            }],
        );
        let result_blocks = message_content_blocks(&user).unwrap();
        assert_eq!(result_blocks[0]["type"], "tool_result");
        assert_eq!(result_blocks[0]["tool_use_id"], "toolu_1");
        assert_eq!(result_blocks[0]["content"][0]["text"], "file body");
        assert!(result_blocks[0].get("is_error").is_none());

        let error_user = Message::new(
            Role::User,
            vec![ContentBlock::ToolResult {
                tool_use_id: "toolu_2".to_string(),
                content: vec![],
                is_error: true,
            }],
        );
        let error_blocks = message_content_blocks(&error_user).unwrap();
        assert_eq!(error_blocks[0]["is_error"], true);
        assert_eq!(
            error_blocks[0]["content"][0]["text"],
            "Tool execution failed"
        );
    }

    #[test]
    fn image_block_uses_base64_source() {
        let block = image_block(b"png bytes", "image/png");
        assert_eq!(block["type"], "image");
        assert_eq!(block["source"]["type"], "base64");
        assert_eq!(block["source"]["media_type"], "image/png");
        assert_eq!(
            block["source"]["data"].as_str().unwrap(),
            BASE64.encode(b"png bytes")
        );
        // Empty media type normalizes to image/png.
        assert_eq!(image_block(b"x", "")["source"]["media_type"], "image/png");
    }

    #[test]
    fn empty_image_bytes_are_skipped() {
        let message = Message::new(
            Role::User,
            vec![
                ContentBlock::Image {
                    data: vec![],
                    media_type: "image/png".to_string(),
                },
                ContentBlock::Text("caption".to_string()),
            ],
        );
        let blocks = message_content_blocks(&message).unwrap();
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0]["type"], "text");
    }

    #[test]
    fn user_message_inline_image_rides_into_request_body() {
        // Composer 📎 path: `AgentSession::send` builds a [Text, Image{bytes}]
        // user turn via `build_image_block`. A non-empty inline image must
        // serialize into the request as a base64 image source alongside its
        // caption — a regression here silently drops user attachments (the exact
        // failure mode the composer vision path must never reintroduce). Parity
        // with the OpenAI `input_image` guard.
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

        let body = build_request_body("claude-opus-4-8", None, &messages, &[], 4096, None)
            .expect("request body should build");

        assert_eq!(body["messages"][0]["role"], "user");
        let content = body["messages"][0]["content"]
            .as_array()
            .expect("user content array");
        assert_eq!(content.len(), 2, "caption + image both survive");
        assert_eq!(content[0]["type"], "text");
        assert_eq!(content[1]["type"], "image");
        assert_eq!(content[1]["source"]["type"], "base64");
        assert_eq!(content[1]["source"]["media_type"], "image/png");
        assert_eq!(
            content[1]["source"]["data"].as_str().unwrap(),
            BASE64.encode(b"png bytes")
        );
    }

    #[test]
    fn tool_result_carries_image_asset_as_base64() {
        let asset = AgentAssetStore::save_image(b"png bytes", "image/png")
            .expect("image asset should save");
        let path = asset.path.clone();
        let message = Message::new(
            Role::User,
            vec![ContentBlock::ToolResult {
                tool_use_id: "toolu_shot".to_string(),
                content: vec![ContentBlock::ImageAsset(asset)],
                is_error: false,
            }],
        );
        let blocks = message_content_blocks(&message).unwrap();
        assert_eq!(blocks[0]["content"][0]["type"], "image");
        assert_eq!(blocks[0]["content"][0]["source"]["type"], "base64");
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn restored_thread_inline_image_reaches_prompt_on_next_turn() {
        // Turn 2 on a restored thread: an inline composer image persisted via
        // the thread store must come back as a disk-backed asset and still
        // reach the request payload instead of being skipped as byteless.
        let image_bytes = b"w5a-anthropic-turn2".to_vec();
        let original = Message::new(
            Role::User,
            vec![ContentBlock::Image {
                data: image_bytes.clone(),
                media_type: "image/png".to_string(),
            }],
        );
        let restored = codescribe_core::agent::ThreadMessage::from(&original).to_message();

        let blocks = message_content_blocks(&restored).expect("restored image should serialize");
        assert_eq!(blocks.len(), 1, "restored image must not be skipped");
        assert_eq!(blocks[0]["type"], "image");
        assert_eq!(
            blocks[0]["source"]["data"].as_str().unwrap(),
            BASE64.encode(&image_bytes)
        );

        if let ContentBlock::ImageAsset(asset) = &restored.content[0] {
            std::fs::remove_file(&asset.path).ok();
        }
    }

    #[test]
    fn validate_endpoint_rejects_non_loopback_http() {
        assert!(validate_anthropic_endpoint("https://api.anthropic.com/v1/messages").is_ok());
        assert!(validate_anthropic_endpoint("http://127.0.0.1:8080/v1/messages").is_ok());
        assert!(validate_anthropic_endpoint("http://example.com/v1/messages").is_err());
        assert!(validate_anthropic_endpoint("ftp://example.com").is_err());
        assert!(validate_anthropic_endpoint("   ").is_err());
    }

    fn provider_for(endpoint: &str) -> AnthropicProvider {
        AnthropicProvider {
            client: Client::new(),
            endpoint: endpoint.to_string(),
            api_key: "test-key".to_string(),
            anthropic_version: ANTHROPIC_VERSION.to_string(),
            default_model: "claude-opus-4-8".to_string(),
            default_max_tokens: 1024,
            initial_response_timeout: Duration::from_secs(2),
            inter_chunk_timeout: Duration::from_secs(2),
        }
    }

    #[tokio::test]
    async fn stream_parses_text_tool_and_terminal_events() {
        let mut server = mockito::Server::new_async().await;
        let body = [
            r#"data: {"type":"message_start","message":{"id":"msg_1"}}"#,
            "",
            r#"data: {"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}"#,
            "",
            r#"data: {"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Hello"}}"#,
            "",
            r#"data: {"type":"content_block_stop","index":0}"#,
            "",
            r#"data: {"type":"content_block_start","index":1,"content_block":{"type":"tool_use","id":"toolu_9","name":"read_file"}}"#,
            "",
            r#"data: {"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"{\"path\":"}}"#,
            "",
            r#"data: {"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"\"/tmp/x\"}"}}"#,
            "",
            r#"data: {"type":"content_block_stop","index":1}"#,
            "",
            r#"data: {"type":"message_delta","delta":{"stop_reason":"tool_use"}}"#,
            "",
            r#"data: {"type":"message_stop"}"#,
            "",
        ]
        .join("\n");
        let mock = server
            .mock("POST", "/v1/messages")
            .match_header("x-api-key", "test-key")
            .match_header("anthropic-version", ANTHROPIC_VERSION)
            .with_status(200)
            .with_header("content-type", "text/event-stream")
            .with_body(body)
            .create_async()
            .await;

        let provider = provider_for(&format!("{}/v1/messages", server.url()));
        let messages = vec![text_message(Role::User, "read it")];
        let mut rx = provider
            .stream(&messages, &[], &StreamOptions::default())
            .await
            .expect("stream should start");

        let mut events = Vec::new();
        while let Ok(Some(event)) = tokio::time::timeout(Duration::from_secs(2), rx.recv()).await {
            events.push(event);
        }

        assert!(events.contains(&AgentEvent::TextDelta("Hello".to_string())));
        assert!(events.contains(&AgentEvent::TextDone("Hello".to_string())));
        assert!(events.iter().any(|event| matches!(
            event,
            AgentEvent::ToolCallReady { id, name, arguments }
                if id == "toolu_9" && name == "read_file" && arguments["path"] == "/tmp/x"
        )));
        assert!(events.iter().any(|event| matches!(
            event,
            AgentEvent::ResponseDone { response_id: Some(id), clean: true } if id == "msg_1"
        )));
        mock.assert_async().await;
    }

    #[tokio::test]
    async fn stream_surfaces_sse_error_event() {
        let mut server = mockito::Server::new_async().await;
        let body = [
            r#"data: {"type":"error","error":{"type":"overloaded_error","message":"try later"}}"#,
            "",
        ]
        .join("\n");
        let mock = server
            .mock("POST", "/v1/messages")
            .with_status(200)
            .with_header("content-type", "text/event-stream")
            .with_body(body)
            .create_async()
            .await;

        let provider = provider_for(&format!("{}/v1/messages", server.url()));
        let messages = vec![text_message(Role::User, "hi")];
        let mut rx = provider
            .stream(&messages, &[], &StreamOptions::default())
            .await
            .expect("stream should start");

        let event = tokio::time::timeout(Duration::from_secs(2), rx.recv())
            .await
            .expect("event before timeout")
            .expect("one event");
        match event {
            AgentEvent::Error(message) => {
                assert!(message.contains("overloaded_error"));
                assert!(message.contains("try later"));
            }
            other => panic!("expected Error, got {other:?}"),
        }
        mock.assert_async().await;
    }

    #[tokio::test]
    async fn stream_maps_http_error_to_error_event() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("POST", "/v1/messages")
            .with_status(400)
            .with_header("content-type", "application/json")
            .with_body(r#"{"type":"error","error":{"type":"invalid_request_error","message":"bad model"}}"#)
            .create_async()
            .await;

        let provider = provider_for(&format!("{}/v1/messages", server.url()));
        let messages = vec![text_message(Role::User, "hi")];
        let mut rx = provider
            .stream(&messages, &[], &StreamOptions::default())
            .await
            .expect("stream should start");

        let event = tokio::time::timeout(Duration::from_secs(2), rx.recv())
            .await
            .expect("event before timeout")
            .expect("one event");
        match event {
            AgentEvent::Error(message) => {
                assert!(message.contains("Anthropic HTTP 400"));
                assert!(message.contains("bad model"));
            }
            other => panic!("expected Error, got {other:?}"),
        }
        mock.assert_async().await;
    }
}
