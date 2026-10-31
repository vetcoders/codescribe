//! Google credential acquisition. API keys and account tokens stay in the
//! Codescribe Keychain bundle; ADC files remain owned by gcloud and are never
//! rewritten. The refresh transaction uses the account owner's serialization
//! and mutation epoch, and never follows an OAuth redirect.

use std::fmt;
use std::io::Read;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

use serde::Deserialize;
use serde_json::Value;

use super::{AccountAuthError, REFRESH_SKEW, credential_io, now_unix, provider_auth_state};
use crate::config::keychain;
use crate::llm::provider::ProviderKind;

/// Google API-key account in the existing Keychain bundle.
pub const GOOGLE_API_KEY_ACCOUNT: &str = "LLM_GOOGLE_API_KEY";
/// Serialized Google OAuth token record. Never mirrored into process env.
pub const GOOGLE_ACCOUNT_TOKENS_ACCOUNT: &str = "LLM_GOOGLE_ACCOUNT_TOKENS";
const TOKEN_ENDPOINT: &str = "https://oauth2.googleapis.com/token";
const MAX_CREDENTIAL_BYTES: u64 = 1024 * 1024;

/// A resolved credential, with its wire authentication mechanism intact.
#[derive(Clone)]
pub enum GoogleCredential {
    /// Sent in `x-goog-api-key`, never in the URL.
    ApiKey(String),
    /// Sent in `Authorization: Bearer …`.
    Bearer(String),
}

impl fmt::Debug for GoogleCredential {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::ApiKey(_) => "GoogleCredential::ApiKey([REDACTED])",
            Self::Bearer(_) => "GoogleCredential::Bearer([REDACTED])",
        })
    }
}

