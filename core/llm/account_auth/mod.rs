//! Provider-account authentication foundation for future "Sign in with ChatGPT".
//!
//! Tokens are stored as serialized JSON in the existing Codescribe Keychain
//! bundle under a provider-specific account. No `auth.json` file is written.

use std::fmt;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::config::UserSettings;
use crate::config::keychain::{delete_key, load_key, save_key};
use crate::llm::provider::ProviderKind;

pub mod device_code;
pub mod paste_code;
pub mod pkce;
pub mod server;

pub use device_code::{
    DeviceAuthConfig, DeviceCode, complete_device_code_login, request_device_code,
};
pub use paste_code::{PasteCodeExchange, exchange_pasted_code, split_pasted_code};
pub use pkce::{PkceCodes, challenge_for_verifier, generate_pkce};
pub use server::{LoginServer, ServerOptions, exchange_code_for_tokens, run_login_server};

pub const OPENAI_ACCOUNT_TOKENS_ACCOUNT: &str = "LLM_OPENAI_ACCOUNT_TOKENS";
/// Router key of the operator-configurable client id (settings.json, non-secret).
pub const OPENAI_CLIENT_ID_SETTING: &str = "LLM_OPENAI_OAUTH_CLIENT_ID";
pub const OPENAI_CLIENT_ID_ENV: &str = "CODESCRIBE_OPENAI_OAUTH_CLIENT_ID";
pub const OPENAI_ISSUER_ENV: &str = "CODESCRIBE_OPENAI_OAUTH_ISSUER";
pub const DEFAULT_ISSUER: &str = "https://auth.openai.com";
pub const NO_CLIENT_ID_MESSAGE: &str = "awaiting app registration";

pub const ANTHROPIC_ACCOUNT_TOKENS_ACCOUNT: &str = "LLM_ANTHROPIC_ACCOUNT_TOKENS";
pub const ANTHROPIC_CLIENT_ID_SETTING: &str = "LLM_ANTHROPIC_OAUTH_CLIENT_ID";
pub const ANTHROPIC_CLIENT_ID_ENV: &str = "CODESCRIBE_ANTHROPIC_OAUTH_CLIENT_ID";
pub const ANTHROPIC_ISSUER_ENV: &str = "CODESCRIBE_ANTHROPIC_OAUTH_ISSUER";
pub const ANTHROPIC_DEFAULT_ISSUER: &str = "https://console.anthropic.com";

/// How a provider's token endpoint wants its request body. OpenAI speaks
/// form-urlencoded (RFC 6749 §4.1.3 to the letter); Anthropic's console
/// endpoint takes JSON.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenRequestEncoding {
    Form,
    Json,
}

/// How the authorization code gets back to Codescribe.
///
/// This is the one provider property the login *entry point* must branch on:
/// the two flows need different machinery (a local HTTP listener vs. a text
/// field), so a mismatch has to be refused rather than approximated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoginFlow {
    /// Provider redirects to a loopback listener this app binds
    /// (`{callback_host}:{port}{callback_path}`) — OpenAI, and xAI later.
    Loopback,
    /// Provider renders `"<code>#<state>"` on its own page for the user to
    /// paste back — Anthropic's console flow. See [`paste_code`].
    PasteCode,
}

