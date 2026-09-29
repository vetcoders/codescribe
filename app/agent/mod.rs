//! Application-side agent wiring: the concrete provider clients, the resident
//! run monitor, and the macOS-only native tool surface.
//!
//! Provider choice is made by protocol (wire family), not by vendor name, so a
//! new vendor speaking an existing protocol needs no new client here.

use anyhow::Result;
use codescribe_core::agent::AgentProvider;
use codescribe_core::config::{
    FormattingPolicy, RuntimeLlmLane, RuntimeLlmLaneKind, RuntimeSettingsSnapshot,
};
use codescribe_core::llm::provider::WireFamily;

/// Anthropic Messages-family assistive provider client.
pub mod anthropic_provider;
/// Formatting-lane host admission for the connected Max consultation.
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

/// Build an Agent provider from an explicitly selected sealed lane.
/// Formatting may enter the tool-capable runtime only under Max; ordinary
/// Agent chat keeps its independent assistive lane.
pub fn create_provider_for_lane(
    runtime_settings: &RuntimeSettingsSnapshot,
    lane_kind: RuntimeLlmLaneKind,
) -> Result<Box<dyn AgentProvider>> {
    anyhow::ensure!(
        lane_kind == RuntimeLlmLaneKind::Assistive
            || runtime_settings.formatting_policy() == FormattingPolicy::Max,
        "tool-capable formatting requires Max policy"
    );
    let lane = match lane_kind {
        RuntimeLlmLaneKind::Assistive => runtime_settings.llm_lanes().assistive(),
        RuntimeLlmLaneKind::Formatting => runtime_settings.llm_lanes().formatting(),
    };
    let request_timing = runtime_settings.ai_execution().request_timing();
    if !lane.request_available() {
        anyhow::bail!(
            "{}",
            lane.unavailable_reason()
                .unwrap_or("selected agent runtime lane is unavailable")
        );
    }
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

/// User-facing reason the assistive lane cannot reach a model right now
/// (`None` when a send can proceed). Kept beside [`create_provider_for_lane`]
/// so the availability gate and provider construction can never drift.
pub fn assistive_unavailable_reason(lane: &RuntimeLlmLane) -> Option<String> {
    (!lane.request_available()).then(|| {
        lane.unavailable_reason()
            .unwrap_or("assistive runtime lane is unavailable")
            .to_string()
    })
}
