use anyhow::{Context, Result};
use futures_util::StreamExt;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::time::Duration;
use tokio::sync::mpsc;
use tracing::{debug, info, warn};

use crate::agent::event::AgentEvent;

use super::ai_formatting::{AiReasoningCallback, AiStreamCallback};

const STREAM_REQUEST_TIMEOUT: Duration = Duration::from_secs(3600);
const STREAM_DEADLINE: Duration = Duration::from_secs(10 * 60);

#[derive(Debug, Clone)]
pub struct ResponsesStreamOutput {
    pub assistant_text: String,
    pub reasoning_text: Option<String>,
    pub response_id: Option<String>,
}

#[derive(Clone)]
pub struct StreamCallbacks {
    pub assistant: Option<AiStreamCallback>,
    pub reasoning: Option<AiReasoningCallback>,
}

pub struct ResponsesStreamingManager<'a> {
    client: &'a Client,
    endpoint: &'a str,
    api_key: &'a str,
    callbacks: StreamCallbacks,
    initial_response_timeout: Duration,
    inter_chunk_timeout: Duration,
}

impl<'a> ResponsesStreamingManager<'a> {
    pub fn new(
        client: &'a Client,
        endpoint: &'a str,
        api_key: &'a str,
        callbacks: StreamCallbacks,
        initial_response_timeout: Duration,
        inter_chunk_timeout: Duration,
    ) -> Self {
        Self {
            client,
            endpoint,
            api_key,
            callbacks,
            initial_response_timeout,
            inter_chunk_timeout,
        }
    }

    pub async fn stream<T: Serialize>(&self, request: &T) -> Result<ResponsesStreamOutput> {
        let endpoint_url =
            validated_endpoint_url(self.endpoint).context("Invalid Responses API endpoint URL")?;
        let request_builder = self
            .client
            // nosemgrep: rust.actix.ssrf.reqwest-taint.reqwest-taint -- URL is validated by `validated_endpoint_url`.
            .post(endpoint_url.clone())
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("Content-Type", "application/json")
            .header("Accept", "text/event-stream")
            .timeout(STREAM_REQUEST_TIMEOUT)
            .json(request);
        let response =
            match tokio::time::timeout(self.initial_response_timeout, request_builder.send()).await
            {
                Ok(Ok(resp)) => resp,
                Ok(Err(e)) => return Err(e).context("SSE request failed"),
                Err(_) => {
                    anyhow::bail!(
                        "SSE initial response timeout after {:?}",
                        self.initial_response_timeout
                    );
                }
            };

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            anyhow::bail!("HTTP {} - {}", status, body);
        }

        let mut assistant_text = String::new();
        let mut reasoning_text = String::new();
        let mut assistant_done: Option<String> = None;
        let mut reasoning_done: Option<String> = None;
        let mut completed_output: Option<(String, Option<String>)> = None;
        let mut saw_completed = false;
        let mut response_id: Option<String> = None;
        let mut stream = response.bytes_stream();
        let mut buffer = String::new();
        let mut saw_done = false;
        let mut last_sequence_number: Option<u64> = None;
        let stream_deadline = tokio::time::Instant::now() + STREAM_DEADLINE;