/// Everything that differs between providers in one place, so adding a
/// provider is a table row rather than a new `match` arm in five functions.
#[derive(Debug, Clone, Copy)]
pub struct ProviderOAuthConfig {
    /// Which provider this row describes.
    pub provider: ProviderKind,
    /// Keychain account name holding the serialized `AccountTokens`.
    pub tokens_account: &'static str,
    /// Settings router key for the operator-pasted client id (non-secret).
    pub client_id_setting: &'static str,
    /// Dev/CI env fallback for the client id.
    pub client_id_env: &'static str,
    /// Reads this provider's operator-pasted client id out of a settings
    /// snapshot. The row carries its own accessor so resolving a client id
    /// never becomes a `match` over setting keys.
    pub client_id_from_settings: fn(&UserSettings) -> Option<String>,
    /// A client id the vendor publishes for desktop apps, used only when the
    /// operator configured none. `None` ⇒ the provider stays gated on
    /// registration; Codescribe never borrows another application's identity.
    pub default_client_id: Option<&'static str>,
    /// Env override for the issuer base URL.
    pub issuer_env: &'static str,
    pub default_issuer: &'static str,
    /// Path appended to the issuer for the authorize endpoint. Unused by
    /// [`LoginFlow::PasteCode`] rows, whose authorize page can live on a
    /// different host than the token endpoint.
    pub authorize_path: &'static str,
    /// Path appended to the issuer for the token endpoint.
    pub token_path: &'static str,
    /// Redirect path the provider is registered to call back on — appended to
    /// the loopback origin for [`LoginFlow::Loopback`], to the issuer for
    /// [`LoginFlow::PasteCode`].
    pub callback_path: &'static str,
    /// Loopback redirect host. Some providers pin the literal `127.0.0.1`
    /// rather than `localhost`; they are not interchangeable in a registered
    /// redirect URI. Empty for non-loopback rows.
    pub callback_host: &'static str,
    /// Port the provider's registered redirect URI expects. `0` ⇒ any free
    /// port (the provider accepts a wildcard loopback port). Unused by
    /// non-loopback rows.
    pub preferred_port: u16,
    /// OAuth scopes requested at authorize time.
    pub scope: &'static str,
    /// Extra authorize-URL query pairs this provider requires.
    pub extra_authorize_params: &'static [(&'static str, &'static str)],
    pub login_flow: LoginFlow,
    pub encoding: TokenRequestEncoding,
}

const OPENAI_OAUTH: ProviderOAuthConfig = ProviderOAuthConfig {
    provider: ProviderKind::OpenAiResponses,
    tokens_account: OPENAI_ACCOUNT_TOKENS_ACCOUNT,
    client_id_setting: OPENAI_CLIENT_ID_SETTING,
    client_id_env: OPENAI_CLIENT_ID_ENV,
    client_id_from_settings: |settings| settings.openai_oauth_client_id.clone(),
    default_client_id: None,
    issuer_env: OPENAI_ISSUER_ENV,
    default_issuer: DEFAULT_ISSUER,
    authorize_path: "/oauth/authorize",
    token_path: "/oauth/token",
    callback_path: "/auth/callback",
    callback_host: "localhost",
    preferred_port: 1455,
    scope: "openid profile email offline_access",
    extra_authorize_params: &[
        ("id_token_add_organizations", "true"),
        ("codescribe_account_flow", "true"),
    ],
    login_flow: LoginFlow::Loopback,
    encoding: TokenRequestEncoding::Form,
};

const ANTHROPIC_OAUTH: ProviderOAuthConfig = ProviderOAuthConfig {
    provider: ProviderKind::AnthropicMessages,
    tokens_account: ANTHROPIC_ACCOUNT_TOKENS_ACCOUNT,
    client_id_setting: ANTHROPIC_CLIENT_ID_SETTING,
    client_id_env: ANTHROPIC_CLIENT_ID_ENV,
    client_id_from_settings: |settings| settings.anthropic_oauth_client_id.clone(),
    default_client_id: None,
    issuer_env: ANTHROPIC_ISSUER_ENV,
    default_issuer: ANTHROPIC_DEFAULT_ISSUER,
    authorize_path: "/oauth/authorize",
    token_path: "/v1/oauth/token",
    // Anthropic redirects to a page on the issuer host that renders the code;
    // there is no loopback origin to fill in.
    callback_path: "/oauth/code/callback",
    callback_host: "",
    preferred_port: 0,
    scope: "user:profile user:inference",
    extra_authorize_params: &[],
    login_flow: LoginFlow::PasteCode,
    encoding: TokenRequestEncoding::Json,
};

/// The OAuth registry: one row per provider that can sign in with an account.
/// A provider absent from this table has no account auth — [`provider_oauth_config`]
/// answers `UnsupportedProvider`, which is the honest answer, not a panic.
const PROVIDER_OAUTH_REGISTRY: [ProviderOAuthConfig; 2] = [OPENAI_OAUTH, ANTHROPIC_OAUTH];

pub fn provider_oauth_config(
    provider: ProviderKind,
) -> Result<ProviderOAuthConfig, AccountAuthError> {
    PROVIDER_OAUTH_REGISTRY
        .iter()
        .find(|row| row.provider == provider)
        .copied()
        .ok_or_else(|| AccountAuthError::UnsupportedProvider(provider.as_str().to_string()))
}

const REFRESH_SKEW: Duration = Duration::from_secs(60);