fn non_empty_env(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

/// Silent key acquisition for runtime-snapshot capture. No Keychain prompt or
/// network request is permitted here. Full precedence is resolved at send time.
pub fn google_api_key() -> Option<String> {
    google_api_key_for_account(GOOGLE_API_KEY_ACCOUNT)
}

/// Silent key eligibility for the selected public Google account. This shares
/// the sender's dedicated-account/global/env rails without reading ADC or
/// prompting Keychain from a UI availability getter.
pub fn google_api_key_available(account: &str) -> bool {
    google_api_key_for_account(account).is_some()
}

fn google_api_key_for_account(account: &str) -> Option<String> {
    stored_api_key(account, keychain::cached_runtime_key)
        .or_else(|| non_empty_env("GEMINI_API_KEY"))
        .or_else(|| non_empty_env("GOOGLE_API_KEY"))
}

fn stored_api_key(account: &str, lookup: impl Fn(&str) -> Option<String>) -> Option<String> {
    lookup(account).or_else(|| {
        if account == GOOGLE_API_KEY_ACCOUNT {
            None
        } else {
            lookup(GOOGLE_API_KEY_ACCOUNT)
        }
    })
}

/// Availability hint captured only when a Google lane is selected. ADC file
/// contents and tokens are acquired later on the blocking pool, at secret use.
pub fn google_credentials_available() -> bool {
    google_api_key().is_some() || google_credential_status().is_ok_and(|status| status.available)
}

/// Secret-free account projection for snapshot capture and the existing
/// provider/account bridge. It neither refreshes tokens nor starts a login.
pub struct GoogleCredentialStatus {
    /// An account record or ADC file is present; acceptance is checked at send.
    pub available: bool,
    /// Only Codescribe's stored account can be removed by its account button.
    pub stored_account: bool,
    /// Honest provisioning guidance, with no token or filesystem contents.
    pub message: String,
}

/// Inspect cached Keychain tokens or ADC file presence, never its secret body.
/// A malformed higher-precedence record never masquerades as a signed-out user.
pub fn google_credential_status() -> Result<GoogleCredentialStatus, AccountAuthError> {
    if let Some(raw) = keychain::cached_runtime_key(GOOGLE_ACCOUNT_TOKENS_ACCOUNT) {
        parse_record(&raw)?;
        return Ok(GoogleCredentialStatus {
            available: true,
            stored_account: true,
            message: "Google account credentials are stored locally and refreshed when needed. Browser sign-in is not available in Codescribe.".into(),
        });
    }
    if adc_path().is_some_and(|path| path.is_file()) {
        return Ok(GoogleCredentialStatus {
            available: true,
            stored_account: false,
            message: "An ADC file is configured; credentials are validated when sending. Only authorized-user ADC is supported. Manage it with gcloud; Codescribe never changes the file. Service-account and external-account credentials are not supported.".into(),
        });
    }
    if std::env::var_os("GOOGLE_APPLICATION_CREDENTIALS").is_some_and(|path| !path.is_empty()) {
        return Err(AccountAuthError::Storage(
            "The configured Google ADC file is unavailable".into(),
        ));
    }
    Ok(GoogleCredentialStatus {
        available: false,
        stored_account: false,
        message: "Use an API key or provision authorized-user ADC with gcloud auth application-default login. Browser sign-in, service-account and external-account credentials are not supported in Codescribe.".into(),
    })
}

/// Remove only Codescribe's account. External ADC remains managed by gcloud.
/// Invalidate both pending refreshes and the ADC refresh cache, including when
/// storage refuses deletion; an old response must never recreate this record.
pub fn clear_google_account_tokens() -> Result<(), AccountAuthError> {
    provider_auth_state(ProviderKind::GoogleGemini).replace_with(|| {
        *refreshed_cache()
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = None;
        keychain::delete_key(GOOGLE_ACCOUNT_TOKENS_ACCOUNT)
            .map_err(|_| AccountAuthError::Storage("Google account could not be removed".into()))?;
        // SAFETY: same account mutation boundary as the other provider owners.
        unsafe { std::env::remove_var(GOOGLE_ACCOUNT_TOKENS_ACCOUNT) };
        keychain::advance_bundle_revision();
        Ok(())
    })
}

fn adc_path() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("GOOGLE_APPLICATION_CREDENTIALS") {
        if !path.is_empty() {
            return Some(PathBuf::from(path));
        }
    }
    directories::BaseDirs::new().map(|dirs| {
        dirs.home_dir()
            .join(".config/gcloud/application_default_credentials.json")
    })
}

#[derive(Clone, PartialEq, Eq)]
enum TokenSource {
    Keychain,
    Adc(PathBuf),
}

struct RecordSnapshot {
    source: TokenSource,
    raw: String,
    record: Value,
}

struct CachedToken {
    source: TokenSource,
    raw: String,
    access_token: String,
    expires_at_unix: i64,
}

fn refreshed_cache() -> &'static Mutex<Option<CachedToken>> {
    static CACHE: OnceLock<Mutex<Option<CachedToken>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(None))
}

fn parse_record(raw: &str) -> Result<Value, AccountAuthError> {
    if raw.len() as u64 > MAX_CREDENTIAL_BYTES {
        return Err(AccountAuthError::Storage(
            "Google credential record exceeds the size limit".into(),
        ));
    }
    let record: Value = serde_json::from_str(raw).map_err(|_| {
        AccountAuthError::Storage("Google credential record is not valid JSON".into())
    })?;
    if !record.is_object() {
        return Err(AccountAuthError::Storage(
            "Google credential record must be an object".into(),
        ));
    }
    match record.get("type").and_then(Value::as_str) {
        None | Some("authorized_user") => {}
        Some(_) => {
            return Err(AccountAuthError::OAuth(
                "Google ADC credential type is unsupported; provision authorized_user credentials with gcloud auth application-default login".into(),
            ));
        }
    }
    if let Some(endpoint) = record.get("token_uri").and_then(Value::as_str) {
        validate_token_endpoint(endpoint)?;
    }
    let access_present = string_field(&record, "access_token").is_some();
    let refresh_present = string_field(&record, "refresh_token").is_some();
    if !access_present && !refresh_present {
        return Err(AccountAuthError::Storage(
            "Google credential record has no access or refresh token".into(),
        ));
    }
    Ok(record)
}

