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
    keychain::cached_runtime_key(GOOGLE_API_KEY_ACCOUNT)
        .or_else(|| non_empty_env("GEMINI_API_KEY"))
        .or_else(|| non_empty_env("GOOGLE_API_KEY"))
}

/// Availability hint captured only when a Google lane is selected. ADC file
/// contents and tokens are acquired later on the blocking pool, at secret use.
pub fn google_credentials_available() -> bool {
    google_api_key().is_some()
        || keychain::cached_runtime_key(GOOGLE_ACCOUNT_TOKENS_ACCOUNT)
            .is_some_and(|raw| parse_record(&raw).is_ok())
        || std::env::var_os("GOOGLE_APPLICATION_CREDENTIALS").is_some_and(|path| !path.is_empty())
        || adc_path().is_some_and(|path| path.is_file())
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
        if let Some(key) = keychain::runtime_key(&account) {
            return Ok(Some(GoogleCredential::ApiKey(key)));
        }
        if account != GOOGLE_API_KEY_ACCOUNT {
            if let Some(key) = keychain::runtime_key(GOOGLE_API_KEY_ACCOUNT) {
                return Ok(Some(GoogleCredential::ApiKey(key)));
            }
        }
        Ok(None)
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
                .unwrap_or_else(|error| error.into_inner())
                .clone();
            Ok((read_snapshot(source)?, epoch))
        })
        .await?;
        if let Some(token) = record_access_token(&snapshot.record) {
            return Ok(token);
        }
        {
            let cache = refreshed_cache()
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            if let Some(cached) = cache.as_ref().filter(|cached| {
                cached.source == snapshot.source
                    && cached.raw == snapshot.raw
                    && cached.expires_at_unix
                        > now_unix().saturating_add(REFRESH_SKEW.as_secs() as i64)
            }) {
                return Ok(cached.access_token.clone());
            }
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
                if read_snapshot(snapshot.source.clone())?.raw != snapshot.raw {
                    return Err(AccountAuthError::OAuth(
                        "Google credentials changed during refresh; retry the request".into(),
                    ));
                }
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
