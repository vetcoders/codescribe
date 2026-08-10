use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use chrono::Utc;
use tokio::sync::mpsc::{Receiver, Sender};
use tracing::{debug, info, warn};

use super::tool_output::ToolOutputStore;
use super::{
    AgentAssetStore, AgentEvent, AgentProvider, AgentUiEvent, ContentBlock, Message, Role,
    StreamOptions, ToolApprovalRequest, ToolDecision, ToolDefinition, ToolOrigin, ToolRegistry,
    ToolResultContent,
};

const DEFAULT_MAX_ITERATIONS: usize = 25;
const AGENT_STREAM_START_RETRY_MAX_ATTEMPTS: usize = 2;
const AGENT_STREAM_START_RETRY_DELAY: Duration = Duration::from_millis(250);
const TOOL_APPROVAL_TIMEOUT: Duration = Duration::from_secs(90);

pub type ToolApprovalFuture = Pin<Box<dyn Future<Output = bool> + Send>>;
pub type ToolApprovalHandler = Arc<dyn Fn(ToolApprovalRequest) -> ToolApprovalFuture + Send + Sync>;

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
    tool_output_store: ToolOutputStore,
    approval_handler: Option<ToolApprovalHandler>,
    approval_timeout: Duration,
    execution_session_id: String,
    execution_thread_id: String,
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
            tool_output_store: ToolOutputStore::new(),
            approval_handler: None,
            approval_timeout: TOOL_APPROVAL_TIMEOUT,
            execution_session_id: uuid::Uuid::new_v4().to_string(),
            execution_thread_id: "unbound".to_string(),
        }
    }

    pub fn with_tool_approval(
        mut self,
        thread_id: impl Into<String>,
        handler: ToolApprovalHandler,
    ) -> Self {
        self.execution_thread_id = thread_id.into();
        self.approval_handler = Some(handler);
        self
    }

    pub fn with_max_iterations(mut self, max_iterations: usize) -> Self {
        self.max_iterations = max_iterations.max(1);
        self
    }

    pub fn thread_id(&self) -> Option<&str> {
        self.thread_id.as_deref()
    }

    /// Bind the durable thread identity used for approval requests and
    /// thread-context tool dispatch. The voice lane has no approval handler, so
    /// this is its only way to attach the ThreadStore id to tool execution.
    pub fn bind_execution_thread(&mut self, thread_id: impl Into<String>) {
        self.execution_thread_id = thread_id.into();
    }

    pub fn messages(&self) -> &[Message] {
        &self.messages
    }

    pub fn restore_messages(&mut self, mut messages: Vec<Message>) {
        if let Err(error) = self.tool_output_store.spill_messages(&mut messages) {
            warn!(%error, "Failed to normalize rehydrated tool-output history");
        }
        self.messages = messages;
        self.thread_id = None;
    }

    #[cfg(test)]
    fn with_tool_output_store(mut self, tool_output_store: ToolOutputStore) -> Self {
        self.tool_output_store = tool_output_store;
        self
    }

    async fn stream_with_retry(
        &self,
        tool_definitions: &[ToolDefinition],
        options: &StreamOptions,
    ) -> Result<Receiver<AgentEvent>> {
        let mut attempt = 1usize;
        loop {
            if let Some((initial_timeout, inter_chunk_timeout)) = self.provider.stream_timeouts() {
                info!(
                    "Agent send attempt {}/{} (provider={}, timeout={}s, inter_chunk_timeout={}s)",
                    attempt,
                    AGENT_STREAM_START_RETRY_MAX_ATTEMPTS,
                    self.provider.name(),
                    initial_timeout.as_secs(),
                    inter_chunk_timeout.as_secs()
                );
            } else {
                info!(
                    "Agent send attempt {}/{} (provider={}, timeout=unknown)",
                    attempt,
                    AGENT_STREAM_START_RETRY_MAX_ATTEMPTS,
                    self.provider.name()
                );
            }

            // Operator's spec 2026-05-26 (4th iteration of same architectural
            // insight): retry attempts must NOT resend prior context. Each retry
            // pass after the first signals provider to clear any stored chain
            // (previous_response_id) BEFORE building the request — fresh start,
            // no context bloat from the failed prior attempt.
            let attempt_options: StreamOptions = if attempt > 1 {
                let mut opts = options.clone();
                opts.reset_chain = true;
                opts
            } else {
                options.clone()
            };

            match self
                .provider
                .stream(&self.messages, tool_definitions, &attempt_options)
                .await
            {
                Ok(rx) => return Ok(rx),
                Err(error) => {
                    let is_transient = is_transient_stream_start_error(&error);
                    if is_transient && attempt < AGENT_STREAM_START_RETRY_MAX_ATTEMPTS {
                        warn!(
                            "Agent stream start failed (provider={}, attempt={}/{}): {}. Retrying in {:?} (next attempt will reset chain)",
                            self.provider.name(),
                            attempt,
                            AGENT_STREAM_START_RETRY_MAX_ATTEMPTS,
                            error,
                            AGENT_STREAM_START_RETRY_DELAY
                        );
                        tokio::time::sleep(AGENT_STREAM_START_RETRY_DELAY).await;
                        attempt += 1;
                        continue;
                    }
                    return Err(error);
                }
            }
        }
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
                .stream_with_retry(&tool_definitions, options)
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
                    AgentEvent::ResponseDone { response_id, clean } => {
                        // Only adopt the provider thread id on a clean terminal.
                        // A dirty terminal (EOF/timeout, failed/incomplete) must
                        // not persist a poisoned chain id; clearing it forces the
                        // next turn to full-replay from local history (P1.6).
                        if clean {
                            self.thread_id = response_id;
                        } else {
                            if let Some(id) = response_id {
                                warn!(
                                    "Agent dirty terminal: discarding response id {} and resetting chain (provider={})",
                                    id,
                                    self.provider.name()
                                );
                            }
                            self.thread_id = None;
                        }
                    }
                    AgentEvent::Error(message) => {
                        // P2.13: reset the chain BEFORE returning. A provider
                        // Error (e.g. a failed/incomplete/cancelled terminal
                        // mapped to Error) must never leave `thread_id` pointing
                        // at a poisoned response. The parser also emits a dirty
                        // `ResponseDone { clean: false }` ahead of this Error so
                        // both the session chain (here) and the provider chain
                        // (`previous_response_id`) reset through the P1.6 path;
                        // clearing here too is the belt-and-suspenders guard that
                        // stays correct even if a provider surfaces an Error
                        // without a preceding dirty terminal.
                        if self.thread_id.is_some() {
                            warn!(
                                "Agent provider error: resetting chain (clearing thread_id) before returning (provider={})",
                                self.provider.name()
                            );
                            self.thread_id = None;
                        }
                        warn!(
                            "Agent provider stream error (provider={}): {}",
                            self.provider.name(),
                            message
                        );
                        // Fatal agent failures use the Result channel only.
                        // Swift already turns the thrown bridge error into the
                        // visible failed assistant bubble; also emitting
                        // AgentUiEvent::Error would double-signal the same
                        // failure through listener.on_error + throw.
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
                let decision = self.tools.decide(
                    &tool_name,
                    &arguments,
                    &call_id,
                    &self.execution_session_id,
                    &self.execution_thread_id,
                );
                let (authorization_error, approved_by_user, audit_decision) = match decision {
                    ToolDecision::Allow => (None, false, "allowed"),
                    ToolDecision::Deny(reason) => (Some(reason), false, "denied"),
                    ToolDecision::RequireApproval(request) => {
                        let request = *request;
                        let approval = self
                            .approval_handler
                            .as_ref()
                            .map(|handler| handler(request.clone()));
                        send_ui_event(
                            &self.ui_tx,
                            AgentUiEvent::ToolApprovalRequested(request.clone()),
                        )
                        .await;
                        match approval {
                            Some(approval) => {
                                match tokio::time::timeout(self.approval_timeout, approval).await {
                                    Ok(true) => (None, true, "approved_once"),
                                    Ok(false) => (
                                        Some("Tool call rejected by the user".to_string()),
                                        false,
                                        "rejected",
                                    ),
                                    Err(_) => (
                                        Some("Tool approval timed out".to_string()),
                                        false,
                                        "timeout",
                                    ),
                                }
                            }
                            None => (
                                Some(
                                    "Tool requires approval, but no approval UI is connected"
                                        .to_string(),
                                ),
                                false,
                                "denied",
                            ),
                        }
                    }
                };

                let dispatch_started = Instant::now();
                let tool_outputs = if let Some(reason) = authorization_error {
                    vec![ToolResultContent::Error(reason)]
                } else {
                    send_ui_event(
                        &self.ui_tx,
                        AgentUiEvent::ToolExecuting {
                            name: tool_name.clone(),
                            id: call_id.clone(),
                        },
                    )
                    .await;
                    match self
                        .tools
                        .dispatch_in_thread(&tool_name, arguments, &self.execution_thread_id)
                        .await
                    {
                        Ok(outputs) => outputs,
                        Err(error) => {
                            warn!(
                                "Tool '{}' execution failed for call {}: {}",
                                tool_name, call_id, error
                            );
                            vec![ToolResultContent::Error(error.to_string())]
                        }
                    }
                };

                let summary = summarize_tool_result(&tool_outputs);
                let is_error = tool_outputs
                    .iter()
                    .any(|output| matches!(output, ToolResultContent::Error(_)));

                // Per-call observability: one INFO line per agent tool call (native
                // or MCP) so failures are diagnosable from codescribe.log after the
                // fact. Never logs arguments or full payloads — only the tool name,
                // duration, outcome, and (on error) the error's first line. For MCP
                // tools the server segment of the public name is surfaced.
                let dispatch_ms = dispatch_started.elapsed().as_millis();
                let policy = self.tools.policy(&tool_name);
                let origin = policy
                    .map(|policy| match &policy.origin {
                        ToolOrigin::Native => "native".to_string(),
                        ToolOrigin::Mcp { server, .. } => format!("mcp:{server}"),
                    })
                    .unwrap_or_else(|| "unregistered".to_string());
                let risk = policy
                    .map(|policy| policy.risk.as_str())
                    .unwrap_or("unknown");
                info!(
                    call_id = %call_id,
                    thread_id = %self.execution_thread_id,
                    session_id = %self.execution_session_id,
                    tool = %tool_name,
                    origin = %origin,
                    risk,
                    decision = audit_decision,
                    approved_by_user,
                    outcome = if is_error { "error" } else { "ok" },
                    duration_ms = dispatch_ms,
                    "agent_tool_audit"
                );
                let server_label = mcp_server_name(&tool_name)
                    .map(|server| format!(" (mcp server `{server}`)"))
                    .unwrap_or_default();
                if is_error {
                    info!(
                        "agent tool `{tool_name}`{server_label} failed in {dispatch_ms}ms: {}",
                        first_error_line(&tool_outputs)
                    );
                } else {
                    info!("agent tool `{tool_name}`{server_label} ok in {dispatch_ms}ms");
                }

                let mut content_blocks = Vec::new();
                for output in tool_outputs {
                    match output {
                        ToolResultContent::Text(text) => {
                            content_blocks.push(ContentBlock::Text(text))
                        }
                        ToolResultContent::Image { data, media_type } => {
                            match AgentAssetStore::save_image(&data, &media_type) {
                                Ok(asset) => content_blocks.push(ContentBlock::ImageAsset(asset)),
                                Err(error) => content_blocks.push(ContentBlock::Text(format!(
                                    "Image result could not be stored as an asset: {error}"
                                ))),
                            }
                        }
                        ToolResultContent::ImageAsset(asset) => {
                            content_blocks.push(ContentBlock::ImageAsset(asset))
                        }
                        ToolResultContent::Error(message) => {
                            content_blocks.push(ContentBlock::Text(message))
                        }
                    }
                }

                if let Err(error) = self.tool_output_store.spill_content(&mut content_blocks) {
                    warn!(
                        tool = %tool_name,
                        call_id = %call_id,
                        %error,
                        "Failed to persist oversized tool output; omitted it from context"
                    );
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
                        is_error,
                    },
                )
                .await;
            }
        }

        let message = format!(
            "Agent loop exceeded max iterations ({})",
            self.max_iterations
        );
        Err(anyhow::anyhow!(message))
    }
}