        loop {
            if tokio::time::Instant::now() > stream_deadline {
                warn!(
                    "SSE global safety timeout (10min); returning {}B partial text",
                    assistant_text.len()
                );
                break;
            }
            let next_chunk = match tokio::time::timeout(self.inter_chunk_timeout, stream.next())
                .await
            {
                Ok(chunk) => chunk,
                Err(_) => {
                    if !assistant_text.is_empty() || assistant_done.is_some() {
                        let partial_len = assistant_text.len();
                        if let (Some(resp_id), Some(seq)) =
                            (response_id.as_deref(), last_sequence_number)
                        {
                            info!(
                                "SSE inter-chunk timeout with {}B partial text; \
                                 attempting resume (response_id={}, seq={})",
                                partial_len, resp_id, seq
                            );
                            match self.resume_stream(resp_id, seq).await {
                                Ok(resumed) => {
                                    assistant_text.push_str(&resumed.assistant_text);
                                    if let Some(r) = resumed.reasoning_text {
                                        reasoning_text.push_str(&r);
                                    }
                                    saw_completed = true;
                                    break;
                                }
                                Err(e) => {
                                    anyhow::bail!(
                                        "SSE inter-chunk timeout after {:?} with {}B partial text; \
                                         resume failed: {}",
                                        self.inter_chunk_timeout,
                                        partial_len,
                                        e
                                    );
                                }
                            }
                        }
                        anyhow::bail!(
                            "SSE inter-chunk timeout after {:?} with {}B partial text \
                             (resume unavailable)",
                            self.inter_chunk_timeout,
                            partial_len
                        );
                    } else {
                        anyhow::bail!(
                            "SSE stream inter-chunk timeout after {:?} (no data received)",
                            self.inter_chunk_timeout
                        );
                    }
                }
            };

            let Some(chunk_result) = next_chunk else {
                break;
            };

            let chunk = chunk_result.context("Stream read error")?;
            buffer.push_str(&String::from_utf8_lossy(&chunk));

            while let Some(newline_pos) = buffer.find('\n') {
                let line = buffer[..newline_pos].trim().to_string();
                buffer = buffer[newline_pos + 1..].to_string();

                if line.is_empty() || line.starts_with(':') {
                    continue;
                }

                if let Some(data) = line
                    .strip_prefix("data: ")
                    .or_else(|| line.strip_prefix("data:"))
                {
                    let data = data.trim_start();
                    if data == "[DONE]" {
                        saw_done = true;
                        break;
                    }

                    if let Ok(chunk) = serde_json::from_str::<StreamChunk>(data) {
                        if let Some(seq) = chunk.sequence_number {
                            last_sequence_number = Some(seq);
                        }
                        if let Some(resp) = &chunk.response
                            && !resp.id.is_empty()
                        {
                            response_id = Some(resp.id.clone());
                        }
                        match chunk.chunk_type.as_str() {
                            "response.output_text.delta" => {
                                if let Some(delta) = chunk.delta {
                                    if let Some(cb) = &self.callbacks.assistant {
                                        cb(&delta);
                                    }
                                    assistant_text.push_str(&delta);
                                }
                            }
                            "response.output_text.done" => {
                                if let Some(text) = chunk.text {
                                    let text = text.trim().to_string();
                                    if !text.is_empty() {
                                        if assistant_text.is_empty()
                                            && let Some(cb) = &self.callbacks.assistant
                                        {
                                            cb(&text);
                                        }
                                        assistant_done = Some(text);
                                    }
                                }
                            }
                            "response.reasoning_summary_text.delta" => {
                                if let Some(delta) = chunk.delta {
                                    if let Some(cb) = &self.callbacks.reasoning {
                                        cb(&delta);
                                    }
                                    reasoning_text.push_str(&delta);
                                }
                            }
                            "response.reasoning_summary_text.done" => {
                                if let Some(text) = chunk.text {
                                    let text = text.trim().to_string();
                                    if !text.is_empty() {
                                        if reasoning_text.is_empty()
                                            && let Some(cb) = &self.callbacks.reasoning
                                        {
                                            cb(&text);
                                        }
                                        reasoning_done = Some(text);
                                    }
                                }
                            }
                            "response.completed" | "response.done" => {
                                saw_completed = true;
                                if let Some(resp) = &chunk.response {
                                    let parsed = extract_output_channels(&resp.output);
                                    if !parsed.0.is_empty() {
                                        completed_output = Some(parsed);
                                    } else {
                                        warn!(
                                            "SSE response.completed without parseable output_text payload; falling back to channel buffers"
                                        );
                                    }
                                }
                            }
                            _ => {}
                        }
                    }
                }
            }

            if saw_done {
                break;
            }
        }