#[derive(Debug)]
pub enum AccountAuthError {
    /// No client id configured for a provider. Carries that provider's own
    /// setting/env keys so the message tells the operator which field to fill —
    /// naming OpenAI's keys during an Anthropic sign-in is worse than useless.
    NoClientId {
        setting: &'static str,
        env: &'static str,
    },
    UnsupportedProvider(String),
    NotSignedIn(String),
    Storage(String),
    Http(String),
    OAuth(String),
    Io(std::io::Error),
}

impl fmt::Display for AccountAuthError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AccountAuthError::NoClientId { setting, env } => write!(
                f,
                "{NO_CLIENT_ID_MESSAGE}; paste the registered client id in Settings → Keys \
                 ({setting}) or set {env}"
            ),
            AccountAuthError::UnsupportedProvider(provider) => {
                write!(f, "provider account auth is not available for {provider}")
            }
            AccountAuthError::NotSignedIn(provider) => {
                write!(f, "no provider account tokens stored for {provider}")
            }
            AccountAuthError::Storage(message) => {
                write!(f, "account token storage failed: {message}")
            }
            AccountAuthError::Http(message) => write!(f, "account auth HTTP failed: {message}"),
            AccountAuthError::OAuth(message) => write!(f, "account auth failed: {message}"),
            AccountAuthError::Io(error) => error.fmt(f),
        }
    }
}

impl std::error::Error for AccountAuthError {}