async fn send_ui_event(tx: &Sender<AgentUiEvent>, event: AgentUiEvent) {
    if tx.send(event).await.is_err() {
        debug!("Dropping UI event because receiver is closed");
        return;
    }

    // Let the controller's select! drain UI events between immediately-ready
    // provider chunks, preserving live rendering instead of end-of-stream dumps.
    tokio::task::yield_now().await;
}

fn is_transient_stream_start_error(error: &anyhow::Error) -> bool {
    let message = error.to_string().to_lowercase();
    [
        "timeout",
        "timed out",
        "connection reset",
        "connection refused",
        "temporarily unavailable",
        "temporary failure",
        "broken pipe",
        "eof",
        "transport",
        "rate limit",
        "429",
        "502",
        "503",
    ]
    .iter()
    .any(|pattern| message.contains(pattern))
}

/// Extract the MCP server segment from a public tool name shaped
/// `mcp__<server>__<tool>`. Native tools return `None`.
fn mcp_server_name(tool_name: &str) -> Option<&str> {
    tool_name
        .strip_prefix("mcp__")
        .and_then(|rest| rest.split_once("__"))
        .map(|(server, _)| server)
}

/// First line of the first error result, trimmed and truncated for a single log
/// line. Never includes tool arguments — only the message the tool produced.
fn first_error_line(outputs: &[ToolResultContent]) -> String {
    outputs
        .iter()
        .find_map(|output| match output {
            ToolResultContent::Error(message) => Some(message),
            _ => None,
        })
        .map(|message| {
            let line = message.lines().next().unwrap_or("").trim();
            truncate_summary(&redact_tool_summary(line), 200)
        })
        .unwrap_or_default()
}

