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

use codescribe_core::agent::{
    AgentEvent, AgentProvider, ContentBlock, Message, Role, StreamOptions, ToolDefinition,
};
use codescribe_core::llm::responses_streaming_manager::{
    ResponsesStreamingManager, StreamCallbacks,
};

const DEFAULT_INITIAL_RESPONSE_TIMEOUT_MS: u64 = 30_000;
const DEFAULT_INTER_CHUNK_TIMEOUT_MS: u64 = 30_000;

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

        let previous_response_id = if self.use_previous_response_id {
            self.previous_response_id.lock().await.clone()
        } else {
            None
        };

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

        let mut provider_rx = manager.stream_agent(&request).await?;

        if !self.use_previous_response_id {
            return Ok(provider_rx);
        }

        let (tx, rx) = mpsc::channel(256);
        let previous_response_id = Arc::clone(&self.previous_response_id);

        tokio::spawn(async move {
            while let Some(event) = provider_rx.recv().await {
                if let AgentEvent::ResponseDone { response_id } = &event
                    && let Some(response_id) = response_id
                    && !response_id.is_empty()
                {
                    let mut lock = previous_response_id.lock().await;
                    *lock = Some(response_id.clone());
                }

                if tx.send(event).await.is_err() {
                    break;
                }
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

    fn name(&self) -> &str {
        "openai-responses"
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
                            "type": "input_text",
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
                    "type": "image",
                    "media_type": media_type,
                    "data": BASE64.encode(data)
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
    use super::{build_request_input_items, request_messages};
    use codescribe_core::agent::{ContentBlock, Message, Role};
    use serde_json::json;

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
}