impl From<std::io::Error> for AccountAuthError {
    fn from(error: std::io::Error) -> Self {
        AccountAuthError::Io(error)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AccountTokens {
    pub provider: String,
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub id_token: Option<String>,
    pub token_type: String,
    pub expires_at_unix: Option<i64>,
}

impl AccountTokens {
    pub fn new(
        provider: ProviderKind,
        access_token: String,
        refresh_token: Option<String>,
        id_token: Option<String>,
        token_type: Option<String>,
        expires_in: Option<u64>,
    ) -> Self {
        let expires_at_unix = expires_in.and_then(|seconds| now_unix().checked_add(seconds as i64));
        Self {
            provider: provider.as_str().to_string(),
            access_token,
            refresh_token,
            id_token,
            token_type: token_type.unwrap_or_else(|| "Bearer".to_string()),
            expires_at_unix,
        }
    }

    pub fn expires_within(&self, skew: Duration) -> bool {
        let Some(expires_at) = self.expires_at_unix else {
            return false;
        };
        let now = now_unix();
        expires_at <= now.saturating_add(skew.as_secs() as i64)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountAuthStatus {
    pub provider: ProviderKind,
    pub signed_in: bool,
    pub client_id_configured: bool,
    pub message: String,
}

pub fn account_status(provider: ProviderKind) -> AccountAuthStatus {
    let client_id_configured = client_id_for_provider(provider).is_ok();
    let tokens = load_account_tokens(provider).ok();
    let signed_in = tokens.is_some();
    let message = if !client_id_configured {
        NO_CLIENT_ID_MESSAGE.to_string()
    } else if let Some(tokens) = tokens {
        match id_token_identity(&tokens) {
            Some(identity) => format!("signed in as {identity}"),
            None => "signed in".to_string(),
        }
    } else {
        "not signed in".to_string()
    };
    AccountAuthStatus {
        provider,
        signed_in,
        client_id_configured,
        message,
    }
}

pub fn client_id_for_provider(provider: ProviderKind) -> Result<String, AccountAuthError> {
    let config = provider_oauth_config(provider)?;
    configured_client_id_for(config).ok_or(AccountAuthError::NoClientId {
        setting: config.client_id_setting,
        env: config.client_id_env,
    })
}

/// Client id for a provider, or `None` (⇒ "awaiting app registration").
/// Resolution order is operator settings → dev env var → the row's published
/// default. Reads the persisted settings snapshot on every call — a Keys panel
/// save takes effect on the very next click, no restart, and env never freezes
/// over a saved setting.
///
/// No client id ships hardcoded today: both rows carry `default_client_id:
/// None`. Reusing another vendor's *private* registration (the trick some CLI
/// ports lean on) would make Codescribe impersonate that application; the field
/// exists only for ids a vendor publishes for third-party desktop clients.
fn configured_client_id_for(config: ProviderOAuthConfig) -> Option<String> {
    let settings = UserSettings::load();
    (config.client_id_from_settings)(&settings)
        .and_then(non_empty_trimmed)
        .or_else(|| {
            std::env::var(config.client_id_env)
                .ok()
                .and_then(non_empty_trimmed)
        })
        .or_else(|| config.default_client_id.map(str::to_string))
}

fn non_empty_trimmed(value: String) -> Option<String> {
    let value = value.trim().to_string();
    (!value.is_empty()).then_some(value)
}

/// Best-effort display identity from the id_token JWT payload (email, else
/// sub). Display-only — the claims are NOT verified here; authorization always
/// rides the access token, never this label.
fn id_token_identity(tokens: &AccountTokens) -> Option<String> {
    use base64::Engine;
    let payload = tokens.id_token.as_deref()?.split('.').nth(1)?;
    let decoded = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload.as_bytes())
        .ok()?;
    let claims: serde_json::Value = serde_json::from_slice(&decoded).ok()?;
    ["email", "sub"].iter().find_map(|key| {
        claims
            .get(key)
            .and_then(serde_json::Value::as_str)
            .and_then(|value| non_empty_trimmed(value.to_string()))
    })
}

/// Issuer base URL for `provider`: env override, else the provider default.
pub fn issuer_for(provider: ProviderKind) -> String {
    let Ok(config) = provider_oauth_config(provider) else {
        return DEFAULT_ISSUER.to_string();
    };
    std::env::var(config.issuer_env)
        .ok()
        .map(|value| value.trim().trim_end_matches('/').to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| config.default_issuer.to_string())
}

pub fn store_account_tokens(
    provider: ProviderKind,
    tokens: &AccountTokens,
) -> Result<(), AccountAuthError> {
    ensure_provider_supported(provider)?;
    let account = token_account(provider)?;
    let payload = serde_json::to_string(tokens)
        .map_err(|error| AccountAuthError::Storage(error.to_string()))?;
    save_key(account, &payload).map_err(|error| AccountAuthError::Storage(error.to_string()))
}

pub fn load_account_tokens(provider: ProviderKind) -> Result<AccountTokens, AccountAuthError> {
    ensure_provider_supported(provider)?;
    let account = token_account(provider)?;
    let payload = std::env::var(account)
        .ok()
        .filter(|value| !value.trim().is_empty())
        .or_else(|| load_key(account))
        .ok_or_else(|| AccountAuthError::NotSignedIn(provider.as_str().to_string()))?;
    serde_json::from_str(&payload).map_err(|error| AccountAuthError::Storage(error.to_string()))
}

pub fn clear_account_tokens(provider: ProviderKind) -> Result<(), AccountAuthError> {
    ensure_provider_supported(provider)?;
    let account = token_account(provider)?;
    delete_key(account).map_err(|error| AccountAuthError::Storage(error.to_string()))?;
    // SAFETY: clears the process-env mirror of the tokens (the test/dev
    // injection channel read by `load_account_tokens`) so sign-out is not
    // undone by a stale override. Sign-out is a single user-driven action,
    // not a hot concurrent path.
    unsafe { std::env::remove_var(account) };
    Ok(())
}

pub async fn authorization_header(provider: ProviderKind) -> Result<String, AccountAuthError> {
    Ok(format!("Bearer {}", access_token(provider).await?))
}

/// Fresh access token for the stored provider account, auto-refreshing within
/// the expiry skew. Raw token (no `Bearer ` prefix) — for request builders
/// that format the Authorization header themselves.
///
/// Refresh is single-flight per provider: concurrent callers that all see an
/// expiring token take turns on the provider's refresh lock, and every caller
/// after the first re-reads storage and finds the token already fresh. Without
/// this, N concurrent lanes fire N refreshes against a provider that ROTATES
/// its refresh token, and every response after the first invalidates the token
/// the others are about to store — signing the user out mid-session.
pub async fn access_token(provider: ProviderKind) -> Result<String, AccountAuthError> {
    let tokens = load_account_tokens(provider)?;
    if !tokens.expires_within(REFRESH_SKEW) {
        return Ok(tokens.access_token);
    }

    let _lock = refresh_lock(provider).lock().await;
    // Re-read under the lock: a concurrent holder may have just refreshed.
    let tokens = load_account_tokens(provider)?;
    if !tokens.expires_within(REFRESH_SKEW) {
        return Ok(tokens.access_token);
    }
    Ok(refresh_tokens(provider, tokens).await?.access_token)
}

/// Per-provider refresh mutex. Held across the network round trip AND the
/// storage write, so the winner's rotated token is durable before the next
/// caller re-reads.
///
/// Keyed by provider rather than one static per vendor: the map grows a lock
/// the first time a provider refreshes, so a new registry row needs no edit
/// here. The leak is bounded by the number of providers that ever refresh.
fn refresh_lock(provider: ProviderKind) -> &'static tokio::sync::Mutex<()> {
    use std::collections::HashMap;
    use std::sync::{Mutex, OnceLock};

    type LockTable = Mutex<HashMap<ProviderKind, &'static tokio::sync::Mutex<()>>>;
    static LOCKS: OnceLock<LockTable> = OnceLock::new();

    let mut table = LOCKS
        .get_or_init(Default::default)
        .lock()
        // A panic while merely inserting into this map cannot corrupt it, so a
        // poisoned guard is safe to take over — refusing would break refresh.
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    table
        .entry(provider)
        .or_insert_with(|| Box::leak(Box::new(tokio::sync::Mutex::new(()))))
}

pub async fn refresh_tokens(
    provider: ProviderKind,
    tokens: AccountTokens,
) -> Result<AccountTokens, AccountAuthError> {
    let config = provider_oauth_config(provider)?;
    let refresh_token = tokens.refresh_token.ok_or_else(|| {
        AccountAuthError::OAuth("stored account has no refresh token".to_string())
    })?;
    let client_id = client_id_for_provider(provider)?;
    let issuer = issuer_for(provider);
    let refreshed =
        refresh_provider_tokens(provider, config, &issuer, &client_id, &refresh_token).await?;
    store_account_tokens(provider, &refreshed)?;
    Ok(refreshed)
}

async fn refresh_provider_tokens(
    provider: ProviderKind,
    config: ProviderOAuthConfig,
    issuer: &str,
    client_id: &str,
    refresh_token: &str,
) -> Result<AccountTokens, AccountAuthError> {
    #[derive(Deserialize)]
    struct RefreshResponse {
        access_token: String,
        refresh_token: Option<String>,
        id_token: Option<String>,
        token_type: Option<String>,
        expires_in: Option<u64>,
    }

    let endpoint = format!("{}{}", issuer.trim_end_matches('/'), config.token_path);
    let client = reqwest::Client::new();
    let request = client.post(endpoint);
    let request = match config.encoding {
        TokenRequestEncoding::Form => request.form(&[
            ("grant_type", "refresh_token"),
            ("client_id", client_id),
            ("refresh_token", refresh_token),
        ]),
        TokenRequestEncoding::Json => request.json(&serde_json::json!({
            "grant_type": "refresh_token",
            "client_id": client_id,
            "refresh_token": refresh_token,
        })),
    };
    let response = request
        .send()
        .await
        .map_err(|error| AccountAuthError::Http(error.to_string()))?;

    if !response.status().is_success() {
        return Err(AccountAuthError::OAuth(format!(
            "refresh endpoint returned status {}",
            response.status()
        )));
    }

    let body: RefreshResponse = response
        .json()
        .await
        .map_err(|error| AccountAuthError::OAuth(error.to_string()))?;
    Ok(AccountTokens::new(
        provider,
        body.access_token,
        // Providers that rotate the refresh token return a new one; those that
        // do not omit the field. Carrying the old one forward keeps both honest.
        body.refresh_token.or(Some(refresh_token.to_string())),
        body.id_token,
        body.token_type,
        body.expires_in,
    ))
}

fn token_account(provider: ProviderKind) -> Result<&'static str, AccountAuthError> {
    Ok(provider_oauth_config(provider)?.tokens_account)
}

fn ensure_provider_supported(provider: ProviderKind) -> Result<(), AccountAuthError> {
    provider_oauth_config(provider).map(|_| ())
}

fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;

