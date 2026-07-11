//! Minimal API-key liveness probes for Settings.
//!
//! This is intentionally not a general health framework. Each probe makes one
//! cheap provider request and classifies the result into UI-safe buckets:
//! key works, invalid key, no quota/credits, network/unknown, missing, or
//! unsupported.

use std::time::Duration;

use reqwest::StatusCode;
use reqwest::blocking::{Client, Response};
use serde_json::json;

use crate::config::Config;
use crate::config::keychain::KEYCHAIN_ACCOUNTS;
use crate::llm::lane_truth;

const PROBE_TIMEOUT: Duration = Duration::from_secs(10);
const DEFAULT_ANTHROPIC_ENDPOINT: &str = "https://api.anthropic.com/v1/messages";
const DEFAULT_ANTHROPIC_MODEL: &str = "claude-opus-4-8";
const ANTHROPIC_VERSION: &str = "2023-06-01";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApiKeyLivenessStatus {
    Ok,
    Invalid,
    NoQuota,
    Network,
    Missing,
    Unsupported,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApiKeyLivenessResult {
    pub account: String,
    pub status: ApiKeyLivenessStatus,
    pub message: String,
    pub probed_endpoint: Option<String>,
}

impl ApiKeyLivenessResult {
    fn new(account: &str, status: ApiKeyLivenessStatus, message: impl Into<String>) -> Self {
        Self {
            account: account.to_string(),
            status,
            message: message.into(),
            probed_endpoint: None,
        }
    }

    fn with_probed_endpoint(mut self, endpoint: String) -> Self {
        self.probed_endpoint = Some(endpoint);
        self
    }
}

pub fn probe_api_key_liveness(account: &str) -> ApiKeyLivenessResult {
    if !KEYCHAIN_ACCOUNTS.contains(&account) {
        return ApiKeyLivenessResult::new(
            account,
            ApiKeyLivenessStatus::Unsupported,
            "unknown Keychain account",
        );
    }

    let config = Config::load();
    let Some(api_key) = account_secret(account) else {
        return ApiKeyLivenessResult::new(
            account,
            ApiKeyLivenessStatus::Missing,
            "key is not configured",
        );
    };

    if account == "STT_API_KEY" {
        return ApiKeyLivenessResult::new(
            account,
            ApiKeyLivenessStatus::Unsupported,
            "no cheap liveness probe is available for this STT key",
        );
    }

    let client = match Client::builder()
        .timeout(PROBE_TIMEOUT)
        .connect_timeout(PROBE_TIMEOUT)
        .build()
    {
        Ok(client) => client,
        Err(error) => {
            return ApiKeyLivenessResult::new(
                account,
                ApiKeyLivenessStatus::Network,
                format!("failed to create HTTP client: {error}"),
            );
        }
    };

    match account {
        "LLM_API_KEY" | "LLM_FORMATTING_API_KEY" | "LLM_ASSISTIVE_API_KEY" => {
            probe_openai_key(&client, &config, account, &api_key)
        }
        "LLM_ANTHROPIC_API_KEY" => probe_anthropic_key(&client, account, &api_key),
        "GITHUB_TOKEN" => probe_github_token(&client, account, &api_key),
        _ => ApiKeyLivenessResult::new(
            account,
            ApiKeyLivenessStatus::Unsupported,
            "no liveness probe is available for this key",
        ),
    }
}

/// Classify one provider HTTP response. This is the tested contract; network
/// errors are classified at the request boundary because there is no HTTP status.
pub fn classify_probe_response(status: StatusCode, body: &str) -> ApiKeyLivenessStatus {
    if status.is_success() {
        return ApiKeyLivenessStatus::Ok;
    }

    let body_lower = body.to_ascii_lowercase();
    if status == StatusCode::TOO_MANY_REQUESTS
        || status == StatusCode::PAYMENT_REQUIRED
        || body_lower.contains("insufficient_quota")
        || body_lower.contains("credit balance is too low")
        || body_lower.contains("billing_error")
    {
        return ApiKeyLivenessStatus::NoQuota;
    }

    if status == StatusCode::UNAUTHORIZED || status == StatusCode::FORBIDDEN {
        return ApiKeyLivenessStatus::Invalid;
    }

    // Any other client error (4xx, e.g. 400 model_not_found, 404) means the
    // server processed the request and the key passed authentication — the key
    // is live even if this particular probe request was malformed. Only real
    // transport failures (handled at the request boundary) and server-side
    // errors (5xx) remain unverifiable.
    if status.is_client_error() {
        return ApiKeyLivenessStatus::Ok;
    }

    ApiKeyLivenessStatus::Network
}

fn probe_openai_key(
    client: &Client,
    config: &Config,
    account: &str,
    api_key: &str,
) -> ApiKeyLivenessResult {
    let endpoint = openai_endpoint_for_account(config, account);
    let model = openai_model_for_account(config, account);
    let request = json!({
        "model": model,
        "input": [{
            "role": "user",
            "content": [{ "type": "input_text", "text": "ping" }]
        }],
        "max_output_tokens": 1,
        "stream": false
    });

    let response = client
        .post(&endpoint)
        .bearer_auth(api_key)
        .header("x-api-key", api_key)
        .header("Content-Type", "application/json")
        .json(&request)
        .send();

    response_result(account, endpoint, response)
}

fn probe_anthropic_key(client: &Client, account: &str, api_key: &str) -> ApiKeyLivenessResult {
    let endpoint = env_non_empty("LLM_ANTHROPIC_ENDPOINT")
        .unwrap_or_else(|| DEFAULT_ANTHROPIC_ENDPOINT.to_string());
    let endpoint = normalize_anthropic_messages_endpoint(&endpoint);
    let model = env_non_empty("LLM_ASSISTIVE_MODEL")
        .filter(|m| m.starts_with("claude"))
        .unwrap_or_else(|| DEFAULT_ANTHROPIC_MODEL.to_string());
    let request = json!({
        "model": model,
        "messages": [{
            "role": "user",
            "content": [{ "type": "text", "text": "ping" }]
        }],
        "max_tokens": 1
    });

    let response = client
        .post(&endpoint)
        .header("x-api-key", api_key)
        .header("anthropic-version", ANTHROPIC_VERSION)
        .header("Content-Type", "application/json")
        .json(&request)
        .send();

    response_result(account, endpoint, response)
}

fn probe_github_token(client: &Client, account: &str, api_key: &str) -> ApiKeyLivenessResult {
    let endpoint = env_non_empty("CODESCRIBE_GITHUB_PROBE_ENDPOINT")
        .unwrap_or_else(|| "https://api.github.com/user".to_string());
    let response = client
        .get(&endpoint)
        .bearer_auth(api_key)
        .header("User-Agent", "Codescribe API key liveness probe")
        .send();

    response_result(account, endpoint, response)
}

fn response_result(
    account: &str,
    probed_endpoint: String,
    response: Result<Response, reqwest::Error>,
) -> ApiKeyLivenessResult {
    let result = match response {
        Ok(response) => {
            let status = response.status();
            let body = response.text().unwrap_or_default();
            let probe_status = classify_probe_response(status, &body);
            ApiKeyLivenessResult::new(account, probe_status, message_for_status(probe_status))
        }
        Err(error) => ApiKeyLivenessResult::new(
            account,
            ApiKeyLivenessStatus::Network,
            format!("network error: {error}"),
        ),
    };
    result.with_probed_endpoint(probed_endpoint)
}

fn message_for_status(status: ApiKeyLivenessStatus) -> &'static str {
    match status {
        ApiKeyLivenessStatus::Ok => "key accepted and quota available",
        ApiKeyLivenessStatus::Invalid => "provider rejected this key",
        ApiKeyLivenessStatus::NoQuota => "key is valid, but the account has no quota or credits",
        ApiKeyLivenessStatus::Network => "could not verify this key",
        ApiKeyLivenessStatus::Missing => "key is not configured",
        ApiKeyLivenessStatus::Unsupported => "probe is not supported for this key",
    }
}

