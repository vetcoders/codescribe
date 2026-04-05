//! External content connectors for CodeScribe attachments.
//!
//! Each connector fetches content from an external source and produces
//! files on disk that become regular `Attachment` objects.

pub mod github;
pub mod web;

use anyhow::{Result, anyhow};
use std::sync::OnceLock;

static SHARED_CLIENT: OnceLock<Result<reqwest::Client, String>> = OnceLock::new();

const CONNECTOR_USER_AGENT: &str = "CodeScribe/1.0 (speech-to-text assistant)";

fn build_shared_client(user_agent: &str) -> Result<reqwest::Client> {
    reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .redirect(reqwest::redirect::Policy::limited(3))
        .user_agent(user_agent)
        .build()
        .map_err(|err| anyhow!("Failed to build shared HTTP client: {err}"))
}

/// Shared HTTP client for all connectors (connection pooling, single TLS init).
pub(crate) fn shared_client() -> Result<&'static reqwest::Client> {
    match SHARED_CLIENT
        .get_or_init(|| build_shared_client(CONNECTOR_USER_AGENT).map_err(|err| format!("{err:#}")))
    {
        Ok(client) => Ok(client),
        Err(err) => Err(anyhow!("Failed to build shared HTTP client: {err}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shared_client_builder_surfaces_invalid_user_agent() {
        let err = build_shared_client("CodeScribe\ninvalid").expect_err("invalid UA should fail");
        assert!(
            err.to_string()
                .contains("Failed to build shared HTTP client"),
            "unexpected error: {err:#}"
        );
    }
}
