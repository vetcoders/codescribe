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
use tracing::info;

use codescribe_core::agent::{
    AgentEvent, AgentProvider, ContentBlock, ImageAsset, Message, Role, StreamOptions,
    ToolDefinition,
};
use codescribe_core::llm::responses_streaming_manager::{
    ResponsesStreamingManager, StreamCallbacks,
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
    previous_response_id: Arc<Mutex<Option<String>>>,
    initial_response_timeout: Duration,
    inter_chunk_timeout: Duration,
}

impl OpenAiProvider {
    pub fn from_env() -> Result<Self> {
        let endpoint = get_env_non_empty("LLM_ASSISTIVE_ENDPOINT", "LLM endpoint (assistive)")?;
        let default_model = get_env_non_empty("LLM_ASSISTIVE_MODEL", "LLM model (assistive)")?;
        let api_key = get_env_non_empty("LLM_ASSISTIVE_API_KEY", "LLM API key (assistive)")?;

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

        let manager = ResponsesStreamingManager::new(
            &self.client,
            &self.endpoint,
            &self.api_key,
            StreamCallbacks {
                assistant: None,
                reasoning: None,
            },
            self.initial_response_timeout,
            self.inter_chunk_timeout,
        );

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

/// Forward provider events to the consumer while advancing the chain id.
///
/// The chain (`previous_response_id`) must only advance for turns the consumer
/// actually received. We capture the candidate id from `ResponseDone`, deliver
/// the event FIRST, and commit the chain ONLY on a successful send. If the
/// consumer's `rx` was dropped (session gone), `tx.send` returns `Err`, we
/// break without writing the chain, and a stale id cannot outlive the session
/// (P3.7).
async fn forward_events_and_track_chain(
    mut provider_rx: mpsc::Receiver<AgentEvent>,
    tx: mpsc::Sender<AgentEvent>,
    previous_response_id: Arc<Mutex<Option<String>>>,
) {
    while let Some(event) = provider_rx.recv().await {
        let chain_update = match &event {
            AgentEvent::ResponseDone {
                response_id: Some(response_id),
            } if !response_id.is_empty() => Some(response_id.clone()),
            _ => None,
        };

        if tx.send(event).await.is_err() {
            break;
        }

        if let Some(response_id) = chain_update {
            let mut lock = previous_response_id.lock().await;
            *lock = Some(response_id);
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

fn get_env_non_empty(key: &str, label: &str) -> Result<String> {
    let value = env::var(key).with_context(|| format!("{label} is required. Set {key}."))?;
    let trimmed = value.trim();
    if trimmed.is_empty() {
        anyhow::bail!("{label} is required. Set {key}.");
    }
    Ok(trimmed.to_string())
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
        forward_events_and_track_chain, request_messages,
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
    fn tool_result_image_asset_adds_native_input_image_item() {
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
            default_model: "programmer".to_string(),
            use_previous_response_id: false,
            previous_response_id: Arc::new(Mutex::new(None)),
            initial_response_timeout: Duration::from_secs(1),
            inter_chunk_timeout: Duration::from_secs(1),
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
            default_model: "programmer".to_string(),
            use_previous_response_id: true,
            previous_response_id: Arc::clone(&stored_chain),
            initial_response_timeout: Duration::from_secs(1),
            inter_chunk_timeout: Duration::from_secs(1),
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
            default_model: "programmer".to_string(),
            use_previous_response_id: true,
            previous_response_id: Arc::clone(&stored_chain),
            initial_response_timeout: Duration::from_secs(1),
            inter_chunk_timeout: Duration::from_secs(1),
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
}