        let (assistant_text, reasoning_text) = if saw_completed {
            if let Some(completed) = completed_output {
                completed
            } else {
                (
                    assistant_done.unwrap_or(assistant_text),
                    fallback_reasoning(reasoning_done, reasoning_text),
                )
            }
        } else {
            (
                assistant_done.unwrap_or(assistant_text),
                fallback_reasoning(reasoning_done, reasoning_text),
            )
        };

        let assistant_text = assistant_text.trim().to_string();
        let reasoning_text = reasoning_text
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(ToString::to_string);

        if assistant_text.is_empty() {
            anyhow::bail!("No text content in SSE stream");
        }

        Ok(ResponsesStreamOutput {
            assistant_text,
            reasoning_text,
            response_id,
        })
    }

    pub async fn stream_agent<T: Serialize>(
        &self,
        request: &T,
    ) -> Result<mpsc::Receiver<AgentEvent>> {
        let request_payload =
            serde_json::to_value(request).context("Failed to serialize agent stream request")?;

        let (tx, rx) = mpsc::channel(256);

        let client = self.client.clone();
        let endpoint = self.endpoint.to_string();
        let api_key = self.api_key.to_string();
        let callbacks = self.callbacks.clone();
        let initial_response_timeout = self.initial_response_timeout;
        let inter_chunk_timeout = self.inter_chunk_timeout;

        tokio::spawn(async move {
            if let Err(error) = run_agent_stream(
                client,
                endpoint,
                api_key,
                callbacks,
                initial_response_timeout,
                inter_chunk_timeout,
                request_payload,
                tx.clone(),
            )
            .await
            {
                let _ = tx.send(AgentEvent::Error(error.to_string())).await;
            }
        });

        Ok(rx)
    }

    async fn resume_stream(
        &self,
        response_id: &str,
        starting_after: u64,
    ) -> Result<ResponsesStreamOutput> {
        let base = self.endpoint.trim_end_matches('/');
        let resume_url =
            format!("{base}/{response_id}?stream=true&starting_after={starting_after}");

        debug!(
            "Resuming SSE stream: {} (after seq={})",
            resume_url, starting_after
        );

        let response = self
            .client
            .get(&resume_url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("Accept", "text/event-stream")
            .timeout(STREAM_REQUEST_TIMEOUT)
            .send()
            .await
            .context("SSE resume request failed")?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            anyhow::bail!("Resume HTTP {} - {}", status, body);
        }

        let mut assistant_text = String::new();
        let mut reasoning_text = String::new();
        let mut stream = response.bytes_stream();
        let mut buffer = String::new();

        loop {
            let next_chunk = match tokio::time::timeout(self.inter_chunk_timeout, stream.next())
                .await
            {
                Ok(chunk) => chunk,
                Err(_) => {
                    warn!(
                        "Resume stream inter-chunk timeout after {:?}; returning what we have ({}B)",
                        self.inter_chunk_timeout,
                        assistant_text.len()
                    );
                    break;
                }
            };

            let Some(chunk_result) = next_chunk else {
                break;
            };
            let chunk = chunk_result.context("Resume stream read error")?;
            buffer.push_str(&String::from_utf8_lossy(&chunk));

            while let Some(newline_pos) = buffer.find('\n') {
                let line = buffer[..newline_pos].trim().to_string();
                buffer = buffer[newline_pos + 1..].to_string();

                if line.is_empty() || line.starts_with(':') {
                    continue;
                }

                if let Some(data) = line
                    .strip_prefix("data: ")
                    .or_else(|| line.strip_prefix("data:"))
                {
                    let data = data.trim_start();
                    if data == "[DONE]" {
                        return Ok(ResponsesStreamOutput {
                            assistant_text,
                            reasoning_text: if reasoning_text.is_empty() {
                                None
                            } else {
                                Some(reasoning_text)
                            },
                            response_id: Some(response_id.to_string()),
                        });
                    }

                    if let Ok(chunk) = serde_json::from_str::<StreamChunk>(data) {
                        match chunk.chunk_type.as_str() {
                            "response.output_text.delta" => {
                                if let Some(delta) = chunk.delta {
                                    if let Some(cb) = &self.callbacks.assistant {
                                        cb(&delta);
                                    }
                                    assistant_text.push_str(&delta);
                                }
                            }
                            "response.reasoning_summary_text.delta" => {
                                if let Some(delta) = chunk.delta {
                                    if let Some(cb) = &self.callbacks.reasoning {
                                        cb(&delta);
                                    }
                                    reasoning_text.push_str(&delta);
                                }
                            }
                            "response.completed" | "response.done" => {
                                debug!(
                                    "Resume stream completed ({}B accumulated text)",
                                    assistant_text.len()
                                );
                                break;
                            }
                            _ => {}
                        }
                    }
                }
            }
        }

        Ok(ResponsesStreamOutput {
            assistant_text,
            reasoning_text: if reasoning_text.is_empty() {
                None
            } else {
                Some(reasoning_text)
            },
            response_id: Some(response_id.to_string()),
        })
    }
}

