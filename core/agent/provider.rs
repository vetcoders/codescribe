use anyhow::Result;
use async_trait::async_trait;
use tokio::sync::mpsc::Receiver;

use super::{AgentEvent, ContentBlock, Message, ToolDefinition};

#[async_trait]
pub trait AgentProvider: Send + Sync {
    async fn stream(
        &self,
        messages: &[Message],
        tools: &[ToolDefinition],
        options: &StreamOptions,
    ) -> Result<Receiver<AgentEvent>>;

    fn build_tool_result(
        &self,
        call_id: &str,
        content: Vec<ContentBlock>,
        is_error: bool,
    ) -> Message;

    fn build_image_block(&self, data: &[u8], media_type: &str) -> ContentBlock;
    fn name(&self) -> &str;
}

#[derive(Debug, Clone, Default)]
pub struct StreamOptions {
    pub model: String,
    pub system_prompt: Option<String>,
    pub max_tokens: Option<u32>,
    pub temperature: Option<f32>,
}
