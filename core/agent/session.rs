use std::collections::HashMap;
use std::sync::Arc;

use anyhow::{Context, Result};
use chrono::Utc;
use tokio::sync::mpsc::Sender;
use tracing::{debug, warn};

use super::{
    AgentEvent, AgentProvider, AgentUiEvent, ContentBlock, Message, Role, StreamOptions,
    ToolRegistry, ToolResultContent,
};

const DEFAULT_MAX_ITERATIONS: usize = 25;

#[derive(Debug, Clone, PartialEq)]
pub struct ImageAttachment {
    pub data: Vec<u8>,
    pub media_type: String,
}

#[derive(Debug, Clone)]
struct PendingToolCall {
    id: String,
    name: String,
    args_buffer: String,
    arguments: Option<serde_json::Value>,
}

pub struct AgentSession {
    pub(crate) messages: Vec<Message>,
    pub(crate) provider: Box<dyn AgentProvider>,
    pub(crate) tools: Arc<ToolRegistry>,
    pub(crate) thread_id: Option<String>,
    pub(crate) max_iterations: usize,
    pub(crate) ui_tx: Sender<AgentUiEvent>,
}

impl AgentSession {
    pub fn new(
        provider: Box<dyn AgentProvider>,
        tools: Arc<ToolRegistry>,
        ui_tx: Sender<AgentUiEvent>,
    ) -> Self {
        Self {
            messages: Vec::new(),
            provider,
            tools,
            thread_id: None,
            max_iterations: DEFAULT_MAX_ITERATIONS,
            ui_tx,
        }
    }

    pub fn with_max_iterations(mut self, max_iterations: usize) -> Self {
        self.max_iterations = max_iterations.max(1);
        self
    }

    pub fn thread_id(&self) -> Option<&str> {
        self.thread_id.as_deref()
    }

    pub fn messages(&self) -> &[Message] {
        &self.messages
    }