fn summarize_tool_result(outputs: &[ToolResultContent]) -> String {
    const SUMMARY_MAX_CHARS: usize = 120;

    let mut first_text: Option<String> = None;
    let mut first_error: Option<String> = None;
    let mut image_count = 0usize;
    let mut error_count = 0usize;

    for output in outputs {
        match output {
            ToolResultContent::Text(text) => {
                if first_text.is_none() {
                    first_text = Some(text.trim().to_string());
                }
            }
            ToolResultContent::Image { .. } | ToolResultContent::ImageAsset(_) => image_count += 1,
            ToolResultContent::Error(message) => {
                error_count += 1;
                if first_error.is_none() {
                    first_error = Some(message.trim().to_string());
                }
            }
        }
    }

    if let Some(text) = first_text {
        if text.is_empty() {
            return "Empty tool output".to_string();
        }
        return truncate_summary(&redact_tool_summary(&text), SUMMARY_MAX_CHARS);
    }

    if image_count > 0 {
        return format!("{image_count} image result(s)");
    }

    // Surface the real failure reason (e.g. "empty index") so the grouped Tool
    // Activity block can show it compactly. Fall back to a count when the error
    // carries no message. The full payload still goes to the debug log.
    if let Some(error) = first_error {
        if error.is_empty() {
            return format!("{error_count} error result(s)");
        }
        return truncate_summary(&redact_tool_summary(&error), SUMMARY_MAX_CHARS);
    }

    "No tool output".to_string()
}

fn redact_tool_summary(text: &str) -> String {
    if let Ok(mut value) = serde_json::from_str::<serde_json::Value>(text) {
        redact_json_value(&mut value);
        return serde_json::to_string(&value)
            .unwrap_or_else(|_| "[redacted tool output]".to_string());
    }

    if text
        .split_whitespace()
        .map(|token| {
            token.trim_matches(|character| matches!(character, '\'' | '"' | ',' | ';' | '(' | ')'))
        })
        .any(|token| {
            token.split_once('=').map_or_else(
                || contains_sensitive_key(token),
                |(key, _)| contains_sensitive_key(key),
            )
        })
    {
        return "[redacted sensitive tool output]".to_string();
    }

    text.to_string()
}

fn redact_json_value(value: &mut serde_json::Value) {
    match value {
        serde_json::Value::Object(object) => {
            for (key, value) in object {
                if contains_sensitive_key(key) {
                    *value = serde_json::Value::String("[REDACTED]".to_string());
                } else {
                    redact_json_value(value);
                }
            }
        }
        serde_json::Value::Array(values) => {
            for value in values {
                redact_json_value(value);
            }
        }
        _ => {}
    }
}

