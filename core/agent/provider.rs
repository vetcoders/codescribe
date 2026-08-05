use anyhow::Result;
use async_trait::async_trait;
use std::time::Duration;
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
    fn stream_timeouts(&self) -> Option<(Duration, Duration)> {
        None
    }
    fn name(&self) -> &str;

    /// Provider-owned Responses-API chain id (`previous_response_id`), if any.
    ///
    /// Default: no chain. OpenAI Responses implements this so a user Stop can
    /// reinstate the pre-turn id instead of wiping continuity.
    async fn response_chain_id(&self) -> Option<String> {
        None
    }

    /// Restore a previously snapshotted chain id (or clear when `None`).
    ///
    /// Used after a user Stop: local history rolls back to the pre-turn
    /// snapshot and the chain must match that history — not a mid-turn id and
    /// not a forced full-reset.
    async fn restore_response_chain(&self, _id: Option<String>) {}
}

#[derive(Debug, Clone, Default)]
pub struct StreamOptions {
    pub model: String,
    pub system_prompt: Option<String>,
    pub max_tokens: Option<u32>,
    pub temperature: Option<f32>,
    /// Per-request chain control (operator's specification 2026-05-26 4th iteration):
    /// retry attempts must NOT resend prior context via stored previous_response_id.
    /// When `true`, provider clears any stored response chain BEFORE building the
    /// request — next call starts fresh, no context bloat from prior failed attempts.
    /// Callers (session retry path) set `true` for retry attempts; normal calls leave
    /// `false` to preserve conversational chain.
    pub reset_chain: bool,
}
