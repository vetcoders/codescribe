use anyhow::Result;
use codescribe_core::agent::AgentProvider;
use codescribe_core::llm::provider::{LlmMode, ProviderKind, resolve_provider};

pub mod anthropic_provider;
pub mod openai_provider;
#[cfg(target_os = "macos")]
pub mod tools;

pub use anthropic_provider::AnthropicProvider;
pub use openai_provider::OpenAiProvider;

pub fn create_openai_provider_from_env() -> Result<OpenAiProvider> {
    OpenAiProvider::from_env()
}

/// Build the agent (assistive-lane) provider from the configured provider
/// identity (`LLM_ASSISTIVE_PROVIDER`, resolved by
/// [`resolve_provider`]). A missing key for the SELECTED provider surfaces its
/// own clear error (`from_env`) rather than silently falling back to the other
/// provider — misconfiguration must never route to an unintended vendor.
pub fn create_default_provider() -> Result<Box<dyn AgentProvider>> {
    match resolve_provider(LlmMode::Assistive) {
        ProviderKind::OpenAiResponses => Ok(Box::new(OpenAiProvider::from_env()?)),
        ProviderKind::AnthropicMessages => Ok(Box::new(AnthropicProvider::from_env()?)),
    }
}