    pub async fn send(
        &mut self,
        user_text: String,
        attachments: Vec<ImageAttachment>,
        options: &StreamOptions,
    ) -> Result<()> {
        let mut user_content = vec![ContentBlock::Text(user_text)];
        for attachment in attachments {
            user_content.push(
                self.provider
                    .build_image_block(&attachment.data, &attachment.media_type),
            );
        }
        self.messages.push(Message {
            role: Role::User,
            content: user_content,
            timestamp: Some(Utc::now()),
        });

        for iteration in 0..self.max_iterations {
            debug!(
                "Agent session iteration {}/{} (provider={})",
                iteration + 1,
                self.max_iterations,
                self.provider.name()
            );

            let tool_definitions = self.tools.definitions();
            let mut event_rx = self
                .provider
                .stream(&self.messages, &tool_definitions, options)
                .await
                .with_context(|| format!("Failed to start '{}' streaming", self.provider.name()))?;

            let mut assistant_text = String::new();
            let mut reasoning_text = String::new();
            let mut text_done_seen = false;

            let mut pending_calls: HashMap<String, PendingToolCall> = HashMap::new();
            let mut tool_call_order: Vec<String> = Vec::new();

            while let Some(event) = event_rx.recv().await {
                match event {
                    AgentEvent::TextDelta(delta) => {
                        assistant_text.push_str(&delta);
                        send_ui_event(&self.ui_tx, AgentUiEvent::TextDelta(delta)).await;
                    }
                    AgentEvent::TextDone(text) => {
                        text_done_seen = true;
                        assistant_text = text.clone();
                        send_ui_event(&self.ui_tx, AgentUiEvent::TextDone(text)).await;
                    }
                    AgentEvent::ReasoningDelta(delta) => {
                        reasoning_text.push_str(&delta);
                        send_ui_event(&self.ui_tx, AgentUiEvent::ReasoningDelta(delta)).await;
                    }
                    AgentEvent::ToolCallStart { id, name } => {
                        if !tool_call_order.iter().any(|existing| existing == &id) {
                            tool_call_order.push(id.clone());
                        }
                        pending_calls.entry(id.clone()).or_insert(PendingToolCall {
                            id,
                            name,
                            args_buffer: String::new(),
                            arguments: None,
                        });
                    }
                    AgentEvent::ToolCallArgsDelta { id, delta } => {
                        if !tool_call_order.iter().any(|existing| existing == &id) {
                            tool_call_order.push(id.clone());
                        }
                        let entry = pending_calls.entry(id.clone()).or_insert(PendingToolCall {
                            id,
                            name: "unknown_tool".to_string(),
                            args_buffer: String::new(),
                            arguments: None,
                        });
                        entry.args_buffer.push_str(&delta);
                    }
                    AgentEvent::ToolCallReady {
                        id,
                        name,
                        arguments,
                    } => {
                        if !tool_call_order.iter().any(|existing| existing == &id) {
                            tool_call_order.push(id.clone());
                        }
                        let entry = pending_calls.entry(id.clone()).or_insert(PendingToolCall {
                            id,
                            name,
                            args_buffer: String::new(),
                            arguments: None,
                        });
                        entry.arguments = Some(arguments);
                    }
                    AgentEvent::ResponseDone { response_id } => {
                        self.thread_id = response_id;
                    }
                    AgentEvent::Error(message) => {
                        send_ui_event(&self.ui_tx, AgentUiEvent::Error(message.clone())).await;
                        return Err(anyhow::anyhow!("Provider stream error: {message}"));
                    }
                }
            }

            if !reasoning_text.trim().is_empty() {
                debug!(
                    "Reasoning trace captured: {} chars (provider={})",
                    reasoning_text.len(),
                    self.provider.name()
                );
            }

            let assistant_text = assistant_text.trim().to_string();
            if !assistant_text.is_empty() {
                self.messages.push(Message {
                    role: Role::Assistant,
                    content: vec![ContentBlock::Text(assistant_text.clone())],
                    timestamp: Some(Utc::now()),
                });
                if !text_done_seen {
                    send_ui_event(&self.ui_tx, AgentUiEvent::TextDone(assistant_text)).await;
                }
            }

            let mut ready_calls: Vec<(String, String, serde_json::Value)> = Vec::new();
            for call_id in tool_call_order {
                let Some(call) = pending_calls.remove(&call_id) else {
                    continue;
                };

                if let Some(arguments) = call.arguments {
                    ready_calls.push((call.id, call.name, arguments));
                    continue;
                }

                let buffered = call.args_buffer.trim();
                if buffered.is_empty() {
                    continue;
                }

                match serde_json::from_str::<serde_json::Value>(buffered) {
                    Ok(arguments) => ready_calls.push((call.id, call.name, arguments)),
                    Err(error) => {
                        send_ui_event(
                            &self.ui_tx,
                            AgentUiEvent::Error(format!(
                                "Failed to parse tool arguments for '{}': {}",
                                call.name, error
                            )),
                        )
                        .await;
                        return Err(anyhow::anyhow!(
                            "Failed to parse tool arguments for '{}': {}",
                            call.name,
                            error
                        ));
                    }
                }
            }

            if ready_calls.is_empty() {
                send_ui_event(&self.ui_tx, AgentUiEvent::Done).await;
                return Ok(());
            }

            let tool_use_blocks = ready_calls
                .iter()
                .map(|(id, name, arguments)| ContentBlock::ToolUse {
                    id: id.clone(),
                    name: name.clone(),
                    input: arguments.clone(),
                })
                .collect::<Vec<_>>();
            self.messages.push(Message {
                role: Role::Assistant,
                content: tool_use_blocks,
                timestamp: Some(Utc::now()),
            });

            for (call_id, tool_name, arguments) in ready_calls {
                send_ui_event(
                    &self.ui_tx,
                    AgentUiEvent::ToolExecuting {
                        name: tool_name.clone(),
                        id: call_id.clone(),
                    },
                )
                .await;

                let tool_outputs = match self.tools.dispatch(&tool_name, arguments).await {
                    Ok(outputs) => outputs,
                    Err(error) => {
                        warn!(
                            "Tool '{}' execution failed for call {}: {}",
                            tool_name, call_id, error
                        );
                        vec![ToolResultContent::Error(error.to_string())]
                    }
                };

                let summary = summarize_tool_result(&tool_outputs);
                let is_error = tool_outputs
                    .iter()
                    .any(|output| matches!(output, ToolResultContent::Error(_)));

                let mut content_blocks = Vec::new();
                for output in tool_outputs {
                    match output {
                        ToolResultContent::Text(text) => {
                            content_blocks.push(ContentBlock::Text(text))
                        }
                        ToolResultContent::Image { data, media_type } => {
                            content_blocks.push(self.provider.build_image_block(&data, &media_type))
                        }
                        ToolResultContent::Error(message) => {
                            content_blocks.push(ContentBlock::Text(message))
                        }
                    }
                }

                let result_message =
                    self.provider
                        .build_tool_result(&call_id, content_blocks, is_error);
                self.messages.push(result_message);

                send_ui_event(
                    &self.ui_tx,
                    AgentUiEvent::ToolResult {
                        name: tool_name,
                        id: call_id,
                        summary,
                    },
                )
                .await;
            }
        }

        let message = format!(
            "Agent loop exceeded max iterations ({})",
            self.max_iterations
        );
        send_ui_event(&self.ui_tx, AgentUiEvent::Error(message.clone())).await;
        Err(anyhow::anyhow!(message))
    }
}