fn contains_sensitive_key(key: &str) -> bool {
    let normalized = key
        .chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .collect::<String>()
        .to_ascii_lowercase();
    [
        "token",
        "password",
        "passwd",
        "secret",
        "apikey",
        "authorization",
        "cookie",
        "environment",
    ]
    .iter()
    .any(|sensitive| normalized.contains(sensitive))
}

fn truncate_summary(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let mut truncated = text
        .chars()
        .take(max_chars.saturating_sub(3))
        .collect::<String>();
    truncated.push_str("...");
    truncated
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::sync::Arc;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::time::Duration;

    use async_trait::async_trait;
    use serde_json::json;
    use tokio::sync::{mpsc, oneshot};

    use crate::agent::tool_output::ToolOutputStore;
    use crate::agent::{
        AgentEvent, AgentProvider, AgentSession, AgentUiEvent, ContentBlock, Message, Role,
        StreamOptions, ToolDefinition, ToolExecutionPolicy, ToolOrigin, ToolRegistry,
        ToolResultContent, ToolRisk,
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
                clean: true,
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

    struct ScriptedProvider {
        scripted_events: Mutex<VecDeque<Vec<AgentEvent>>>,
    }

    impl ScriptedProvider {
        fn new(scripted_events: Vec<Vec<AgentEvent>>) -> Self {
            Self {
                scripted_events: Mutex::new(scripted_events.into()),
            }
        }
    }

    #[async_trait]
    impl AgentProvider for ScriptedProvider {
        async fn stream(
            &self,
            _messages: &[Message],
            _tools: &[ToolDefinition],
            _options: &StreamOptions,
        ) -> anyhow::Result<mpsc::Receiver<AgentEvent>> {
            let events = self
                .scripted_events
                .lock()
                .expect("script lock should not be poisoned")
                .pop_front()
                .unwrap_or_default();

            let (tx, rx) = mpsc::channel(16);
            for event in events {
                tx.send(event)
                    .await
                    .expect("test stream channel should accept scripted event");
            }
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
            "scripted-provider"
        }
    }

    struct RetryThenSuccessProvider {
        attempts: Arc<AtomicUsize>,
    }

    #[async_trait]
    impl AgentProvider for RetryThenSuccessProvider {
        async fn stream(
            &self,
            _messages: &[Message],
            _tools: &[ToolDefinition],
            _options: &StreamOptions,
        ) -> anyhow::Result<mpsc::Receiver<AgentEvent>> {
            let current_attempt = self.attempts.fetch_add(1, Ordering::SeqCst);
            if current_attempt == 0 {
                return Err(anyhow::anyhow!("timed out while connecting to upstream"));
            }

            let (tx, rx) = mpsc::channel(8);
            tx.send(AgentEvent::TextDone("Recovered response".to_string()))
                .await
                .expect("test stream channel should accept completion text");
            tx.send(AgentEvent::ResponseDone {
                response_id: Some("resp_retry_success".to_string()),
                clean: true,
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
            "retry-then-success-provider"
        }
    }

    struct PermanentFailureProvider {
        attempts: Arc<AtomicUsize>,
    }

    #[async_trait]
    impl AgentProvider for PermanentFailureProvider {
        async fn stream(
            &self,
            _messages: &[Message],
            _tools: &[ToolDefinition],
            _options: &StreamOptions,
        ) -> anyhow::Result<mpsc::Receiver<AgentEvent>> {
            self.attempts.fetch_add(1, Ordering::SeqCst);
            Err(anyhow::anyhow!("authentication failed"))
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
            "permanent-failure-provider"
        }
    }

    #[test]
    fn restore_messages_seeds_history_and_clears_provider_thread_id() {
        let (ui_tx, _ui_rx) = mpsc::channel(4);
        let mut session = AgentSession::new(
            Box::new(ScriptedProvider::new(Vec::new())),
            Arc::new(ToolRegistry::new()),
            ui_tx,
        );
        session.thread_id = Some("resp_old".to_string());

        let restored = vec![
            Message::new(Role::User, vec![ContentBlock::Text("First".to_string())]),
            Message::new(
                Role::Assistant,
                vec![ContentBlock::Text("Second".to_string())],
            ),
        ];
        session.restore_messages(restored.clone());

        assert_eq!(session.messages(), restored.as_slice());
        assert_eq!(session.thread_id(), None);
    }

    #[test]
    fn restore_messages_keeps_stored_tool_reference_without_reinflating() {
        let temp = tempfile::tempdir().expect("temp dir");
        let root = temp.path().join("tool_outputs");
        let reference = format!(
            "[tool output stored: {}/tool-output-existing.txt (90000 bytes)]",
            root.display()
        );
        let (ui_tx, _ui_rx) = mpsc::channel(4);
        let mut session = AgentSession::new(
            Box::new(ScriptedProvider::new(Vec::new())),
            Arc::new(ToolRegistry::new()),
            ui_tx,
        )
        .with_tool_output_store(ToolOutputStore::new_in(root.clone(), 1024));

        session.restore_messages(vec![Message::new(
            Role::User,
            vec![ContentBlock::ToolResult {
                tool_use_id: "call_existing".to_string(),
                content: vec![ContentBlock::Text(reference.clone())],
                is_error: false,
            }],
        )]);

        let restored = &session.messages()[0].content[0];
        assert!(matches!(
            restored,
            ContentBlock::ToolResult { content, .. }
                if content == &vec![ContentBlock::Text(reference)]
        ));
        assert!(
            !root.exists(),
            "rehydration must not rewrite stored references"
        );
        assert_eq!(session.thread_id(), None);
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
                    reset_chain: false,
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

    #[tokio::test]
    async fn send_completes_successfully_without_tool_calls() {
        let provider = ScriptedProvider::new(vec![vec![
            AgentEvent::TextDelta("Hello ".to_string()),
            AgentEvent::TextDone("Hello from agent".to_string()),
            AgentEvent::ResponseDone {
                response_id: Some("resp_success_1".to_string()),
                clean: true,
            },
        ]]);
        let (ui_tx, mut ui_rx) = mpsc::channel(16);
        let mut session =
            AgentSession::new(Box::new(provider), Arc::new(ToolRegistry::new()), ui_tx);

        session
            .send(
                "status update".to_string(),
                Vec::new(),
                &StreamOptions {
                    model: "gpt-test".to_string(),
                    system_prompt: None,
                    max_tokens: None,
                    temperature: None,
                    reset_chain: false,
                },
            )
            .await
            .expect("agent session should complete");

        assert_eq!(session.thread_id(), Some("resp_success_1"));
        assert_eq!(session.messages().len(), 2);

        let assistant = &session.messages()[1];
        assert_eq!(assistant.role, Role::Assistant);
        assert_eq!(
            assistant.content,
            vec![ContentBlock::Text("Hello from agent".to_string())]
        );

        let mut ui_events = Vec::new();
        while let Ok(event) = ui_rx.try_recv() {
            ui_events.push(event);
        }
        assert!(
            ui_events.contains(&AgentUiEvent::TextDone("Hello from agent".to_string())),
            "expected TextDone event, got {ui_events:?}"
        );
        assert!(
            ui_events.contains(&AgentUiEvent::Done),
            "expected Done event, got {ui_events:?}"
        );
    }

    #[tokio::test]
    async fn send_yields_after_text_delta_before_finishing_buffered_stream() {
        let provider = ScriptedProvider::new(vec![vec![
            AgentEvent::TextDelta("Hel".to_string()),
            AgentEvent::TextDelta("lo".to_string()),
            AgentEvent::TextDone("Hello".to_string()),
            AgentEvent::ResponseDone {
                response_id: Some("resp_buffered".to_string()),
                clean: true,
            },
        ]]);
        let (ui_tx, mut ui_rx) = mpsc::channel(16);
        let mut session =
            AgentSession::new(Box::new(provider), Arc::new(ToolRegistry::new()), ui_tx);

        let options = StreamOptions {
            model: "gpt-test".to_string(),
            system_prompt: None,
            max_tokens: None,
            temperature: None,
            reset_chain: false,
        };
        let send_future = session.send("buffered stream".to_string(), Vec::new(), &options);
        tokio::pin!(send_future);

        tokio::select! {
            biased;
            result = &mut send_future => {
                panic!("send completed before UI could drain first delta: {result:?}");
            }
            maybe_event = ui_rx.recv() => {
                assert_eq!(maybe_event, Some(AgentUiEvent::TextDelta("Hel".to_string())));
            }
        }

        send_future
            .await
            .expect("agent session should complete after yielding first delta");
    }

    #[tokio::test]
    async fn send_executes_buffered_tool_call_and_handles_tool_failure_fallback() {
        let provider = ScriptedProvider::new(vec![
            vec![
                AgentEvent::ToolCallStart {
                    id: "call_missing".to_string(),
                    name: "missing_tool".to_string(),
                },
                AgentEvent::ToolCallArgsDelta {
                    id: "call_missing".to_string(),
                    delta: "{\"animal\":\"cat\"}".to_string(),
                },
                AgentEvent::ResponseDone {
                    response_id: Some("resp_after_tool".to_string()),
                    clean: true,
                },
            ],
            vec![
                AgentEvent::TextDone("Recovered after tool fallback".to_string()),
                AgentEvent::ResponseDone {
                    response_id: Some("resp_final".to_string()),
                    clean: true,
                },
            ],
        ]);

        let (ui_tx, mut ui_rx) = mpsc::channel(32);
        let mut session =
            AgentSession::new(Box::new(provider), Arc::new(ToolRegistry::new()), ui_tx)
                .with_max_iterations(3);

        session
            .send(
                "run missing tool".to_string(),
                Vec::new(),
                &StreamOptions {
                    model: "gpt-test".to_string(),
                    system_prompt: None,
                    max_tokens: None,
                    temperature: None,
                    reset_chain: false,
                },
            )
            .await
            .expect("agent session should recover from missing tool dispatch");

        assert_eq!(session.thread_id(), Some("resp_final"));
        assert_eq!(session.messages().len(), 4);

        let tool_use = session
            .messages()
            .iter()
            .flat_map(|message| message.content.iter())
            .find_map(|block| match block {
                ContentBlock::ToolUse { id, name, input } => {
                    Some((id.clone(), name.clone(), input.clone()))
                }
                _ => None,
            })
            .expect("tool_use block should be persisted");
        assert_eq!(tool_use.0, "call_missing");
        assert_eq!(tool_use.1, "missing_tool");
        assert_eq!(tool_use.2, json!({"animal":"cat"}));

        let tool_result = session
            .messages()
            .iter()
            .flat_map(|message| message.content.iter())
            .find_map(|block| match block {
                ContentBlock::ToolResult {
                    tool_use_id,
                    content,
                    is_error,
                } => Some((tool_use_id.clone(), content.clone(), *is_error)),
                _ => None,
            })
            .expect("tool_result block should be persisted");
        assert_eq!(tool_result.0, "call_missing");
        assert!(
            tool_result.2,
            "missing tool dispatch should emit error tool result"
        );
        assert!(
            tool_result.1.iter().any(
                |value| matches!(value, ContentBlock::Text(text) if text.contains("not registered"))
            ),
            "expected missing tool error text, got {:?}",
            tool_result.1
        );

        let mut ui_events = Vec::new();
        while let Ok(event) = ui_rx.try_recv() {
            ui_events.push(event);
        }
        assert!(
            ui_events
                .iter()
                .all(|event| !matches!(event, AgentUiEvent::ToolExecuting { .. })),
            "denied/unregistered tools must not emit ToolExecuting: {ui_events:?}"
        );
        let tool_result_event = ui_events
            .iter()
            .find_map(|event| match event {
                AgentUiEvent::ToolResult {
                    name,
                    id,
                    summary,
                    is_error,
                } => Some((name.clone(), id.clone(), summary.clone(), *is_error)),
                _ => None,
            })
            .expect("expected a ToolResult UI event");
        assert_eq!(tool_result_event.0, "missing_tool");
        assert_eq!(tool_result_event.1, "call_missing");
        assert!(
            tool_result_event.3,
            "missing tool dispatch must flag the UI event as an error, got {ui_events:?}"
        );
        assert!(
            tool_result_event.2.contains("not registered"),
            "expected the failure reason to reach the UI summary, got {:?}",
            tool_result_event.2
        );
        assert!(
            ui_events
                .iter()
                .all(|event| !matches!(event, AgentUiEvent::Error(_))),
            "fallback path should not surface a fatal UI error: {ui_events:?}"
        );
        assert!(
            ui_events.contains(&AgentUiEvent::Done),
            "expected Done event, got {ui_events:?}"
        );
    }

    #[tokio::test]
    async fn approval_is_bound_before_handler_starts_and_allow_once_resumes_exact_call() {
        let provider = ScriptedProvider::new(vec![
            vec![
                AgentEvent::ToolCallReady {
                    id: "call_approved".to_string(),
                    name: "external_mutation".to_string(),
                    arguments: json!({"path": "/workspace/file"}),
                },
                AgentEvent::ResponseDone {
                    response_id: Some("resp_approval".to_string()),
                    clean: true,
                },
            ],
            vec![
                AgentEvent::TextDone("mutation complete".to_string()),
                AgentEvent::ResponseDone {
                    response_id: Some("resp_done".to_string()),
                    clean: true,
                },
            ],
        ]);
        let handler_started = Arc::new(AtomicBool::new(false));
        let started = Arc::clone(&handler_started);
        let mut registry = ToolRegistry::new();
        registry
            .register_with_policy(
                ToolDefinition {
                    name: "external_mutation".to_string(),
                    description: "mutates".to_string(),
                    input_schema: json!({"type": "object"}),
                },
                Box::new(move |_| {
                    let started = Arc::clone(&started);
                    Box::pin(async move {
                        started.store(true, Ordering::SeqCst);
                        vec![ToolResultContent::Text("changed".to_string())]
                    })
                }),
                ToolExecutionPolicy {
                    origin: ToolOrigin::Mcp {
                        server: "desktop-commander".to_string(),
                        upstream_tool: "write_file".to_string(),
                    },
                    risk: ToolRisk::Mutating,
                    requires_approval: true,
                },
                None,
            )
            .expect("register tool");

        let (approval_tx, approval_rx) = oneshot::channel::<bool>();
        let approval_rx = Arc::new(Mutex::new(Some(approval_rx)));
        let approval_handler = {
            let approval_rx = Arc::clone(&approval_rx);
            Arc::new(move |request: crate::agent::ToolApprovalRequest| {
                assert_eq!(request.call_id, "call_approved");
                assert_eq!(request.thread_id, "thread-exact");
                let rx = approval_rx
                    .lock()
                    .expect("approval lock")
                    .take()
                    .expect("single approval");
                Box::pin(async move { rx.await.unwrap_or(false) })
                    as crate::agent::ToolApprovalFuture
            })
        };
        let (ui_tx, mut ui_rx) = mpsc::channel(32);
        let mut session = AgentSession::new(Box::new(provider), Arc::new(registry), ui_tx)
            .with_tool_approval("thread-exact", approval_handler);
        let options = StreamOptions {
            model: "gpt-test".to_string(),
            system_prompt: None,
            max_tokens: None,
            temperature: None,
            reset_chain: false,
        };
        let send = session.send("mutate".to_string(), Vec::new(), &options);
        tokio::pin!(send);

        loop {
            let event = tokio::select! {
                result = &mut send => panic!("send completed before approval: {result:?}"),
                event = ui_rx.recv() => event.expect("approval event"),
            };
            if let AgentUiEvent::ToolApprovalRequested(request) = event {
                assert_eq!(request.call_id, "call_approved");
                assert!(
                    !handler_started.load(Ordering::SeqCst),
                    "handler must not start while approval is pending"
                );
                break;
            }
        }
        approval_tx.send(true).expect("approve");
        send.await.expect("approved send completes");
        assert!(handler_started.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn rejected_approval_never_starts_handler() {
        let provider = ScriptedProvider::new(vec![
            vec![AgentEvent::ToolCallReady {
                id: "call_rejected".to_string(),
                name: "external_mutation".to_string(),
                arguments: json!({}),
            }],
            vec![AgentEvent::TextDone("rejection handled".to_string())],
        ]);
        let handler_started = Arc::new(AtomicBool::new(false));
        let started = Arc::clone(&handler_started);
        let mut registry = ToolRegistry::new();
        registry
            .register_with_policy(
                ToolDefinition {
                    name: "external_mutation".to_string(),
                    description: "mutates".to_string(),
                    input_schema: json!({"type": "object"}),
                },
                Box::new(move |_| {
                    let started = Arc::clone(&started);
                    Box::pin(async move {
                        started.store(true, Ordering::SeqCst);
                        vec![]
                    })
                }),
                ToolExecutionPolicy {
                    origin: ToolOrigin::Mcp {
                        server: "desktop-commander".to_string(),
                        upstream_tool: "write_file".to_string(),
                    },
                    risk: ToolRisk::Mutating,
                    requires_approval: true,
                },
                None,
            )
            .expect("register tool");
        let approval_handler =
            Arc::new(|_| Box::pin(async { false }) as crate::agent::ToolApprovalFuture);
        let (ui_tx, _ui_rx) = mpsc::channel(32);
        let mut session = AgentSession::new(Box::new(provider), Arc::new(registry), ui_tx)
            .with_tool_approval("thread-reject", approval_handler);
        session
            .send(
                "mutate".to_string(),
                Vec::new(),
                &StreamOptions {
                    model: "gpt-test".to_string(),
                    system_prompt: None,
                    max_tokens: None,
                    temperature: None,
                    reset_chain: false,
                },
            )
            .await
            .expect("rejection becomes a tool result");
        assert!(!handler_started.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn timed_out_approval_never_starts_handler() {
        let provider = ScriptedProvider::new(vec![
            vec![AgentEvent::ToolCallReady {
                id: "call_timeout".to_string(),
                name: "external_mutation".to_string(),
                arguments: json!({}),
            }],
            vec![AgentEvent::TextDone("timeout handled".to_string())],
        ]);
        let handler_started = Arc::new(AtomicBool::new(false));
        let started = Arc::clone(&handler_started);
        let mut registry = ToolRegistry::new();
        registry
            .register_with_policy(
                ToolDefinition {
                    name: "external_mutation".to_string(),
                    description: "mutates".to_string(),
                    input_schema: json!({"type": "object"}),
                },
                Box::new(move |_| {
                    let started = Arc::clone(&started);
                    Box::pin(async move {
                        started.store(true, Ordering::SeqCst);
                        Vec::new()
                    })
                }),
                ToolExecutionPolicy {
                    origin: ToolOrigin::Mcp {
                        server: "desktop-commander".to_string(),
                        upstream_tool: "write_file".to_string(),
                    },
                    risk: ToolRisk::Mutating,
                    requires_approval: true,
                },
                None,
            )
            .expect("register tool");
        let approval_handler = Arc::new(|_| {
            Box::pin(std::future::pending::<bool>()) as crate::agent::ToolApprovalFuture
        });
        let (ui_tx, _ui_rx) = mpsc::channel(32);
        let mut session = AgentSession::new(Box::new(provider), Arc::new(registry), ui_tx)
            .with_tool_approval("thread-timeout", approval_handler);
        session.approval_timeout = Duration::from_millis(10);
        session
            .send(
                "mutate".to_string(),
                Vec::new(),
                &StreamOptions {
                    model: "gpt-test".to_string(),
                    system_prompt: None,
                    max_tokens: None,
                    temperature: None,
                    reset_chain: false,
                },
            )
            .await
            .expect("timeout becomes a tool result");
        assert!(!handler_started.load(Ordering::SeqCst));
    }

    #[test]
    fn tool_summary_redacts_structured_and_plain_secrets() {
        assert_eq!(
            super::redact_tool_summary(r#"{"token":"hunter2","nested":{"password":"swordfish"}}"#),
            r#"{"nested":{"password":"[REDACTED]"},"token":"[REDACTED]"}"#
        );
        assert_eq!(
            super::redact_tool_summary("TOKEN=hunter2 result ok"),
            "[redacted sensitive tool output]"
        );
        assert_eq!(
            super::redact_tool_summary("Authorization: Bearer hunter2"),
            "[redacted sensitive tool output]"
        );
    }

    #[tokio::test]
    async fn send_retries_once_for_transient_stream_start_failure() {
        let attempts = Arc::new(AtomicUsize::new(0));
        let provider = RetryThenSuccessProvider {
            attempts: Arc::clone(&attempts),
        };

        let (ui_tx, mut ui_rx) = mpsc::channel(16);
        let mut session =
            AgentSession::new(Box::new(provider), Arc::new(ToolRegistry::new()), ui_tx);

        session
            .send(
                "transient retry".to_string(),
                Vec::new(),
                &StreamOptions {
                    model: "gpt-test".to_string(),
                    system_prompt: None,
                    max_tokens: None,
                    temperature: None,
                    reset_chain: false,
                },
            )
            .await
            .expect("session should retry transient start failure");

        assert_eq!(attempts.load(Ordering::SeqCst), 2);
        assert_eq!(session.thread_id(), Some("resp_retry_success"));
        assert_eq!(session.messages().len(), 2);

        let mut ui_events = Vec::new();
        while let Ok(event) = ui_rx.try_recv() {
            ui_events.push(event);
        }
        assert!(
            ui_events.contains(&AgentUiEvent::TextDone("Recovered response".to_string())),
            "expected recovered TextDone event, got {ui_events:?}"
        );
        assert!(
            ui_events.contains(&AgentUiEvent::Done),
            "expected Done event, got {ui_events:?}"
        );
    }

    /// P2.13 end-to-end: a failed/incomplete/cancelled terminal reaches the
    /// session as a dirty `ResponseDone { clean: false }` followed by an
    /// `Error`. The session MUST clear `thread_id` (chain reset) before
    /// returning the error, so the next turn full-replays instead of resuming a
    /// poisoned response chain. A follow-up clean send then adopts a fresh id.
    #[tokio::test]
    async fn dirty_terminal_then_error_resets_thread_id_and_next_turn_replays() {
        let provider = ScriptedProvider::new(vec![
            // Turn 1: failed terminal -> dirty ResponseDone, then Error.
            vec![
                AgentEvent::ResponseDone {
                    response_id: Some("resp_failed".to_string()),
                    clean: false,
                },
                AgentEvent::Error("Agent response failed: server_error: boom".to_string()),
            ],
            // Turn 2: clean success -> fresh chain id adopted.
            vec![
                AgentEvent::TextDone("Recovered next turn".to_string()),
                AgentEvent::ResponseDone {
                    response_id: Some("resp_recovered".to_string()),
                    clean: true,
                },
            ],
        ]);

        let (ui_tx, mut ui_rx) = mpsc::channel(32);
        let mut session =
            AgentSession::new(Box::new(provider), Arc::new(ToolRegistry::new()), ui_tx);
        // Pre-poison the chain as if a prior clean turn had advanced it.
        session.thread_id = Some("resp_poisoned".to_string());

        let options = StreamOptions {
            model: "gpt-test".to_string(),
            system_prompt: None,
            max_tokens: None,
            temperature: None,
            reset_chain: false,
        };

        let error = session
            .send("trigger failed terminal".to_string(), Vec::new(), &options)
            .await
            .expect_err("failed terminal must surface an error");
        assert!(
            error.to_string().contains("Provider stream error"),
            "expected provider stream error, got: {error}"
        );

        // Chain reset: the poisoned id must be gone after the dirty terminal.
        assert_eq!(
            session.thread_id(),
            None,
            "dirty terminal + error must reset the chain (thread_id == None)"
        );

        // Next turn succeeds and adopts a fresh id (proving the chain recovered
        // rather than staying stuck on the poisoned id).
        session
            .send("next turn".to_string(), Vec::new(), &options)
            .await
            .expect("recovered turn should complete");
        assert_eq!(
            session.thread_id(),
            Some("resp_recovered"),
            "a subsequent clean turn must adopt a fresh chain id"
        );

        let mut ui_events = Vec::new();
        while let Ok(event) = ui_rx.try_recv() {
            ui_events.push(event);
        }
        assert!(
            ui_events
                .iter()
                .all(|event| !matches!(event, AgentUiEvent::Error(_))),
            "fatal provider errors are surfaced through the Result channel only: {ui_events:?}"
        );
    }

    /// P2.13 belt-and-suspenders: even if a provider surfaces a bare `Error`
    /// WITHOUT a preceding dirty `ResponseDone`, the session must still reset the
    /// chain before returning, so a pre-existing `thread_id` cannot poison the
    /// next turn.
    #[tokio::test]
    async fn bare_error_without_dirty_terminal_still_resets_thread_id() {
        let provider = ScriptedProvider::new(vec![vec![AgentEvent::Error(
            "Agent response was cancelled before completion".to_string(),
        )]]);

        let (ui_tx, mut ui_rx) = mpsc::channel(16);
        let mut session =
            AgentSession::new(Box::new(provider), Arc::new(ToolRegistry::new()), ui_tx);
        session.thread_id = Some("resp_poisoned".to_string());

        let error = session
            .send(
                "bare error".to_string(),
                Vec::new(),
                &StreamOptions {
                    model: "gpt-test".to_string(),
                    system_prompt: None,
                    max_tokens: None,
                    temperature: None,
                    reset_chain: false,
                },
            )
            .await
            .expect_err("bare error must surface");
        assert!(error.to_string().contains("Provider stream error"));

        assert_eq!(
            session.thread_id(),
            None,
            "a bare Error must still clear the chain (belt-and-suspenders)"
        );
        let mut ui_events = Vec::new();
        while let Ok(event) = ui_rx.try_recv() {
            ui_events.push(event);
        }
        assert!(
            ui_events
                .iter()
                .all(|event| !matches!(event, AgentUiEvent::Error(_))),
            "fatal provider errors are surfaced through the Result channel only: {ui_events:?}"
        );
    }

    #[tokio::test]
    async fn send_does_not_retry_non_transient_stream_start_failure() {
        let attempts = Arc::new(AtomicUsize::new(0));
        let provider = PermanentFailureProvider {
            attempts: Arc::clone(&attempts),
        };

        let (ui_tx, mut ui_rx) = mpsc::channel(16);
        let mut session =
            AgentSession::new(Box::new(provider), Arc::new(ToolRegistry::new()), ui_tx);

        let error = session
            .send(
                "non transient".to_string(),
                Vec::new(),
                &StreamOptions {
                    model: "gpt-test".to_string(),
                    system_prompt: None,
                    max_tokens: None,
                    temperature: None,
                    reset_chain: false,
                },
            )
            .await
            .expect_err("session should fail fast for non-transient start errors");

        assert!(
            error.to_string().contains("Failed to start"),
            "expected stream start context, got: {error}"
        );
        assert_eq!(attempts.load(Ordering::SeqCst), 1);

        let mut ui_events = Vec::new();
        while let Ok(event) = ui_rx.try_recv() {
            ui_events.push(event);
        }
        assert!(
            ui_events
                .iter()
                .all(|event| !matches!(event, AgentUiEvent::Done)),
            "non-transient failure should not emit Done: {ui_events:?}"
        );
    }
}
