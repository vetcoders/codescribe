//! Native Gemini / Cloud Code Assist streaming Agent provider.
//!
//! The session owns history, tool execution and approvals. This provider parses
//! in-flight Google wire parts and hands one clean response to canonical Message
//! metadata. Signatures are never reconstructed from text or moved to a part.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail, ensure};
use async_trait::async_trait;
use base64::{Engine, engine::general_purpose::STANDARD as BASE64};
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tokio::sync::mpsc;

use codescribe_core::agent::{
    AgentEvent, AgentProvider, ContentBlock, Message, ProviderResponseMetadata, Role,
    StreamOptions, ToolDefinition,
};
use codescribe_core::config::{RuntimeAiRequestTiming, RuntimeLlmLane};
use codescribe_core::llm::account_auth::{
    GoogleCredential, redact_google_secrets, resolve_google_account_credential,
    resolve_google_credential,
};
use codescribe_core::llm::responses_streaming_manager::StreamWatchdog;
use codescribe_core::llm::vendors::google;

const STREAM_DEADLINE: Duration = Duration::from_secs(600);
const MAX_SSE_EVENT_BYTES: usize = 4 * 1024 * 1024;
const MAX_SSE_DATA_LINES: usize = 8192;
const MAX_TURN_PART_BYTES: usize = 8 * 1024 * 1024;
const MAX_TURN_PARTS: usize = 8192;
const MAX_RETAINED_HISTORY_BYTES: usize = 16 * 1024 * 1024;
const MAX_RETAINED_TURNS: usize = 128;

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
    first_event_timeout: Duration,
    completed_response: Arc<Mutex<Option<ProviderResponseMetadata>>>,
    turn_lock: Arc<tokio::sync::Mutex<()>>,
}

#[derive(Clone)]
struct Call {
    id: String,
    name: String,
    arguments: Value,
    wire_id: Option<String>,
}

/// A bounded projection of canonical message metadata for this one request.
/// It is rebuilt from Message/ThreadMessage; the provider retains no history.
struct RetainedTurn {
    prefix_len: usize,
    message_count: usize,
    parts: Vec<Value>,
    calls: Vec<Call>,
    metadata_bytes: usize,
}

