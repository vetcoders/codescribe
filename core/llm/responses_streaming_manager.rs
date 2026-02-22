use anyhow::{Context, Result};
use futures_util::StreamExt;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::time::Duration;
use tracing::{debug, info, warn};

use super::ai_formatting::{AiReasoningCallback, AiStreamCallback};

const STREAM_REQUEST_TIMEOUT: Duration = Duration::from_secs(3600);
const STREAM_DEADLINE: Duration = Duration::from_secs(10 * 60);

#[derive(Debug, Clone)]
pub(super) struct ResponsesStreamOutput {
    pub assistant_text: String,
    pub reasoning_text: Option<String>,
    pub response_id: Option<String>,
}

#[derive(Clone)]
pub(super) struct StreamCallbacks {
    pub assistant: Option<AiStreamCallback>,
    pub reasoning: Option<AiReasoningCallback>,
}

pub(super) struct ResponsesStreamingManager<'a> {
    client: &'a Client,
    endpoint: &'a str,
    api_key: &'a str,
    callbacks: StreamCallbacks,
    initial_response_timeout: Duration,
    inter_chunk_timeout: Duration,
}

impl<'a> ResponsesStreamingManager<'a> {
    pub(super) fn new(
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

    pub(super) async fn stream<T: Serialize>(&self, request: &T) -> Result<ResponsesStreamOutput> {
        let request_builder = self
            .client
            .post(self.endpoint)
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
    use super::{StreamOutputItem, extract_output_channels, fallback_reasoning};
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
}
