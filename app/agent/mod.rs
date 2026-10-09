//! Application-side agent wiring: the concrete provider clients, the resident
//! run monitor, and the macOS-only native tool surface.
//!
//! Provider choice is made by protocol (wire family), not by vendor name, so a
//! new vendor speaking an existing protocol needs no new client here.

use anyhow::Result;
use codescribe_core::agent::{AgentProvider, ContentBlock, Message, Role, StreamOptions};
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

/// The Agent's canonical per-request configuration: the Agent lane model and
/// the composed Agent system prompt from one immutable snapshot, plus the
/// Agent token cap the caller already reads. The controller chat, the bridge
/// chat and Max all take their options from here, so an Agent prompt or
/// config edit reaches every Agent surface and no formatter prompt can. A
/// non-positive `ai_assistive_max_tokens` resolves to `None` (provider
/// default) instead of a zero-token request. `reset_chain` stays false: only
/// retry paths override it.
pub fn agent_stream_options(
    runtime_settings: &RuntimeSettingsSnapshot,
    ai_assistive_max_tokens: i32,
    use_assistive_persona: bool,
) -> StreamOptions {
    StreamOptions {
        model: runtime_settings.llm_lanes().assistive().model().to_string(),
        system_prompt: Some(compose_agent_system_prompt(
            use_assistive_persona,
            runtime_settings
                .ai_execution()
                .formatter()
                .assistive_prompt()
                .composed_content(),
        )),
        max_tokens: u32::try_from(ai_assistive_max_tokens)
            .ok()
            .filter(|tokens| *tokens > 0),
        temperature: None,
        reset_chain: false,
    }
}

/// Compose the Agent system prompt.
///
/// - `use_assistive_persona=true` (act-on-selection lane, bridge chat, Max):
///   base is the configured Agent prompt (`assistive.txt`).
/// - `use_assistive_persona=false` (voice-chat lane, W10-D): agent persona only,
///   no "text assistant" identity.
///
/// Both carry the WORKSPACE section (project roots, resolve names via
/// `list_projects`), the review-tool and connector doctrine, and the measured
/// Responses/streaming API ground truth with the answer-first rule (operator
/// incident 2026-08-14: a spoken engine question got a clarification
/// questionnaire instead of an answer). Those sections describe the macOS tool
/// surface and are absent where that surface does not exist.
pub fn compose_agent_system_prompt(use_assistive_persona: bool, assistive_prompt: &str) -> String {
    let base = if use_assistive_persona {
        assistive_prompt
    } else {
        "You are the Codescribe agent. Answer and act on the user's spoken request using the available tools when helpful."
    };
    let mut sections = vec![base.to_string()];
    sections.extend(agent_context_sections());
    sections.join("\n\n")
}

#[cfg(target_os = "macos")]
fn agent_context_sections() -> Vec<String> {
    vec![
        tools::workspace::workspace_prompt_section(),
        tools::doctrine::review_doctrine_prompt_section(),
        tools::api_truth::responses_api_prompt_section(),
    ]
}

#[cfg(not(target_os = "macos"))]
fn agent_context_sections() -> Vec<String> {
    Vec::new()
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