fn string_field<'a>(record: &'a Value, key: &str) -> Option<&'a str> {
    record
        .get(key)?
        .as_str()
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

fn validate_token_endpoint(endpoint: &str) -> Result<(), AccountAuthError> {
    if endpoint == TOKEN_ENDPOINT {
        return Ok(());
    }
    #[cfg(test)]
    if reqwest::Url::parse(endpoint).ok().is_some_and(|url| {
        url.scheme() == "http"
            && url.host_str().is_some_and(|host| {
                host.trim_matches(['[', ']'])
                    .parse::<std::net::IpAddr>()
                    .is_ok_and(|ip| ip.is_loopback())
            })
            && url.username().is_empty()
            && url.password().is_none()
            && url.query().is_none()
            && url.fragment().is_none()
    }) {
        return Ok(());
    }
    Err(AccountAuthError::OAuth(
        "Google token endpoint must be the official HTTPS OAuth endpoint".into(),
    ))
}

fn read_snapshot(source: TokenSource) -> Result<RecordSnapshot, AccountAuthError> {
    let raw = match &source {
        TokenSource::Keychain => keychain::runtime_key(GOOGLE_ACCOUNT_TOKENS_ACCOUNT)
            .ok_or_else(|| AccountAuthError::NotSignedIn("google-gemini".into()))?,
        TokenSource::Adc(path) => {
            let file = std::fs::File::open(path).map_err(|_| {
                AccountAuthError::Storage("Google ADC file could not be opened".into())
            })?;
            let mut raw = String::new();
            file.take(MAX_CREDENTIAL_BYTES + 1)
                .read_to_string(&mut raw)
                .map_err(|_| {
                    AccountAuthError::Storage("Google ADC file could not be read as UTF-8".into())
                })?;
            if raw.len() as u64 > MAX_CREDENTIAL_BYTES {
                return Err(AccountAuthError::Storage(
                    "Google ADC file exceeds the credential size limit".into(),
                ));
            }
            raw
        }
    };
    if raw.len() as u64 > MAX_CREDENTIAL_BYTES {
        return Err(AccountAuthError::Storage(
            "Google credential record exceeds the size limit".into(),
        ));
    }
    let record = parse_record(&raw)?;
    Ok(RecordSnapshot {
        source,
        raw,
        record,
    })
}

fn record_access_token(record: &Value) -> Option<String> {
    let token = string_field(record, "access_token")?;
    let expires_at = record
        .get("expires_at_unix")
        .and_then(Value::as_i64)
        .or_else(|| {
            record
                .get("expiry_date")
                .and_then(Value::as_i64)
                .map(|millis| millis / 1000)
        });
    if expires_at
        .is_some_and(|expiry| expiry <= now_unix().saturating_add(REFRESH_SKEW.as_secs() as i64))
    {
        return None;
    }
    Some(token.to_string())
}

/// Acquire at the explicit secret-use boundary. Keychain key, Keychain OAuth,
/// vendor environment keys, then ADC. A configured but invalid token record or
/// ADC file produces an error rather than silently selecting another account.
pub async fn resolve_google_credential(
    account: &str,
) -> Result<GoogleCredential, AccountAuthError> {
    let account = account.to_string();
    let keychain_credential = credential_io(move || {
        Ok(stored_api_key(&account, keychain::runtime_key).map(GoogleCredential::ApiKey))
    })
    .await?;
    if let Some(credential) = keychain_credential {
        return Ok(credential);
    }
    let source = credential_io(|| {
        if keychain::runtime_key(GOOGLE_ACCOUNT_TOKENS_ACCOUNT).is_some() {
            return Ok(Some(TokenSource::Keychain));
        }
        Ok(None)
    })
    .await?;
    if let Some(source) = source {
        return resolve_token_source(source)
            .await
            .map(GoogleCredential::Bearer);
    }
    if let Some(key) = non_empty_env("GEMINI_API_KEY").or_else(|| non_empty_env("GOOGLE_API_KEY")) {
        return Ok(GoogleCredential::ApiKey(key));
    }
    let source = adc_path().ok_or_else(|| AccountAuthError::NotSignedIn("google-gemini".into()))?;
    resolve_token_source(TokenSource::Adc(source))
        .await
        .map(GoogleCredential::Bearer)
}

