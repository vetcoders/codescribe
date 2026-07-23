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
use codescribe_core::llm::account_auth;
use codescribe_core::llm::lane_truth::AssistiveLaneSnapshot;
use codescribe_core::llm::provider::ProviderKind;
use codescribe_core::llm::responses_streaming_manager::{
    AuthHeaderMode, ResponsesStreamingManager, StreamCallbacks,
};

const DEFAULT_INITIAL_RESPONSE_TIMEOUT_MS: u64 = 90_000;
const DEFAULT_INTER_CHUNK_TIMEOUT_MS: u64 = 90_000;

#[derive(Clone)]
pub struct OpenAiProvider {
    client: Client,
    endpoint: String,
    api_key: String,
    default_model: String,
    use_previous_response_id: bool,
    /// Single source of truth for the AGENT path's response chain
    /// (`previous_response_id`).
    ///
    /// P2.12 (source-of-truth contract): the assistive feature has TWO distinct
    /// execution paths, each owning its own chain — they are intentionally
    /// separate, not redundant:
    ///   1. Agent 2.0 path (this provider): owns the chain HERE, in this
    ///      per-provider `Arc<Mutex>`. Advanced/reset by
    ///      `forward_events_and_track_chain` and `apply_chain_reset`.
    ///   2. Legacy formatter fallback path (`run_legacy_send_path` ->
    ///      `ai_formatting`): owns its chain in the global
    ///      `core::state::conversation` store under `AiMode::Assistive`
    ///      (`assistive_response_id`).
    ///
    /// A given turn runs through exactly one path, so the two chains never both
    /// drive the same request. Do NOT cross-wire them: the agent path must never
    /// read/write `conversation::*_response_id`, and the legacy path must never
    /// touch this field. If the legacy fallback is ever retired, the
    /// `AiMode::Assistive` branch in `core::state::conversation` becomes dead and
    /// should be removed (owner: GROUP state).
    previous_response_id: Arc<Mutex<Option<String>>>,
    initial_response_timeout: Duration,
    inter_chunk_timeout: Duration,
    /// Lane resolved to "Sign in with ChatGPT" account auth (no API key, official
    /// endpoint, stored tokens). Each request fetches a FRESH access token via
    /// `account_auth` so the auto-refresh path keeps long sessions alive.
    use_account_auth: bool,
}

impl OpenAiProvider {
    /// Build from the resolved assistive lane (fresh settings → env →
    /// Keychain) instead of the frozen bootstrap process env. `api_key: None`
    /// becomes an empty key, which the streaming manager translates into a
    /// clean unauthenticated request — key-optional local endpoints are a
    /// first-class configuration, not an error.
    pub fn from_lane(lane: AssistiveLaneSnapshot) -> Result<Self> {
        let AssistiveLaneSnapshot {
            endpoint,
            model: default_model,
            api_key,
            account_auth: use_account_auth,
            provider: _,
        } = lane;
        let api_key = api_key.unwrap_or_default();

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
            "OpenAI agent provider configured (model={}, initial_timeout={}s, inter_chunk_timeout={}s, previous_response_id={})",
            default_model,
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
        })
    }
}

#[async_trait]
impl AgentProvider for OpenAiProvider {
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
            model,
            input: build_request_input_items(messages, previous_response_id.as_deref())?,
            previous_response_id,
            instructions: options.system_prompt.clone(),
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
                account_auth::access_token(ProviderKind::OpenAiResponses)
                    .await
                    .map_err(|error| {
                        anyhow::anyhow!("ChatGPT account authentication failed: {error}")
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
        "openai-responses"
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

#[derive(Debug, Serialize)]
struct OpenAiResponsesRequest {
    model: String,
    input: Vec<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    previous_response_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    instructions: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_output_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tools: Vec<OpenAiToolDefinition>,
    stream: bool,
}

#[derive(Debug, Serialize)]
struct OpenAiToolDefinition {
    #[serde(rename = "type")]
    tool_type: &'static str,
    name: String,
    description: String,
    parameters: Value,
}

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

fn build_request_input_items(
    messages: &[Message],
    previous_response_id: Option<&str>,
) -> Result<Vec<Value>> {
    build_input_items(request_messages(messages, previous_response_id))
}

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

fn text_content_type(role: Role) -> &'static str {
    match role {
        Role::Assistant => "output_text",
        Role::User | Role::System => "input_text",
    }
}

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

fn role_to_str(role: Role) -> &'static str {
    match role {
        Role::User => "user",
        Role::Assistant => "assistant",
        Role::System => "system",
    }
}

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

fn parse_env_u64(key: &str, default: u64) -> u64 {
    env::var(key)
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(default)
}

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

#[cfg(test)]
mod tests {
    use super::{
        OpenAiProvider, build_request_input_items, format_tool_output,
        forward_events_and_track_chain, request_messages, to_data_uri,
    };
    use std::sync::Arc;
    use std::time::Duration;

    use codescribe_core::agent::{
        AgentAssetStore, AgentEvent, AgentProvider, ContentBlock, Message, Role, StreamOptions,
    };
    use reqwest::Client;
    use serde_json::json;
    use tokio::sync::{Mutex, mpsc};

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
