//! Minimal API-key liveness probes for Settings.
//!
//! This is intentionally not a general health framework. Each probe makes one
//! cheap provider request and classifies the result into UI-safe buckets:
//! key works, invalid key, no quota/credits, network/unknown, missing, or
//! unsupported.

use std::time::Duration;

use reqwest::StatusCode;
use reqwest::blocking::multipart::{Form, Part};
use reqwest::blocking::{Client, Response};
use serde_json::json;

use crate::config::{Config, RuntimeLlmLane, RuntimeSettingsSnapshot};
use crate::config::keychain::KEYCHAIN_ACCOUNTS;
#[cfg(test)]
use crate::llm::provider::ProviderKind;
use crate::llm::provider::WireFamily;

/// Wall-clock budget for connect and request; probes must stay cheap for Settings.
const PROBE_TIMEOUT: Duration = Duration::from_secs(10);
/// Anthropic Messages API version header required by the wire family probe.
const ANTHROPIC_VERSION: &str = "2023-06-01";

/// UI-safe outcome of one liveness probe.
///
/// Deliberately coarse: Settings needs to tell the user what to *do*, so
/// "provider processed the request" collapses to [`Ok`] even for a 4xx that
/// only means the probe body was wrong — the key itself authenticated. Only
/// transport failures and 5xx stay unverifiable ([`Network`]).
///
/// [`Ok`]: ApiKeyLivenessStatus::Ok
/// [`Network`]: ApiKeyLivenessStatus::Network
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApiKeyLivenessStatus {
    Ok,
    Invalid,
    NoQuota,
    Network,
    Missing,
    Unsupported,
}

/// One probe verdict for one Keychain account.
///
/// `probed_endpoint` records the URL actually called after lane/provider
/// resolution — the answer to "which server rejected my key", which is the
/// difference between a bad key and a misrouted lane.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApiKeyLivenessResult {
    pub account: String,
    pub status: ApiKeyLivenessStatus,
    pub message: String,
    pub probed_endpoint: Option<String>,
}

impl ApiKeyLivenessResult {
    /// Verdict with no endpoint attached — for the cases decided before any
    /// request is made (unknown account, missing key, unsupported probe).
    fn new(account: &str, status: ApiKeyLivenessStatus, message: impl Into<String>) -> Self {
        Self {
            account: account.to_string(),
            status,
            message: message.into(),
            probed_endpoint: None,
        }
    }

    /// Record the endpoint this verdict came from.
    fn with_probed_endpoint(mut self, endpoint: String) -> Self {
        self.probed_endpoint = Some(endpoint);
        self
    }
}