/// CCA accepts the account rail, not a public Gemini API key. Keep account
/// selection and refresh in this credential owner rather than in providers.
pub async fn resolve_google_account_credential() -> Result<GoogleCredential, AccountAuthError> {
    let keychain_present =
        credential_io(|| Ok(keychain::runtime_key(GOOGLE_ACCOUNT_TOKENS_ACCOUNT).is_some()))
            .await?;
    let source = if keychain_present {
        TokenSource::Keychain
    } else {
        TokenSource::Adc(
            adc_path().ok_or_else(|| AccountAuthError::NotSignedIn("google-gemini".into()))?,
        )
    };
    resolve_token_source(source)
        .await
        .map(GoogleCredential::Bearer)
}

async fn resolve_token_source(source: TokenSource) -> Result<String, AccountAuthError> {
    // The detached transaction owns the refresh lock through persistence even
    // if a recording or caller is cancelled while Google is replying.
    let transaction = tokio::spawn(async move {
        let state = provider_auth_state(ProviderKind::GoogleGemini);
        let _refresh = state.refresh.lock().await;
        let snapshot_state = state.clone();
        let (mut snapshot, epoch) = credential_io(move || {
            let epoch = snapshot_state
                .mutation
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            Ok((read_snapshot(source)?, std::sync::Arc::clone(&epoch)))
        })
        .await?;
        let ready_token = record_access_token(&snapshot.record).or_else(|| {
            let cache = refreshed_cache()
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            cache
                .as_ref()
                .filter(|cached| {
                    cached.source == snapshot.source
                        && cached.raw == snapshot.raw
                        && cached.expires_at_unix
                            > now_unix().saturating_add(REFRESH_SKEW.as_secs() as i64)
                })
                .map(|cached| cached.access_token.clone())
        });
        if let Some(token) = ready_token {
            let check_state = state.clone();
            return credential_io(move || {
                check_state.commit_with(ProviderKind::GoogleGemini, &epoch, || {
                    ensure_snapshot_current(&snapshot)?;
                    Ok(token)
                })
            })
            .await;
        }
        let refreshed = refresh_record(&snapshot.record).await?;
        snapshot.record["access_token"] = Value::String(refreshed.access_token.clone());
        snapshot.record["expires_at_unix"] = Value::from(refreshed.expires_at_unix);
        if let Some(refresh_token) = refreshed.refresh_token {
            snapshot.record["refresh_token"] = Value::String(refresh_token);
        }
        let token = refreshed.access_token;
        let cache_token = token.clone();
        let commit_state = state.clone();
        credential_io(move || {
            commit_state.commit_with(ProviderKind::GoogleGemini, &epoch, || {
                // Also reject an external Keychain/ADC replacement that did not
                // pass through Codescribe's own account mutation entry point.
                ensure_snapshot_current(&snapshot)?;
                if snapshot.source == TokenSource::Keychain {
                    let raw = serde_json::to_string(&snapshot.record).map_err(|_| {
                        AccountAuthError::Storage(
                            "Google token record could not be serialized".into(),
                        )
                    })?;
                    keychain::save_key(GOOGLE_ACCOUNT_TOKENS_ACCOUNT, &raw).map_err(|_| {
                        AccountAuthError::Storage(
                            "Google token record could not be stored in Keychain".into(),
                        )
                    })?;
                    snapshot.raw = raw;
                }
                *refreshed_cache()
                    .lock()
                    .unwrap_or_else(|error| error.into_inner()) = Some(CachedToken {
                    source: snapshot.source,
                    raw: snapshot.raw,
                    access_token: cache_token,
                    expires_at_unix: refreshed.expires_at_unix,
                });
                Ok(())
            })
        })
        .await?;
        Ok(token)
    });
    transaction
        .await
        .map_err(|_| AccountAuthError::Storage("Google credential refresh task ended".into()))?
}

