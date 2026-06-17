use anyhow::{Context, Result};
use futures_util::StreamExt;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, ToSocketAddrs};
use std::time::Duration;
use tokio::sync::mpsc;
use tracing::{debug, info, warn};

use crate::agent::event::AgentEvent;

use super::ai_formatting::{AiReasoningCallback, AiStreamCallback};

const STREAM_REQUEST_TIMEOUT: Duration = Duration::from_secs(3600);
const STREAM_DEADLINE: Duration = Duration::from_secs(10 * 60);
const REASONING_INTER_CHUNK_TIMEOUT: Duration = Duration::from_secs(180);

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
        let request_builder = apply_auth_headers(
            // nosemgrep: rust.actix.ssrf.reqwest-taint.reqwest-taint -- URL is validated by `validated_endpoint_url`.
            self.client.post(endpoint_url.clone()),
            self.api_key,
        )
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
        let mut current_event: Option<String> = None;
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
            let chunk_timeout = if reasoning_content_available(&reasoning_done, &reasoning_text)
                && assistant_text.is_empty()
                && assistant_done.is_none()
            {
                self.inter_chunk_timeout.max(REASONING_INTER_CHUNK_TIMEOUT)
            } else {
                self.inter_chunk_timeout
            };
            let next_chunk = match tokio::time::timeout(chunk_timeout, stream.next()).await {
                Ok(chunk) => chunk,
                Err(_) => {
                    if !assistant_text.is_empty()
                        || assistant_done.is_some()
                        || reasoning_content_available(&reasoning_done, &reasoning_text)
                    {
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
                                    if reasoning_content_available(&reasoning_done, &reasoning_text)
                                        && assistant_text.is_empty()
                                        && assistant_done.is_none()
                                    {
                                        warn!(
                                            "SSE inter-chunk timeout after {:?} with reasoning but \
                                             no output_text; resume failed: {}; falling back to \
                                             reasoning summary",
                                            chunk_timeout, e
                                        );
                                        break;
                                    }
                                    anyhow::bail!(
                                        "SSE inter-chunk timeout after {:?} with {}B partial text; \
                                         resume failed: {}",
                                        chunk_timeout,
                                        partial_len,
                                        e
                                    );
                                }
                            }
                        }
                        if reasoning_content_available(&reasoning_done, &reasoning_text)
                            && assistant_text.is_empty()
                            && assistant_done.is_none()
                        {
                            warn!(
                                "SSE inter-chunk timeout after {:?} with reasoning but no \
                                 output_text; resume unavailable; falling back to reasoning summary",
                                chunk_timeout
                            );
                            break;
                        }
                        anyhow::bail!(
                            "SSE inter-chunk timeout after {:?} with {}B partial text \
                             (resume unavailable)",
                            chunk_timeout,
                            partial_len
                        );
                    } else {
                        anyhow::bail!(
                            "SSE stream inter-chunk timeout after {:?} (no data received)",
                            chunk_timeout
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

                if let Some(event) = line
                    .strip_prefix("event: ")
                    .or_else(|| line.strip_prefix("event:"))
                {
                    current_event = Some(event.trim().to_string());
                    continue;
                }

                if let Some(data) = line
                    .strip_prefix("data: ")
                    .or_else(|| line.strip_prefix("data:"))
                {
                    let data = data.trim_start();
                    let data_event = current_event.take();
                    if data == "[DONE]" {
                        saw_done = true;
                        break;
                    }

                    if data_event.as_deref() == Some("error") {
                        let error = parse_sse_error_event(data);
                        anyhow::bail!("{}", format_sse_error("SSE", &error));
                    }

                    let chunk = match serde_json::from_str::<StreamChunk>(data) {
                        Ok(parsed) => parsed,
                        Err(error) => {
                            warn!("Skipping malformed SSE chunk: {}", error);
                            continue;
                        }
                    };
                    {
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
                            "response.output_item.added" | "response.output_item.done" => {
                                log_sse_lifecycle_event("SSE", &chunk);
                            }
                            "response.content_part.added" | "response.content_part.done" => {
                                log_sse_lifecycle_event("SSE", &chunk);
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

        let assistant_text = if assistant_text.is_empty() {
            if let Some(reasoning_fallback) = reasoning_text.as_ref() {
                warn!(
                    "SSE stream ended without output_text after reasoning; returning reasoning \
                     summary as fallback content"
                );
                reasoning_fallback.clone()
            } else {
                anyhow::bail!("No text content in SSE stream");
            }
        } else {
            assistant_text
        };

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

        let response = self.client.get(&resume_url);
        let response = apply_auth_headers(response, self.api_key)
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
        let mut current_event: Option<String> = None;

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

                if let Some(event) = line
                    .strip_prefix("event: ")
                    .or_else(|| line.strip_prefix("event:"))
                {
                    current_event = Some(event.trim().to_string());
                    continue;
                }

                if let Some(data) = line
                    .strip_prefix("data: ")
                    .or_else(|| line.strip_prefix("data:"))
                {
                    let data = data.trim_start();
                    let data_event = current_event.take();
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

                    if data_event.as_deref() == Some("error") {
                        let error = parse_sse_error_event(data);
                        anyhow::bail!("{}", format_sse_error("Resume SSE", &error));
                    }

                    let chunk = match serde_json::from_str::<StreamChunk>(data) {
                        Ok(parsed) => parsed,
                        Err(error) => {
                            warn!("Skipping malformed SSE chunk: {}", error);
                            continue;
                        }
                    };
                    {
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
                            "response.output_item.added" | "response.output_item.done" => {
                                log_sse_lifecycle_event("Resume SSE", &chunk);
                            }
                            "response.content_part.added" | "response.content_part.done" => {
                                log_sse_lifecycle_event("Resume SSE", &chunk);
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

// allow(too_many_arguments): task entry point for one agent SSE stream; all
// eight values are owned moves into the spawned task.
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
    let request_builder = apply_auth_headers(
        // nosemgrep: rust.actix.ssrf.reqwest-taint.reqwest-taint -- URL is validated by `validated_endpoint_url`.
        client.post(endpoint_url),
        &api_key,
    )
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
    let mut current_event: Option<String> = None;
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

            if let Some(event) = line
                .strip_prefix("event: ")
                .or_else(|| line.strip_prefix("event:"))
            {
                current_event = Some(event.trim().to_string());
                continue;
            }

            let Some(data) = line
                .strip_prefix("data: ")
                .or_else(|| line.strip_prefix("data:"))
            else {
                continue;
            };
            let data = data.trim_start();
            let data_event = current_event.take();
            if data == "[DONE]" {
                saw_done = true;
                break;
            }

            if data_event.as_deref() == Some("error") {
                let error = parse_sse_error_event(data);
                anyhow::bail!("{}", format_sse_error("Agent SSE", &error));
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

            debug!("Agent SSE received event type={}", chunk.chunk_type);
            log_sse_lifecycle_event("Agent SSE", &chunk);

            if let Some(event) =
                parse_agent_event(&chunk, &mut tool_tracker, response_id.as_deref())
            {
                match &event {
                    AgentEvent::TextDelta(delta) => {
                        debug!(
                            "Agent SSE dispatch response.output_text.delta ({}B)",
                            delta.len()
                        );
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

fn apply_auth_headers(builder: reqwest::RequestBuilder, api_key: &str) -> reqwest::RequestBuilder {
    builder
        .header("Authorization", format!("Bearer {}", api_key))
        .header("x-api-key", api_key)
}

fn validated_endpoint_url(endpoint: &str) -> Result<reqwest::Url> {
    let endpoint = endpoint.trim();
    if endpoint.is_empty() {
        anyhow::bail!("Endpoint URL is empty");
    }

    let url = reqwest::Url::parse(endpoint).context("Endpoint is not a valid URL")?;
    let is_loopback = is_loopback_host(&url);

    match url.scheme() {
        "https" => {}
        "http" if is_loopback => {}
        "http" => anyhow::bail!("Plain HTTP is only allowed for localhost loopback endpoints"),
        other => anyhow::bail!("Unsupported endpoint URL scheme: {}", other),
    }

    if url.host_str().is_none() {
        anyhow::bail!("Endpoint URL is missing a host");
    }

    if is_private_host(&url) && !is_loopback {
        anyhow::bail!("Private/internal endpoint URLs are not allowed");
    }

    if resolves_to_private_host(&url) && !is_loopback {
        anyhow::bail!("Endpoint resolves to a private/internal address");
    }

    Ok(url)
}

fn is_loopback_host(url: &reqwest::Url) -> bool {
    let Some(host_raw) = url.host_str() else {
        return false;
    };
    let host = host_raw.trim_matches(['[', ']']);
    matches!(host, "localhost" | "127.0.0.1" | "::1")
}

fn is_private_host(url: &reqwest::Url) -> bool {
    let Some(host_raw) = url.host_str() else {
        return true;
    };
    let host = host_raw.trim_matches(['[', ']']);

    if matches!(host, "localhost" | "127.0.0.1" | "::1" | "0.0.0.0") {
        return true;
    }

    if host.ends_with(".local") || host.ends_with(".internal") {
        return true;
    }

    if let Some(is_private) = check_ipv4_private(host) {
        return is_private;
    }

    if let Some(is_private) = check_ipv6_private(host) {
        return is_private;
    }

    false
}

fn resolves_to_private_host(url: &reqwest::Url) -> bool {
    let Some(host_raw) = url.host_str() else {
        return true;
    };
    let host = host_raw.trim_matches(['[', ']']);

    if host.parse::<IpAddr>().is_ok() {
        return false;
    }

    let port = url
        .port_or_known_default()
        .unwrap_or(if url.scheme() == "http" { 80 } else { 443 });

    let addrs = (host, port).to_socket_addrs();
    let Ok(iter) = addrs else {
        return true;
    };

    let mut resolved_any = false;
    for addr in iter {
        resolved_any = true;
        if is_private_ip(addr.ip()) {
            return true;
        }
    }

    !resolved_any
}

fn is_private_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => is_private_ipv4(v4),
        IpAddr::V6(v6) => is_private_ipv6(v6),
    }
}

fn check_ipv4_private(host: &str) -> Option<bool> {
    let ip = host.parse::<Ipv4Addr>().ok()?;
    Some(is_private_ipv4(ip))
}

fn check_ipv6_private(host: &str) -> Option<bool> {
    let ip = host.parse::<Ipv6Addr>().ok()?;
    Some(is_private_ipv6(ip))
}

fn is_private_ipv4(ip: Ipv4Addr) -> bool {
    let octets = ip.octets();
    match octets[0] {
        10 => true,
        172 => (16..=31).contains(&octets[1]),
        192 if octets[1] == 168 => true,
        169 if octets[1] == 254 => true,
        127 => true,
        0 => true,
        _ => false,
    }
}

fn is_private_ipv6(ip: Ipv6Addr) -> bool {
    ip.is_loopback() || ip.is_unspecified() || ip.is_unicast_link_local() || ip.is_unique_local()
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

fn reasoning_content_available(reasoning_done: &Option<String>, reasoning_text: &str) -> bool {
    reasoning_done
        .as_deref()
        .map(str::trim)
        .is_some_and(|text| !text.is_empty())
        || !reasoning_text.trim().is_empty()
}

#[derive(Debug, Clone)]
struct SseErrorEvent {
    code: Option<String>,
    message: String,
}

#[derive(Debug, Deserialize)]
struct SseErrorEnvelope {
    #[serde(default)]
    error: Option<SseErrorDetail>,
    #[serde(default)]
    code: Option<String>,
    #[serde(default)]
    message: Option<String>,
}

#[derive(Debug, Deserialize)]
struct SseErrorDetail {
    #[serde(default)]
    code: Option<String>,
    #[serde(default, rename = "type")]
    error_type: Option<String>,
    #[serde(default)]
    message: Option<String>,
}

fn parse_sse_error_event(data: &str) -> SseErrorEvent {
    let parsed = serde_json::from_str::<SseErrorEnvelope>(data).ok();
    let code = parsed.as_ref().and_then(|envelope| {
        envelope
            .error
            .as_ref()
            .and_then(|error| {
                error
                    .code
                    .as_deref()
                    .or(error.error_type.as_deref())
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(ToString::to_string)
            })
            .or_else(|| {
                envelope
                    .code
                    .as_deref()
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(ToString::to_string)
            })
    });
    let message = parsed
        .as_ref()
        .and_then(|envelope| {
            envelope
                .error
                .as_ref()
                .and_then(|error| error.message.as_deref())
                .or(envelope.message.as_deref())
        })
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
        .unwrap_or_else(|| data.trim().to_string());

    SseErrorEvent { code, message }
}

fn format_sse_error(prefix: &str, error: &SseErrorEvent) -> String {
    if let Some(code) = error.code.as_deref() {
        format!("{prefix} error {code}: {}", error.message)
    } else {
        format!("{prefix} error: {}", error.message)
    }
}

fn log_sse_lifecycle_event(prefix: &str, chunk: &StreamChunk) {
    match chunk.chunk_type.as_str() {
        "response.output_item.added" | "response.output_item.done" => {
            let item_type = chunk
                .item
                .as_ref()
                .map(|item| item.item_type.as_str())
                .unwrap_or("unknown");
            debug!(
                "{} lifecycle event type={} item_type={}",
                prefix, chunk.chunk_type, item_type
            );
        }
        "response.content_part.added" | "response.content_part.done" => {
            let part_type = chunk
                .part
                .as_ref()
                .map(|part| part.part_type.as_str())
                .unwrap_or("unknown");
            debug!(
                "{} lifecycle event type={} part_type={}",
                prefix, chunk.chunk_type, part_type
            );
        }
        _ => {}
    }
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
    part: Option<StreamContentPart>,
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

    for item in output {
        let Some(parts) = item.content.as_ref() else {
            continue;
        };
        let is_message = item.item_type == "message";
        let is_reasoning = item.item_type == "reasoning";

        for part in parts {
            let text = part
                .text
                .as_deref()
                .or(part.summary.as_deref())
                .map(str::trim)
                .filter(|s| !s.is_empty());
            match part.part_type.as_str() {
                "output_text" | "text" if is_message => {
                    if let Some(text) = text {
                        assistant_parts.push(text.to_string());
                    }
                }
                "reasoning_summary_text" if is_message || is_reasoning => {
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
        AgentEvent, ResponsesStreamingManager, StreamCallbacks, StreamChunk, StreamOutputItem,
        ToolCallTracker, apply_auth_headers, extract_output_channels, fallback_reasoning,
        parse_agent_event, reasoning_content_available, validated_endpoint_url,
    };
    use reqwest::Client;
    use serde_json::json;
    use std::time::Duration;

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
    fn reasoning_content_available_tracks_delta_or_done_text() {
        assert!(reasoning_content_available(&None, "thinking"));
        assert!(reasoning_content_available(
            &Some("done thinking".to_string()),
            ""
        ));
        assert!(!reasoning_content_available(&Some("   ".to_string()), ""));
    }

    #[tokio::test]
    async fn stream_returns_reasoning_summary_when_reasoning_only_stream_completes() {
        let mut server = mockito::Server::new_async().await;
        let body = [
            r#"data: {"type":"response.reasoning_summary_text.delta","delta":"Reasoned ","sequence_number":1,"response":{"id":"resp_reasoning"}}"#,
            "",
            r#"data: {"type":"response.reasoning_summary_text.done","text":"Reasoned fallback","sequence_number":2,"response":{"id":"resp_reasoning"}}"#,
            "",
            r#"data: {"type":"response.completed","sequence_number":3,"response":{"id":"resp_reasoning","output":[{"type":"message","content":[{"type":"reasoning_summary_text","text":"Reasoned fallback"}]}]}}"#,
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
        let endpoint = format!("{}/v1/responses", server.url());
        let client = Client::new();
        let manager = ResponsesStreamingManager::new(
            &client,
            &endpoint,
            "test-key",
            StreamCallbacks {
                assistant: None,
                reasoning: None,
            },
            Duration::from_secs(1),
            Duration::from_secs(1),
        );

        let output = manager
            .stream(&json!({"model": "programmer", "stream": true}))
            .await
            .expect("reasoning-only completed streams should not bail");

        assert_eq!(output.assistant_text, "Reasoned fallback");
        assert_eq!(output.reasoning_text.as_deref(), Some("Reasoned fallback"));
        assert_eq!(output.response_id.as_deref(), Some("resp_reasoning"));
        mock.assert_async().await;
    }

    #[tokio::test]
    async fn stream_bails_with_specific_sse_error_event() {
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
        let endpoint = format!("{}/v1/responses", server.url());
        let client = Client::new();
        let manager = ResponsesStreamingManager::new(
            &client,
            &endpoint,
            "test-key",
            StreamCallbacks {
                assistant: None,
                reasoning: None,
            },
            Duration::from_secs(1),
            Duration::from_secs(1),
        );

        let error = manager
            .stream(&json!({"model": "programmer", "stream": true}))
            .await
            .expect_err("SSE error events should bail before empty-content fallback");
        let message = error.to_string();

        assert!(message.contains("SSE error internal_error"));
        assert!(message.contains("'list' object has no attribute 'uid'"));
        assert!(!message.contains("No text content in SSE stream"));
        mock.assert_async().await;
    }

    #[tokio::test]
    async fn stream_handles_output_item_and_content_part_lifecycle_events() {
        let mut server = mockito::Server::new_async().await;
        let body = [
            r#"data: {"type":"response.created","sequence_number":0,"response":{"id":"resp_full"}}"#,
            "",
            r#"data: {"type":"response.in_progress","sequence_number":1,"response":{"id":"resp_full"}}"#,
            "",
            r#"data: {"type":"response.output_item.added","sequence_number":2,"output_index":0,"item":{"id":"rs_1","type":"reasoning"}}"#,
            "",
            r#"data: {"type":"response.reasoning_summary_text.delta","sequence_number":3,"item_id":"rs_1","delta":"Thinking"}"#,
            "",
            r#"data: {"type":"response.reasoning_summary_text.done","sequence_number":4,"item_id":"rs_1","text":"Thinking"}"#,
            "",
            r#"data: {"type":"response.output_item.done","sequence_number":5,"output_index":0,"item":{"id":"rs_1","type":"reasoning","content":[{"type":"reasoning_summary_text","text":"Thinking"}]}}"#,
            "",
            r#"data: {"type":"response.output_item.added","sequence_number":6,"output_index":1,"item":{"id":"msg_1","type":"message"}}"#,
            "",
            r#"data: {"type":"response.content_part.added","sequence_number":7,"output_index":1,"content_index":0,"part":{"type":"output_text"}}"#,
            "",
            r#"data: {"type":"response.output_text.delta","sequence_number":8,"output_index":1,"content_index":0,"delta":"Hello"}"#,
            "",
            r#"data: {"type":"response.output_text.delta","sequence_number":9,"output_index":1,"content_index":0,"delta":" world"}"#,
            "",
            r#"data: {"type":"response.output_text.done","sequence_number":10,"output_index":1,"content_index":0,"text":"Hello world"}"#,
            "",
            r#"data: {"type":"response.content_part.done","sequence_number":11,"output_index":1,"content_index":0,"part":{"type":"output_text","text":"Hello world"}}"#,
            "",
            r#"data: {"type":"response.output_item.done","sequence_number":12,"output_index":1,"item":{"id":"msg_1","type":"message","content":[{"type":"output_text","text":"Hello world"}]}}"#,
            "",
            r#"data: {"type":"response.completed","sequence_number":13,"response":{"id":"resp_full","output":[{"type":"reasoning","content":[{"type":"reasoning_summary_text","text":"Thinking"}]},{"type":"message","content":[{"type":"output_text","text":"Hello world"}]}]}}"#,
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
        let endpoint = format!("{}/v1/responses", server.url());
        let client = Client::new();
        let manager = ResponsesStreamingManager::new(
            &client,
            &endpoint,
            "test-key",
            StreamCallbacks {
                assistant: None,
                reasoning: None,
            },
            Duration::from_secs(1),
            Duration::from_secs(1),
        );

        let output = manager
            .stream(&json!({"model": "working-model", "stream": true}))
            .await
            .expect("full Responses SSE lifecycle should parse output_text");

        assert_eq!(output.assistant_text, "Hello world");
        assert_eq!(output.reasoning_text.as_deref(), Some("Thinking"));
        assert_eq!(output.response_id.as_deref(), Some("resp_full"));
        mock.assert_async().await;
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
                    {"type": "output_text", "text": "ignored"},
                    {"type": "reasoning_summary_text", "text": "r3"}
                ]
            }
        ]))
        .expect("valid stream output fixture");

        let (assistant, reasoning) = extract_output_channels(&output);
        assert_eq!(assistant, "foobar");
        assert_eq!(reasoning.as_deref(), Some("r1r2r3"));
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

    #[test]
    fn apply_auth_headers_sets_both_bearer_and_x_api_key() {
        let client = Client::new();
        let request = apply_auth_headers(client.post("https://example.com/v1/responses"), "secret")
            .build()
            .expect("request should build");

        assert_eq!(
            request
                .headers()
                .get("authorization")
                .and_then(|value| value.to_str().ok()),
            Some("Bearer secret")
        );
        assert_eq!(
            request
                .headers()
                .get("x-api-key")
                .and_then(|value| value.to_str().ok()),
            Some("secret")
        );
    }

    #[test]
    fn validated_endpoint_url_allows_https_public_and_loopback_http() {
        let public = validated_endpoint_url("https://1.1.1.1/v1/responses")
            .expect("public https endpoint should be allowed");
        assert_eq!(public.as_str(), "https://1.1.1.1/v1/responses");

        let localhost = validated_endpoint_url("http://127.0.0.1:11434/v1/responses")
            .expect("loopback http endpoint should be allowed");
        assert_eq!(localhost.as_str(), "http://127.0.0.1:11434/v1/responses");
    }

    #[test]
    fn validated_endpoint_url_rejects_plain_http_and_private_remote_hosts() {
        let public_http = validated_endpoint_url("http://1.1.1.1/v1/responses")
            .expect_err("public http endpoint should be rejected");
        assert!(
            public_http
                .to_string()
                .contains("Plain HTTP is only allowed for localhost loopback endpoints")
        );

        let private_https = validated_endpoint_url("https://192.168.1.10/v1/responses")
            .expect_err("private https endpoint should be rejected");
        assert!(
            private_https
                .to_string()
                .contains("Private/internal endpoint URLs are not allowed")
        );
    }
}
