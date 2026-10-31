//! Native Gemini / Cloud Code Assist streaming Agent provider.
//!
//! The session owns tool execution and its approval gate. This provider owns
//! only Google wire parts, including opaque signatures that must never be
//! reconstructed from display text or moved to another part.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail, ensure};
use async_trait::async_trait;
use base64::{Engine, engine::general_purpose::STANDARD as BASE64};
use futures_util::StreamExt;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tokio::sync::mpsc;

use codescribe_core::agent::{
    AgentEvent, AgentProvider, ContentBlock, Message, Role, StreamOptions, ToolDefinition,
};
use codescribe_core::config::{RuntimeAiRequestTiming, RuntimeLlmLane};
use codescribe_core::llm::account_auth::{
    GoogleCredential, redact_google_secrets, resolve_google_credential,
};
use codescribe_core::llm::vendors::google;

const STREAM_DEADLINE: Duration = Duration::from_secs(600);
const MAX_SSE_EVENT_BYTES: usize = 4 * 1024 * 1024;

/// Google protocol client. A provider instance belongs to one AgentSession.
#[derive(Clone)]
pub struct GoogleProvider {
    client: reqwest::Client,
    endpoint: String,
    default_model: String,
    credential_account: Option<String>,
    fixture_key: Option<String>,
    initial_timeout: Duration,
    chunk_timeout: Duration,
    history: Arc<Mutex<Vec<RetainedTurn>>>,
}

#[derive(Clone)]
struct Call {
    id: String,
    name: String,
    arguments: Value,
    wire_id: Option<String>,
}

/// The prefix binds wire metadata to the exact session history that produced
/// it. A rollback/truncation or a different model cannot borrow a signature.
#[derive(Clone)]
struct RetainedTurn {
    prefix_len: usize,
    prefix_digest: [u8; 32],
    model: String,
    text: String,
    calls: Vec<Call>,
    parts: Vec<Value>,
}

impl GoogleProvider {
    /// Resolve credentials at each send, so rotation and ADC refresh are live.
    pub fn from_lane(lane: &RuntimeLlmLane, timing: &RuntimeAiRequestTiming) -> Result<Self> {
        let mut provider = Self::from_configuration(
            lane.endpoint().to_string(),
            lane.model().to_string(),
            String::new(),
            timing,
        )?;
        provider.credential_account = Some(lane.credential().key_account().to_string());
        Ok(provider)
    }

    /// Explicit embedding configuration, also used by isolated mock fixtures.
    pub fn from_configuration(
        endpoint: String,
        default_model: String,
        api_key: String,
        timing: &RuntimeAiRequestTiming,
    ) -> Result<Self> {
        google::prepare_request(&endpoint, &default_model, json!({}), true)
            .map_err(|error| anyhow!(error))?;
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(STREAM_DEADLINE)
            .build()
            .context("Failed to create Google HTTP client")?;
        Ok(Self {
            client,
            endpoint,
            default_model,
            credential_account: None,
            fixture_key: (!api_key.is_empty()).then_some(api_key),
            initial_timeout: timing.attempt_timeout(),
            chunk_timeout: timing.inter_chunk_timeout(),
            history: Arc::new(Mutex::new(Vec::new())),
        })
    }
}

