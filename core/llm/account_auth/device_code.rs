// Portions derived from openai/codex (Apache-2.0).
// xAI RFC 8628 device grant path is aligned with OpenCode
// `packages/opencode/src/plugin/xai.ts` (XaiAuthPlugin) — read-only reference.

//! Device-code login for headless / second-screen sign-in.
//!
//! Two provider shapes live behind one entry point, and they are not
//! interchangeable:
//!
//! - **OpenAI Codex** — JSON `/api/accounts/deviceauth/{usercode,token}`. The
//!   poll returns an authorization code plus its PKCE pair, which is then
//!   exchanged for tokens through the shared server module.
//! - **xAI** — standard RFC 8628: form-encoded `/oauth2/device/code` and
//!   `/oauth2/token`, polled with `authorization_pending` / `slow_down`
//!   backoff. Tokens come straight out of the poll, with no PKCE exchange.
//!
//! Callers go through [`request_device_code`] then
//! [`complete_device_code_login`]; provider dispatch stays internal so no call
//! site has to know which shape it is talking to.

use std::time::{Duration, Instant};

use reqwest::StatusCode;
use serde::{Deserialize, Deserializer, Serialize, de};

use crate::llm::account_auth::pkce::PkceCodes;
use crate::llm::account_auth::server::exchange_code_for_tokens;
use crate::llm::account_auth::{
    AccountAuthError, AccountTokens, issuer_for, provider_oauth_config, store_account_tokens,
};
use crate::llm::provider::ProviderKind;

/// RFC 8628 device-code grant type (OpenCode xai.ts `DEVICE_CODE_GRANT_TYPE`).
const XAI_DEVICE_CODE_GRANT_TYPE: &str = "urn:ietf:params:oauth:grant-type:device_code";
/// OpenCode floor + slow_down increment for xAI device polling.
const XAI_DEVICE_MIN_INTERVAL_MS: u64 = 1_000;
/// Added to the poll interval each time the server answers `slow_down`.
const XAI_DEVICE_SLOW_DOWN_INCREMENT_MS: u64 = 5_000;
/// Poll interval used when the device-code response omits one.
const XAI_DEVICE_DEFAULT_INTERVAL_MS: u64 = 5_000;

/// One device-code login attempt. `provider` decides both the issuer default
/// and the account the resulting tokens are filed under.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceAuthConfig {
    /// Selects the wire shape and the account slot tokens are stored under.
    pub provider: ProviderKind,
    /// OAuth issuer base URL, env-overridable per provider.
    pub issuer: String,
    /// Public OAuth client id for this provider.
    pub client_id: String,
    /// Total budget for the poll loop before the attempt is abandoned.
    pub max_wait: Duration,
}

impl DeviceAuthConfig {
    /// Registry-derived issuer for `provider`, env override honoured.
    pub fn new(provider: ProviderKind, client_id: String) -> Self {
        Self {
            provider,
            issuer: issuer_for(provider),
            client_id,
            max_wait: Duration::from_secs(15 * 60),
        }
    }
}

/// Provider-agnostic handle for an in-flight device login: what to show the
/// user, and what to poll with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceCode {
    /// URL the user opens to approve the login.
    pub verification_url: String,
    /// Short code the user confirms on that page.
    pub user_code: String,
    /// Opaque poll handle. Codex sends its device-auth id here; xAI sends the
    /// RFC 8628 `device_code`.
    pub device_auth_id: String,
    /// Server-suggested poll interval in seconds.
    pub interval: u64,
}

/// Codex `/deviceauth/usercode` response body.
#[derive(Debug, Deserialize)]
struct UserCodeResp {
    device_auth_id: String,
    #[serde(alias = "user_code", alias = "usercode")]
    user_code: String,
    #[serde(default, deserialize_with = "deserialize_interval")]
    interval: u64,
}

/// Codex `/deviceauth/usercode` request body.
#[derive(Debug, Serialize)]
struct UserCodeReq {
    client_id: String,
}

/// Codex `/deviceauth/token` poll request body.
#[derive(Debug, Serialize)]
struct TokenPollReq {
    device_auth_id: String,
    user_code: String,
}

/// Codex poll success: an authorization code plus the PKCE pair needed to
/// redeem it.
#[derive(Debug, Deserialize)]
struct CodeSuccessResp {
    authorization_code: String,
    code_challenge: String,
    code_verifier: String,
}