/// Probe one Keychain account and classify the result for Settings.
///
/// Resolution order is what keeps this honest: unknown accounts and missing
/// keys are answered without a request; `GITHUB_TOKEN` gets its own REST probe;
/// everything else resolves to a provider — the three generic lane accounts to
/// the default one, a vendor's own key through the registry — and is probed by
/// *wire family*, not by vendor, so a new provider on an existing protocol
/// gets a real verdict instead of "unsupported".
pub fn probe_api_key_liveness(
    account: &str,
    snapshot: &RuntimeSettingsSnapshot,
) -> ApiKeyLivenessResult {
    if !KEYCHAIN_ACCOUNTS.contains(&account) {
        return ApiKeyLivenessResult::new(
            account,
            ApiKeyLivenessStatus::Unsupported,
            "unknown Keychain account",
        );
    }

    let config = snapshot.values();
    let llm_lane = [
        snapshot.llm_lanes().main(),
        snapshot.llm_lanes().formatting(),
        snapshot.llm_lanes().assistive(),
    ]
    .into_iter()
    .find(|lane| lane.credential().key_account() == account);
    let api_key = llm_lane
        .and_then(|lane| lane.credential().api_key().map(str::to_string))
        .or_else(|| (account == "STT_API_KEY").then(|| config.stt_api_key.clone()).flatten())
        .or_else(|| {
            (account == "GITHUB_TOKEN")
                .then(|| crate::config::keychain::cached_runtime_key(account))
                .flatten()
        });
    let stt_is_unauthenticated = account == "STT_API_KEY"
        && config
            .stt_endpoint
            .as_deref()
            .filter(|value| !value.trim().is_empty())
            .map(crate::stt::tail_provider::stt_auth_mode)
            .unwrap_or(crate::stt::tail_provider::SttAuthMode::Unauthenticated)
            == crate::stt::tail_provider::SttAuthMode::Unauthenticated;
    let Some(api_key) = api_key.or_else(|| stt_is_unauthenticated.then(String::new)) else {
        return ApiKeyLivenessResult::new(
            account,
            ApiKeyLivenessStatus::Missing,
            "key is not configured",
        );
    };

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

    if account == "STT_API_KEY" {
        return probe_stt_key(&client, &config, account, &api_key);
    }

    if account == "GITHUB_TOKEN" {
        return probe_github_token(&client, account, &api_key);
    }

    let Some(lane) = llm_lane else {
        return ApiKeyLivenessResult::new(
            account,
            ApiKeyLivenessStatus::Unsupported,
            "no sealed runtime LLM lane uses this key account",
        );
    };

    // Probe shape follows the protocol, not the vendor: xAI answers the same
    // Responses ping as OpenAI.
    match lane.wire_family() {
        WireFamily::OpenAiResponses => {
            probe_responses_key(&client, lane, account, &api_key)
        }
        WireFamily::AnthropicMessages => probe_anthropic_key(&client, lane, account, &api_key),
    }
}

