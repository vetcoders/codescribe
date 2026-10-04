//! Responses `output` array → assistant text and reasoning summary.
//!
//! One reader for a JSON Responses body and for a terminal `response.output`
//! on the SSE stream. Live deltas stay on the streaming manager; they are a
//! different wire shape and do not come through here.
//!
//! Assistant text is taken only from `message` items (`output_text` or `text`).
//! Reasoning summaries are taken from `message` or `reasoning` items
//! (`reasoning_summary_text`). Blank parts are dropped. That item-type gate is
//! what keeps a reasoning model's internal narration out of the transcript.

use serde::Deserialize;

/// One item of a Responses API `output` array.
#[derive(Debug, Deserialize)]
pub(super) struct ResponsesOutputItem {
    #[serde(rename = "type")]
    item_type: String,
    #[serde(default)]
    content: Option<Vec<ResponsesContentPart>>,
}

/// A content part inside an output item.
///
/// Text arrives under `text` for output parts and under `summary` for reasoning
/// summaries. Both are accepted.
#[derive(Debug, Deserialize)]
struct ResponsesContentPart {
    #[serde(rename = "type")]
    part_type: String,
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    summary: Option<String>,
}

/// Assistant transcript text and optional reasoning summary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ResponsesChannels {
    pub(super) assistant_text: String,
    pub(super) reasoning_text: Option<String>,
}

fn part_text(part: &ResponsesContentPart) -> Option<&str> {
    part.text
        .as_deref()
        .or(part.summary.as_deref())
        .map(str::trim)
        .filter(|text| !text.is_empty())
}

/// Fold a Responses output array into the assistant and reasoning channels.
pub(super) fn extract_output_channels(output: &[ResponsesOutputItem]) -> ResponsesChannels {
    let mut assistant_parts = Vec::new();
    let mut reasoning_parts = Vec::new();

    for item in output {
        let Some(parts) = item.content.as_ref() else {
            continue;
        };
        let is_message = item.item_type == "message";
        let is_reasoning = item.item_type == "reasoning";

        for part in parts {
            match part.part_type.as_str() {
                "output_text" | "text" if is_message => {
                    if let Some(text) = part_text(part) {
                        assistant_parts.push(text.to_string());
                    }
                }
                "reasoning_summary_text" if is_message || is_reasoning => {
                    if let Some(text) = part_text(part) {
                        reasoning_parts.push(text.to_string());
                    }
                }
                _ => {}
            }
        }
    }

    let assistant_text = assistant_parts.join("").trim().to_string();
    let reasoning_text = reasoning_parts.join("").trim().to_string();
    ResponsesChannels {
        assistant_text,
        reasoning_text: if reasoning_text.is_empty() {
            None
        } else {
            Some(reasoning_text)
        },
    }
}