async fn send_ui_event(tx: &Sender<AgentUiEvent>, event: AgentUiEvent) {
    if tx.send(event).await.is_err() {
        debug!("Dropping UI event because receiver is closed");
    }
}

fn summarize_tool_result(outputs: &[ToolResultContent]) -> String {
    const SUMMARY_MAX_CHARS: usize = 120;

    let mut first_text: Option<String> = None;
    let mut image_count = 0usize;
    let mut error_count = 0usize;

    for output in outputs {
        match output {
            ToolResultContent::Text(text) => {
                if first_text.is_none() {
                    first_text = Some(text.trim().to_string());
                }
            }
            ToolResultContent::Image { .. } => image_count += 1,
            ToolResultContent::Error(_) => error_count += 1,
        }
    }

    if let Some(text) = first_text {
        if text.is_empty() {
            return "Empty tool output".to_string();
        }
        return truncate_summary(&text, SUMMARY_MAX_CHARS);
    }

    if image_count > 0 {
        return format!("{image_count} image result(s)");
    }

    if error_count > 0 {
        return format!("{error_count} error result(s)");
    }

    "No tool output".to_string()
}

fn truncate_summary(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let mut truncated = text
        .chars()
        .take(max_chars.saturating_sub(1))
        .collect::<String>();
    truncated.push('…');
    truncated
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use async_trait::async_trait;
    use serde_json::json;
    use tokio::sync::mpsc;

    use crate::agent::{
        AgentEvent, AgentProvider, AgentSession, ContentBlock, Message, Role, StreamOptions,
        ToolDefinition, ToolRegistry, ToolResultContent,
    };

    struct LoopingProvider;

    #[async_trait]
    impl AgentProvider for LoopingProvider {
        async fn stream(
            &self,
            _messages: &[Message],
            _tools: &[ToolDefinition],
            _options: &StreamOptions,
        ) -> anyhow::Result<mpsc::Receiver<AgentEvent>> {
            let (tx, rx) = mpsc::channel(8);
            tx.send(AgentEvent::ToolCallReady {
                id: "call_loop".to_string(),
                name: "loop_tool".to_string(),
                arguments: json!({"count": 1}),
            })
            .await
            .expect("test stream channel should accept tool call");
            tx.send(AgentEvent::ResponseDone {
                response_id: Some("resp_loop".to_string()),
            })
            .await
            .expect("test stream channel should accept completion event");
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
            "looping-provider"
        }
    }

    #[tokio::test]
    async fn stops_when_iteration_limit_is_reached() {
        let mut registry = ToolRegistry::new();
        registry
            .register(
                ToolDefinition {
                    name: "loop_tool".to_string(),
                    description: "Always emits output".to_string(),
                    input_schema: json!({"type": "object"}),
                },
                Box::new(|_input| {
                    Box::pin(async { vec![ToolResultContent::Text("still looping".to_string())] })
                }),
            )
            .expect("tool registration should succeed");

        let (ui_tx, mut _ui_rx) = mpsc::channel(16);
        let mut session = AgentSession::new(Box::new(LoopingProvider), Arc::new(registry), ui_tx)
            .with_max_iterations(2);

        let result = session
            .send(
                "hello".to_string(),
                Vec::new(),
                &StreamOptions {
                    model: "gpt-test".to_string(),
                    system_prompt: None,
                    max_tokens: None,
                    temperature: None,
                },
            )
            .await;

        let error = result.expect_err("session should stop at max iteration limit");
        assert!(
            error.to_string().contains("max iterations"),
            "expected max iteration error, got: {}",
            error
        );
    }
}