    /// Point the settings store at an isolated scratch dir so these tests never
    /// read (or depend on) the operator's real settings.json.
    fn isolated_settings_dir(tag: &str) -> (EnvGuard, tempfile::TempDir) {
        let dir = tempfile::Builder::new()
            .prefix(&format!("cs_account_auth_{tag}_"))
            .tempdir()
            .expect("create scratch settings dir");
        (EnvGuard::set_path("CODESCRIBE_DATA_DIR", dir.path()), dir)
    }

    #[test]
    #[serial]
    fn no_client_id_reports_registration_gate() {
        let (_data_dir, _dir) = isolated_settings_dir("gate");
        let _guard = EnvGuard::unset(OPENAI_CLIENT_ID_ENV);
        // Pin the Anthropic env too: the operator's dotenv is inherited by the
        // test process, so an unpinned var makes this pass or fail by machine.
        let _anthropic_guard = EnvGuard::unset(ANTHROPIC_CLIENT_ID_ENV);
        let err = client_id_for_provider(ProviderKind::OpenAiResponses).unwrap_err();
        assert!(matches!(err, AccountAuthError::NoClientId { .. }));
        assert!(err.to_string().contains(NO_CLIENT_ID_MESSAGE));
        // The message must name *this* provider's fields, not OpenAI's by reflex.
        let anthropic = client_id_for_provider(ProviderKind::AnthropicMessages).unwrap_err();
        assert!(anthropic.to_string().contains(ANTHROPIC_CLIENT_ID_SETTING));
        assert!(anthropic.to_string().contains(ANTHROPIC_CLIENT_ID_ENV));
    }