fn ensure_snapshot_current(snapshot: &RecordSnapshot) -> Result<(), AccountAuthError> {
    if read_snapshot(snapshot.source.clone())?.raw != snapshot.raw {
        return Err(AccountAuthError::OAuth(
            "Google credentials changed during acquisition; retry the request".into(),
        ));
    }
    Ok(())
}

struct RefreshedToken {
    access_token: String,
    refresh_token: Option<String>,
    expires_at_unix: i64,
}

async fn refresh_record(record: &Value) -> Result<RefreshedToken, AccountAuthError> {
    let refresh_token = string_field(record, "refresh_token").ok_or_else(|| {
        AccountAuthError::OAuth("Google credential expired and has no refresh token".into())
    })?;
    let client_id = string_field(record, "client_id").ok_or_else(|| {
        AccountAuthError::OAuth("Google refresh requires client_id in the credential record".into())
    })?;
    let client_secret = string_field(record, "client_secret").ok_or_else(|| {
        AccountAuthError::OAuth(
            "Google refresh requires client_secret in the credential record".into(),
        )
    })?;
    let endpoint = string_field(record, "token_uri").unwrap_or(TOKEN_ENDPOINT);
    validate_token_endpoint(endpoint)?;
    let client = super::oauth_http_client(endpoint)?;
    let response = client
        .post(endpoint)
        .form(&[
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh_token),
            ("client_id", client_id),
            ("client_secret", client_secret),
        ])
        .send()
        .await
        .map_err(|_| AccountAuthError::Http("Google OAuth token request failed".into()))?;
    if !response.status().is_success() {
        // Never read or interpolate the error body: OAuth endpoints can echo
        // refresh tokens and client secrets with no recognizable token prefix.
        return Err(AccountAuthError::OAuth(format!(
            "Google token endpoint returned HTTP {}",
            response.status().as_u16()
        )));
    }
    #[derive(Deserialize)]
    struct TokenResponse {
        access_token: String,
        refresh_token: Option<String>,
        expires_in: u64,
    }
    let body: TokenResponse = response
        .json()
        .await
        .map_err(|_| AccountAuthError::OAuth("Google token response is invalid".into()))?;
    if body.access_token.trim().is_empty() || body.expires_in == 0 {
        return Err(AccountAuthError::OAuth(
            "Google token response has no usable token lifetime".into(),
        ));
    }
    let seconds = i64::try_from(body.expires_in)
        .map_err(|_| AccountAuthError::OAuth("Google token lifetime is invalid".into()))?;
    let expires_at_unix = now_unix()
        .checked_add(seconds)
        .ok_or_else(|| AccountAuthError::OAuth("Google token lifetime is invalid".into()))?;
    Ok(RefreshedToken {
        access_token: body.access_token,
        refresh_token: body.refresh_token,
        expires_at_unix,
    })
}