#[allow(clippy::too_many_arguments)]
async fn run_agent_stream(
    client: Client,
    endpoint: String,
    api_key: String,
    callbacks: StreamCallbacks,
    initial_response_timeout: Duration,
    inter_chunk_timeout: Duration,
    request_payload: serde_json::Value,
    tx: mpsc::Sender<AgentEvent>,
) -> Result<()> {
    let endpoint_url =
        validated_endpoint_url(&endpoint).context("Invalid agent streaming endpoint URL")?;
    let request_builder = client
        // nosemgrep: rust.actix.ssrf.reqwest-taint.reqwest-taint -- URL is validated by `validated_endpoint_url`.
        .post(endpoint_url)
        .header("Authorization", format!("Bearer {}", api_key))
        .header("Content-Type", "application/json")
        .header("Accept", "text/event-stream")
        .timeout(STREAM_REQUEST_TIMEOUT)
        .json(&request_payload);

    let response =
        match tokio::time::timeout(initial_response_timeout, request_builder.send()).await {
            Ok(Ok(resp)) => resp,
            Ok(Err(error)) => return Err(error).context("Agent SSE request failed"),
            Err(_) => anyhow::bail!(
                "Agent SSE initial response timeout after {:?}",
                initial_response_timeout
            ),
        };

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        anyhow::bail!("Agent SSE HTTP {} - {}", status, body);
    }

    let mut tool_tracker = ToolCallTracker::default();
    let mut response_id: Option<String> = None;
    let mut sent_done_event = false;

    let mut stream = response.bytes_stream();
    let mut buffer = String::new();
    let mut saw_done = false;
    let stream_deadline = tokio::time::Instant::now() + STREAM_DEADLINE;

    loop {
        if tokio::time::Instant::now() > stream_deadline {
            anyhow::bail!(
                "Agent SSE global safety timeout after {:?}",
                STREAM_DEADLINE
            );
        }

        let next_chunk = match tokio::time::timeout(inter_chunk_timeout, stream.next()).await {
            Ok(chunk) => chunk,
            Err(_) => {
                anyhow::bail!(
                    "Agent SSE inter-chunk timeout after {:?}",
                    inter_chunk_timeout
                );
            }
        };

        let Some(chunk_result) = next_chunk else {
            break;
        };
        let chunk = chunk_result.context("Agent SSE stream read error")?;
        buffer.push_str(&String::from_utf8_lossy(&chunk));

        while let Some(newline_pos) = buffer.find('\n') {
            let line = buffer[..newline_pos].trim().to_string();
            buffer = buffer[newline_pos + 1..].to_string();

            if line.is_empty() || line.starts_with(':') {
                continue;
            }

            let Some(data) = line
                .strip_prefix("data: ")
                .or_else(|| line.strip_prefix("data:"))
            else {
                continue;
            };
            let data = data.trim_start();
            if data == "[DONE]" {
                saw_done = true;
                break;
            }

            let chunk = match serde_json::from_str::<StreamChunk>(data) {
                Ok(parsed) => parsed,
                Err(error) => {
                    warn!("Skipping malformed SSE chunk: {}", error);
                    continue;
                }
            };

            if let Some(response_meta) = &chunk.response
                && !response_meta.id.is_empty()
            {
                response_id = Some(response_meta.id.clone());
            }

            if let Some(event) =
                parse_agent_event(&chunk, &mut tool_tracker, response_id.as_deref())
            {
                match &event {
                    AgentEvent::TextDelta(delta) => {
                        if let Some(callback) = &callbacks.assistant {
                            callback(delta);
                        }
                    }
                    AgentEvent::ReasoningDelta(delta) => {
                        if let Some(callback) = &callbacks.reasoning {
                            callback(delta);
                        }
                    }
                    AgentEvent::ResponseDone { .. } => {
                        sent_done_event = true;
                    }
                    AgentEvent::TextDone(_)
                    | AgentEvent::ToolCallStart { .. }
                    | AgentEvent::ToolCallArgsDelta { .. }
                    | AgentEvent::ToolCallReady { .. }
                    | AgentEvent::Error(_) => {}
                }

                if tx.send(event).await.is_err() {
                    return Ok(());
                }
            }
        }

        if saw_done {
            break;
        }
    }

    if !sent_done_event
        && tx
            .send(AgentEvent::ResponseDone { response_id })
            .await
            .is_err()
    {
        return Ok(());
    }

    Ok(())
}

