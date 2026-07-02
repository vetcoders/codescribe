use anyhow::Result;
use codescribe_core::agent::AgentProvider;

pub mod anthropic_provider;
pub mod openai_provider;
#[cfg(target_os = "macos")]
pub mod tools;

pub use anthropic_provider::AnthropicProvider;
pub use openai_provider::OpenAiProvider;

pub fn create_openai_provider_from_env() -> Result<OpenAiProvider> {
    OpenAiProvider::from_env()
}

pub fn create_default_provider() -> Result<Box<dyn AgentProvider>> {
    Ok(Box::new(OpenAiProvider::from_env()?))
}