/// Scrub recognizable Google API keys and OAuth access tokens from diagnostics.
/// Callers must additionally replace their exact active credential, because
/// configured test/proxy credentials need not use Google's public prefixes.
pub fn redact_google_secrets(message: &str) -> String {
    static PATTERN: OnceLock<regex::Regex> = OnceLock::new();
    PATTERN
        .get_or_init(|| {
            regex::Regex::new(r"(?:AIza|ya29\.)[A-Za-z0-9_.~+/-]*")
                .expect("constant Google credential regex")
        })
        .replace_all(message, "[REDACTED]")
        .into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::sync::Arc;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[test]
    fn google_credential_shapes_and_diagnostics_fail_closed() {
        assert!(parse_record(r#"{"type":"service_account","private_key":"fixture"}"#).is_err());
        assert!(parse_record(r#"{"type":"external_account"}"#).is_err());
        assert!(validate_token_endpoint("https://example.invalid/token").is_err());
        // Build public fixture markers without embedding a scanner-shaped token.
        assert!(
            !redact_google_secrets(concat!("AI", "zaFixtureKey ya", "29.FixtureToken"))
                .contains("Fixture")
        );
        assert!(
            !format!("{:?}", GoogleCredential::ApiKey("private-fixture".into()))
                .contains("private-fixture")
        );
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn adc_refresh_cannot_overwrite_replacement_or_survive_account_removal() {
        for replace_source in [false, true] {
            let _tokens = crate::test_isolation::EnvGuard::remove(GOOGLE_ACCOUNT_TOKENS_ACCOUNT);
            let root = tempfile::tempdir().unwrap();
            let adc = root.path().join("adc.json");
            let _bundle = keychain::test_support::install_bundle(&[]);
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let endpoint = format!("http://{}/token", listener.local_addr().unwrap());
            let record = json!({"type":"authorized_user","access_token":"expired-fixture", "expires_at_unix":1,"refresh_token":"fixture-refresh", "client_id":"fixture-id","client_secret":"fixture-client","token_uri":endpoint});
            std::fs::write(&adc, record.to_string()).unwrap();
            let seen = Arc::new(tokio::sync::Notify::new());
            let release = Arc::new(tokio::sync::Notify::new());
            let observed = seen.clone();
            let released = release.clone();
            let server = tokio::spawn(async move {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut request = Vec::new();
                loop {
                    let mut buf = [0u8; 4096];
                    let read = socket.read(&mut buf).await.unwrap();
                    assert_ne!(read, 0);
                    request.extend_from_slice(&buf[..read]);
                    if let Some(end) = request.windows(4).position(|bytes| bytes == b"\r\n\r\n") {
                        let headers = std::str::from_utf8(&request[..end]).unwrap();
                        let size: usize = headers
                            .lines()
                            .filter_map(|line| line.split_once(':'))
                            .find(|(key, _)| key.eq_ignore_ascii_case("content-length"))
                            .unwrap()
                            .1
                            .trim()
                            .parse()
                            .unwrap();
                        if request.len() >= end + 4 + size {
                            break;
                        }
                    }
                }
                observed.notify_one();
                released.notified().await;
                let body =
                    json!({"access_token":"old-refresh-result","expires_in":3600}).to_string();
                let response = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                    body.len()
                );
                socket.write_all(response.as_bytes()).await.unwrap();
            });
            let path = adc.clone();
            let acquisition =
                tokio::spawn(async move { resolve_token_source(TokenSource::Adc(path)).await });
            tokio::time::timeout(std::time::Duration::from_secs(3), seen.notified())
                .await
                .unwrap();
            let expected = if replace_source {
                let mut changed = record.clone();
                changed["refresh_token"] = json!("replacement-fixture");
                let raw = changed.to_string();
                std::fs::write(&adc, &raw).unwrap();
                raw
            } else {
                clear_google_account_tokens().unwrap();
                record.to_string()
            };
            release.notify_one();
            assert!(
                tokio::time::timeout(std::time::Duration::from_secs(3), acquisition)
                    .await
                    .unwrap()
                    .unwrap()
                    .is_err()
            );
            server.await.unwrap();
            assert_eq!(std::fs::read_to_string(&adc).unwrap(), expected);
        }
    }
}