fn validated_endpoint_url(endpoint: &str) -> Result<reqwest::Url> {
    let endpoint = endpoint.trim();
    if endpoint.is_empty() {
        anyhow::bail!("Endpoint URL is empty");
    }

    let url = reqwest::Url::parse(endpoint).context("Endpoint is not a valid URL")?;
    match url.scheme() {
        "https" | "http" => {}
        other => anyhow::bail!("Unsupported endpoint URL scheme: {}", other),
    }

    if url.host_str().is_none() {
        anyhow::bail!("Endpoint URL is missing a host");
    }

    Ok(url)
}

#[derive(Debug, Clone, Default)]
struct ToolCallTracker {
    by_item_id: HashMap<String, ToolCallMeta>,
    names_by_call_id: HashMap<String, String>,
    arguments_by_call_id: HashMap<String, String>,
}

#[derive(Debug, Clone)]
struct ToolCallMeta {
    call_id: String,
    name: String,
}

fn parse_agent_event(
    chunk: &StreamChunk,
    tracker: &mut ToolCallTracker,
    fallback_response_id: Option<&str>,
) -> Option<AgentEvent> {
    match chunk.chunk_type.as_str() {
        "response.output_text.delta" => chunk.delta.clone().map(AgentEvent::TextDelta),
        "response.output_text.done" => chunk
            .text
            .as_deref()
            .map(str::trim)
            .filter(|text| !text.is_empty())
            .map(|text| AgentEvent::TextDone(text.to_string())),
        "response.reasoning_summary_text.delta" => {
            chunk.delta.clone().map(AgentEvent::ReasoningDelta)
        }
        "response.output_item.added" => parse_tool_call_start(chunk, tracker),
        "response.function_call_arguments.delta" => parse_tool_call_args_delta(chunk, tracker),
        "response.function_call_arguments.done" => parse_tool_call_ready(chunk, tracker),
        "response.completed" | "response.done" => {
            let response_id = chunk
                .response
                .as_ref()
                .map(|response| response.id.clone())
                .filter(|id| !id.is_empty())
                .or_else(|| {
                    fallback_response_id
                        .map(str::trim)
                        .filter(|id| !id.is_empty())
                        .map(ToString::to_string)
                });
            Some(AgentEvent::ResponseDone { response_id })
        }
        _ => None,
    }
}

