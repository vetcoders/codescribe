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

use crate::config::keychain::{self, KEYCHAIN_ACCOUNTS};
use crate::config::{
    Config, DEFAULT_ASSISTIVE_MODEL, DEFAULT_FORMATTING_MODEL, DEFAULT_LLM_MODEL,
    DEFAULT_OPENAI_RESPONSES_ENDPOINT,
};

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
}

impl ApiKeyLivenessResult {
    fn new(account: &str, status: ApiKeyLivenessStatus, message: impl Into<String>) -> Self {
        Self {
            account: account.to_string(),
            status,
            message: message.into(),
        }
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
    if status == StatusCode::TOO_MANY_REQUESTS || body_lower.contains("insufficient_quota") {
        return ApiKeyLivenessStatus::NoQuota;
    }

    if status == StatusCode::UNAUTHORIZED || status == StatusCode::FORBIDDEN {
        return ApiKeyLivenessStatus::Invalid;
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
    let endpoint = normalize_openai_responses_endpoint(&endpoint);
    let model = openai_model_for_account(account);
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
        .post(endpoint)
        .bearer_auth(api_key)
        .header("x-api-key", api_key)
        .header("Content-Type", "application/json")
        .json(&request)
        .send();

    response_result(account, response)
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
        .post(endpoint)
        .header("x-api-key", api_key)
        .header("anthropic-version", ANTHROPIC_VERSION)
        .header("Content-Type", "application/json")
        .json(&request)
        .send();

    response_result(account, response)
}

fn probe_github_token(client: &Client, account: &str, api_key: &str) -> ApiKeyLivenessResult {
    let endpoint = env_non_empty("CODESCRIBE_GITHUB_PROBE_ENDPOINT")
        .unwrap_or_else(|| "https://api.github.com/user".to_string());
    let response = client
        .get(endpoint)
        .bearer_auth(api_key)
        .header("User-Agent", "Codescribe API key liveness probe")
        .send();

    response_result(account, response)
}

fn response_result(
    account: &str,
    response: Result<Response, reqwest::Error>,
) -> ApiKeyLivenessResult {
    match response {
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
    }
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
    env_non_empty(account).or_else(|| {
        keychain::load_key(account)
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
    })
}

fn openai_endpoint_for_account(config: &Config, account: &str) -> String {
    let fallback = || {
        env_non_empty("LLM_ENDPOINT")
            .or_else(|| config.llm_endpoint.clone())
            .unwrap_or_else(|| DEFAULT_OPENAI_RESPONSES_ENDPOINT.to_string())
    };

    match account {
        "LLM_FORMATTING_API_KEY" => {
            env_non_empty("LLM_FORMATTING_ENDPOINT").unwrap_or_else(fallback)
        }
        "LLM_ASSISTIVE_API_KEY" => env_non_empty("LLM_ASSISTIVE_ENDPOINT").unwrap_or_else(fallback),
        _ => fallback(),
    }
}

fn openai_model_for_account(account: &str) -> String {
    match account {
        "LLM_FORMATTING_API_KEY" => env_non_empty("LLM_FORMATTING_MODEL")
            .or_else(|| env_non_empty("LLM_MODEL"))
            .unwrap_or_else(|| DEFAULT_FORMATTING_MODEL.to_string()),
        "LLM_ASSISTIVE_API_KEY" => env_non_empty("LLM_ASSISTIVE_MODEL")
            .or_else(|| env_non_empty("LLM_MODEL"))
            .unwrap_or_else(|| DEFAULT_ASSISTIVE_MODEL.to_string()),
        _ => env_non_empty("LLM_MODEL").unwrap_or_else(|| DEFAULT_LLM_MODEL.to_string()),
    }
}

fn normalize_openai_responses_endpoint(endpoint: &str) -> String {
    normalize_endpoint(
        endpoint,
        "/v1/responses",
        &["/v1/responses", "/v1/chat/completions", "/v1/completions"],
    )
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
    fn classifies_other_http_failures_as_network_unknown() {
        assert_eq!(
            classify_probe_response(StatusCode::INTERNAL_SERVER_ERROR, "try later"),
            ApiKeyLivenessStatus::Network
        );
        assert_eq!(
            classify_probe_response(StatusCode::BAD_REQUEST, "bad request"),
            ApiKeyLivenessStatus::Network
        );
    }
}
