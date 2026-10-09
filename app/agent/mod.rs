//! Application-side agent wiring: the concrete provider clients, the resident
//! run monitor, and the macOS-only native tool surface.
//!
//! Provider choice is made by protocol (wire family), not by vendor name, so a
//! new vendor speaking an existing protocol needs no new client here.

use anyhow::Result;
use codescribe_core::agent::{AgentProvider, ContentBlock, Message, Role};
use codescribe_core::config::{FormattingPolicy, RuntimeLlmLane, RuntimeSettingsSnapshot};
use codescribe_core::llm::provider::WireFamily;

/// Anthropic Messages-family assistive provider client.
pub mod anthropic_provider;
/// Max consultation host admission on the Agent lane.
pub mod max_consultation;
/// Resident agent-run monitor (progress, cancel, status surfaces).
pub mod monitor;
/// OpenAI Responses-family client (also carries xAI and other Responses vendors).
pub mod openai_provider;
/// OpenAI / Codex-backend JSON Schema subset adapter for tool parameters.
mod openai_schema;
/// macOS-only native tool surface (filesystem, process, MCP, guards).
#[cfg(target_os = "macos")]
pub mod tools;

pub use anthropic_provider::AnthropicProvider;
pub use openai_provider::OpenAiProvider;

/// Build the tool-capable Agent provider from the sealed Agent (assistive)
/// lane. Chat and Max share this one constructor, so Max runs on exactly the
/// Agent's provider, endpoint, model and account; the formatting lane never
/// enters the tool-capable runtime.
pub fn create_agent_provider(
    runtime_settings: &RuntimeSettingsSnapshot,
) -> Result<Box<dyn AgentProvider>> {
    let lane = runtime_settings.llm_lanes().assistive();
    if let Some(reason) = assistive_unavailable_reason(lane) {
        anyhow::bail!("{reason}");
    }
    let request_timing = runtime_settings.ai_execution().request_timing();
    // Selected by protocol, not vendor: `OpenAiProvider` is the Responses-family
    // client and carries the lane's provider identity, so xAI rides it without a
    // second implementation.
    match lane.wire_family() {
        WireFamily::OpenAiResponses => {
            Ok(Box::new(OpenAiProvider::from_lane(lane, request_timing)?))
        }
        WireFamily::AnthropicMessages => Ok(Box::new(AnthropicProvider::from_lane(
            lane,
            request_timing,
        )?)),
    }
}

/// User-facing reason the Agent lane cannot reach a model right now
/// (`None` when a send can proceed). Kept beside [`create_agent_provider`]
/// so the availability gate and provider construction can never drift.
pub fn assistive_unavailable_reason(lane: &RuntimeLlmLane) -> Option<String> {
    lane.request_unavailable_reason()
}

/// Whether the selected formatting level has an engine under this
/// generation. Max is the Agent: it is available only when the Agent lane is,
/// and never borrows the formatting lane or Apple. Smart/Corrections are text
/// passes: Apple on-device (when selected) or the formatting lane.
pub fn formatting_unavailable_reason(runtime_settings: &RuntimeSettingsSnapshot) -> Option<String> {
    match runtime_settings.formatting_policy() {
        FormattingPolicy::Off => Some("Formatting is off".to_string()),
        FormattingPolicy::Max => max_unavailable_reason(runtime_settings),
        FormattingPolicy::Correction | FormattingPolicy::Smart => {
            codescribe_core::ai_formatting::text_formatting_unavailable_reason(runtime_settings)
        }
    }
}

/// Max needs a usable Agent configuration. The reason names the Agent lane so
/// the user fixes the Agent endpoint rather than the formatter.
pub fn max_unavailable_reason(runtime_settings: &RuntimeSettingsSnapshot) -> Option<String> {
    assistive_unavailable_reason(runtime_settings.llm_lanes().assistive())
        .map(|reason| format!("Max uses the Agent model. {reason}"))
}

/// In-memory user turn carrying one tool result.
///
/// The Messages client and the Responses client both park tool output in a
/// user turn. Each provider still encodes its own wire body.
pub(crate) fn user_tool_result(
    call_id: &str,
    content: Vec<ContentBlock>,
    is_error: bool,
) -> Message {
    Message::new(
        Role::User,
        vec![ContentBlock::ToolResult {
            tool_use_id: call_id.to_string(),
            content,
            is_error,
        }],
    )
}

/// In-memory image block. Empty bytes stay here; request builders skip them.
pub(crate) fn image_block(data: &[u8], media_type: &str) -> ContentBlock {
    ContentBlock::Image {
        data: data.to_vec(),
        media_type: media_type.to_string(),
    }
}
