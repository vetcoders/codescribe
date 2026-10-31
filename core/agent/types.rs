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
#[serde(tag = "type", content = "payload", rename_all = "snake_case")]
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

/// Wire state for one completed response, handed to the canonical session.
/// Opaque provider parts never become display content or a second history.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderResponseMetadata {
    pub wire: String,
    pub provider_id: String,
    pub model: String,
    pub context_digest: String,
    pub payload: serde_json::Value,
}

impl std::fmt::Debug for ProviderResponseMetadata {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProviderResponseMetadata")
            .field("wire", &self.wire)
            .field("model", &self.model)
            .field("payload", &"[PRIVATE WIRE STATE]")
            .finish()
    }
}

/// One canonical Assistant occurrence's membership in a provider response.
/// The first segment owns the parts; later segments only reference its UUID.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderWireMessage {
    pub version: u32,
    pub occurrence_id: String,
    pub segment_index: u8,
    pub segment_count: u8,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub response: Option<ProviderResponseMetadata>,
}

impl ProviderWireMessage {
    pub fn validate(&self) -> anyhow::Result<()> {
        anyhow::ensure!(self.version == 1, "Unsupported provider wire metadata version");
        anyhow::ensure!(
            uuid::Uuid::parse_str(&self.occurrence_id).is_ok_and(|id| !id.is_nil())
                && (1..=2).contains(&self.segment_count)
                && self.segment_index < self.segment_count
                && self.response.is_some() == (self.segment_index == 0),
            "Invalid provider wire occurrence or segment"
        );
        if let Some(response) = &self.response {
            let digest = |value: &str| value.len() == 64 && value.bytes().all(|b| b.is_ascii_hexdigit());
            anyhow::ensure!(
                !response.wire.is_empty() && response.wire.len() <= 128
                    && !response.model.is_empty() && response.model.len() <= 1024
                    && digest(&response.provider_id) && digest(&response.context_digest),
                "Invalid provider wire identity"
            );
        }
        Ok(())
    }
}

/// Non-display message metadata. Storage keeps unrelated namespaces intact;
/// Debug deliberately omits all values, including signed thought payloads.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct MessageMetadata(serde_json::Value);

impl std::fmt::Debug for MessageMetadata {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("MessageMetadata([PRIVATE])")
    }
}

impl MessageMetadata {
    pub fn from_stored(value: serde_json::Value) -> Self { Self(value) }
    pub fn as_stored(&self) -> &serde_json::Value { &self.0 }

    pub fn provider_wire(&self) -> anyhow::Result<Option<ProviderWireMessage>> {
        let Some(value) = self.0.get("provider_wire") else { return Ok(None); };
        anyhow::ensure!(
            serde_json::to_vec(value)?.len() <= 16 * 1024 * 1024,
            "Provider wire metadata exceeds the size limit"
        );
        let wire: ProviderWireMessage = serde_json::from_value(value.clone())
            .map_err(|_| anyhow::anyhow!("Malformed provider wire metadata"))?;
        wire.validate()?;
        Ok(Some(wire))
    }

    pub fn for_provider_wire(wire: ProviderWireMessage) -> anyhow::Result<Self> {
        wire.validate()?;
        let metadata = Self(serde_json::json!({"provider_wire": wire}));
        let _ = metadata.provider_wire()?;
        Ok(metadata)
    }
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
    /// Private wire/occurrence state, never rendered as a content block.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<MessageMetadata>,
}

impl Message {
    /// Build a message stamped with the current UTC time.
    pub fn new(role: Role, content: Vec<ContentBlock>) -> Self {
        Self {
            role,
            content,
            timestamp: Some(Utc::now()),
            metadata: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runtime_message_roundtrip_preserves_text_images_and_nested_tools() {
        let text = ContentBlock::Text("Roman, zachowaj dokładnie tę wiadomość.".into());
        let message = Message::new(
            Role::User,
            vec![
                text.clone(),
                ContentBlock::Image {
                    data: vec![0, 127, 255],
                    media_type: "image/png".into(),
                },
                ContentBlock::ImageAsset(ImageAsset {
                    asset_id: "retained-image".into(),
                    path: PathBuf::from("/retained/image.png"),
                    media_type: "image/png".into(),
                    size_bytes: 3,
                }),
                ContentBlock::ToolUse {
                    id: "call-1".into(),
                    name: "read_file".into(),
                    input: serde_json::json!({"path": "README.md"}),
                },
                ContentBlock::ToolResult {
                    tool_use_id: "call-1".into(),
                    content: vec![text],
                    is_error: false,
                },
            ],
        );
        let encoded = serde_json::to_vec(&message).expect("all content variants serialize");
        let restored: Message = serde_json::from_slice(&encoded).expect("restore runtime message");
        assert_eq!(restored, message);
        let encoded: serde_json::Value = serde_json::from_slice(&encoded).unwrap();
        assert_eq!(encoded["content"][0]["type"], "text");
        assert_eq!(
            encoded["content"][0]["payload"],
            "Roman, zachowaj dokładnie tę wiadomość."
        );
    }
}