/// Accept a poll interval sent either as a JSON number or as a quoted string.
///
/// Codex has shipped both spellings; refusing one would break login on a
/// response that is otherwise valid.
fn deserialize_interval<'de, D>(deserializer: D) -> Result<u64, D::Error>
where
    D: Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum StringOrU64 {
        String(String),
        U64(u64),
    }

    match StringOrU64::deserialize(deserializer)? {
        StringOrU64::String(value) => value
            .trim()
            .parse::<u64>()
            .map_err(|error| de::Error::custom(format!("invalid u64 string: {error}"))),
        StringOrU64::U64(value) => Ok(value),
    }
}

/// Begin a device login and return the code to show the user.
///
/// Dispatches on `config.provider` to the matching wire shape.
pub async fn request_device_code(
    config: &DeviceAuthConfig,
) -> Result<DeviceCode, AccountAuthError> {
    let client = reqwest::Client::new();
    request_device_code_with_client(&client, config).await
}

/// Provider dispatch for the device-code request, over a caller-owned client
/// so tests can point it at a mock server.
async fn request_device_code_with_client(
    client: &reqwest::Client,
    config: &DeviceAuthConfig,
) -> Result<DeviceCode, AccountAuthError> {
    match config.provider {
        ProviderKind::XaiResponses => request_xai_device_code(client, config).await,
        _ => request_openai_codex_device_code(client, config).await,
    }
}

/// OpenAI Codex desktop device-auth (JSON `/api/accounts/deviceauth/usercode`).
async fn request_openai_codex_device_code(
    client: &reqwest::Client,
    config: &DeviceAuthConfig,
) -> Result<DeviceCode, AccountAuthError> {
    let base_url = api_accounts_base(&config.issuer);
    let response = client
        .post(format!("{base_url}/deviceauth/usercode"))
        .json(&UserCodeReq {
            client_id: config.client_id.clone(),
        })
        .send()
        .await
        .map_err(|error| AccountAuthError::Http(error.to_string()))?;

    if !response.status().is_success() {
        return Err(AccountAuthError::OAuth(format!(
            "device code request failed with status {}",
            response.status()
        )));
    }

    let body: UserCodeResp = response
        .json()
        .await
        .map_err(|error| AccountAuthError::OAuth(error.to_string()))?;
    Ok(DeviceCode {
        verification_url: format!("{}/codex/device", config.issuer.trim_end_matches('/')),
        user_code: body.user_code,
        device_auth_id: body.device_auth_id,
        interval: body.interval,
    })
}

/// xAI RFC 8628 device authorization — OpenCode `requestDeviceCode` shape.
///
/// Source: `opencode/.../plugin/xai.ts` (`DEVICE_AUTHORIZATION_URL`, form body
/// `client_id` + `scope`). Not the Codex JSON usercode path.
async fn request_xai_device_code(
    client: &reqwest::Client,
    config: &DeviceAuthConfig,
) -> Result<DeviceCode, AccountAuthError> {
    let oauth = provider_oauth_config(config.provider)?;
    let url = format!("{}/oauth2/device/code", config.issuer.trim_end_matches('/'));
    let response = client
        .post(&url)
        .header("Content-Type", "application/x-www-form-urlencoded")
        .header("Accept", "application/json")
        .body(urlencoding_form(&[
            ("client_id", config.client_id.as_str()),
            ("scope", oauth.scope),
        ]))
        .send()
        .await
        .map_err(|error| AccountAuthError::Http(error.to_string()))?;

    if !response.status().is_success() {
        let detail = response.text().await.unwrap_or_default();
        return Err(AccountAuthError::OAuth(format!(
            "xAI device code request failed{detail_suffix}",
            detail_suffix = if detail.is_empty() {
                String::new()
            } else {
                format!(": {detail}")
            }
        )));
    }

    let body: XaiDeviceCodeResp = response
        .json()
        .await
        .map_err(|error| AccountAuthError::OAuth(error.to_string()))?;
    if body.device_code.is_empty() || body.user_code.is_empty() || body.verification_uri.is_empty()
    {
        return Err(AccountAuthError::OAuth(
            "xAI device code response is missing device_code / user_code / verification_uri"
                .to_string(),
        ));
    }
    Ok(DeviceCode {
        verification_url: body
            .verification_uri_complete
            .filter(|s| !s.is_empty())
            .unwrap_or(body.verification_uri),
        user_code: body.user_code,
        // For xAI this field carries RFC 8628 `device_code` (opaque poll handle).
        device_auth_id: body.device_code,
        interval: body.interval.unwrap_or(5),
    })
}

