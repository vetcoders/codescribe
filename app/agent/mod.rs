use anyhow::Result;
use codescribe_core::agent::AgentProvider;
use codescribe_core::config::Config;
use codescribe_core::llm::lane_truth;
use codescribe_core::llm::provider::ProviderKind;

pub mod anthropic_provider;
pub mod openai_provider;
#[cfg(target_os = "macos")]
pub mod tools;

pub use anthropic_provider::AnthropicProvider;
pub use openai_provider::OpenAiProvider;

/// Build the agent (assistive-lane) provider from the lane-truth snapshot
/// (fresh settings → env → Keychain), so a Settings save is honored on the
/// very next send — no restart, no stale bootstrap env. A lane that cannot be
/// reached fails with the same actionable reason
/// [`assistive_unavailable_reason`] reports, never a generic key demand, and
/// never a silent fallback to an unintended vendor.
pub fn create_default_provider() -> Result<Box<dyn AgentProvider>> {
    let config = Config::load();
    let lane = lane_truth::assistive_availability(&config).map_err(anyhow::Error::msg)?;
    match lane.provider {
        ProviderKind::OpenAiResponses => Ok(Box::new(OpenAiProvider::from_lane(lane)?)),
        ProviderKind::AnthropicMessages => Ok(Box::new(AnthropicProvider::from_lane(lane)?)),
    }
}

/// User-facing reason the assistive lane cannot reach a model right now
/// (`None` when a send can proceed). Kept beside [`create_default_provider`]
/// so the availability gate and provider construction can never drift.
pub fn assistive_unavailable_reason() -> Option<String> {
    lane_truth::assistive_availability(&Config::load()).err()
}