fn parse_tool_call_start(chunk: &StreamChunk, tracker: &mut ToolCallTracker) -> Option<AgentEvent> {
    let item = chunk.item.as_ref()?;
    if item.item_type != "function_call" {
        return None;
    }

    let call_id = item
        .call_id
        .as_deref()
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .map(ToString::to_string)
        .or_else(|| {
            item.id
                .as_deref()
                .map(str::trim)
                .filter(|id| !id.is_empty())
                .map(ToString::to_string)
        });

    let Some(call_id) = call_id else {
        return Some(AgentEvent::Error(
            "Received function_call item without an id".to_string(),
        ));
    };

    let name = item
        .name
        .as_deref()
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(ToString::to_string)
        .unwrap_or_else(|| "unknown_tool".to_string());

    if let Some(item_id) = item
        .id
        .as_deref()
        .map(str::trim)
        .filter(|id| !id.is_empty())
    {
        tracker.by_item_id.insert(
            item_id.to_string(),
            ToolCallMeta {
                call_id: call_id.clone(),
                name: name.clone(),
            },
        );
    }
    tracker
        .names_by_call_id
        .insert(call_id.clone(), name.clone());
    tracker
        .arguments_by_call_id
        .entry(call_id.clone())
        .or_default();

    Some(AgentEvent::ToolCallStart { id: call_id, name })
}

fn parse_tool_call_args_delta(
    chunk: &StreamChunk,
    tracker: &mut ToolCallTracker,
) -> Option<AgentEvent> {
    let delta = chunk
        .delta
        .as_deref()
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)?;

    let (call_id, name) = resolve_call_id_and_name(chunk, tracker)?;
    tracker
        .names_by_call_id
        .entry(call_id.clone())
        .or_insert(name);
    tracker
        .arguments_by_call_id
        .entry(call_id.clone())
        .or_default()
        .push_str(&delta);

    Some(AgentEvent::ToolCallArgsDelta { id: call_id, delta })
}

fn parse_tool_call_ready(chunk: &StreamChunk, tracker: &mut ToolCallTracker) -> Option<AgentEvent> {
    let (call_id, name) = resolve_call_id_and_name(chunk, tracker)?;

    let raw_arguments = chunk
        .arguments
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
        .or_else(|| tracker.arguments_by_call_id.get(&call_id).cloned())
        .unwrap_or_else(|| "{}".to_string());

    tracker.arguments_by_call_id.remove(&call_id);
    tracker
        .names_by_call_id
        .insert(call_id.clone(), name.clone());

    match serde_json::from_str::<serde_json::Value>(&raw_arguments) {
        Ok(arguments) => Some(AgentEvent::ToolCallReady {
            id: call_id,
            name,
            arguments,
        }),
        Err(error) => Some(AgentEvent::Error(format!(
            "Failed to parse arguments for tool '{}': {}",
            name, error
        ))),
    }
}

fn resolve_call_id_and_name(
    chunk: &StreamChunk,
    tracker: &ToolCallTracker,
) -> Option<(String, String)> {
    if let Some(call_id) = chunk
        .call_id
        .as_deref()
        .map(str::trim)
        .filter(|id| !id.is_empty())
    {
        let name = chunk
            .name
            .as_deref()
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .map(ToString::to_string)
            .or_else(|| tracker.names_by_call_id.get(call_id).cloned())
            .unwrap_or_else(|| "unknown_tool".to_string());
        return Some((call_id.to_string(), name));
    }

    if let Some(item_id) = chunk
        .item_id
        .as_deref()
        .map(str::trim)
        .filter(|id| !id.is_empty())
    {
        if let Some(meta) = tracker.by_item_id.get(item_id) {
            return Some((meta.call_id.clone(), meta.name.clone()));
        }
        return Some((item_id.to_string(), "unknown_tool".to_string()));
    }

    None
}

fn fallback_reasoning(reasoning_done: Option<String>, reasoning_text: String) -> Option<String> {
    reasoning_done.or_else(|| {
        let trimmed = reasoning_text.trim().to_string();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed)
        }
    })
}

