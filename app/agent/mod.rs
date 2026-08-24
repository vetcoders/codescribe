//! Application-side agent wiring: the concrete provider clients, the resident
//! run monitor, and the macOS-only native tool surface.
//!
//! Provider choice is made by protocol (wire family), not by vendor name, so a
//! new vendor speaking an existing protocol needs no new client here.

use anyhow::Result;
use codescribe_core::agent::AgentProvider;
use codescribe_core::config::{RuntimeLlmLane, RuntimeLlmLaneKind};
use codescribe_core::llm::provider::WireFamily;

/// Anthropic Messages-family assistive provider client.
pub mod anthropic_provider;
/// Resident agent-run monitor (progress, cancel, status surfaces).
pub mod monitor;
/// OpenAI Responses-family client (also carries xAI and other Responses vendors).
pub mod openai_provider;
/// macOS-only native tool surface (filesystem, process, MCP, guards).
#[cfg(target_os = "macos")]
pub mod tools;

pub use anthropic_provider::AnthropicProvider;
pub use openai_provider::OpenAiProvider;

/// Build the Agent provider from the exact assistive lane sealed in the
/// controller-owned runtime settings snapshot.
pub fn create_default_provider(lane: &RuntimeLlmLane) -> Result<Box<dyn AgentProvider>> {
    anyhow::ensure!(
        lane.lane() == RuntimeLlmLaneKind::Assistive,
        "agent provider requires the assistive runtime lane"
    );
    if !lane.available() {
        anyhow::bail!(
            "{}",
            lane.unavailable_reason()
                .unwrap_or("assistive runtime lane is unavailable")
        );
    }
    // Selected by protocol, not vendor: `OpenAiProvider` is the Responses-family
    // client and carries the lane's provider identity, so xAI rides it without a
    // second implementation.
    match lane.wire_family() {
        WireFamily::OpenAiResponses => Ok(Box::new(OpenAiProvider::from_lane(lane)?)),
        WireFamily::AnthropicMessages => Ok(Box::new(AnthropicProvider::from_lane(lane)?)),
    }
}

/// User-facing reason the assistive lane cannot reach a model right now
/// (`None` when a send can proceed). Kept beside [`create_default_provider`]
/// so the availability gate and provider construction can never drift.
pub fn assistive_unavailable_reason(lane: &RuntimeLlmLane) -> Option<String> {
    (!lane.available()).then(|| {
        lane.unavailable_reason()
            .unwrap_or("assistive runtime lane is unavailable")
            .to_string()
    })
}
