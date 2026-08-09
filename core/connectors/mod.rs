//! External content connectors for Codescribe attachments.
//!
//! Each connector fetches content from an external source and produces
//! files on disk that become regular `Attachment` objects.

pub mod github;
/// HTTP web connector helpers (shared client, fetch, page extract).
pub mod web;

use std::sync::OnceLock;

/// Process-wide reqwest::Client for connector fetches; built once on first use.
static SHARED_CLIENT: OnceLock<reqwest::Client> = OnceLock::new();

/// Shared HTTP client for all connectors (connection pooling, single TLS init).
pub(crate) fn shared_client() -> &'static reqwest::Client {
    SHARED_CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(15))
            .redirect(reqwest::redirect::Policy::limited(3))
            .user_agent("Codescribe/1.0 (speech-to-text assistant)")
            .build()
            .expect("Failed to build shared HTTP client")
    })
}