#[derive(Debug, Deserialize)]
struct StreamChunk {
    #[serde(rename = "type")]
    chunk_type: String,
    #[serde(default)]
    delta: Option<String>,
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    arguments: Option<String>,
    #[serde(default)]
    call_id: Option<String>,
    #[serde(default)]
    item_id: Option<String>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    item: Option<StreamItem>,
    #[serde(default)]
    response: Option<StreamResponse>,
    #[serde(default)]
    sequence_number: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct StreamResponse {
    #[serde(default)]
    id: String,
    #[serde(default)]
    output: Vec<StreamOutputItem>,
}

#[derive(Debug, Deserialize)]
struct StreamItem {
    #[serde(rename = "type")]
    item_type: String,
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    call_id: Option<String>,
    #[serde(default)]
    name: Option<String>,
}

#[derive(Debug, Deserialize)]
struct StreamOutputItem {
    #[serde(rename = "type")]
    item_type: String,
    #[serde(default)]
    content: Option<Vec<StreamContentPart>>,
}

#[derive(Debug, Deserialize)]
struct StreamContentPart {
    #[serde(rename = "type")]
    part_type: String,
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    summary: Option<String>,
}

fn extract_output_channels(output: &[StreamOutputItem]) -> (String, Option<String>) {
    let mut assistant_parts = Vec::new();
    let mut reasoning_parts = Vec::new();

    for item in output.iter().filter(|o| o.item_type == "message") {
        let Some(parts) = item.content.as_ref() else {
            continue;
        };

        for part in parts {
            let text = part
                .text
                .as_deref()
                .or(part.summary.as_deref())
                .map(str::trim)
                .filter(|s| !s.is_empty());
            match part.part_type.as_str() {
                "output_text" | "text" => {
                    if let Some(text) = text {
                        assistant_parts.push(text.to_string());
                    }
                }
                "reasoning_summary_text" => {
                    if let Some(text) = text {
                        reasoning_parts.push(text.to_string());
                    }
                }
                _ => {}
            }
        }
    }

    let assistant_text = assistant_parts.join("").trim().to_string();
    let reasoning_text = reasoning_parts.join("").trim().to_string();
    let reasoning_text = if reasoning_text.is_empty() {
        None
    } else {
        Some(reasoning_text)
    };
    (assistant_text, reasoning_text)
}

#[cfg(test)]
mod tests {
    use super::{
        AgentEvent, StreamChunk, StreamOutputItem, ToolCallTracker, extract_output_channels,
        fallback_reasoning, parse_agent_event,
    };
    use serde_json::json;

    #[test]
    fn fallback_reasoning_prefers_done_text() {
        let reasoning =
            fallback_reasoning(Some("final reasoning".to_string()), "delta".to_string());
        assert_eq!(reasoning.as_deref(), Some("final reasoning"));
    }

    #[test]
    fn fallback_reasoning_uses_trimmed_delta() {
        let reasoning = fallback_reasoning(None, "  partial reasoning  ".to_string());
        assert_eq!(reasoning.as_deref(), Some("partial reasoning"));
    }

    #[test]
    fn fallback_reasoning_returns_none_for_empty_values() {
        let reasoning = fallback_reasoning(None, "   ".to_string());
        assert_eq!(reasoning, None);
    }

    #[test]
    fn extract_output_channels_collects_assistant_and_reasoning() {
        let output: Vec<StreamOutputItem> = serde_json::from_value(json!([
            {
                "type": "message",
                "content": [
                    {"type": "output_text", "text": "foo"},
                    {"type": "text", "text": "bar"},
                    {"type": "reasoning_summary_text", "text": "r1"},
                    {"type": "reasoning_summary_text", "summary": "r2"}
                ]
            },
            {
                "type": "reasoning",
                "content": [
                    {"type": "output_text", "text": "ignored"}
                ]
            }
        ]))
        .expect("valid stream output fixture");

        let (assistant, reasoning) = extract_output_channels(&output);
        assert_eq!(assistant, "foobar");
        assert_eq!(reasoning.as_deref(), Some("r1r2"));
    }