    #[test]
    #[serial]
    fn settings_client_id_beats_env_and_applies_without_restart() {
        let (_data_dir, _dir) = isolated_settings_dir("resolution");
        let _env = EnvGuard::set(OPENAI_CLIENT_ID_ENV, "env-client");

        // Env alone (dev fallback) resolves.
        assert_eq!(
            client_id_for_provider(ProviderKind::OpenAiResponses).unwrap(),
            "env-client"
        );

        // A Keys-panel save lands in settings.json mid-process — the very next
        // resolution must see it (fresh read per call, no frozen env).
        UserSettings {
            openai_oauth_client_id: Some("settings-client".to_string()),
            ..Default::default()
        }
        .save()
        .expect("persist client id");
        assert_eq!(
            client_id_for_provider(ProviderKind::OpenAiResponses).unwrap(),
            "settings-client"
        );

        // Clearing the setting falls back to env, again without restart.
        UserSettings {
            openai_oauth_client_id: None,
            ..Default::default()
        }
        .save()
        .expect("clear client id");
        assert_eq!(
            client_id_for_provider(ProviderKind::OpenAiResponses).unwrap(),
            "env-client"
        );
    }

    /// The three UI states the Keys panel renders, in the order an operator
    /// walks them: registration gate → not signed in → signed in as <email>.
    #[test]
    #[serial]
    fn account_status_maps_gate_then_not_signed_in_then_signed_in() {
        use base64::Engine;
        let (_data_dir, _dir) = isolated_settings_dir("status");
        let _disable = EnvGuard::set("CODESCRIBE_DISABLE_KEYCHAIN", "1");
        let _tokens = EnvGuard::unset(OPENAI_ACCOUNT_TOKENS_ACCOUNT);
        let _env = EnvGuard::unset(OPENAI_CLIENT_ID_ENV);

        // 1. No client id anywhere ⇒ registration gate, button disabled.
        let status = account_status(ProviderKind::OpenAiResponses);
        assert!(!status.client_id_configured);
        assert!(!status.signed_in);
        assert_eq!(status.message, NO_CLIENT_ID_MESSAGE);

        // 2. Operator pastes a client id in Settings ⇒ gate lifts mid-process.
        UserSettings {
            openai_oauth_client_id: Some("app_registered".to_string()),
            ..Default::default()
        }
        .save()
        .expect("persist client id");
        let status = account_status(ProviderKind::OpenAiResponses);
        assert!(status.client_id_configured);
        assert!(!status.signed_in);
        assert_eq!(status.message, "not signed in");

        // 3. Stored tokens with an id_token email ⇒ signed in as <email>.
        let claims = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(r#"{"email":"maciej@example.com"}"#);
        let tokens = AccountTokens::new(
            ProviderKind::OpenAiResponses,
            "access".to_string(),
            Some("refresh".to_string()),
            Some(format!("header.{claims}.signature")),
            None,
            Some(3600),
        );
        store_account_tokens(ProviderKind::OpenAiResponses, &tokens).expect("store tokens");
        let status = account_status(ProviderKind::OpenAiResponses);
        assert!(status.client_id_configured);
        assert!(status.signed_in);
        assert_eq!(status.message, "signed in as maciej@example.com");
    }

    #[test]
    fn signed_in_status_carries_the_id_token_email_when_present() {
        use base64::Engine;
        let claims = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(r#"{"email":"maciej@example.com","sub":"user-123"}"#);
        let tokens = AccountTokens {
            provider: ProviderKind::OpenAiResponses.as_str().to_string(),
            access_token: "access".to_string(),
            refresh_token: None,
            id_token: Some(format!("header.{claims}.signature")),
            token_type: "Bearer".to_string(),
            expires_at_unix: None,
        };
        assert_eq!(
            id_token_identity(&tokens).as_deref(),
            Some("maciej@example.com")
        );

        let no_id_token = AccountTokens {
            id_token: None,
            ..tokens
        };
        assert_eq!(id_token_identity(&no_id_token), None);
    }

    #[test]
    #[serial]
    fn keychain_mock_round_trips_serialized_account_tokens() {
        let _disable = EnvGuard::set("CODESCRIBE_DISABLE_KEYCHAIN", "1");
        let _tokens = EnvGuard::unset(OPENAI_ACCOUNT_TOKENS_ACCOUNT);
        let tokens = AccountTokens::new(
            ProviderKind::OpenAiResponses,
            "access".to_string(),
            Some("refresh".to_string()),
            Some("id".to_string()),
            None,
            Some(3600),
        );

        store_account_tokens(ProviderKind::OpenAiResponses, &tokens).unwrap();

        let loaded = load_account_tokens(ProviderKind::OpenAiResponses).unwrap();
        assert_eq!(loaded.access_token, "access");
        assert_eq!(loaded.refresh_token.as_deref(), Some("refresh"));
    }

    #[test]
    #[serial]
    fn provider_accounts_never_share_a_keychain_slot() {
        let _disable = EnvGuard::set("CODESCRIBE_DISABLE_KEYCHAIN", "1");
        let _openai = EnvGuard::unset(OPENAI_ACCOUNT_TOKENS_ACCOUNT);
        let _anthropic = EnvGuard::unset(ANTHROPIC_ACCOUNT_TOKENS_ACCOUNT);

        let openai = AccountTokens::new(
            ProviderKind::OpenAiResponses,
            "openai-access".to_string(),
            Some("openai-refresh".to_string()),
            None,
            None,
            Some(3600),
        );
        store_account_tokens(ProviderKind::OpenAiResponses, &openai).unwrap();

        // Signing into one provider must not make the other look signed in —
        // a shared slot would hand Anthropic requests an OpenAI bearer token.
        assert!(load_account_tokens(ProviderKind::AnthropicMessages).is_err());

        let anthropic = AccountTokens::new(
            ProviderKind::AnthropicMessages,
            "anthropic-access".to_string(),
            Some("anthropic-refresh".to_string()),
            None,
            None,
            Some(3600),
        );
        store_account_tokens(ProviderKind::AnthropicMessages, &anthropic).unwrap();

        assert_eq!(
            load_account_tokens(ProviderKind::OpenAiResponses)
                .unwrap()
                .access_token,
            "openai-access"
        );
        assert_eq!(
            load_account_tokens(ProviderKind::AnthropicMessages)
                .unwrap()
                .access_token,
            "anthropic-access"
        );

        clear_account_tokens(ProviderKind::AnthropicMessages).unwrap();
        assert!(load_account_tokens(ProviderKind::AnthropicMessages).is_err());
        assert!(load_account_tokens(ProviderKind::OpenAiResponses).is_ok());
    }

    #[test]
    #[serial]
    fn each_provider_reads_its_own_client_id_and_issuer() {
        let (_settings_guard, _dir) = isolated_settings_dir("provider_identity");
        let _openai_env = EnvGuard::unset(OPENAI_CLIENT_ID_ENV);
        let _anthropic_env = EnvGuard::set(ANTHROPIC_CLIENT_ID_ENV, "anthropic-from-env");
        let _issuer = EnvGuard::unset(ANTHROPIC_ISSUER_ENV);

        // OpenAI has neither setting nor env ⇒ still gated on registration,
        // even though Anthropic's env id is present.
        assert!(matches!(
            client_id_for_provider(ProviderKind::OpenAiResponses),
            Err(AccountAuthError::NoClientId { .. })
        ));
        assert_eq!(
            client_id_for_provider(ProviderKind::AnthropicMessages).unwrap(),
            "anthropic-from-env"
        );
        assert_eq!(
            issuer_for(ProviderKind::AnthropicMessages),
            ANTHROPIC_DEFAULT_ISSUER
        );
        assert_eq!(issuer_for(ProviderKind::OpenAiResponses), DEFAULT_ISSUER);
    }

    /// The registry is the substrate every other guarantee rests on: a row
    /// pointing at another row's keychain account or settings key would hand
    /// one vendor another vendor's credential. Adding a provider by copy-paste
    /// is exactly how that happens, so it is pinned here rather than reviewed.
    #[test]
    fn oauth_rows_never_share_an_account_or_a_client_id_channel() {
        let mut accounts: Vec<&str> = Vec::new();
        let mut settings_keys: Vec<&str> = Vec::new();
        let mut envs: Vec<&str> = Vec::new();
        for row in PROVIDER_OAUTH_REGISTRY {
            assert_eq!(
                provider_oauth_config(row.provider).unwrap().tokens_account,
                row.tokens_account,
                "{} resolves to a different row than it declares",
                row.provider
            );
            assert!(row.token_path.starts_with('/'), "{}", row.provider);
            assert!(row.callback_path.starts_with('/'), "{}", row.provider);
            if row.login_flow == LoginFlow::Loopback {
                assert!(
                    !row.callback_host.is_empty(),
                    "{} needs a loopback host for its registered redirect URI",
                    row.provider
                );
            }
            accounts.push(row.tokens_account);
            settings_keys.push(row.client_id_setting);
            envs.push(row.client_id_env);
        }
        for list in [&accounts, &settings_keys, &envs] {
            let mut sorted = list.clone();
            sorted.sort_unstable();
            let before = sorted.len();
            sorted.dedup();
            assert_eq!(
                before,
                sorted.len(),
                "duplicate OAuth row value in {list:?}"
            );
        }
    }

    /// Every provider that can be selected in Settings must resolve to a row —
    /// or fail loudly. A silent `UnsupportedProvider` on a provider the picker
    /// offers is a dead sign-in button with no explanation.
    #[test]
    fn every_selectable_provider_has_an_oauth_row() {
        use crate::llm::provider::ALL_PROVIDERS;
        for provider in ALL_PROVIDERS {
            assert!(
                provider_oauth_config(provider).is_ok(),
                "{provider} is selectable but has no OAuth row"
            );
        }
    }

    /// Client-id resolution reads the row's own accessor, so each provider sees
    /// only its own saved value — no cross-reads through a shared settings key.
    #[test]
    #[serial]
    fn each_row_reads_only_its_own_saved_client_id() {
        let (_data_dir, _dir) = isolated_settings_dir("row_accessor");
        let _openai_env = EnvGuard::unset(OPENAI_CLIENT_ID_ENV);
        let _anthropic_env = EnvGuard::unset(ANTHROPIC_CLIENT_ID_ENV);

        UserSettings {
            openai_oauth_client_id: Some("openai-app".to_string()),
            ..Default::default()
        }
        .save()
        .expect("persist client id");

        assert_eq!(
            client_id_for_provider(ProviderKind::OpenAiResponses).unwrap(),
            "openai-app"
        );
        assert!(matches!(
            client_id_for_provider(ProviderKind::AnthropicMessages),
            Err(AccountAuthError::NoClientId { .. })
        ));
    }

    /// No row ships a client id today; the field exists for vendor-published
    /// desktop ids. If one ever lands here it must be a deliberate edit, not a
    /// silent lift of someone else's registration.
    #[test]
    fn no_row_ships_a_borrowed_client_id() {
        for row in PROVIDER_OAUTH_REGISTRY {
            assert_eq!(
                row.default_client_id, None,
                "{} ships a hardcoded client id",
                row.provider
            );
        }
    }

    #[derive(Debug)]
    struct EnvGuard {
        key: &'static str,
        previous: Option<String>,
    }

    impl EnvGuard {
        fn set(key: &'static str, value: &str) -> Self {
            let previous = std::env::var(key).ok();
            // SAFETY: these process-env tests are serialized with `serial`.
            unsafe { std::env::set_var(key, value) };
            Self { key, previous }
        }

        fn set_path(key: &'static str, value: &std::path::Path) -> Self {
            let previous = std::env::var(key).ok();
            // SAFETY: these process-env tests are serialized with `serial`.
            unsafe { std::env::set_var(key, value) };
            Self { key, previous }
        }

        fn unset(key: &'static str) -> Self {
            let previous = std::env::var(key).ok();
            // SAFETY: these process-env tests are serialized with `serial`.
            unsafe { std::env::remove_var(key) };
            Self { key, previous }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            match &self.previous {
                // SAFETY: these process-env tests are serialized with `serial`.
                Some(value) => unsafe { std::env::set_var(self.key, value) },
                // SAFETY: these process-env tests are serialized with `serial`.
                None => unsafe { std::env::remove_var(self.key) },
            }
        }
    }
}