/// Probe the configured multipart STT slot with 100 ms of synthetic silence.
/// The response body is never surfaced; only auth/quota/transport status is.
/// A live WebSocket URL is remapped to the file worker first — Test is not a
/// handshake against the Voice Lab socket.
fn probe_stt_key(
    client: &Client,
    config: &Config,
    account: &str,
    api_key: &str,
) -> ApiKeyLivenessResult {
    let endpoint = crate::stt::tail_provider::file_probe_endpoint(
        config
            .stt_endpoint
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or("http://127.0.0.1:8000/v1/audio/transcriptions"),
    );
    if crate::stt::tail_provider::validate_remote_endpoint(&endpoint).is_err() {
        return ApiKeyLivenessResult::new(
            account,
            ApiKeyLivenessStatus::Network,
            "configured STT endpoint is invalid or insecure",
        )
        .with_probed_endpoint(endpoint);
    }
    let silence = [0.0_f32; 1_600];
    let wav = match crate::stt::tail_provider::pcm16_wav(&silence, 16_000) {
        Ok(wav) => wav,
        Err(_) => {
            return ApiKeyLivenessResult::new(
                account,
                ApiKeyLivenessStatus::Network,
                "could not build the STT liveness probe",
            )
            .with_probed_endpoint(endpoint);
        }
    };
    let file = match Part::bytes(wav)
        .file_name("codescribe-key-probe.wav")
        .mime_str("audio/wav")
    {
        Ok(file) => file,
        Err(_) => {
            return ApiKeyLivenessResult::new(
                account,
                ApiKeyLivenessStatus::Network,
                "could not build the STT liveness probe",
            )
            .with_probed_endpoint(endpoint);
        }
    };
    let mut form = Form::new()
        .part("file", file)
        .text("model", "whisper-1")
        .text("language", "pl")
        .text("response_format", "json");
    if let Some((field, value)) =
        crate::stt::request_vocabulary::codescribe_stt_vocabulary_form_part(&endpoint)
    {
        form = form.text(field, value.to_string());
    }
    let request = client.post(&endpoint);
    let auth_mode = crate::stt::tail_provider::stt_auth_mode(&endpoint);
    let request = match auth_mode {
        crate::stt::tail_provider::SttAuthMode::Unauthenticated => request,
        crate::stt::tail_provider::SttAuthMode::Bearer => request.bearer_auth(api_key),
        crate::stt::tail_provider::SttAuthMode::ApiKey => request.header("x-api-key", api_key),
    };
    let response = request.multipart(form).send();
    let mut result = response_result(account, endpoint, response);
    if auth_mode == crate::stt::tail_provider::SttAuthMode::Unauthenticated
        && result.status == ApiKeyLivenessStatus::Ok
    {
        result.message = "local STT endpoint accepts unauthenticated requests".to_string();
    }
    result
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

/// One-token Responses ping.
///
/// Endpoint and model resolution splits on who owns the lane config: the
/// generic lane accounts keep their per-account resolution (they may point at
/// a self-hosted server), while a vendor's own key is probed against that
/// vendor's endpoint and model.
fn probe_responses_key(
    client: &Client,
    lane: &RuntimeLlmLane,
    account: &str,
    api_key: &str,
) -> ApiKeyLivenessResult {
    let endpoint = lane.endpoint().to_string();
    let model = lane.model();
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

/// One-token Messages ping against the Anthropic wire family, with the
/// `x-api-key` + `anthropic-version` header pair that endpoint requires.
fn probe_anthropic_key(
    client: &Client,
    lane: &RuntimeLlmLane,
    account: &str,
    api_key: &str,
) -> ApiKeyLivenessResult {
    let endpoint = lane.endpoint().to_string();
    let model = lane.model();
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

/// Probe a GitHub token with an authenticated `GET /user`.
///
/// Not an LLM provider, so it bypasses the registry entirely. The endpoint is
/// overridable via `CODESCRIBE_GITHUB_PROBE_ENDPOINT` for tests.
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

/// Turn a probe's transport result into a verdict, tagging it with the endpoint
/// that was called.
///
/// This is the request boundary referenced by [`classify_probe_response`]: a
/// transport failure has no HTTP status to classify, so it is resolved to
/// [`ApiKeyLivenessStatus::Network`] here rather than there.
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

/// User-facing sentence for a status. Written for Settings, not for logs.
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

/// Read an env var, treating whitespace-only as unset.
fn env_non_empty(key: &str) -> Option<String> {
    std::env::var(key)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

/// Classification contract tests and a loopback Responses probe for endpoint truth.
#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::thread;
    use tempfile::TempDir;

    /// Live probe records the normalized `/v1/responses` URL after lane endpoint resolution.
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
        let result = probe_responses_key(
            &client,
            &Config::default(),
            ProviderKind::OpenAiResponses,
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

    /// The STT slot has a real multipart probe instead of the historical
    /// Unsupported verdict, and reports the endpoint that answered.
    #[test]
    fn stt_probe_uses_the_multipart_endpoint() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind STT probe server");
        let address = listener.local_addr().expect("STT probe address");
        let endpoint = format!("http://{address}/v1/audio/transcriptions");
        let expected_endpoint = endpoint.clone();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept STT probe request");
            let mut buffer = [0_u8; 8192];
            let bytes_read = stream.read(&mut buffer).expect("read STT probe request");
            stream
                .write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 11\r\nConnection: close\r\n\r\n{\"text\":\"\"}",
                )
                .expect("write STT probe response");
            String::from_utf8_lossy(&buffer[..bytes_read]).to_string()
        });
        let client = Client::builder()
            .timeout(PROBE_TIMEOUT)
            .connect_timeout(PROBE_TIMEOUT)
            .build()
            .expect("build STT probe client");
        let config = Config {
            stt_endpoint: Some(endpoint),
            ..Config::default()
        };
        let result = probe_stt_key(&client, &config, "STT_API_KEY", "test-key");
        assert_eq!(result.status, ApiKeyLivenessStatus::Ok);
        assert_eq!(
            result.probed_endpoint.as_deref(),
            Some(expected_endpoint.as_str())
        );
        let request = server.join().expect("STT probe server");
        assert!(request.starts_with("POST /v1/audio/transcriptions HTTP/1.1"));
        let request_lower = request.to_ascii_lowercase();
        assert!(!request_lower.contains("x-api-key:"));
        assert!(!request_lower.contains("authorization:"));
        assert!(request.contains("codescribe-key-probe.wav"));
        assert!(
            request.contains("name=\"vocabulary\""),
            "loopback Codescribe probe must name the programming domain"
        );
        assert!(request.contains("programming"));
        assert_eq!(
            result.message,
            "local STT endpoint accepts unauthenticated requests"
        );
    }

    /// A stored Voice Lab socket is remapped onto the file worker before POST.
    #[test]
    fn stt_probe_maps_live_websocket_to_multipart() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind STT live-socket probe server");
        let address = listener
            .local_addr()
            .expect("STT live-socket probe address");
        let live = format!("ws://127.0.0.1:{}/v1/audio/transcribe", address.port());
        let expected = format!(
            "http://127.0.0.1:{}/v1/audio/transcriptions",
            address.port()
        );
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept remapped STT probe");
            let mut buffer = [0_u8; 8192];
            let bytes_read = stream.read(&mut buffer).expect("read remapped STT probe");
            stream
                .write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 11\r\nConnection: close\r\n\r\n{\"text\":\"\"}",
                )
                .expect("write remapped STT probe response");
            String::from_utf8_lossy(&buffer[..bytes_read]).to_string()
        });
        let client = Client::builder()
            .timeout(PROBE_TIMEOUT)
            .connect_timeout(PROBE_TIMEOUT)
            .build()
            .expect("build remapped STT probe client");
        let config = Config {
            stt_endpoint: Some(live),
            ..Config::default()
        };
        let result = probe_stt_key(&client, &config, "STT_API_KEY", "test-key");
        assert_eq!(result.status, ApiKeyLivenessStatus::Ok);
        assert_eq!(result.probed_endpoint.as_deref(), Some(expected.as_str()));
        let request = server.join().expect("remapped STT probe server");
        assert!(request.starts_with("POST /v1/audio/transcriptions HTTP/1.1"));
    }

    /// 2xx means the provider accepted the key and returned a usable response.
    #[test]
    fn classifies_success_as_ok() {
        assert_eq!(
            classify_probe_response(StatusCode::OK, r#"{"id":"resp_123"}"#),
            ApiKeyLivenessStatus::Ok
        );
    }

    /// Auth failures are the only client errors that map to Invalid.
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

    /// Quota markers in the body override a non-auth 4xx into NoQuota.
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

    /// 429 alone is treated as exhausted quota even without a body string.
    #[test]
    fn classifies_429_without_body_as_no_quota() {
        assert_eq!(
            classify_probe_response(StatusCode::TOO_MANY_REQUESTS, ""),
            ApiKeyLivenessStatus::NoQuota
        );
    }

    /// Anthropic low-credit copy is a body-only NoQuota signal on 400.
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

    /// 402 and billing_error body both mean valid key without spendable credit.
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

    /// Request-level 400 (model missing) still proves the key authenticated.
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

    /// Other 4xx after auth are Ok — probe shape may be wrong, key is live.
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

    /// 5xx stays Network/unknown — key validity cannot be asserted.
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

    /// Restores a process env var on drop; tests using it must be `serial`.
    struct EnvGuard {
        key: &'static str,
        previous: Option<String>,
    }

    impl EnvGuard {
        /// Set `key` for the test lifetime and remember the prior value.
        fn set(key: &'static str, value: &str) -> Self {
            let previous = std::env::var(key).ok();
            // SAFETY: process-env tests in this module are serialized.
            unsafe { std::env::set_var(key, value) };
            Self { key, previous }
        }

        /// Unset `key` for the test lifetime and remember the prior value.
        fn remove(key: &'static str) -> Self {
            let previous = std::env::var(key).ok();
            // SAFETY: process-env tests in this module are serialized.
            unsafe { std::env::remove_var(key) };
            Self { key, previous }
        }
    }

    impl Drop for EnvGuard {
        /// Put the previous env value back (or remove the key if it was absent).
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