    #[test]
    fn extract_output_channels_ignores_blank_and_unknown_parts() {
        let output: Vec<StreamOutputItem> = serde_json::from_value(json!([
            {
                "type": "message",
                "content": [
                    {"type": "unknown", "text": "ignored"},
                    {"type": "output_text", "text": "   "},
                    {"type": "reasoning_summary_text", "summary": "   "}
                ]
            },
            {"type": "message"}
        ]))
        .expect("valid stream output fixture");

        let (assistant, reasoning) = extract_output_channels(&output);
        assert!(assistant.is_empty());
        assert_eq!(reasoning, None);
    }

    #[test]
    fn parse_agent_event_handles_function_call_lifecycle() {
        let mut tracker = ToolCallTracker::default();

        let start_chunk: StreamChunk = serde_json::from_value(json!({
            "type": "response.output_item.added",
            "item": {
                "type": "function_call",
                "id": "item_1",
                "call_id": "call_1",
                "name": "search_notes"
            }
        }))
        .expect("valid start chunk");

        let event =
            parse_agent_event(&start_chunk, &mut tracker, None).expect("expected tool start event");
        assert_eq!(
            event,
            AgentEvent::ToolCallStart {
                id: "call_1".to_string(),
                name: "search_notes".to_string(),
            }
        );

        let delta_chunk: StreamChunk = serde_json::from_value(json!({
            "type": "response.function_call_arguments.delta",
            "item_id": "item_1",
            "delta": "{\"query\":\"vet"
        }))
        .expect("valid delta chunk");

        let event =
            parse_agent_event(&delta_chunk, &mut tracker, None).expect("expected tool args delta");
        assert_eq!(
            event,
            AgentEvent::ToolCallArgsDelta {
                id: "call_1".to_string(),
                delta: "{\"query\":\"vet".to_string(),
            }
        );

        let done_chunk: StreamChunk = serde_json::from_value(json!({
            "type": "response.function_call_arguments.done",
            "item_id": "item_1",
            "arguments": "{\"query\":\"vetcoders\"}"
        }))
        .expect("valid done chunk");

        let event = parse_agent_event(&done_chunk, &mut tracker, None)
            .expect("expected tool call ready event");
        assert_eq!(
            event,
            AgentEvent::ToolCallReady {
                id: "call_1".to_string(),
                name: "search_notes".to_string(),
                arguments: json!({"query": "vetcoders"}),
            }
        );
    }

    #[test]
    fn parse_agent_event_uses_delta_buffer_when_done_has_no_arguments() {
        let mut tracker = ToolCallTracker::default();

        let start_chunk: StreamChunk = serde_json::from_value(json!({
            "type": "response.output_item.added",
            "item": {
                "type": "function_call",
                "id": "item_2",
                "call_id": "call_2",
                "name": "fetch_summary"
            }
        }))
        .expect("valid start chunk");
        let _ = parse_agent_event(&start_chunk, &mut tracker, None);

        let first_delta: StreamChunk = serde_json::from_value(json!({
            "type": "response.function_call_arguments.delta",
            "item_id": "item_2",
            "delta": "{\"id\":"
        }))
        .expect("valid first delta");
        let _ = parse_agent_event(&first_delta, &mut tracker, None);

        let second_delta: StreamChunk = serde_json::from_value(json!({
            "type": "response.function_call_arguments.delta",
            "item_id": "item_2",
            "delta": "\"abc\"}"
        }))
        .expect("valid second delta");
        let _ = parse_agent_event(&second_delta, &mut tracker, None);

        let done_chunk: StreamChunk = serde_json::from_value(json!({
            "type": "response.function_call_arguments.done",
            "item_id": "item_2"
        }))
        .expect("valid done chunk");

        let event = parse_agent_event(&done_chunk, &mut tracker, None)
            .expect("expected tool call ready event");
        assert_eq!(
            event,
            AgentEvent::ToolCallReady {
                id: "call_2".to_string(),
                name: "fetch_summary".to_string(),
                arguments: json!({"id": "abc"}),
            }
        );
    }
}
