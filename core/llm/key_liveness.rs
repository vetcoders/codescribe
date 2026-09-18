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

use crate::config::{Config, keychain};
use crate::llm::provider::{
    ALL_PROVIDERS, LlmMode, ProviderRef, ProviderRegistry, ResolvedProvider, WireFamily,
};
use crate::llm::vendors;

/// Wall-clock budget for connect and request; probes must stay cheap for Settings.
const PROBE_TIMEOUT: Duration = Duration::from_secs(10);

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
/// `probed_endpoint` records the URL actually called after provider
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
/// keys are answered without a request; STT lane accounts and `GITHUB_TOKEN` get
/// their own probes; every LLM account is probed against a provider — the
/// row passed in, or (for a vendor account) the vendor that owns the account
/// — by *wire family*, not by vendor, so a new provider on an existing
/// protocol gets a real verdict instead of "unsupported". A Custom account
/// needs its row: the account name alone carries no endpoint.
///
/// The secret is read at the explicit secret-use boundary
/// ([`keychain::runtime_key`]): Test is the one place a Keychain prompt is
/// appropriate, and a Custom key never lives in process env.
pub fn probe_api_key_liveness(
    account: &str,
    provider: Option<&ResolvedProvider>,
) -> ApiKeyLivenessResult {
    if !keychain::is_known_account(account) {
        return ApiKeyLivenessResult::new(
            account,
            ApiKeyLivenessStatus::Unsupported,
            "unknown Keychain account",
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
    if account == "STT_FILE_API_KEY" || account == "STT_LIVE_API_KEY" {
        let config = Config::load_without_keychain();
        let api_key = keychain::runtime_key(account);
        return if account == "STT_FILE_API_KEY" {
            probe_stt_file_key(&client, &config, account, api_key)
        } else {
            probe_stt_live_key(&config, account, api_key)
        };
    }
    let api_key = keychain::runtime_key(account);
    let Some(api_key) = api_key else {
        return ApiKeyLivenessResult::new(
            account,
            ApiKeyLivenessStatus::Missing,
            "key is not configured",
        );
    };
    if account == "GITHUB_TOKEN" {
        return probe_github_token(&client, account, &api_key);
    }

    let Some(provider) = provider
        .cloned()
        .or_else(|| vendor_provider_for_account(account))
    else {
        return ApiKeyLivenessResult::new(
            account,
            ApiKeyLivenessStatus::Unsupported,
            "no provider row uses this key account",
        );
    };
    // The vendor's seed model keeps the probe cheap and valid; a Custom row
    // has no seed, and an empty model is a request-level 4xx — still proof
    // the key authenticated.
    let model = provider
        .reference
        .vendor()
        .map(|vendor| vendor.default_model(LlmMode::Assistive))
        .unwrap_or_default();

    // Probe shape follows the protocol, not the vendor: xAI and Libraxis
    // answer the same Responses ping as OpenAI.
    match provider.wire {
        WireFamily::OpenAiResponses => {
            probe_responses_key(&client, &provider, model, account, &api_key)
        }
        WireFamily::AnthropicMessages => {
            probe_anthropic_key(&client, &provider, model, account, &api_key)
        }
    }
}

/// The vendor row whose pinned account is `account`, resolved through an
/// empty registry (no Custom rows are needed to probe a vendor key).
fn vendor_provider_for_account(account: &str) -> Option<ResolvedProvider> {
    let vendor = ALL_PROVIDERS
        .into_iter()
        .find(|vendor| vendor.api_key_account() == account)?;
    ProviderRegistry::default().resolve(&ProviderRef::Vendor(vendor))
}

/// Probe the configured multipart STT slot with 100 ms of synthetic silence.
/// The response body is never surfaced; only auth/quota/transport status is.
/// The File row owns the URL and credential; no runtime inversion.
fn probe_stt_file_key(
    client: &Client,
    config: &Config,
    account: &str,
    api_key: Option<String>,
) -> ApiKeyLivenessResult {
    let Some(row) = config.stt_lane(crate::stt::SttLane::File) else {
        return ApiKeyLivenessResult::new(
            account,
            ApiKeyLivenessStatus::Unsupported,
            "no file transcription endpoint configured",
        );
    };
    let endpoint = row.endpoint;
    let auth_mode = row.auth_mode;
    let Some(api_key) = api_key
        .or(row.api_key)
        .filter(|key| !key.trim().is_empty())
        .or_else(|| {
            (auth_mode == crate::stt::tail_provider::SttAuthMode::Unauthenticated).then(String::new)
        })
    else {
        return ApiKeyLivenessResult::new(
            account,
            ApiKeyLivenessStatus::Missing,
            "key is not configured",
        );
    };
    let api_key = api_key.as_str();
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

/// Probe only the WebSocket upgrade, then immediately close without audio.
fn probe_stt_live_key(
    config: &Config,
    account: &str,
    api_key: Option<String>,
) -> ApiKeyLivenessResult {
    use crate::stt::{SttLane, tail_provider::SttAuthMode, validate_stt_endpoint};
    use tokio_tungstenite::tungstenite::{Error, client::IntoClientRequest};
    let Some(mut row) = config.stt_lane(SttLane::Live) else {
        return ApiKeyLivenessResult::new(
            account,
            ApiKeyLivenessStatus::Unsupported,
            "no live transcription endpoint configured",
        );
    };
    row.api_key = api_key.or(row.api_key);
    let verdict = |status, message| {
        ApiKeyLivenessResult::new(account, status, message)
            .with_probed_endpoint(row.endpoint.clone())
    };
    if validate_stt_endpoint(SttLane::Live, &row.endpoint).is_err() {
        return verdict(
            ApiKeyLivenessStatus::Network,
            "configured live STT endpoint is invalid or insecure",
        );
    }
    if row.key_missing() {
        return verdict(ApiKeyLivenessStatus::Missing, "key is not configured");
    }
    let Ok(mut request) = row.endpoint.as_str().into_client_request() else {
        return verdict(
            ApiKeyLivenessStatus::Network,
            "could not build WebSocket handshake",
        );
    };
    let key = row.api_key.as_deref().unwrap_or_default();
    let header = match row.auth_mode {
        SttAuthMode::Unauthenticated => None,
        SttAuthMode::Bearer => Some(("authorization", format!("Bearer {key}"))),
        SttAuthMode::ApiKey => Some(("x-api-key", key.to_string())),
    };
    if let Some((name, value)) = header {
        let Ok(value) = value.parse() else {
            return verdict(
                ApiKeyLivenessStatus::Invalid,
                "credential is not a valid header value",
            );
        };
        request.headers_mut().insert(name, value);
    }
    let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    else {
        return verdict(
            ApiKeyLivenessStatus::Network,
            "could not create WebSocket probe runtime",
        );
    };
    let status = runtime.block_on(async {
        match tokio::time::timeout(PROBE_TIMEOUT, tokio_tungstenite::connect_async(request)).await {
            Ok(Ok((mut socket, _))) => {
                let _ = tokio::time::timeout(PROBE_TIMEOUT, socket.close(None)).await;
                ApiKeyLivenessStatus::Ok
            }
            Ok(Err(Error::Http(response))) => {
                let status = StatusCode::from_u16(response.status().as_u16())
                    .unwrap_or(StatusCode::BAD_GATEWAY);
                if status == StatusCode::UNAUTHORIZED || status == StatusCode::FORBIDDEN {
                    classify_probe_response(status, "")
                } else {
                    ApiKeyLivenessStatus::Network
                }
            }
            _ => ApiKeyLivenessStatus::Network,
        }
    });
    verdict(
        status,
        match status {
            ApiKeyLivenessStatus::Ok => "socket accepted the credential",
            ApiKeyLivenessStatus::Invalid => "socket rejected the credential",
            _ => "socket handshake could not be verified",
        },
    )
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

/// One-token Responses ping against the provider's resolved endpoint.
fn probe_responses_key(
    client: &Client,
    provider: &ResolvedProvider,
    model: &str,
    account: &str,
    api_key: &str,
) -> ApiKeyLivenessResult {
    let endpoint = provider.endpoint.clone();
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
/// `x-api-key` + `anthropic-version` header pair that endpoint requires
/// (`vendors::anthropic`, read from the docs).
fn probe_anthropic_key(
    client: &Client,
    provider: &ResolvedProvider,
    model: &str,
    account: &str,
    api_key: &str,
) -> ApiKeyLivenessResult {
    let endpoint = provider.endpoint.clone();
    let mut request = client
        .post(&endpoint)
        .header(vendors::anthropic::AUTH_HEADER, api_key)
        .header("Content-Type", "application/json");
    for (name, value) in vendors::anthropic::EXTRA_HEADERS {
        request = request.header(*name, *value);
    }
    let response = request
        .json(&vendors::anthropic::liveness_probe_body(model))
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
            let mut message = message_for_status(probe_status).to_string();
            if let Some(detail) = provider_error_detail(&body).filter(|_| !status.is_success()) {
                message.push_str(&format!(" ({detail})"));
            }
            ApiKeyLivenessResult::new(account, probe_status, message)
        }
        Err(error) => ApiKeyLivenessResult::new(
            account,
            ApiKeyLivenessStatus::Network,
            format!("network error: {error}"),
        ),
    };
    result.with_probed_endpoint(probed_endpoint)
}

/// The provider's own `error.code` / `error.type` and `error.message` from an
/// error body, as `code: message`. A gateway that answers
/// `503 {"error":{"code":"no_eligible_provider",…}}` is routing, not auth —
/// Test connection must say which (`vendors::libraxis`, live 2026-09-07).
fn provider_error_detail(body: &str) -> Option<String> {
    let error = serde_json::from_str::<serde_json::Value>(body).ok()?;
    let error = error.get("error")?;
    let code = error
        .get("code")
        .or_else(|| error.get("type"))
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|code| !code.is_empty());
    let message = error
        .get("message")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|message| !message.is_empty());
    match (code, message) {
        (Some(code), Some(message)) => Some(format!("{code}: {message}")),
        (Some(code), None) => Some(code.to_string()),
        (None, Some(message)) => Some(message.to_string()),
        (None, None) => None,
    }
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
    use crate::llm::provider::CustomProvider;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::thread;

    /// A Custom row at `base` on the Responses wire, resolved through the registry.
    fn custom_responses_provider(base: &str) -> ResolvedProvider {
        let row = CustomProvider::new("Probe Box", WireFamily::OpenAiResponses, base)
            .expect("valid custom row");
        let id = row.id.clone();
        ProviderRegistry::new(vec![row])
            .resolve(&ProviderRef::Custom(id))
            .expect("custom row resolves")
    }

    /// Live probe calls the provider's resolved `/v1/responses` URL and records it.
    #[test]
    fn responses_probe_reports_the_normalized_endpoint_it_called() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind probe server");
        let address = listener.local_addr().expect("probe server address");
        let base_endpoint = format!("http://{address}");
        let expected_endpoint = format!("{base_endpoint}/v1/responses");

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
        let provider = custom_responses_provider(&base_endpoint);
        let result = probe_responses_key(
            &client,
            &provider,
            "gpt-probe",
            "LLM_CUSTOM_PROBE_BOX_API_KEY",
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

    /// A vendor account resolves to its own row without any Custom rows.
    #[test]
    fn vendor_accounts_resolve_to_their_pinned_rows() {
        let libraxis = vendor_provider_for_account("LLM_LIBRAXIS_API_KEY").expect("vendor row");
        assert_eq!(libraxis.endpoint, vendors::libraxis::ENDPOINT);
        assert_eq!(libraxis.wire, WireFamily::OpenAiResponses);
        assert!(vendor_provider_for_account("LLM_CUSTOM_MY_BOX_API_KEY").is_none());
    }

    /// The gateway's routing error (`503 no_eligible_provider`, live 2026-09-07)
    /// is reported by code, not mistaken for a rejected key.
    #[test]
    fn gateway_error_code_is_reported_not_unauthorized() {
        let body = r#"{"error":{"code":"no_eligible_provider","message":"No healthy provider supports every requested capability."}}"#;
        assert_eq!(
            classify_probe_response(StatusCode::SERVICE_UNAVAILABLE, body),
            ApiKeyLivenessStatus::Network
        );
        assert_eq!(
            provider_error_detail(body).as_deref(),
            Some("no_eligible_provider: No healthy provider supports every requested capability.")
        );
        assert_eq!(
            provider_error_detail(r#"{"type":"error","error":{"type":"authentication_error","message":"invalid x-api-key"}}"#).as_deref(),
            Some("authentication_error: invalid x-api-key")
        );
        assert_eq!(provider_error_detail("upstream down"), None);
    }

    /// The STT slot has a real multipart probe instead of the historical
    /// Unsupported verdict, and reports the endpoint that answered.
    #[test]
    fn stt_file_probe_uses_the_file_lane_without_inversion() {
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
            stt_file_endpoint: Some(endpoint),
            stt_live_endpoint: Some("ws://127.0.0.1:1/wrong-lane".into()),
            ..Config::default()
        };
        let result = probe_stt_file_key(
            &client,
            &config,
            "STT_FILE_API_KEY",
            Some("test-key".to_string()),
        );
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

    #[test]
    fn stt_live_probe_classifies_handshake_status() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = format!(
            concat!("ws", "://{}/v1/audio/transcribe"),
            listener.local_addr().unwrap()
        );
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream.set_read_timeout(Some(PROBE_TIMEOUT)).unwrap();
            let mut buffer = [0_u8; 8192];
            let count = stream.read(&mut buffer).unwrap();
            stream
                .write_all(
                    b"HTTP/1.1 401 Unauthorized\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                )
                .unwrap();
            String::from_utf8_lossy(&buffer[..count]).to_string()
        });
        let config = Config {
            stt_live_endpoint: Some(endpoint.clone()),
            ..Default::default()
        };
        let result = probe_stt_live_key(&config, "STT_LIVE_API_KEY", Some("test-key".into()));
        assert_eq!(result.status, ApiKeyLivenessStatus::Invalid);
        assert_eq!(result.probed_endpoint, Some(endpoint));
        let request = server.join().unwrap().to_ascii_lowercase();
        assert!(request.starts_with("get /v1/audio/transcribe http/1.1"));
        assert!(request.contains("upgrade: websocket"));
        assert!(
            !request.contains("test-key"),
            "loopback must not send secrets"
        );
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
}
