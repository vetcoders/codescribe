use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Provider-agnostic message role.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    /// Human input.
    User,
    /// Model output.
    Assistant,
    /// Instructions framing the exchange.
    System,
}

/// Provider-agnostic multimodal content block.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentBlock {
    /// Plain text.
    Text(String),
    /// Image carried inline as bytes.
    Image {
        /// Raw encoded image bytes.
        data: Vec<u8>,
        /// MIME type of `data`.
        media_type: String,
    },
    /// Image kept on disk and referenced, not inlined.
    ImageAsset(ImageAsset),
    /// A tool invocation requested by the model.
    ToolUse {
        /// Correlates with the matching [`ContentBlock::ToolResult`].
        id: String,
        /// Registered tool name.
        name: String,
        /// Arguments, shaped by the tool's input schema.
        input: serde_json::Value,
    },
    /// The outcome of a [`ContentBlock::ToolUse`].
    ToolResult {
        /// `id` of the invocation this answers.
        tool_use_id: String,
        /// Result payload, itself a content block list.
        content: Vec<ContentBlock>,
        /// Whether the payload describes a failure.
        is_error: bool,
    },
}

/// Reference to an image persisted under the agent assets directory.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImageAsset {
    /// Stable identifier, also the file stem on disk.
    pub asset_id: String,
    /// Absolute path to the stored file.
    pub path: PathBuf,
    /// MIME type of the stored bytes.
    pub media_type: String,
    /// Size of the stored file in bytes.
    pub size_bytes: u64,
}

/// Canonical message used by the agent session regardless of provider.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Message {
    /// Who produced this message.
    pub role: Role,
    /// Ordered multimodal payload.
    pub content: Vec<ContentBlock>,
    /// Creation time; `None` for messages restored without one.
    pub timestamp: Option<DateTime<Utc>>,
}

impl Message {
    /// Build a message stamped with the current UTC time.
    pub fn new(role: Role, content: Vec<ContentBlock>) -> Self {
        Self {
            role,
            content,
            timestamp: Some(Utc::now()),
        }
    }
}