fn account_secret(account: &str) -> Option<String> {
    lane_truth::secret(account)
}

fn openai_endpoint_for_account(config: &Config, account: &str) -> String {
    lane_truth::endpoint_for_account(config, account)
}

fn openai_model_for_account(config: &Config, account: &str) -> String {
    lane_truth::model_for_account(config, account)
}

fn normalize_anthropic_messages_endpoint(endpoint: &str) -> String {
    normalize_endpoint(endpoint, "/v1/messages", &["/v1/messages", "/v1/responses"])
}

fn normalize_endpoint(endpoint: &str, canonical_suffix: &str, known_suffixes: &[&str]) -> String {
    let mut base = endpoint.trim().trim_end_matches('/').to_string();
    for suffix in known_suffixes {
        if base.ends_with(suffix) {
            base.truncate(base.len() - suffix.len());
            return format!("{base}{canonical_suffix}");
        }
    }
    if base.ends_with("/v1") {
        base.truncate(base.len() - "/v1".len());
    }
    format!("{base}{canonical_suffix}")
}

fn env_non_empty(key: &str) -> Option<String> {
    std::env::var(key)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::thread;
    use tempfile::TempDir;

    #[test]
    #[serial]
    fn openai_probe_reports_the_normalized_endpoint_it_called() {
        let data_dir = TempDir::new().expect("isolated data dir");
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind probe server");
        let address = listener.local_addr().expect("probe server address");
        let base_endpoint = format!("http://{address}");
        let expected_endpoint = format!("{base_endpoint}/v1/responses");

        let _data_dir = EnvGuard::set(
            "CODESCRIBE_DATA_DIR",
            data_dir.path().to_string_lossy().as_ref(),
        );
        let _shared_endpoint = EnvGuard::remove("LLM_ENDPOINT");
        let _assistive_endpoint = EnvGuard::set("LLM_ASSISTIVE_ENDPOINT", &base_endpoint);
        let _assistive_model = EnvGuard::set("LLM_ASSISTIVE_MODEL", "gpt-probe");

        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept probe request");
            let mut buffer = [0_u8; 4096];
            let bytes_read = stream.read(&mut buffer).expect("read probe request");
            stream
                .write_all(
                    b"HTTP/1.1 401 Unauthorized\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}",
                )
                .expect("write probe response");
            String::from_utf8_lossy(&buffer[..bytes_read])
                .lines()
                .next()
                .unwrap_or_default()
                .to_string()
        });

        let client = Client::builder()
            .timeout(PROBE_TIMEOUT)
            .connect_timeout(PROBE_TIMEOUT)
            .build()
            .expect("build probe client");
        let result = probe_openai_key(
            &client,
            &Config::default(),
            "LLM_ASSISTIVE_API_KEY",
            "test-key",
        );

        assert_eq!(result.status, ApiKeyLivenessStatus::Invalid);
        assert_eq!(
            result.probed_endpoint.as_deref(),
            Some(expected_endpoint.as_str())
        );
        assert_eq!(
            server.join().expect("probe server thread"),
            "POST /v1/responses HTTP/1.1"
        );
    }

    #[test]
    fn classifies_success_as_ok() {
        assert_eq!(
            classify_probe_response(StatusCode::OK, r#"{"id":"resp_123"}"#),
            ApiKeyLivenessStatus::Ok
        );
    }

    #[test]
    fn classifies_401_and_403_as_invalid() {
        assert_eq!(
            classify_probe_response(StatusCode::UNAUTHORIZED, "{}"),
            ApiKeyLivenessStatus::Invalid
        );
        assert_eq!(
            classify_probe_response(StatusCode::FORBIDDEN, "{}"),
            ApiKeyLivenessStatus::Invalid
        );
    }

    #[test]
    fn classifies_insufficient_quota_body_as_no_quota() {
        assert_eq!(
            classify_probe_response(
                StatusCode::BAD_REQUEST,
                r#"{"error":{"code":"insufficient_quota","message":"billing hard limit"}}"#
            ),
            ApiKeyLivenessStatus::NoQuota
        );
    }

    #[test]
    fn classifies_429_without_body_as_no_quota() {
        assert_eq!(
            classify_probe_response(StatusCode::TOO_MANY_REQUESTS, ""),
            ApiKeyLivenessStatus::NoQuota
        );
    }

    #[test]
    fn classifies_anthropic_low_credit_body_as_no_quota() {
        assert_eq!(
            classify_probe_response(
                StatusCode::BAD_REQUEST,
                r#"{"type":"error","error":{"type":"invalid_request_error","message":"Your credit balance is too low to access the Anthropic API."}}"#
            ),
            ApiKeyLivenessStatus::NoQuota
        );
    }

    #[test]
    fn classifies_402_billing_error_as_no_quota() {
        assert_eq!(
            classify_probe_response(
                StatusCode::PAYMENT_REQUIRED,
                r#"{"error":{"type":"billing_error","message":"payment required"}}"#
            ),
            ApiKeyLivenessStatus::NoQuota
        );
    }

    #[test]
    fn classifies_400_model_not_found_as_ok() {
        // A 400 with a request-level error (not quota/auth) means the server
        // accepted the key and processed the request: the key is live.
        assert_eq!(
            classify_probe_response(
                StatusCode::BAD_REQUEST,
                r#"{"error":{"code":"model_not_found","message":"The model does not exist"}}"#
            ),
            ApiKeyLivenessStatus::Ok
        );
    }

    #[test]
    fn classifies_other_client_errors_as_ok() {
        assert_eq!(
            classify_probe_response(StatusCode::BAD_REQUEST, "bad request"),
            ApiKeyLivenessStatus::Ok
        );
        assert_eq!(
            classify_probe_response(StatusCode::NOT_FOUND, "no such endpoint"),
            ApiKeyLivenessStatus::Ok
        );
    }

    #[test]
    fn classifies_server_errors_as_network_unknown() {
        assert_eq!(
            classify_probe_response(StatusCode::INTERNAL_SERVER_ERROR, "try later"),
            ApiKeyLivenessStatus::Network
        );
        assert_eq!(
            classify_probe_response(StatusCode::BAD_GATEWAY, "upstream down"),
            ApiKeyLivenessStatus::Network
        );
    }

    struct EnvGuard {
        key: &'static str,
        previous: Option<String>,
    }

    impl EnvGuard {
        fn set(key: &'static str, value: &str) -> Self {
            let previous = std::env::var(key).ok();
            // SAFETY: process-env tests in this module are serialized.
            unsafe { std::env::set_var(key, value) };
            Self { key, previous }
        }

        fn remove(key: &'static str) -> Self {
            let previous = std::env::var(key).ok();
            // SAFETY: process-env tests in this module are serialized.
            unsafe { std::env::remove_var(key) };
            Self { key, previous }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            // SAFETY: process-env tests in this module are serialized.
            unsafe {
                match self.previous.as_deref() {
                    Some(value) => std::env::set_var(self.key, value),
                    None => std::env::remove_var(self.key),
                }
            }
        }
    }
}
