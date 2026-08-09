//! The provider seam: one trait every agent backend implements.
//!
//! Backends differ in how they stream, how they shape tool results, and whether
//! they keep server-side conversation state. This module hides those differences
//! behind [`AgentProvider`] so the session loop stays backend-agnostic — including
//! the Responses-API chain (`previous_response_id`), which only some providers own.

use anyhow::Result;
use async_trait::async_trait;
use std::time::Duration;
use tokio::sync::mpsc::Receiver;

use super::{AgentEvent, ContentBlock, Message, ToolDefinition};

/// A streaming agent backend (Anthropic, OpenAI Responses, …).
///
/// Implementors are shared across tasks, hence `Send + Sync`.
#[async_trait]
pub trait AgentProvider: Send + Sync {
    /// Start a turn and return the channel carrying its [`AgentEvent`] stream.
    ///
    /// The receiver closes when the turn ends; errors mid-turn arrive as events
    /// rather than as an `Err` from this call.
    async fn stream(
        &self,
        messages: &[Message],
        tools: &[ToolDefinition],
        options: &StreamOptions,
    ) -> Result<Receiver<AgentEvent>>;

    /// Wrap a tool's output in the message shape this backend expects.
    ///
    /// `is_error` marks the result as a failed call rather than a normal return.
    fn build_tool_result(
        &self,
        call_id: &str,
        content: Vec<ContentBlock>,
        is_error: bool,
    ) -> Message;

    /// Wrap raw image bytes in this backend's content-block shape.
    fn build_image_block(&self, data: &[u8], media_type: &str) -> ContentBlock;
    /// Optional `(initial, inter_chunk)` stream timeouts; `None` means "do not police the stream".
    ///
    /// The first bounds the wait for the opening chunk, the second the gap
    /// between chunks thereafter (see `session.rs`).
    fn stream_timeouts(&self) -> Option<(Duration, Duration)> {
        None
    }
    /// Stable identifier for this backend, used in logs and telemetry.
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

/// Per-turn knobs handed to [`AgentProvider::stream`].
#[derive(Debug, Clone, Default)]
pub struct StreamOptions {
    /// Backend-specific model identifier.
    pub model: String,
    /// System prompt for this turn, when the caller sets one.
    pub system_prompt: Option<String>,
    /// Upper bound on generated tokens; `None` leaves the backend default.
    pub max_tokens: Option<u32>,
    /// Sampling temperature; `None` leaves the backend default.
    pub temperature: Option<f32>,
    /// Per-request chain control (operator's specification 2026-05-26 4th iteration):
    /// retry attempts must NOT resend prior context via stored previous_response_id.
    /// When `true`, provider clears any stored response chain BEFORE building the
    /// request — next call starts fresh, no context bloat from prior failed attempts.
    /// Callers (session retry path) set `true` for retry attempts; normal calls leave
    /// `false` to preserve conversational chain.
    pub reset_chain: bool,
}