#[async_trait]
impl AgentProvider for GoogleProvider {
    async fn stream(
        &self,
        messages: &[Message],
        tools: &[ToolDefinition],
        options: &StreamOptions,
    ) -> Result<mpsc::Receiver<AgentEvent>> {
        let model = if options.model.trim().is_empty() {
            &self.default_model
        } else {
            &options.model
        };
        let retained = {
            let mut history = self
                .history
                .lock()
                .unwrap_or_else(|poison| poison.into_inner());
            history.retain(|turn| turn.model == *model && turn.matches(messages));
            history.clone()
        };
        let body = build_request(messages, tools, options, &retained)?;
        let (endpoint, body) = google::prepare_request(&self.endpoint, model, body, true)
            .map_err(|error| anyhow!(error))?;
        let credential = if let Some(key) = &self.fixture_key {
            GoogleCredential::ApiKey(key.clone())
        } else {
            resolve_google_credential(
                self.credential_account
                    .as_deref()
                    .unwrap_or(google::API_KEY_ACCOUNT),
            )
            .await?
        };
        let (tx, rx) = mpsc::channel(256);
        let transport = self.clone();
        let pending = RetainedTurn {
            prefix_len: messages.len(),
            prefix_digest: history_digest(messages)?,
            model: model.clone(),
            text: String::new(),
            calls: Vec::new(),
            parts: Vec::new(),
        };
        tokio::spawn(async move {
            let result = transport
                .run_stream(endpoint, body, credential, pending, &tx)
                .await;
            if let Err(error) = result {
                let _ = tx
                    .send(AgentEvent::ResponseDone {
                        response_id: None,
                        clean: false,
                    })
                    .await;
                let _ = tx
                    .send(AgentEvent::Error(redact_google_secrets(&error.to_string())))
                    .await;
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
        super::user_tool_result(call_id, content, is_error)
    }
    fn build_image_block(&self, data: &[u8], media_type: &str) -> ContentBlock {
        super::image_block(data, media_type)
    }
    fn stream_timeouts(&self) -> Option<(Duration, Duration)> {
        Some((self.initial_timeout, self.chunk_timeout))
    }
    fn name(&self) -> &str {
        "google-generate-content"
    }
}

fn history_digest(messages: &[Message]) -> Result<[u8; 32]> {
    Ok(Sha256::digest(serde_json::to_vec(messages)?).into())
}

impl RetainedTurn {
    fn assistant_messages(&self) -> Vec<Vec<ContentBlock>> {
        let mut result = Vec::new();
        if !self.text.trim().is_empty() {
            result.push(vec![ContentBlock::Text(self.text.trim().to_string())]);
        }
        if !self.calls.is_empty() {
            result.push(
                self.calls
                    .iter()
                    .map(|call| ContentBlock::ToolUse {
                        id: call.id.clone(),
                        name: call.name.clone(),
                        input: call.arguments.clone(),
                    })
                    .collect(),
            );
        }
        result
    }
    fn matches(&self, messages: &[Message]) -> bool {
        let expected = self.assistant_messages();
        !expected.is_empty()
            && self.prefix_len + expected.len() <= messages.len()
            && history_digest(&messages[..self.prefix_len]).ok() == Some(self.prefix_digest)
            && expected.iter().enumerate().all(|(offset, content)| {
                let message = &messages[self.prefix_len + offset];
                message.role == Role::Assistant && message.content == *content
            })
    }
}

fn build_request(
    messages: &[Message],
    tools: &[ToolDefinition],
    options: &StreamOptions,
    retained: &[RetainedTurn],
) -> Result<Value> {
    let mut contents: Vec<Value> = Vec::new();
    let mut system = Vec::new();
    if let Some(prompt) = options
        .system_prompt
        .as_deref()
        .filter(|text| !text.is_empty())
    {
        system.push(json!({"text": prompt}));
    }
    let mut index = 0;
    while index < messages.len() {
        if let Some(turn) = retained.iter().find(|turn| turn.prefix_len == index) {
            contents.push(json!({"role":"model","parts":turn.parts}));
            index += turn.assistant_messages().len();
            continue;
        }
        let message = &messages[index];
        let mut parts = Vec::new();
        for block in &message.content {
            match block {
                ContentBlock::Text(text) => parts.push(json!({"text":text})),
                ContentBlock::Image { data, media_type } => {
                    if !data.is_empty() {
                        parts.push(json!({"inlineData":{"mimeType":media_type,"data":BASE64.encode(data)}}));
                    }
                }
                ContentBlock::ImageAsset(asset) => {
                    let data = std::fs::read(&asset.path)
                        .context("Unable to read Google image attachment")?;
                    if !data.is_empty() {
                        parts.push(json!({"inlineData":{"mimeType":asset.media_type,"data":BASE64.encode(data)}}));
                    }
                }
                ContentBlock::ToolUse { .. } => bail!(
                    "Google tool history has no retained signed model parts; start a new turn or restore its original provider session"
                ),
                ContentBlock::ToolResult {
                    tool_use_id,
                    content,
                    is_error,
                } => {
                    let call = retained
                        .iter()
                        .flat_map(|turn| &turn.calls)
                        .find(|call| call.id == *tool_use_id)
                        .ok_or_else(|| {
                            anyhow!("Google tool result has no matching model function call")
                        })?;
                    let mut response = json!({"name":call.name,"response":{"result":tool_result_text(content),"is_error":is_error}});
                    if let Some(id) = &call.wire_id {
                        response["id"] = json!(id);
                    }
                    parts.push(json!({"functionResponse":response}));
                }
            }
        }
        if message.role == Role::System {
            system.extend(parts);
        } else if !parts.is_empty() {
            let role = if message.role == Role::Assistant {
                "model"
            } else {
                "user"
            };
            let results_only = parts
                .iter()
                .all(|part| part.get("functionResponse").is_some());
            if results_only
                && contents.last().is_some_and(|last| {
                    last["role"] == "user"
                        && last["parts"].as_array().is_some_and(|parts| {
                            parts
                                .iter()
                                .all(|part| part.get("functionResponse").is_some())
                        })
                })
            {
                contents.last_mut().unwrap()["parts"]
                    .as_array_mut()
                    .unwrap()
                    .extend(parts);
            } else {
                contents.push(json!({"role":role,"parts":parts}));
            }
        }
        index += 1;
    }
    let mut body = json!({"contents":contents});
    if !system.is_empty() {
        body["systemInstruction"] = json!({"parts":system});
    }
    let mut generation = json!({});
    if let Some(tokens) = options.max_tokens.filter(|tokens| *tokens > 0) {
        generation["maxOutputTokens"] = json!(tokens);
    }
    if let Some(temperature) = options.temperature {
        generation["temperature"] = json!(temperature);
    }
    if generation
        .as_object()
        .is_some_and(|object| !object.is_empty())
    {
        body["generationConfig"] = generation;
    }
    if !tools.is_empty() {
        let declarations = tools
            .iter()
            .map(|tool| -> Result<Value> {
                let schema = google::normalize_schema_for_gemini(&tool.input_schema)
                    .map_err(|error| anyhow!(error))?;
                Ok(json!({"name":tool.name,"description":tool.description,"parameters":schema}))
            })
            .collect::<Result<Vec<_>>>()?;
        body["tools"] = json!([{"functionDeclarations":declarations}]);
    }
    Ok(body)
}

fn tool_result_text(content: &[ContentBlock]) -> String {
    content
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Text(text) => Some(text.clone()),
            ContentBlock::Image { .. } | ContentBlock::ImageAsset(_) => {
                Some("[image in tool result]".into())
            }
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

impl GoogleProvider {
    async fn run_stream(
        &self,
        endpoint: String,
        body: Value,
        credential: GoogleCredential,
        mut turn: RetainedTurn,
        tx: &mpsc::Sender<AgentEvent>,
    ) -> Result<()> {
        let mut request = self
            .client
            .post(endpoint)
            .header("accept", "text/event-stream")
            .json(&body);
        match credential {
            GoogleCredential::ApiKey(key) => request = request.header("x-goog-api-key", key),
            GoogleCredential::Bearer(token) => request = request.bearer_auth(token),
        }
        let response = tokio::select! {
            biased;
            _ = tx.closed() => return Ok(()),
            result = tokio::time::timeout(self.initial_timeout, request.send()) => {
                result.map_err(|_| anyhow!("Google response headers timed out"))?
                    .map_err(|error| anyhow!("Google transport failed: {}", redact_google_secrets(&error.without_url().to_string())))?
            }
        };
        if !response.status().is_success() {
            // Never surface provider error bodies: they can echo arbitrary
            // credential material (not just recognized AIza/ya29 prefixes).
            bail!(
                "Google request rejected (HTTP {})",
                response.status().as_u16()
            );
        }
        ensure!(
            response
                .headers()
                .get(reqwest::header::CONTENT_TYPE)
                .and_then(|value| value.to_str().ok())
                .is_some_and(|value| value.starts_with("text/event-stream")),
            "Google response was not an SSE stream"
        );
        let mut stream = response.bytes_stream();
        let deadline = tokio::time::Instant::now() + STREAM_DEADLINE;
        let mut parser = GoogleSse::default();
        let mut state = ResponseState::default();
        loop {
            let chunk = tokio::select! {
                biased;
                _ = tx.closed() => return Ok(()),
                _ = tokio::time::sleep_until(deadline) => bail!("Google stream exceeded its deadline"),
                chunk = tokio::time::timeout(self.chunk_timeout, stream.next()) => chunk.map_err(|_| anyhow!("Google stream stalled"))?,
            };
            match chunk {
                Some(Ok(bytes)) => {
                    for event in parser.push(&bytes)? {
                        state.consume(event, tx).await?;
                    }
                }
                Some(Err(_)) => bail!("Google stream transport failed"),
                None => {
                    for event in parser.finish()? {
                        state.consume(event, tx).await?;
                    }
                    break;
                }
            }
        }
        ensure!(
            state.clean_terminal,
            "Google stream ended without a clean STOP terminal"
        );
        if let Some(text) = state.text.finish() {
            state.emit_text(text, tx).await?;
        }
        ensure!(
            !state.visible.trim().is_empty() || !state.calls.is_empty(),
            "Google returned no answer or tool calls"
        );
        turn.text = state.visible;
        turn.calls = state.calls;
        turn.parts = state.parts;
        // Persist before exposing the clean terminal: the next session step
        // can immediately build functionResponse from these exact signed parts.
        self.history
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .push(turn.clone());
        tx.send(AgentEvent::TextDone(turn.text))
            .await
            .map_err(|_| anyhow!("Google receiver closed"))?;
        tx.send(AgentEvent::ResponseDone {
            response_id: None,
            clean: true,
        })
        .await
        .map_err(|_| anyhow!("Google receiver closed"))?;
        Ok(())
    }
}

/// Byte framing defers UTF-8 decoding until a complete line exists. HTTP may
/// split at any byte, including inside a Polish character or an emoji.
#[derive(Default)]
struct GoogleSse {
    bytes: Vec<u8>,
    data: Vec<String>,
    event_bytes: usize,
}
impl GoogleSse {
    fn push(&mut self, bytes: &[u8]) -> Result<Vec<Value>> {
        self.bytes.extend_from_slice(bytes);
        ensure!(
            self.bytes.len() <= MAX_SSE_EVENT_BYTES,
            "Google SSE line exceeds limit"
        );
        let mut events = Vec::new();
        while let Some(end) = self.bytes.iter().position(|byte| *byte == b'\n') {
            let line = self.bytes.drain(..=end).collect::<Vec<_>>();
            let line = std::str::from_utf8(&line[..line.len() - 1])
                .context("Google SSE contains invalid UTF-8")?
                .trim_end_matches('\r');
            self.line(line, &mut events)?;
        }
        Ok(events)
    }
    fn line(&mut self, line: &str, events: &mut Vec<Value>) -> Result<()> {
        if line.is_empty() {
            if !self.data.is_empty() {
                let data = self.data.join("\n");
                self.data.clear();
                self.event_bytes = 0;
                if data != "[DONE]" {
                    events.push(serde_json::from_str(&data).context("Invalid Google SSE JSON")?);
                }
            }
        } else if let Some(data) = line.strip_prefix("data:") {
            let data = data.strip_prefix(' ').unwrap_or(data);
            self.event_bytes += data.len();
            ensure!(
                self.event_bytes <= MAX_SSE_EVENT_BYTES,
                "Google SSE event exceeds limit"
            );
            self.data.push(data.to_string());
        }
        Ok(())
    }
    fn finish(&mut self) -> Result<Vec<Value>> {
        let mut events = Vec::new();
        let bytes = std::mem::take(&mut self.bytes);
        let line = std::str::from_utf8(&bytes)
            .context("Truncated Google SSE UTF-8")?
            .trim_end_matches('\r');
        if !line.is_empty() {
            self.line(line, &mut events)?;
        }
        self.line("", &mut events)?;
        Ok(events)
    }
}

#[derive(Default)]
struct ResponseState {
    parts: Vec<Value>,
    calls: Vec<Call>,
    visible: String,
    text: PlanningFilter,
    clean_terminal: bool,
}
impl ResponseState {
    async fn emit_text(&mut self, text: String, tx: &mpsc::Sender<AgentEvent>) -> Result<()> {
        if !text.is_empty() {
            self.visible.push_str(&text);
            tx.send(AgentEvent::TextDelta(text))
                .await
                .map_err(|_| anyhow!("Google receiver closed"))?;
        }
        Ok(())
    }
    async fn consume(&mut self, envelope: Value, tx: &mpsc::Sender<AgentEvent>) -> Result<()> {
        let value = envelope.get("response").unwrap_or(&envelope);
        ensure!(value.get("error").is_none(), "Google reported an SSE error");
        ensure!(
            value.pointer("/promptFeedback/blockReason").is_none(),
            "Google blocked the prompt"
        );
        let Some(candidates) = value.get("candidates").and_then(Value::as_array) else {
            return Ok(());
        };
        // The Agent asks for one candidate. Never mix alternative answers/tools.
        let Some(candidate) = candidates
            .iter()
            .find(|candidate| candidate.get("index").and_then(Value::as_u64).unwrap_or(0) == 0)
        else {
            return Ok(());
        };
        if let Some(parts) = candidate
            .pointer("/content/parts")
            .and_then(Value::as_array)
        {
            ensure!(
                !self.clean_terminal,
                "Google emitted content after its terminal"
            );
            for part in parts {
                retain_thought_signature(&mut self.parts, part);
                if let Some(text) = part.get("text").and_then(Value::as_str) {
                    if part
                        .get("thought")
                        .and_then(Value::as_bool)
                        .unwrap_or(false)
                    {
                        tx.send(AgentEvent::ReasoningDelta(text.to_string()))
                            .await
                            .map_err(|_| anyhow!("Google receiver closed"))?;
                    } else if let Some(text) = self.text.push(text) {
                        self.emit_text(text, tx).await?;
                    }
                }
                if let Some(call) = part.get("functionCall") {
                    ensure!(
                        call.get("partialArgs").is_none(),
                        "Google streamed partial function arguments without a complete call"
                    );
                    let name = call
                        .get("name")
                        .and_then(Value::as_str)
                        .filter(|name| !name.is_empty())
                        .ok_or_else(|| anyhow!("Google function call has no name"))?;
                    let arguments = call.get("args").cloned().unwrap_or_else(|| json!({}));
                    ensure!(
                        arguments.is_object(),
                        "Google function arguments must be an object"
                    );
                    let id = format!("google_{}", uuid::Uuid::new_v4());
                    self.calls.push(Call {
                        id: id.clone(),
                        name: name.to_string(),
                        arguments: arguments.clone(),
                        wire_id: call.get("id").and_then(Value::as_str).map(str::to_string),
                    });
                    tx.send(AgentEvent::ToolCallReady {
                        id,
                        name: name.to_string(),
                        arguments,
                    })
                    .await
                    .map_err(|_| anyhow!("Google receiver closed"))?;
                }
            }
        }
        if let Some(reason) = candidate.get("finishReason").and_then(Value::as_str) {
            ensure!(
                reason == "STOP",
                "Google did not complete the response (finish reason {})",
                reason
            );
            self.clean_terminal = true;
        }
        Ok(())
    }
}

/// Retain each part verbatim, including empty signature-only text parts. Never
/// merge parts: a signature's position is part of the provider's contract.
fn retain_thought_signature(parts: &mut Vec<Value>, part: &Value) {
    parts.push(part.clone());
}

/// Recognize a leaked planning envelope before any of its bytes reach the UI.
fn is_planning_leak_prefix(text: &str) -> bool {
    let text = text.trim_start();
    let text = text
        .strip_prefix("```json")
        .or_else(|| text.strip_prefix("```"))
        .unwrap_or(text)
        .trim_start();
    let Some(text) = text.strip_prefix('{') else {
        return false;
    };
    let text = text.trim_start();
    ["\"thought\"", "\"thinking\"", "\"plan\""]
        .iter()
        .any(|key| {
            text.strip_prefix(key)
                .is_some_and(|rest| rest.trim_start().starts_with(':'))
        })
}

#[derive(Default)]
struct PlanningFilter {
    pending: String,
    decided: bool,
    suppress: bool,
}
impl PlanningFilter {
    fn push(&mut self, text: &str) -> Option<String> {
        if self.decided {
            return (!self.suppress).then(|| text.to_string());
        }
        self.pending.push_str(text);
        let trimmed = self.pending.trim_start();
        if is_planning_leak_prefix(trimmed) {
            self.decided = true;
            self.suppress = true;
            self.pending.clear();
            return None;
        }
        if trimmed.is_empty() {
            return None;
        }
        // Hold a possible object/fence prefix across arbitrary SSE parts. Once
        // the first key differs it is ordinary answer text and can flow.
        let possible = ["{", "```"]
            .iter()
            .any(|prefix| prefix.starts_with(trimmed) || trimmed.starts_with(prefix));
        if possible && !trimmed.contains(':') && self.pending.len() < 512 {
            return None;
        }
        self.decided = true;
        Some(std::mem::take(&mut self.pending))
    }
    fn finish(&mut self) -> Option<String> {
        // An unfinished possible planning JSON must never leak on EOF/STOP.
        if !self.decided && self.pending.trim_start().starts_with(['{', '`']) {
            self.pending.clear();
            return None;
        }
        (!self.suppress).then(|| std::mem::take(&mut self.pending))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use codescribe_core::agent::{ToolExecutionPolicy, ToolOrigin, ToolRegistry, ToolRisk};
    use mockito::Matcher;

    fn timing() -> RuntimeAiRequestTiming {
        RuntimeAiRequestTiming::default()
    }
    fn user(text: &str) -> Message {
        Message::new(Role::User, vec![ContentBlock::Text(text.into())])
    }
    fn sse(body: Value) -> String {
        format!("data: {body}\n\n")
    }
    async fn events(
        provider: &GoogleProvider,
        messages: &[Message],
        tools: &[ToolDefinition],
    ) -> Vec<AgentEvent> {
        let mut rx = provider
            .stream(messages, tools, &StreamOptions::default())
            .await
            .unwrap();
        let mut events = Vec::new();
        while let Some(event) = rx.recv().await {
            events.push(event);
        }
        events
    }

    /// F1 uses the actual HTTP/SSE provider seam and a session-shaped second
    /// step, so signatures cannot pass by testing only a serialization helper.
    #[tokio::test]
    async fn google_provider_signed_tool_roundtrip_cca_and_parallel_responses() {
        let mut server = mockito::Server::new_async().await;
        let parts = json!([
            {"thought":true,"text":"Sprawdzę","thoughtSignature":"sig_thought"},
            {"functionCall":{"name":"mcp_search_tools","args":{"query":"repo"}},"thoughtSignature":"sig_abc123"},
            {"functionCall":{"name":"call_mcp_tool","id":"wire-call","args":{"tool":"read"}},"thoughtSignature":"sig_parallel"}
        ]);
        let response =
            json!({"response":{"candidates":[{"content":{"parts":parts},"finishReason":"STOP"}]}});
        let first = server
            .mock("POST", "/v1internal:streamGenerateContent?alt=sse")
            .match_header("x-goog-api-key", "fixture-key")
            .match_body(Matcher::PartialJson(
                json!({"model":"gemini-test","project":"p"}),
            ))
            .with_header("content-type", "text/event-stream")
            .with_body(sse(response))
            .create_async()
            .await;
        let provider = GoogleProvider::from_configuration(
            format!("{}/v1internal?project=p", server.url()),
            "gemini-test".into(),
            "fixture-key".into(),
            &timing(),
        )
        .unwrap();
        let mut messages = vec![user("Szukaj")];
        let first_events = events(&provider, &messages, &[]).await;
        assert!(
            first_events.iter().any(
                |event| matches!(event, AgentEvent::ReasoningDelta(text) if text == "Sprawdzę")
            )
        );
        let calls = first_events
            .iter()
            .filter_map(|event| match event {
                AgentEvent::ToolCallReady {
                    id,
                    name,
                    arguments,
                } => Some((id.clone(), name.clone(), arguments.clone())),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(calls.len(), 2, "{first_events:?}");
        messages.push(Message::new(
            Role::Assistant,
            calls
                .iter()
                .map(|(id, name, input)| ContentBlock::ToolUse {
                    id: id.clone(),
                    name: name.clone(),
                    input: input.clone(),
                })
                .collect(),
        ));
        for (id, _, _) in &calls {
            messages.push(provider.build_tool_result(
                id,
                vec![ContentBlock::Text("found".into())],
                false,
            ));
        }
        let expected = json!({"request":{"contents":[
            {"role":"user","parts":[{"text":"Szukaj"}]},
            {"role":"model","parts":parts},
            {"role":"user","parts":[{"functionResponse":{"name":"mcp_search_tools","response":{"result":"found","is_error":false}}},{"functionResponse":{"name":"call_mcp_tool","id":"wire-call","response":{"result":"found","is_error":false}}}]}
        ]}});
        let second = server.mock("POST", "/v1internal:streamGenerateContent?alt=sse").match_body(Matcher::PartialJson(expected))
            .with_header("content-type","text/event-stream")
            .with_body(sse(json!({"response":{"candidates":[{"content":{"parts":[{"text":"Gotowe 🐾"}]},"finishReason":"STOP"}]}}))).create_async().await;
        let second_events = events(&provider, &messages, &[]).await;
        assert!(
            second_events
                .iter()
                .any(|event| matches!(event,AgentEvent::TextDone(text) if text == "Gotowe 🐾")),
            "{second_events:?}"
        );
        first.assert_async().await;
        second.assert_async().await;
        let mut rolled_back = vec![user("Different thread")];
        rolled_back.extend(messages[1..].iter().cloned());
        assert!(
            provider
                .stream(&rolled_back, &[], &StreamOptions::default())
                .await
                .is_err(),
            "No cross-thread signature reuse"
        );
    }

    /// F2 exercises the real MCP projection, rather than hand-copying the two
    /// gateway schemas and accidentally missing changes to wire_definitions.
    #[test]
    fn google_provider_normalizes_real_mcp_gateway_schemas() {
        let mut registry = ToolRegistry::new();
        registry.register_with_policy(ToolDefinition{name:"mcp__local__read".into(),description:"read".into(),input_schema:json!({"type":"object","properties":{"path":{"type":"string"}},"required":["path"]})},Box::new(|_| Box::pin(async {Vec::new()})),ToolExecutionPolicy{origin:ToolOrigin::Mcp{server:"local".into(),upstream_tool:"read".into()},risk:ToolRisk::ReadOnly,requires_approval:false},None).unwrap();
        let tools = registry.wire_definitions();
        assert_eq!(
            tools
                .iter()
                .map(|tool| tool.name.as_str())
                .collect::<Vec<_>>(),
            vec!["mcp_search_tools", "call_mcp_tool"]
        );
        let body = build_request(&[user("hello")], &tools, &StreamOptions::default(), &[]).unwrap();
        for (tool, decl) in tools
            .iter()
            .zip(body["tools"][0]["functionDeclarations"].as_array().unwrap())
        {
            assert_eq!(decl["name"], tool.name);
            assert_eq!(
                decl["parameters"].get("required"),
                tool.input_schema.get("required")
            );
            validate_schema(&decl["parameters"]);
        }
    }
    fn validate_schema(schema: &Value) {
        assert!(schema.is_object());
        assert!(schema.get("$schema").is_none());
        assert!(schema.get("additionalProperties").is_none());
        assert!(!schema.get("type").is_some_and(Value::is_array));
        if let Some(properties) = schema.get("properties").and_then(Value::as_object) {
            for value in properties.values() {
                validate_schema(value);
            }
        }
        if let Some(items) = schema.get("items") {
            validate_schema(items);
        }
    }

    #[test]
    fn google_provider_sse_split_utf8_multiline_and_truncated_stream() {
        let wire = format!(
            ": ping\r\ndata: {{\"candidates\":\r\ndata: [{{\"content\":{{\"parts\":[{{\"text\":\"Zażółć 🐾\"}}]}},\"finishReason\":\"STOP\"}}]}}\r\n\r\n"
        );
        for split in 0..wire.len() {
            let mut parser = GoogleSse::default();
            let mut result = parser.push(&wire.as_bytes()[..split]).unwrap();
            result.extend(parser.push(&wire.as_bytes()[split..]).unwrap());
            result.extend(parser.finish().unwrap());
            assert_eq!(result.len(), 1);
            assert_eq!(
                result[0]["candidates"][0]["content"]["parts"][0]["text"],
                "Zażółć 🐾"
            );
        }
        let mut parser = GoogleSse::default();
        parser.push(b"data: {\"x\":\"").unwrap();
        assert!(parser.finish().is_err());
    }

    /// F3 checks both rejected status codes at the real transport boundary.
    #[tokio::test]
    async fn google_provider_rejects_dirty_terminals_and_redacts_http_errors() {
        for status in [401, 403] {
            let mut server = mockito::Server::new_async().await;
            let mock = server
                .mock(
                    "POST",
                    "/v1beta/models/gemini-test:streamGenerateContent?alt=sse",
                )
                .with_status(status)
                .with_body("AIzaDoNotLeak ya29.NeverReveal arbitrary-fixture-secret")
                .create_async()
                .await;
            let provider = GoogleProvider::from_configuration(
                format!("{}/v1beta", server.url()),
                "gemini-test".into(),
                "arbitrary-fixture-secret".into(),
                &timing(),
            )
            .unwrap();
            let result = events(&provider, &[user("test")], &[]).await;
            let rendered = format!("{result:?}");
            assert!(!rendered.contains("AIza"));
            assert!(!rendered.contains("ya29."));
            assert!(!rendered.contains("arbitrary-fixture-secret"));
            assert!(
                result
                    .iter()
                    .any(|event| matches!(event, AgentEvent::ResponseDone { clean: false, .. }))
            );
            mock.assert_async().await;
        }
        let (tx, mut rx) = mpsc::channel(20);
        let mut state = ResponseState::default();
        state
            .consume(
                json!({"candidates":[{"content":{"parts":[{"text":"{\"th"}]}}]}),
                &tx,
            )
            .await
            .unwrap();
        state.consume(json!({"candidates":[{"content":{"parts":[{"text":"ought\": \"planuję...\"}"}]},"finishReason":"STOP"}]}),&tx).await.unwrap();
        drop(tx);
        while let Some(event) = rx.recv().await {
            assert!(!matches!(event, AgentEvent::TextDelta(_)));
        }
        assert!(
            state
                .consume(
                    json!({"candidates":[{"finishReason":"SAFETY"}]}),
                    &mpsc::channel(1).0
                )
                .await
                .is_err()
        );
    }
}