struct PendingTurn {
    context_digest: String,
    provider_id: String,
    model: String,
    history_bytes: usize,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct GoogleTurnPayload {
    parts: Vec<Value>,
    /// In original functionCall part order; these are canonical native call IDs.
    call_ids: Vec<String>,
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
        ensure!(
            !google::is_cloud_code_assist(&endpoint)
                || api_key.is_empty()
                || google::is_loopback_endpoint(&endpoint),
            "Cloud Code Assist requires account credentials; an API key is permitted only in a loopback fixture"
        );
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
            first_event_timeout: timing.first_event_timeout(),
            completed_response: Arc::new(Mutex::new(None)),
            turn_lock: Arc::new(tokio::sync::Mutex::new(())),
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
        let turn_guard = self.turn_lock.clone().lock_owned().await;
        ensure!(
            self.completed_response
                .lock()
                .unwrap_or_else(|poison| poison.into_inner())
                .is_none(),
            "The canonical session has not consumed the previous Google response metadata"
        );
        let model = if options.model.trim().is_empty() {
            &self.default_model
        } else {
            &options.model
        };
        ensure!(
            !model.is_empty() && model.len() <= 1024,
            "Invalid Google model identity"
        );
        // Endpoint/account identity is private routing provenance, not a key.
        let provider_id = hex::encode(Sha256::digest(
            format!(
                "{}|{}",
                self.endpoint,
                self.credential_account
                    .as_deref()
                    .unwrap_or(google::API_KEY_ACCOUNT)
            )
            .as_bytes(),
        ));
        let retained = restore_turns(messages, model, &provider_id)?;
        ensure!(
            retained.len() < MAX_RETAINED_TURNS,
            "Google signed history reached its retention limit; start a new conversation"
        );
        let body = build_request(messages, tools, options, &retained)?;
        let (endpoint, body) = google::prepare_request(&self.endpoint, model, body, true)
            .map_err(|error| anyhow!(error))?;
        let credential = if let Some(key) = &self.fixture_key {
            GoogleCredential::ApiKey(key.clone())
        } else if google::is_cloud_code_assist(&self.endpoint) {
            resolve_google_account_credential().await?
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
        let pending = PendingTurn {
            context_digest: history_digest(messages)?,
            provider_id,
            model: model.clone(),
            history_bytes: retained.iter().map(|turn| turn.metadata_bytes).sum(),
        };
        tokio::spawn(async move {
            let _turn_guard = turn_guard;
            let result = transport
                .run_stream(endpoint, body, credential, pending, &tx)
                .await;
            if let Err(error) = result {
                drop(
                    transport
                        .completed_response
                        .lock()
                        .unwrap_or_else(|poison| poison.into_inner())
                        .take(),
                );
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

    fn take_completed_response_metadata(&self) -> Option<ProviderResponseMetadata> {
        self.completed_response
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .take()
    }

    async fn restore_response_chain(&self, _id: Option<String>) {
        // The receiver's drop cancels the transport. Wait for its handoff to
        // settle, then discard only ephemeral state; canonical history rolls
        // back with the owning AgentSession's message vector.
        let _turn = self.turn_lock.lock().await;
        drop(
            self.completed_response
                .lock()
                .unwrap_or_else(|poison| poison.into_inner())
                .take(),
        );
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

fn hash_message(digest: &mut Sha256, message: &Message) -> Result<()> {
    let bytes = serde_json::to_vec(message)?;
    digest.update((bytes.len() as u64).to_be_bytes());
    digest.update(bytes);
    Ok(())
}

fn history_digest(messages: &[Message]) -> Result<String> {
    let mut digest = Sha256::new();
    for message in messages {
        hash_message(&mut digest, message)?;
    }
    Ok(hex::encode(digest.finalize()))
}

fn assistant_segments(text: &str, calls: &[Call]) -> Vec<Vec<ContentBlock>> {
    let mut segments = Vec::new();
    if !text.trim().is_empty() {
        segments.push(vec![ContentBlock::Text(text.trim().to_string())]);
    }
    if !calls.is_empty() {
        segments.push(
            calls
                .iter()
                .map(|call| ContentBlock::ToolUse {
                    id: call.id.clone(),
                    name: call.name.clone(),
                    input: call.arguments.clone(),
                })
                .collect(),
        );
    }
    segments
}

/// Validate a payload before either committing it or replaying it. Occurrence
/// UUIDs establish identity; content comparisons below only check integrity.
fn decode_payload(payload: &GoogleTurnPayload) -> Result<(String, Vec<Call>)> {
    ensure!(
        payload.parts.len() <= MAX_TURN_PARTS
            && serde_json::to_vec(&payload.parts)?.len() <= MAX_TURN_PART_BYTES,
        "Google signed parts exceed their bounds"
    );
    let mut filter = PlanningFilter::default();
    let mut visible = String::new();
    let mut calls = Vec::new();
    let mut ids = HashSet::new();
    let mut wire_ids = HashSet::new();
    for part in &payload.parts {
        let object = part
            .as_object()
            .ok_or_else(|| anyhow!("Malformed Google stored part"))?;
        ensure!(
            object.keys().all(|key| matches!(
                key.as_str(),
                "text" | "thought" | "thoughtSignature" | "functionCall"
            )) && part.get("text").is_none_or(Value::is_string)
                && part.get("thought").is_none_or(Value::is_boolean)
                && part.get("thoughtSignature").is_none_or(Value::is_string)
                && !(part.get("text").is_some() && part.get("functionCall").is_some()),
            "Unsupported Google stored part schema"
        );
        if let Some(text) = part.get("text").and_then(Value::as_str)
            && part.get("thought").and_then(Value::as_bool) != Some(true)
            && let Some(text) = filter.push(text)
        {
            visible.push_str(&text);
        }
        if let Some(raw) = part.get("functionCall") {
            let call = raw
                .as_object()
                .ok_or_else(|| anyhow!("Malformed Google stored call"))?;
            ensure!(
                call.keys()
                    .all(|key| matches!(key.as_str(), "name" | "args" | "id")),
                "Unsupported Google stored function call schema"
            );
            let name = raw
                .get("name")
                .and_then(Value::as_str)
                .filter(|name| !name.is_empty() && name.len() <= 1024)
                .ok_or_else(|| anyhow!("Malformed Google stored tool name"))?;
            let arguments = raw.get("args").cloned().unwrap_or_else(|| json!({}));
            ensure!(
                arguments.is_object(),
                "Malformed Google stored tool arguments"
            );
            let id = payload
                .call_ids
                .get(calls.len())
                .ok_or_else(|| anyhow!("Missing Google native call identity"))?;
            ensure!(
                id.strip_prefix("google_")
                    .is_some_and(|id| uuid::Uuid::parse_str(id).is_ok_and(|id| !id.is_nil()))
                    && ids.insert(id.clone()),
                "Invalid or duplicate Google native call identity"
            );
            let wire_id = match raw.get("id") {
                Some(Value::String(id))
                    if !id.is_empty() && id.len() <= 1024 && wire_ids.insert(id.clone()) =>
                {
                    Some(id.clone())
                }
                None => None,
                _ => bail!("Invalid or duplicate Google wire call identity"),
            };
            calls.push(Call {
                id: id.clone(),
                name: name.to_string(),
                arguments,
                wire_id,
            });
        }
    }
    ensure!(
        calls.len() == payload.call_ids.len(),
        "Orphan Google native call identity"
    );
    if let Some(text) = filter.finish() {
        visible.push_str(&text);
    }
    Ok((visible, calls))
}

fn restore_turns(
    messages: &[Message],
    model: &str,
    provider_id: &str,
) -> Result<Vec<RetainedTurn>> {
    let mut turns = Vec::new();
    let mut groups = HashSet::new();
    let mut native_calls = HashSet::new();
    let mut digest = Sha256::new();
    let mut total_bytes = 0usize;
    let mut index = 0usize;
    while index < messages.len() {
        let metadata = messages[index]
            .metadata
            .as_ref()
            .map(|metadata| metadata.provider_wire())
            .transpose()?
            .flatten();
        let Some(first) = metadata else {
            ensure!(
                messages[index].role != Role::Assistant,
                "Google history has no original signed parts; start a new conversation"
            );
            hash_message(&mut digest, &messages[index])?;
            index += 1;
            continue;
        };
        ensure!(
            messages[index].role == Role::Assistant
                && first.segment_index == 0
                && groups.insert(first.occurrence_id.clone()),
            "Orphan or duplicate Google response occurrence"
        );
        let count = usize::from(first.segment_count);
        let response = first
            .response
            .as_ref()
            .ok_or_else(|| anyhow!("Missing Google wire response"))?;
        ensure!(
            response.wire == "google-generate-content"
                && response.model == model
                && response.provider_id == provider_id
                && response.context_digest == hex::encode(digest.clone().finalize()),
            "Google history provider, model or context does not match its signed response"
        );
        let metadata_bytes = serde_json::to_vec(response)?.len();
        total_bytes = total_bytes.saturating_add(metadata_bytes);
        ensure!(
            turns.len() < MAX_RETAINED_TURNS && total_bytes <= MAX_RETAINED_HISTORY_BYTES,
            "Google signed history exceeds its bounds"
        );
        let payload: GoogleTurnPayload = serde_json::from_value(response.payload.clone())
            .map_err(|_| anyhow!("Malformed Google signed payload"))?;
        let (visible, calls) = decode_payload(&payload)?;
        let expected = assistant_segments(&visible, &calls);
        ensure!(
            expected.len() == count && index + count <= messages.len(),
            "Invalid Google response segment count"
        );
        for offset in 0..count {
            let message = &messages[index + offset];
            let segment = message
                .metadata
                .as_ref()
                .map(|metadata| metadata.provider_wire())
                .transpose()?
                .flatten()
                .ok_or_else(|| anyhow!("Missing Google response segment"))?;
            ensure!(
                message.role == Role::Assistant
                    && segment.occurrence_id == first.occurrence_id
                    && segment.segment_count == first.segment_count
                    && usize::from(segment.segment_index) == offset
                    && message.content == expected[offset],
                "Google response occurrence does not match canonical content"
            );
            hash_message(&mut digest, message)?;
        }
        for call in &calls {
            ensure!(
                native_calls.insert(call.id.clone()),
                "Google call identity belongs to more than one occurrence"
            );
        }
        turns.push(RetainedTurn {
            prefix_len: index,
            message_count: count,
            parts: payload.parts,
            calls,
            metadata_bytes,
        });
        index += count;
    }
    Ok(turns)
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
    let mut pending_calls: HashMap<&str, &Call> = HashMap::new();
    let mut answered = HashSet::new();
    let mut index = 0;
    while index < messages.len() {
        if let Some(turn) = retained.iter().find(|turn| turn.prefix_len == index) {
            ensure!(
                pending_calls.is_empty(),
                "Google model turn precedes outstanding tool results"
            );
            contents.push(json!({"role":"model","parts":turn.parts}));
            for call in &turn.calls {
                pending_calls.insert(call.id.as_str(), call);
            }
            index += turn.message_count;
            continue;
        }
        let message = &messages[index];
        ensure!(
            message.role != Role::Assistant,
            "Google cannot resume this history without its original signed parts (provider/model changed, history restored or compacted); start a new conversation"
        );
        let mut parts = Vec::new();
        for block in &message.content {
            match block {
                ContentBlock::Text(text) => {
                    ensure!(
                        pending_calls.is_empty(),
                        "Google conversation text precedes outstanding tool results"
                    );
                    parts.push(json!({"text":text}));
                }
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
                    ensure!(
                        message.role == Role::User && answered.insert(tool_use_id.as_str()),
                        "Duplicate or non-user Google tool result"
                    );
                    let call = pending_calls.remove(tool_use_id.as_str()).ok_or_else(|| {
                        anyhow!("Google tool result has no preceding outstanding model call")
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
    ensure!(
        pending_calls.is_empty(),
        "Google history has incomplete tool results"
    );
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
        turn: PendingTurn,
        tx: &mpsc::Sender<AgentEvent>,
    ) -> Result<()> {
        let mut watchdog = StreamWatchdog::start(self.first_event_timeout, self.chunk_timeout);
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
            result = tokio::time::timeout(self.initial_timeout.min(watchdog.next_wait()), request.send()) => {
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
                chunk = tokio::time::timeout(
                    watchdog.next_wait(),
                    stream.next(),
                ) => chunk.map_err(|_| anyhow!(if state.content_started {
                    "Google stream stalled"
                } else {
                    "Google stream produced no content within the initial request budget"
                }))?,
            };
            match chunk {
                Some(Ok(bytes)) => {
                    for event in parser.push(&bytes)? {
                        state.consume(event, tx).await?;
                        if state.content_started {
                            watchdog.generation_started();
                        }
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
        let payload = GoogleTurnPayload {
            parts: state.parts,
            call_ids: state.calls.iter().map(|call| call.id.clone()).collect(),
        };
        let (visible, calls) = decode_payload(&payload)?;
        ensure!(
            assistant_segments(&visible, &calls)
                == assistant_segments(&state.visible, &state.calls),
            "Google completed wire state does not match its canonical response"
        );
        let metadata = ProviderResponseMetadata {
            wire: "google-generate-content".into(),
            provider_id: turn.provider_id,
            model: turn.model,
            context_digest: turn.context_digest,
            payload: serde_json::to_value(payload)?,
        };
        ensure!(
            turn.history_bytes
                .saturating_add(serde_json::to_vec(&metadata)?.len())
                <= MAX_RETAINED_HISTORY_BYTES,
            "Google signed history reached its retention limit; start a new conversation"
        );
        // One clean response handoff, not provider-owned conversation history.
        // AgentSession takes this only after accepting the terminal and EOF.
        *self
            .completed_response
            .lock()
            .unwrap_or_else(|poison| poison.into_inner()) = Some(metadata);
        tx.send(AgentEvent::TextDone(state.visible))
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
            self.event_bytes = self
                .event_bytes
                .saturating_add(data.len())
                .saturating_add(1);
            ensure!(
                self.event_bytes <= MAX_SSE_EVENT_BYTES && self.data.len() < MAX_SSE_DATA_LINES,
                "Google SSE event exceeds limit"
            );
            self.data.push(data.to_string());
        }
        Ok(())
    }
    fn finish(&mut self) -> Result<Vec<Value>> {
        std::str::from_utf8(&self.bytes).context("Truncated Google SSE UTF-8")?;
        ensure!(
            self.bytes.is_empty() && self.data.is_empty(),
            "Google SSE ended inside an unfinished frame"
        );
        Ok(Vec::new())
    }
}

#[derive(Default)]
struct ResponseState {
    parts: Vec<Value>,
    part_bytes: usize,
    calls: Vec<Call>,
    visible: String,
    text: PlanningFilter,
    clean_terminal: bool,
    content_started: bool,
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
        ensure!(envelope.is_object(), "Google SSE event must be an object");
        ensure!(
            envelope.get("error").is_none(),
            "Google reported an SSE error"
        );
        let value = envelope.get("response").unwrap_or(&envelope);
        ensure!(value.is_object(), "Google response must be an object");
        ensure!(value.get("error").is_none(), "Google reported an SSE error");
        ensure!(
            value.pointer("/promptFeedback/blockReason").is_none(),
            "Google blocked the prompt"
        );
        let Some(raw_candidates) = value.get("candidates") else {
            ensure!(
                value.get("usageMetadata").is_some(),
                "Google SSE event has no candidate or usage metadata"
            );
            return Ok(());
        };
        let candidates = raw_candidates
            .as_array()
            .ok_or_else(|| anyhow!("Google candidates must be an array"))?;
        ensure!(
            candidates.iter().all(|candidate| candidate.is_object()
                && candidate
                    .get("index")
                    .is_none_or(|index| index.as_u64().is_some())),
            "Google candidate or index is malformed"
        );
        ensure!(
            candidates
                .iter()
                .filter(
                    |candidate| candidate.get("index").and_then(Value::as_u64).unwrap_or(0) == 0
                )
                .count()
                <= 1,
            "Google returned more than one primary candidate"
        );
        // The Agent asks for one candidate. Never mix alternative answers/tools.
        let Some(candidate) = candidates
            .iter()
            .find(|candidate| candidate.get("index").and_then(Value::as_u64).unwrap_or(0) == 0)
        else {
            return Ok(());
        };
        if let Some(raw_parts) = candidate.pointer("/content/parts") {
            let parts = raw_parts
                .as_array()
                .ok_or_else(|| anyhow!("Google parts must be an array"))?;
            ensure!(
                !self.clean_terminal,
                "Google emitted content after its terminal"
            );
            for part in parts {
                ensure!(part.is_object(), "Google content part must be an object");
                ensure!(
                    part.get("text").is_none_or(Value::is_string)
                        && part.get("thought").is_none_or(Value::is_boolean)
                        && part.get("thoughtSignature").is_none_or(Value::is_string),
                    "Google content part has malformed text or signature metadata"
                );
                let part_bytes = serde_json::to_vec(part)?.len();
                self.part_bytes = self.part_bytes.saturating_add(part_bytes);
                ensure!(
                    self.parts.len() < MAX_TURN_PARTS && self.part_bytes <= MAX_TURN_PART_BYTES,
                    "Google response exceeds the signed-part retention limit"
                );
                retain_thought_signature(&mut self.parts, part);
                if let Some(text) = part.get("text").and_then(Value::as_str) {
                    self.content_started |= !text.is_empty();
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
                    self.content_started = true;
                    ensure!(
                        call.is_object() && call.get("id").is_none_or(Value::is_string),
                        "Google function call is malformed"
                    );
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
        if let Some(reason) = candidate.get("finishReason") {
            let reason = reason
                .as_str()
                .ok_or_else(|| anyhow!("Google finish reason must be a string"))?;
            ensure!(
                !self.clean_terminal,
                "Google emitted more than one terminal"
            );
            ensure!(
                reason == "STOP",
                "Google did not complete the response (non-STOP finish reason)"
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
    suppress_until_object_end: bool,
    suppress_permanently: bool,
}
impl PlanningFilter {
    fn push(&mut self, text: &str) -> Option<String> {
        if self.suppress_permanently {
            return None;
        }
        if self.decided && !self.suppress_until_object_end {
            return Some(text.to_string());
        }
        self.pending.push_str(text);
        let trimmed = self.pending.trim_start();
        if self.suppress_until_object_end || is_planning_leak_prefix(trimmed) {
            self.decided = true;
            self.suppress_until_object_end = true;
            if let Some(remainder) = strip_leading_planning_leak(trimmed) {
                self.suppress_until_object_end = false;
                let out = remainder.to_string();
                self.pending.clear();
                return (!out.is_empty()).then_some(out);
            }
            if self.pending.len() > 16_384 {
                self.suppress_until_object_end = false;
                self.suppress_permanently = true;
                self.pending.clear();
            }
            return None;
        }
        if trimmed.is_empty() {
            if self.pending.len() > 512 {
                self.decided = true;
                self.suppress_permanently = true;
                self.pending.clear();
            }
            return None;
        }
        // Hold a possible object/fence prefix across arbitrary SSE parts. Once
        // the first key differs it is ordinary answer text and can flow.
        let possible = ["{", "```"]
            .iter()
            .any(|prefix| prefix.starts_with(trimmed) || trimmed.starts_with(prefix));
        if possible && !trimmed.contains(':') {
            if self.pending.len() > 512 {
                self.decided = true;
                self.suppress_permanently = true;
                self.pending.clear();
            }
            return None;
        }
        self.decided = true;
        Some(std::mem::take(&mut self.pending))
    }
    fn finish(&mut self) -> Option<String> {
        // An unfinished possible planning JSON must never leak on EOF/STOP.
        if self.suppress_permanently
            || self.suppress_until_object_end
            || (!self.decided && self.pending.trim_start().starts_with(['{', '`']))
        {
            self.pending.clear();
            return None;
        }
        let out = std::mem::take(&mut self.pending);
        (!out.is_empty()).then_some(out)
    }
}

fn strip_leading_planning_leak(trimmed: &str) -> Option<&str> {
    let fenced = trimmed.starts_with("```");
    let body = trimmed
        .strip_prefix("```json")
        .or_else(|| trimmed.strip_prefix("```"))
        .map(str::trim_start)
        .unwrap_or(trimmed);
    let (_, after_obj) = split_leading_json_object(body)?;
    let rest = after_obj.trim_start();
    let rest = if fenced {
        rest.strip_prefix("```")?.trim_start()
    } else {
        rest
    };
    Some(rest)
}

fn split_leading_json_object(text: &str) -> Option<(&str, &str)> {
    let bytes = text.as_bytes();
    if bytes.first().copied() != Some(b'{') {
        return None;
    }
    let mut depth = 0usize;
    let mut in_string = false;
    let mut escaped = false;
    for (idx, &byte) in bytes.iter().enumerate() {
        if in_string {
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == b'"' {
                in_string = false;
            }
            continue;
        }
        match byte {
            b'"' => in_string = true,
            b'{' => depth += 1,
            b'}' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    let end = idx + 1;
                    return Some((&text[..end], &text[end..]));
                }
            }
            _ => {}
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use codescribe_core::agent::{ToolExecutionPolicy, ToolOrigin, ToolRegistry, ToolRisk};

    fn timing() -> RuntimeAiRequestTiming {
        codescribe_core::config::Config::runtime_snapshot_from_captured(
            codescribe_core::config::CapturedRuntimeInputs::defaults_at(
                "/fixture/google".into(),
                0,
            ),
        )
        .ai_execution()
        .request_timing()
        .clone()
    }
    fn user(text: &str) -> Message {
        Message::new(Role::User, vec![ContentBlock::Text(text.into())])
    }

    #[tokio::test]
    async fn real_session_lazy_mcp_restores_thread_and_rejects_tampering() {
        use codescribe_core::agent::{AgentSession, ToolResultContent};
        use std::sync::atomic::{AtomicUsize, Ordering};
        let mut server = mockito::Server::new_async().await;
        let path = "/v1beta/models/gemini-test:streamGenerateContent?alt=sse";
        let parts = json!([
            {"thought":true,"text":"planning","thoughtSignature":"thought-distinct"},
            {"text":"","thoughtSignature":"empty-distinct"},
            {"functionCall":{"name":"mcp_search_tools","args":{"query":"read"}},"thoughtSignature":"search-distinct"}
        ]);
        let first = server
            .mock("POST", path)
            .match_request(|request| {
                serde_json::from_slice::<Value>(request.body().unwrap()).unwrap()["contents"]
                    .as_array()
                    .unwrap()
                    .len()
                    == 1
            })
            .with_header("content-type", "text/event-stream")
            .with_body(sse(
                json!({"candidates":[{"content":{"parts":parts},"finishReason":"STOP"}]}),
            ))
            .create_async()
            .await;
        let expected = parts.clone();
        let call_parts = json!([{ "functionCall": {"name":"call_mcp_tool", "id":"wire-read", "args":{"server":"local","tool":"read","arguments":{"path":"fixture.txt"}}}, "thoughtSignature":"read-distinct" }]);
        let second = server
            .mock("POST", path)
            .match_request(move |request| {
                let body: Value = serde_json::from_slice(request.body().unwrap()).unwrap();
                body["contents"].as_array().unwrap().len() == 3
                    && body["contents"][1]["parts"] == expected
                    && body["contents"][2]["parts"][0]["functionResponse"]["name"]
                        == "mcp_search_tools"
            })
            .with_header("content-type", "text/event-stream")
            .with_body(sse(
                json!({"candidates":[{"content":{"parts":call_parts},"finishReason":"STOP"}]}),
            ))
            .create_async()
            .await;
        let expected = parts.clone();
        let expected_call = call_parts.clone();
        let third = server.mock("POST", path)
            .match_request(move |request| {
                let body: Value = serde_json::from_slice(request.body().unwrap()).unwrap();
                body["contents"].as_array().unwrap().len() == 5 && body["contents"][1]["parts"] == expected
                    && body["contents"][3]["parts"] == expected_call
                    && body["contents"][4]["parts"][0]["functionResponse"]["id"] == "wire-read"
            })
            .with_header("content-type", "text/event-stream")
            .with_body(sse(json!({"candidates":[{"content":{"parts":[{"text":"done"}]},"finishReason":"STOP"}]})))
            .create_async().await;
        let count = Arc::new(AtomicUsize::new(0));
        let called = count.clone();
        let mut tools = ToolRegistry::new();
        tools.register_with_policy(ToolDefinition {name:"mcp__local__read".into(),description:"read fixture".into(),input_schema:json!({"type":"object","properties":{"path":{"type":"string"}},"required":["path"]})}, Box::new(move |_| {
            let called = called.clone();
            Box::pin(async move {called.fetch_add(1, Ordering::SeqCst); vec![ToolResultContent::Text("fixture result".into())]})
        }), ToolExecutionPolicy {origin:ToolOrigin::Mcp {server:"local".into(),upstream_tool:"read".into()},risk:ToolRisk::ReadOnly,requires_approval:false},None).unwrap();
        let endpoint = format!("{}/v1beta", server.url());
        let provider = GoogleProvider::from_configuration(
            endpoint.clone(),
            "gemini-test".into(),
            "fixture-key".into(),
            &timing(),
        )
        .unwrap();
        let (tx, _rx) = mpsc::channel(128);
        let mut session = AgentSession::new(Box::new(provider), Arc::new(tools), tx);
        session
            .send("read".into(), Vec::new(), &StreamOptions::default())
            .await
            .unwrap();
        assert_eq!(count.load(Ordering::SeqCst), 1);
        for mock in [first, second, third] {
            mock.assert_async().await;
        }
        use codescribe_core::agent::thread_store::{Thread, ThreadMessage, ThreadStore};
        let root = tempfile::tempdir().unwrap();
        let store = ThreadStore::new_in(root.path().join("threads")).unwrap();
        let now = chrono::Utc::now();
        let thread = Thread {
            id: ThreadStore::generate_id(),
            created_at: now,
            updated_at: now,
            title: "fixture".into(),
            title_is_custom: false,
            title_is_generated: false,
            mode: "assistive".into(),
            tags: Vec::new(),
            notes: Vec::new(),
            messages: session.messages().iter().map(ThreadMessage::from).collect(),
            summary: None,
            total_tokens: None,
            provider: "google-gemini".into(),
            model: "gemini-test".into(),
        };
        store.save_thread(&thread).unwrap();
        assert!(
            std::fs::read_to_string(store.thread_file_path(&thread.id).unwrap())
                .unwrap()
                .contains("empty-distinct")
        );
        for include_details in [false, true] {
            assert!(
                !codescribe_core::agent::thread_export::thread_to_markdown(
                    &thread,
                    include_details
                )
                .contains("empty-distinct")
            );
        }
        assert!(!format!("{thread:?}").contains("empty-distinct"));
        let restored_messages = store
            .load_thread(&thread.id)
            .unwrap()
            .messages
            .iter()
            .map(ThreadMessage::try_to_message)
            .collect::<Result<Vec<_>>>()
            .unwrap();
        let fourth = server.mock("POST", path)
            .match_request(|request| {
                let body: Value = serde_json::from_slice(request.body().unwrap()).unwrap();
                body["contents"].as_array().unwrap().len() == 7
                    && body["contents"][1]["parts"][1]["thoughtSignature"] == "empty-distinct"
                    && body["contents"][6]["parts"][0]["text"] == "again"
            })
            .with_header("content-type", "text/event-stream")
            .with_body(sse(json!({"candidates":[{"content":{"parts":[{"text":"second answer"}]},"finishReason":"STOP"}]})))
            .create_async().await;
        let restored = GoogleProvider::from_configuration(
            endpoint.clone(),
            "gemini-test".into(),
            "fixture-key".into(),
            &timing(),
        )
        .unwrap();
        let (tx, _rx) = mpsc::channel(128);
        let mut resumed = AgentSession::new(Box::new(restored), Arc::new(ToolRegistry::new()), tx);
        resumed.restore_messages(restored_messages);
        resumed
            .send("again".into(), Vec::new(), &StreamOptions::default())
            .await
            .unwrap();
        fourth.assert_async().await;
        assert!(!format!("{:?}", resumed.messages()).contains("empty-distinct"));
        for mutation in ["version", "context", "call", "missing"] {
            let mut messages = resumed.messages().to_vec();
            let message = messages
                .iter_mut()
                .find(|message| message.role == Role::Assistant)
                .unwrap();
            let mut raw = message.metadata.as_ref().unwrap().as_stored().clone();
            match mutation {
                "version" => raw["provider_wire"]["version"] = json!(2),
                "context" => {
                    raw["provider_wire"]["response"]["context_digest"] = json!("0".repeat(64))
                }
                "call" => {
                    raw["provider_wire"]["response"]["payload"]["parts"][2]["functionCall"]["args"]
                        ["query"] = json!("tampered")
                }
                _ => {}
            }
            message.metadata = (mutation != "missing")
                .then(|| codescribe_core::agent::MessageMetadata::from_stored(raw));
            let provider = GoogleProvider::from_configuration(
                endpoint.clone(),
                "gemini-test".into(),
                "fixture-key".into(),
                &timing(),
            )
            .unwrap();
            assert!(
                provider
                    .stream(&messages, &[], &StreamOptions::default())
                    .await
                    .is_err(),
                "mutation={mutation}"
            );
        }
    }

    #[tokio::test]
    async fn real_session_never_executes_google_tool_from_dirty_terminal() {
        use codescribe_core::agent::{AgentSession, ToolResultContent};
        use std::sync::atomic::{AtomicUsize, Ordering};
        let mut server = mockito::Server::new_async().await;
        let mock = server.mock("POST", "/v1beta/models/gemini-test:streamGenerateContent?alt=sse")
            .with_header("content-type", "text/event-stream")
            .with_body(sse(json!({"candidates":[{"content":{"parts":[{"functionCall":{"name":"fixture_mutation","args":{}}}]},"finishReason":"MAX_TOKENS"}]})))
            .create_async().await;
        let count = Arc::new(AtomicUsize::new(0));
        let called = count.clone();
        let mut tools = ToolRegistry::new();
        tools
            .register_with_policy(
                ToolDefinition {
                    name: "fixture_mutation".into(),
                    description: "fixture".into(),
                    input_schema: json!({"type":"object"}),
                },
                Box::new(move |_| {
                    let called = called.clone();
                    Box::pin(async move {
                        called.fetch_add(1, Ordering::SeqCst);
                        Vec::<ToolResultContent>::new()
                    })
                }),
                ToolExecutionPolicy::native(ToolRisk::ReadOnly),
                None,
            )
            .unwrap();
        let provider = GoogleProvider::from_configuration(
            format!("{}/v1beta", server.url()),
            "gemini-test".into(),
            "fixture-key".into(),
            &timing(),
        )
        .unwrap();
        let (tx, _rx) = mpsc::channel(32);
        let mut session = AgentSession::new(Box::new(provider), Arc::new(tools), tx);
        assert!(
            session
                .send("run".into(), Vec::new(), &StreamOptions::default())
                .await
                .is_err()
        );
        assert_eq!(count.load(Ordering::SeqCst), 0);
        mock.assert_async().await;
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
        use codescribe_core::agent::{AgentSession, AgentUiEvent, ToolResultContent};
        let mut server = mockito::Server::new_async().await;
        let path = "/v1internal:streamGenerateContent?alt=sse";
        let parts = json!([
            {"thought":true,"text":"Sprawdzę","thoughtSignature":"sig_thought"},
            {"text":"Zobaczę."},
            {"functionCall":{"name":"mcp_search_tools","args":{"query":"repo"}},"thoughtSignature":"sig_abc123"},
            {"functionCall":{"name":"call_mcp_tool","id":"wire-call","args":{"server":"local","tool":"read","arguments":{"path":"fixture.txt"}}},"thoughtSignature":"sig_parallel"}
        ]);
        let first = server.mock("POST",path).match_header("x-goog-api-key","fixture-key")
            .match_request(|request| {
                let body: Value = serde_json::from_slice(request.body().unwrap()).unwrap();
                body["request"]["contents"].as_array().unwrap().len() == 1 && body["project"] == "p"
            })
            .with_header("content-type","text/event-stream")
            .with_body(sse(json!({"response":{"candidates":[{"content":{"parts":parts},"finishReason":"STOP"}]}})))
            .create_async().await;
        let expected = parts.clone();
        let second = server.mock("POST",path).match_request(move |request| {
            let body: Value = serde_json::from_slice(request.body().unwrap()).unwrap();
            let contents = &body["request"]["contents"];
            contents.as_array().unwrap().len() == 3 && contents[1]["parts"] == expected
                && contents[2]["parts"].as_array().unwrap().len() == 2
                && contents[2]["parts"][0]["functionResponse"]["name"] == "mcp_search_tools"
                && contents[2]["parts"][1]["functionResponse"]["id"] == "wire-call"
        }).with_header("content-type","text/event-stream")
            .with_body(sse(json!({"response":{"candidates":[{"content":{"parts":[{"text":"Gotowe 🐾"}]},"finishReason":"STOP"}]}})))
            .create_async().await;
        let provider = GoogleProvider::from_configuration(
            format!("{}/v1internal?project=p", server.url()),
            "gemini-test".into(),
            "fixture-key".into(),
            &timing(),
        )
        .unwrap();
        let retained_handle = provider.clone();
        let mut tools = ToolRegistry::new();
        tools.register_with_policy(ToolDefinition {name:"mcp__local__read".into(),description:"read fixture repository".into(),input_schema:json!({"type":"object","properties":{"path":{"type":"string"}},"required":["path"]})}, Box::new(|_| Box::pin(async {vec![ToolResultContent::Text("found".into())]})), ToolExecutionPolicy {origin:ToolOrigin::Mcp {server:"local".into(),upstream_tool:"read".into()},risk:ToolRisk::ReadOnly,requires_approval:false},None).unwrap();
        let (tx, mut rx) = mpsc::channel(128);
        let mut session = AgentSession::new(Box::new(provider), Arc::new(tools), tx);
        session
            .send("Szukaj".into(), Vec::new(), &StreamOptions::default())
            .await
            .unwrap();
        let mut reasoning = false;
        let mut done = false;
        while let Ok(event) = rx.try_recv() {
            reasoning |= matches!(&event,AgentUiEvent::ReasoningDelta(text) if text=="Sprawdzę");
            done |= matches!(&event,AgentUiEvent::TextDone(text) if text=="Gotowe 🐾");
            assert!(!format!("{event:?}").contains("sig_parallel"));
        }
        assert!(reasoning && done);
        first.assert_async().await;
        second.assert_async().await;
        let mut rolled_back = session.messages().to_vec();
        rolled_back[0] = user("Different thread");
        assert!(
            retained_handle
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
        let wire = ": ping\r\ndata: {\"candidates\":\r\ndata: [{\"content\":{\"parts\":[{\"text\":\"Zażółć 🐾\"}]},\"finishReason\":\"STOP\"}]}\r\n\r\n";
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
                .with_body(concat!(
                    "AI",
                    "zaDoNotLeak ya",
                    "29.NeverReveal arbitrary-fixture-secret"
                ))
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

        let mut filter = PlanningFilter::default();
        assert_eq!(filter.push("{\"thought\": \"plan"), None);
        assert_eq!(
            filter.push("ning step\"} Real visible answer"),
            Some("Real visible answer".into())
        );
        assert_eq!(filter.push(" continues."), Some(" continues.".into()));
        assert_eq!(filter.finish(), None);
    }
}