/// xAI `/oauth2/device/code` response (RFC 8628 device authorization).
#[derive(Debug, Deserialize)]
struct XaiDeviceCodeResp {
    device_code: String,
    user_code: String,
    verification_uri: String,
    #[serde(default)]
    verification_uri_complete: Option<String>,
    #[serde(default)]
    #[allow(dead_code)]
    // OpenCode reads this for poll deadline; we use DeviceAuthConfig.max_wait.
    expires_in: Option<u64>,
    #[serde(default)]
    interval: Option<u64>,
}

/// Build an `application/x-www-form-urlencoded` body from key/value pairs.
fn urlencoding_form(pairs: &[(&str, &str)]) -> String {
    pairs
        .iter()
        .map(|(k, v)| {
            format!(
                "{}={}",
                form_urlencoded_escape(k),
                form_urlencoded_escape(v)
            )
        })
        .collect::<Vec<_>>()
        .join("&")
}

/// Percent-encode one form value, keeping only the RFC 3986 unreserved set.
///
/// Spaces become `%20` rather than `+`: the device-code grant type is a URN
/// full of colons, and mixing the two space conventions is how these bodies
/// silently fail to match server-side.
fn form_urlencoded_escape(value: &str) -> String {
    // Minimal application/x-www-form-urlencoded encoder (spaces as %20, not +).
    let mut out = String::with_capacity(value.len());
    for b in value.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// Block until the user approves the login, then store the resulting tokens.
///
/// xAI polls straight to tokens; Codex polls for an authorization code and
/// redeems it with PKCE. Either way the tokens are filed under the provider's
/// account slot before returning.
pub async fn complete_device_code_login(
    config: &DeviceAuthConfig,
    device_code: &DeviceCode,
) -> Result<(), AccountAuthError> {
    let client = reqwest::Client::new();
    match config.provider {
        ProviderKind::XaiResponses => {
            let tokens = poll_xai_device_token(&client, config, device_code).await?;
            store_account_tokens(config.provider, &tokens)
        }
        _ => {
            let code_resp = poll_for_authorization_code(
                &client,
                &api_accounts_base(&config.issuer),
                &device_code.device_auth_id,
                &device_code.user_code,
                device_code.interval,
                config.max_wait,
            )
            .await?;
            let pkce = PkceCodes {
                code_verifier: code_resp.code_verifier,
                code_challenge: code_resp.code_challenge,
            };
            let redirect_uri = format!(
                "{}/deviceauth/callback",
                config.issuer.trim_end_matches('/')
            );
            let tokens = exchange_code_for_tokens(
                config.provider,
                &config.issuer,
                &config.client_id,
                &redirect_uri,
                &pkce,
                &code_resp.authorization_code,
            )
            .await?;
            store_account_tokens(config.provider, &tokens)
        }
    }
}

/// OpenCode `pollDeviceCodeToken` — form POST to token endpoint with
/// `grant_type=urn:ietf:params:oauth:grant-type:device_code`.
async fn poll_xai_device_token(
    client: &reqwest::Client,
    config: &DeviceAuthConfig,
    device: &DeviceCode,
) -> Result<AccountTokens, AccountAuthError> {
    let token_url = format!("{}/oauth2/token", config.issuer.trim_end_matches('/'));
    let deadline = Instant::now() + config.max_wait;
    let mut interval_ms = (device.interval.saturating_mul(1000))
        .max(XAI_DEVICE_DEFAULT_INTERVAL_MS)
        .max(XAI_DEVICE_MIN_INTERVAL_MS);

    loop {
        if Instant::now() >= deadline {
            return Err(AccountAuthError::OAuth(
                "xAI device authorization timed out".to_string(),
            ));
        }

        let response = client
            .post(&token_url)
            .header("Content-Type", "application/x-www-form-urlencoded")
            .header("Accept", "application/json")
            .body(urlencoding_form(&[
                ("grant_type", XAI_DEVICE_CODE_GRANT_TYPE),
                ("client_id", config.client_id.as_str()),
                ("device_code", device.device_auth_id.as_str()),
            ]))
            .send()
            .await
            .map_err(|error| AccountAuthError::Http(error.to_string()))?;

        if response.status().is_success() {
            let body: XaiTokenResp = response
                .json()
                .await
                .map_err(|error| AccountAuthError::OAuth(error.to_string()))?;
            if body.access_token.is_empty() {
                return Err(AccountAuthError::OAuth(
                    "xAI device token response missing access_token".to_string(),
                ));
            }
            return Ok(AccountTokens::new(
                config.provider,
                body.access_token,
                body.refresh_token,
                body.id_token,
                body.token_type,
                body.expires_in,
            ));
        }

        let err_body: XaiDeviceTokenError = response.json().await.unwrap_or_default();
        let remaining = deadline.saturating_duration_since(Instant::now());
        match err_body.error.as_deref() {
            Some("authorization_pending") => {
                tokio::time::sleep(Duration::from_millis(interval_ms).min(remaining)).await;
            }
            Some("slow_down") => {
                interval_ms = interval_ms.saturating_add(XAI_DEVICE_SLOW_DOWN_INCREMENT_MS);
                tokio::time::sleep(Duration::from_millis(interval_ms).min(remaining)).await;
            }
            Some("access_denied") | Some("authorization_denied") => {
                return Err(AccountAuthError::OAuth(
                    "xAI device authorization was denied".to_string(),
                ));
            }
            Some("expired_token") => {
                return Err(AccountAuthError::OAuth(
                    "xAI device code expired - please re-run login".to_string(),
                ));
            }
            other => {
                let detail = err_body
                    .error_description
                    .or_else(|| other.map(str::to_string))
                    .unwrap_or_default();
                return Err(AccountAuthError::OAuth(format!(
                    "xAI device token exchange failed{}",
                    if detail.is_empty() {
                        String::new()
                    } else {
                        format!(": {detail}")
                    }
                )));
            }
        }
    }
}

/// xAI `/oauth2/token` success body.
#[derive(Debug, Deserialize)]
struct XaiTokenResp {
    access_token: String,
    #[serde(default)]
    refresh_token: Option<String>,
    #[serde(default)]
    id_token: Option<String>,
    #[serde(default)]
    token_type: Option<String>,
    #[serde(default)]
    expires_in: Option<u64>,
}

/// xAI `/oauth2/token` error body. Defaults to empty so an unparseable error
/// response degrades to "unknown error" instead of failing the poll loop.
#[derive(Debug, Default, Deserialize)]
struct XaiDeviceTokenError {
    #[serde(default)]
    error: Option<String>,
    #[serde(default)]
    error_description: Option<String>,
}

/// Poll the Codex device-auth token endpoint until the user approves.
///
/// `403`/`404` mean "not approved yet" and are retried within `max_wait`; any
/// other non-success status is terminal. Each sleep is clamped to the time
/// actually left, so the loop cannot overshoot its budget.
async fn poll_for_authorization_code(
    client: &reqwest::Client,
    api_base_url: &str,
    device_auth_id: &str,
    user_code: &str,
    interval: u64,
    max_wait: Duration,
) -> Result<CodeSuccessResp, AccountAuthError> {
    let url = format!("{api_base_url}/deviceauth/token");
    let started = Instant::now();

    loop {
        let response = client
            .post(&url)
            .json(&TokenPollReq {
                device_auth_id: device_auth_id.to_string(),
                user_code: user_code.to_string(),
            })
            .send()
            .await
            .map_err(|error| AccountAuthError::Http(error.to_string()))?;
        let status = response.status();

        if status.is_success() {
            return response
                .json()
                .await
                .map_err(|error| AccountAuthError::OAuth(error.to_string()));
        }

        if status == StatusCode::FORBIDDEN || status == StatusCode::NOT_FOUND {
            if started.elapsed() >= max_wait {
                return Err(AccountAuthError::OAuth(
                    "device auth timed out after 15 minutes".to_string(),
                ));
            }
            let remaining = max_wait.saturating_sub(started.elapsed());
            tokio::time::sleep(Duration::from_secs(interval).min(remaining)).await;
            continue;
        }

        return Err(AccountAuthError::OAuth(format!(
            "device auth failed with status {status}"
        )));
    }
}

/// Codex accounts API base for an issuer, tolerating a trailing slash.
fn api_accounts_base(issuer: &str) -> String {
    format!("{}/api/accounts", issuer.trim_end_matches('/'))
}

#[cfg(test)]
mod tests {
    use super::*;
    use mockito::Matcher;

    #[tokio::test]
    async fn device_code_polling_waits_through_pending_then_returns_code() {
        let mut server = mockito::Server::new_async().await;
        let _pending = server
            .mock("POST", "/api/accounts/deviceauth/token")
            .match_body(Matcher::JsonString(
                r#"{"device_auth_id":"dev-1","user_code":"USER-1"}"#.to_string(),
            ))
            .with_status(403)
            .expect(1)
            .create_async()
            .await;
        let _ok = server
            .mock("POST", "/api/accounts/deviceauth/token")
            .match_body(Matcher::JsonString(
                r#"{"device_auth_id":"dev-1","user_code":"USER-1"}"#.to_string(),
            ))
            .with_status(200)
            .with_body(
                r#"{"authorization_code":"auth-code","code_challenge":"challenge","code_verifier":"verifier"}"#,
            )
            .expect(1)
            .create_async()
            .await;

        let client = reqwest::Client::new();
        let response = poll_for_authorization_code(
            &client,
            &format!("{}/api/accounts", server.url()),
            "dev-1",
            "USER-1",
            0,
            Duration::from_secs(2),
        )
        .await
        .unwrap();

        assert_eq!(response.authorization_code, "auth-code");
        assert_eq!(response.code_verifier, "verifier");
    }

    #[tokio::test]
    async fn request_device_code_maps_usercode_response() {
        let mut server = mockito::Server::new_async().await;
        let _mock = server
            .mock("POST", "/api/accounts/deviceauth/usercode")
            .with_status(200)
            .with_body(r#"{"device_auth_id":"dev-1","user_code":"USER-1","interval":"0"}"#)
            .expect(1)
            .create_async()
            .await;
        let config = DeviceAuthConfig {
            provider: ProviderKind::OpenAiResponses,
            issuer: server.url(),
            client_id: "client".to_string(),
            max_wait: Duration::from_secs(1),
        };

        let code = request_device_code(&config).await.unwrap();

        assert_eq!(code.device_auth_id, "dev-1");
        assert_eq!(code.user_code, "USER-1");
        assert_eq!(code.interval, 0);
        assert!(code.verification_url.ends_with("/codex/device"));
    }

    /// OpenCode xai.ts `requestDeviceCode` — form POST `/oauth2/device/code`.
    #[tokio::test]
    async fn xai_device_code_uses_rfc8628_form_endpoint_not_codex_json() {
        let mut server = mockito::Server::new_async().await;
        let _mock = server
            .mock("POST", "/oauth2/device/code")
            .match_header("content-type", "application/x-www-form-urlencoded")
            .match_body(Matcher::Regex(
                r"client_id=b1a00492.*scope=openid".to_string(),
            ))
            .with_status(200)
            .with_body(
                r#"{"device_code":"dc-1","user_code":"ABCD-EFGH","verification_uri":"https://auth.x.ai/device","interval":5}"#,
            )
            .expect(1)
            .create_async()
            .await;
        let config = DeviceAuthConfig {
            provider: ProviderKind::XaiResponses,
            issuer: server.url(),
            client_id: "b1a00492-073a-47ea-816f-4c329264a828".to_string(),
            max_wait: Duration::from_secs(1),
        };

        let code = request_device_code(&config).await.unwrap();

        assert_eq!(code.device_auth_id, "dc-1");
        assert_eq!(code.user_code, "ABCD-EFGH");
        assert_eq!(code.verification_url, "https://auth.x.ai/device");
        assert_eq!(code.interval, 5);
    }

    #[tokio::test]
    async fn xai_device_poll_pending_then_token() {
        let mut server = mockito::Server::new_async().await;
        let _pending = server
            .mock("POST", "/oauth2/token")
            .match_body(Matcher::Regex(
                r"grant_type=urn%3Aietf%3Aparams%3Aoauth%3Agrant-type%3Adevice_code".to_string(),
            ))
            .with_status(400)
            .with_body(r#"{"error":"authorization_pending"}"#)
            .expect(1)
            .create_async()
            .await;
        let _ok = server
            .mock("POST", "/oauth2/token")
            .with_status(200)
            .with_body(
                r#"{"access_token":"at-1","refresh_token":"rt-1","token_type":"Bearer","expires_in":3600}"#,
            )
            .expect(1)
            .create_async()
            .await;

        let client = reqwest::Client::new();
        let config = DeviceAuthConfig {
            provider: ProviderKind::XaiResponses,
            issuer: server.url(),
            client_id: "client".to_string(),
            // Floor interval is 1s (OpenCode min); pending+token needs room to poll twice.
            max_wait: Duration::from_secs(20),
        };
        let device = DeviceCode {
            verification_url: "https://auth.x.ai/device".into(),
            user_code: "U".into(),
            device_auth_id: "dc-1".into(),
            // 1 second → after first pending we sleep ~1s then hit the ok mock.
            interval: 1,
        };
        let tokens = poll_xai_device_token(&client, &config, &device)
            .await
            .unwrap();
        assert_eq!(tokens.access_token, "at-1");
        assert_eq!(tokens.refresh_token.as_deref(), Some("rt-1"));
        assert_eq!(tokens.provider, "xai-responses");
    }
}
