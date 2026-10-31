//! Provider-account authentication foundation for future "Sign in with ChatGPT".
//!
//! Tokens are stored as serialized JSON in the existing Codescribe Keychain
//! bundle under a provider-specific account. No `auth.json` file is written.

use std::fmt;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::config::UserSettings;
use crate::config::keychain::{cached_runtime_key, delete_key, ensure_bundle_loaded, save_key};
use crate::llm::provider::ProviderKind;

/// Device-code OAuth grant for providers that cannot do loopback redirects.
pub mod device_code;
/// Paste-code flow: user copies `"code#state"` from the provider's page.
pub mod paste_code;
/// PKCE verifier/challenge helpers shared by loopback and paste-code logins.
pub mod pkce;
/// Local loopback HTTP server that receives OAuth redirect callbacks.
pub mod server;

pub use device_code::{
    DeviceAuthConfig, DeviceCode, complete_device_code_login, request_device_code,
};
pub use paste_code::{PasteCodeExchange, exchange_pasted_code, split_pasted_code};
pub use pkce::{PkceCodes, challenge_for_verifier, generate_pkce};
pub use server::{LoginServer, ServerOptions, exchange_code_for_tokens, run_login_server};

/// Keychain account holding OpenAI's serialized [`AccountTokens`]. Doubles as
/// the env var name of the test/dev token-injection channel.
pub const OPENAI_ACCOUNT_TOKENS_ACCOUNT: &str = "LLM_OPENAI_ACCOUNT_TOKENS";
/// Router key of the operator-configurable client id (settings.json, non-secret).
pub const OPENAI_CLIENT_ID_SETTING: &str = "LLM_OPENAI_OAUTH_CLIENT_ID";
/// Dev/CI env fallback for OpenAI's client id, checked after settings.
pub const OPENAI_CLIENT_ID_ENV: &str = "CODESCRIBE_OPENAI_OAUTH_CLIENT_ID";
/// Env override for OpenAI's issuer base URL.
pub const OPENAI_ISSUER_ENV: &str = "CODESCRIBE_OPENAI_OAUTH_ISSUER";
/// OpenAI's issuer, and the fallback [`issuer_for`] returns for a provider with
/// no registry row.
pub const DEFAULT_ISSUER: &str = "https://auth.openai.com";
/// Operator-facing text for "this provider has no client id yet". Shared by the
/// error and the status message so the UI never shows two spellings of one state.
pub const NO_CLIENT_ID_MESSAGE: &str = "awaiting app registration";

/// Keychain account holding Anthropic's serialized [`AccountTokens`].
pub const ANTHROPIC_ACCOUNT_TOKENS_ACCOUNT: &str = "LLM_ANTHROPIC_ACCOUNT_TOKENS";
/// Settings router key for Anthropic's operator-pasted client id.
pub const ANTHROPIC_CLIENT_ID_SETTING: &str = "LLM_ANTHROPIC_OAUTH_CLIENT_ID";
/// Dev/CI env fallback for Anthropic's client id.
pub const ANTHROPIC_CLIENT_ID_ENV: &str = "CODESCRIBE_ANTHROPIC_OAUTH_CLIENT_ID";
/// Env override for Anthropic's issuer base URL.
pub const ANTHROPIC_ISSUER_ENV: &str = "CODESCRIBE_ANTHROPIC_OAUTH_ISSUER";
/// Anthropic's console issuer — the paste-code flow's authorize page and token
/// endpoint both live here.
pub const ANTHROPIC_DEFAULT_ISSUER: &str = "https://console.anthropic.com";

/// Keychain account holding xAI's serialized [`AccountTokens`].
pub const XAI_ACCOUNT_TOKENS_ACCOUNT: &str = "LLM_XAI_ACCOUNT_TOKENS";
/// Settings router key for xAI's operator-pasted client id.
pub const XAI_CLIENT_ID_SETTING: &str = "LLM_XAI_OAUTH_CLIENT_ID";
/// Dev/CI env fallback for xAI's client id.
pub const XAI_CLIENT_ID_ENV: &str = "CODESCRIBE_XAI_OAUTH_CLIENT_ID";
/// Env override for xAI's issuer base URL.
pub const XAI_ISSUER_ENV: &str = "CODESCRIBE_XAI_OAUTH_ISSUER";
/// xAI's issuer base URL.
pub const XAI_DEFAULT_ISSUER: &str = "https://auth.x.ai";

/// The client id OpenAI's Codex CLI uses for the ChatGPT desktop loopback
/// OAuth flow — a **public** app id (`app_…`), not a secret. Shipped so
/// "Sign in with ChatGPT" works without the operator pasting a registration.
/// Disclosed in `NOTICE`. Redirect contract: `localhost` wildcard port path
/// `/auth/callback` (Codex-compatible preferred port 1455).
pub const OPENAI_CODEX_CLI_CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";

/// The client id xAI publishes for the Grok CLI — a **public** desktop OAuth
/// client, not a private registration lifted from another product. Shipped as
/// the xAI row default and disclosed in `NOTICE`.
///
/// The redirect URI is not free to choose: xAI registered this client against
/// `http://127.0.0.1:56121/callback` exactly, so the loopback host and port
/// below are part of the contract, not preferences.
pub const XAI_GROK_CLI_CLIENT_ID: &str = "b1a00492-073a-47ea-816f-4c329264a828";

/// How a provider's token endpoint wants its request body. OpenAI speaks
/// form-urlencoded (RFC 6749 §4.1.3 to the letter); Anthropic's console
/// endpoint takes JSON.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenRequestEncoding {
    /// `application/x-www-form-urlencoded` body.
    Form,
    /// JSON body.
    Json,
}

/// How the authorization code gets back to Codescribe.
///
/// This is the one provider property the login *entry point* must branch on:
/// the flows need different machinery (loopback HTTP, paste field, or RFC 8628
/// device poll), so a mismatch has to be refused rather than approximated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoginFlow {
    /// Provider redirects to a loopback listener this app binds
    /// (`{callback_host}:{port}{callback_path}`) — OpenAI ChatGPT.
    Loopback,
    /// Provider renders `"<code>#<state>"` on its own page for the user to
    /// paste back — Anthropic's console flow. See [`paste_code`].
    PasteCode,
    /// RFC 8628 device authorization: open a verification URL (any device),
    /// poll the token endpoint. OpenCode's SuperGrok path for xAI — no
    /// loopback server, so consent cannot hang waiting on `127.0.0.1:56121`.
    /// See [`device_code`].
    DeviceCode,
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
    /// Issuer used when the env override is unset.
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
    /// How the authorization code comes back — the one property the login entry
    /// point must branch on. See [`LoginFlow`].
    pub login_flow: LoginFlow,
    /// Body encoding this provider's token endpoint expects.
    pub encoding: TokenRequestEncoding,
}

/// OpenAI account-auth row: loopback redirect, form-encoded token endpoint.
const OPENAI_OAUTH: ProviderOAuthConfig = ProviderOAuthConfig {
    provider: ProviderKind::OpenAiResponses,
    tokens_account: OPENAI_ACCOUNT_TOKENS_ACCOUNT,
    client_id_setting: OPENAI_CLIENT_ID_SETTING,
    client_id_env: OPENAI_CLIENT_ID_ENV,
    client_id_from_settings: |settings| settings.openai_oauth_client_id.clone(),
    // Codex CLI public app id — see `OPENAI_CODEX_CLI_CLIENT_ID` + NOTICE.
    // Operator override via settings/env still wins.
    default_client_id: Some(OPENAI_CODEX_CLI_CLIENT_ID),
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

/// Anthropic account-auth row: paste-code flow, JSON token endpoint.
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

/// xAI account-auth row: RFC 8628 device code (OpenCode SuperGrok path) and
/// public Grok CLI client id. Loopback redirect metadata is retained for
/// documentation / NOTICE; the product sign-in path is device-code only.
const XAI_OAUTH: ProviderOAuthConfig = ProviderOAuthConfig {
    provider: ProviderKind::XaiResponses,
    tokens_account: XAI_ACCOUNT_TOKENS_ACCOUNT,
    client_id_setting: XAI_CLIENT_ID_SETTING,
    client_id_env: XAI_CLIENT_ID_ENV,
    client_id_from_settings: |settings| settings.xai_oauth_client_id.clone(),
    // xAI publishes this id for third-party desktop clients
    // (`XAI_GROK_CLI_CLIENT_ID`). Operator settings/env still win.
    default_client_id: Some(XAI_GROK_CLI_CLIENT_ID),
    issuer_env: XAI_ISSUER_ENV,
    default_issuer: XAI_DEFAULT_ISSUER,
    authorize_path: "/oauth2/authorize",
    token_path: "/oauth2/token",
    // Historical Grok-CLI loopback registration (OpenCode abandoned this path
    // for SuperGrok in favour of device code). Kept so NOTICE/tests stay honest
    // about the published client; Codescribe does not bind this port for login.
    callback_path: "/callback",
    callback_host: "127.0.0.1",
    preferred_port: 56121,
    scope: "openid profile email offline_access grok-cli:access api:access",
    // Loopback-only extras (plan=generic). Device-code request body uses
    // client_id + scope + referrer=codescribe in `device_code.rs`.
    extra_authorize_params: &[("plan", "generic"), ("referrer", "codescribe")],
    login_flow: LoginFlow::DeviceCode,
    encoding: TokenRequestEncoding::Form,
};

/// The OAuth registry: one row per provider that can sign in with an account.
/// A provider absent from this table has no account auth — [`provider_oauth_config`]
/// answers `UnsupportedProvider`, which is the honest answer, not a panic.
const PROVIDER_OAUTH_REGISTRY: [ProviderOAuthConfig; 3] =
    [OPENAI_OAUTH, ANTHROPIC_OAUTH, XAI_OAUTH];

/// Look up a provider's registry row, or `UnsupportedProvider` if it has none.
pub fn provider_oauth_config(
    provider: ProviderKind,
) -> Result<ProviderOAuthConfig, AccountAuthError> {
    PROVIDER_OAUTH_REGISTRY
        .iter()
        .find(|row| row.provider == provider)
        .copied()
        .ok_or_else(|| AccountAuthError::UnsupportedProvider(provider.as_str().to_string()))
}

/// How long before actual expiry a token is treated as already expired, so a
/// request is never sent with a token that dies in flight.
const REFRESH_SKEW: Duration = Duration::from_secs(60);

/// Everything that can go wrong on the provider-account path.
///
/// The variants deliberately separate "not configured" from "not signed in"
/// from "the provider said no": each one asks the operator for a different next
/// action, and collapsing them would make the Keys panel unhelpful.
#[derive(Debug)]
pub enum AccountAuthError {
    /// No client id configured for a provider. Carries that provider's own
    /// setting/env keys so the message tells the operator which field to fill —
    /// naming OpenAI's keys during an Anthropic sign-in is worse than useless.
    NoClientId {
        setting: &'static str,
        env: &'static str,
    },
    /// Provider has no row in the OAuth registry — account auth simply does not
    /// exist for it. See [`provider_oauth_config`].
    UnsupportedProvider(String),
    /// No stored tokens for a provider that does support account auth.
    NotSignedIn(String),
    /// Keychain read/write or (de)serialization of the stored tokens failed.
    Storage(String),
    /// The token request never completed (transport-level).
    Http(String),
    /// The provider answered, and the answer was a refusal.
    OAuth(String),
    Io(std::io::Error),
}

impl fmt::Display for AccountAuthError {
    /// Operator-facing text; `NoClientId` names the provider's own setting/env keys.
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
    /// Lift I/O failures from the loopback server into the account-auth error enum.
    fn from(error: std::io::Error) -> Self {
        AccountAuthError::Io(error)
    }
}

/// One provider's stored OAuth tokens, serialized as JSON into the Keychain.
///
/// Expiry is persisted as an absolute Unix timestamp rather than the
/// `expires_in` the provider returns: a relative lifetime is meaningless once
/// it has been sitting in storage across an app restart.
#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AccountTokens {
    /// Provider tag, from `ProviderKind::as_str`.
    pub provider: String,
    /// Bearer credential for API calls.
    pub access_token: String,
    /// Used to mint a new access token. `None` ⇒ sign-in ends at expiry.
    pub refresh_token: Option<String>,
    /// Identity JWT, kept only for the display label. Never authorization.
    pub id_token: Option<String>,
    /// Normally `"Bearer"`; defaulted when the provider omits it.
    pub token_type: String,
    /// Absolute expiry. `None` ⇒ the provider declared no lifetime, so the
    /// token is never proactively refreshed.
    pub expires_at_unix: Option<i64>,
}

impl fmt::Debug for AccountTokens {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AccountTokens")
            .field("provider", &self.provider)
            .field("access_token_present", &!self.access_token.is_empty())
            .field("refresh_token_present", &self.refresh_token.is_some())
            .field("id_token_present", &self.id_token.is_some())
            .field("token_type", &self.token_type)
            .field("expires_at_unix", &self.expires_at_unix)
            .finish()
    }
}

impl AccountTokens {
    /// Build a token set from a provider's token response, converting the
    /// relative `expires_in` into an absolute timestamp.
    ///
    /// `token_type` defaults to `"Bearer"`; overflow on the expiry addition
    /// yields `None`, i.e. "no known expiry", never a wrapped past instant.
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

    /// Whether the token expires within `skew` of now. Tokens with no recorded
    /// expiry answer `false` — unknown lifetime is not the same as expiring.
    pub fn expires_within(&self, skew: Duration) -> bool {
        let Some(expires_at) = self.expires_at_unix else {
            return false;
        };
        let now = now_unix();
        expires_at <= now.saturating_add(skew.as_secs() as i64)
    }
}

/// What the Keys panel shows for one provider.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountAuthStatus {
    pub provider: ProviderKind,
    /// Tokens are stored for this provider.
    pub signed_in: bool,
    /// A client id resolved from settings, env, or a published default.
    pub client_id_configured: bool,
    /// Operator-facing one-liner for the current state.
    pub message: String,
    /// Who the stored id token says is signed in (email, else subject), when
    /// the token carries one. `None` while signed out or without a claim.
    pub identity: Option<String>,
}

/// Current sign-in state for one provider, safe to call for any provider.
///
/// The gate is reported before sign-in: with no client id there is nothing the
/// operator can do about tokens yet, so "awaiting app registration" outranks
/// "not signed in" as the message. Never fails — an unsupported provider simply
/// reads as not configured, not signed in.
pub fn account_status(provider: ProviderKind) -> AccountAuthStatus {
    project_account_status(provider, load_account_tokens(provider).ok())
}

/// Passive UI metadata: this never opens the credential store.
pub fn cached_account_status(provider: ProviderKind) -> AccountAuthStatus {
    match account_status_snapshot(provider) {
        Ok(status) => status,
        Err(_) => AccountAuthStatus {
            provider,
            signed_in: false,
            client_id_configured: client_id_for_provider(provider).is_ok(),
            message: "Account access unavailable. Remove the stored account and sign in again."
                .into(),
            identity: None,
        },
    }
}

/// Strict UI refresh preserves storage/corruption errors rather than sign-out.
pub fn account_status_snapshot(
    provider: ProviderKind,
) -> Result<AccountAuthStatus, AccountAuthError> {
    let tokens = match cached_account_tokens(provider) {
        Ok(tokens) => Some(tokens),
        Err(AccountAuthError::NotSignedIn(_)) => None,
        Err(error) => return Err(error),
    };
    Ok(project_account_status(provider, tokens))
}

fn project_account_status(
    provider: ProviderKind,
    tokens: Option<AccountTokens>,
) -> AccountAuthStatus {
    let client_id_configured = client_id_for_provider(provider).is_ok();
    let signed_in = tokens.is_some();
    let identity = tokens.as_ref().and_then(id_token_identity);
    let message = if !client_id_configured {
        NO_CLIENT_ID_MESSAGE.to_string()
    } else if signed_in {
        match &identity {
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
        identity,
    }
}

/// Resolved client id for a provider, or [`AccountAuthError::NoClientId`]
/// carrying that provider's own setting and env keys so the message names the
/// field the operator has to fill.
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
/// Only **public** desktop OAuth client ids may ship as `default_client_id` —
/// today OpenAI's Codex CLI app id and xAI's Grok CLI id (both disclosed in
/// `NOTICE`). Anthropic stays gated on the operator pasting their own.
fn configured_client_id_for(config: ProviderOAuthConfig) -> Option<String> {
    let settings = UserSettings::load_projection();
    (config.client_id_from_settings)(&settings)
        .and_then(non_empty_trimmed)
        .or_else(|| {
            std::env::var(config.client_id_env)
                .ok()
                .and_then(non_empty_trimmed)
        })
        .or_else(|| config.default_client_id.map(str::to_string))
}

/// Trim and reject blanks. A whitespace-only settings field is an empty one,
/// not a configured client id.
fn non_empty_trimmed(value: String) -> Option<String> {
    let value = value.trim().to_string();
    (!value.is_empty()).then_some(value)
}

/// Unverified JWT payload claims. Display and routing metadata only —
/// authorization always rides the access token itself.
fn jwt_claims(token: &str) -> Option<serde_json::Value> {
    use base64::Engine;
    let payload = token.split('.').nth(1)?;
    let decoded = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload.as_bytes())
        .ok()?;
    serde_json::from_slice(&decoded).ok()
}

/// Best-effort display identity from the id_token JWT payload (email, else
/// sub). Display-only — the claims are NOT verified here; authorization always
/// rides the access token, never this label.
fn id_token_identity(tokens: &AccountTokens) -> Option<String> {
    let claims = jwt_claims(tokens.id_token.as_deref()?)?;
    ["email", "sub"].iter().find_map(|key| {
        claims
            .get(key)
            .and_then(serde_json::Value::as_str)
            .and_then(|value| non_empty_trimmed(value.to_string()))
    })
}

/// ChatGPT workspace id a request on the vendor's own backend must carry as
/// `ChatGPT-Account-ID`. Claim path per codex-rs `token_data.rs`
/// (`https://api.openai.com/auth`.`chatgpt_account_id` on the id_token), with
/// opencode's fallbacks (top-level `chatgpt_account_id`, `organizations[0].id`),
/// then the same walk over the access token.
pub fn account_id_from_tokens(tokens: &AccountTokens) -> Option<String> {
    tokens
        .id_token
        .as_deref()
        .and_then(account_id_from_jwt)
        .or_else(|| account_id_from_jwt(&tokens.access_token))
}

fn account_id_from_jwt(token: &str) -> Option<String> {
    let claims = jwt_claims(token)?;
    let as_id = |value: Option<&serde_json::Value>| {
        value
            .and_then(serde_json::Value::as_str)
            .and_then(|id| non_empty_trimmed(id.to_string()))
    };
    as_id(
        claims
            .get("https://api.openai.com/auth")
            .and_then(|auth| auth.get("chatgpt_account_id")),
    )
    .or_else(|| as_id(claims.get("chatgpt_account_id")))
    .or_else(|| {
        as_id(
            claims
                .get("organizations")
                .and_then(|orgs| orgs.get(0))
                .and_then(|org| org.get("id")),
        )
    })
}

/// [`account_id_from_tokens`] over the stored account for `provider`; `None`
/// when not signed in or when the tokens carry no workspace claim.
pub fn account_id(provider: ProviderKind) -> Option<String> {
    load_account_tokens(provider)
        .ok()
        .and_then(|tokens| account_id_from_tokens(&tokens))
}

/// The ChatGPT account's own Responses backend (codex-rs
/// `model_provider_info.rs`: `https://chatgpt.com/backend-api/codex` under
/// `AuthMode::Chatgpt`). Account tokens are refused by `api.openai.com`, so a
/// signed-in OpenAI lane streams here. Env override for tests / proxies.
pub const CODEX_BACKEND_RESPONSES_ENDPOINT: &str =
    "https://chatgpt.com/backend-api/codex/responses";
/// Process env override for [`CODEX_BACKEND_RESPONSES_ENDPOINT`].
pub const CODEX_BACKEND_ENDPOINT_ENV: &str = "CODESCRIBE_CODEX_BACKEND_ENDPOINT";
/// `originator` header the Codex backend expects from every client.
pub const CODEX_ORIGINATOR: &str = "codescribe";

/// Where a signed-in account request must go when the vendor does not accept
/// its account token at the lane's public endpoint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountResponsesRoute {
    /// Full Responses endpoint URL for the account backend.
    pub endpoint: String,
    /// Headers that backend requires on every request.
    pub headers: Vec<(String, String)>,
}

/// The account backend for one Responses vendor, or `None` when the vendor's
/// own lane endpoint serves its account bearer token. The Codex backend keeps
/// no server-side chain: callers send `store: false`, no
/// `previous_response_id`, no temperature or output ceiling, and stream.
/// Shared by the Agent provider and the formatter so both lanes reach the same
/// backend with the same headers.
pub fn account_responses_route(provider: ProviderKind) -> Option<AccountResponsesRoute> {
    if provider != ProviderKind::OpenAiResponses {
        return None;
    }
    let mut headers = vec![("originator".to_string(), CODEX_ORIGINATOR.to_string())];
    match account_id(provider) {
        Some(account_id) => headers.push(("ChatGPT-Account-ID".to_string(), account_id)),
        None => tracing::warn!(
            "Codex backend route without a ChatGPT-Account-ID: the stored tokens carry no workspace claim"
        ),
    }
    let endpoint = std::env::var(CODEX_BACKEND_ENDPOINT_ENV)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| CODEX_BACKEND_RESPONSES_ENDPOINT.to_string());
    Some(AccountResponsesRoute { endpoint, headers })
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

/// All credential-bearing OAuth requests require TLS and never follow redirects.
/// Loopback callback listeners are a separate, inbound HTTP surface.
fn oauth_http_client(issuer: &str) -> Result<reqwest::Client, AccountAuthError> {
    let builder = reqwest::Client::builder()
        .https_only(true)
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(30));
    // Unit fixtures use numeric loopback HTTP servers. This exception is absent
    // from shipped builds, including builds with the test-isolation feature.
    #[cfg(test)]
    let builder = {
        let local_fixture = reqwest::Url::parse(issuer).ok().is_some_and(|url| {
            url.scheme() == "http"
                && url.host_str().is_some_and(|host| {
                    host.trim_matches(['[', ']'])
                        .parse::<std::net::IpAddr>()
                        .is_ok_and(|ip| ip.is_loopback())
                })
        });
        if local_fixture {
            builder.https_only(false)
        } else {
            builder
        }
    };
    #[cfg(not(test))]
    let _ = issuer;
    builder
        .build()
        .map_err(|error| AccountAuthError::Http(error.to_string()))
}

/// Persist a provider's tokens into its own Keychain slot.
pub fn store_account_tokens(
    provider: ProviderKind,
    tokens: &AccountTokens,
) -> Result<(), AccountAuthError> {
    ensure_provider_supported(provider)?;
    provider_auth_state(provider).replace_with(|| persist_account_tokens(provider, tokens))
}

/// Storage only; callers hold the provider mutation boundary.
fn persist_account_tokens(
    provider: ProviderKind,
    tokens: &AccountTokens,
) -> Result<(), AccountAuthError> {
    let account = token_account(provider)?;
    let payload = serde_json::to_string(tokens)
        .map_err(|error| AccountAuthError::Storage(error.to_string()))?;
    save_key(account, &payload).map_err(|error| AccountAuthError::Storage(error.to_string()))
}

/// Optional official Responses URL for a *capability* probe. `None` ⇒ this
/// provider has no such probe. Sign-in must not call this: ChatGPT OAuth is
/// an identity row; Responses write is a separate lane/credential. Coupling
/// them (2026-08-14) made Codex public tokens unable to save a session.
fn responses_probe_endpoint(provider: ProviderKind) -> Option<String> {
    match provider {
        // The env override exists for hermetic tests (and emergency ops):
        // account tokens only ever ride to the official endpoint, so the
        // probe defaults to the same place the runtime will send them.
        ProviderKind::OpenAiResponses => Some(
            std::env::var(RESPONSES_PROBE_URL_ENV)
                .ok()
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| ProviderKind::OpenAiResponses.endpoint().to_string()),
        ),
        _ => None,
    }
}

/// Env override for the sign-in Responses probe target. Hermetic tests point
/// it at a mock; production leaves it unset (official endpoint).
pub const RESPONSES_PROBE_URL_ENV: &str = "CODESCRIBE_RESPONSES_PROBE_URL";

/// Classify whether a token can write the official Responses API.
///
/// Not a sign-in gate. Persist is provider-local identity; this probe is a
/// later row/lane Test. Empty body: 400 = authorized, 401 = no write scope.
/// Transport errors stay fail-open.
pub async fn verify_responses_write_access(
    provider: ProviderKind,
    access_token: &str,
) -> Result<(), AccountAuthError> {
    let Some(endpoint) = responses_probe_endpoint(provider) else {
        return Ok(());
    };
    verify_responses_write_access_at(&endpoint, access_token).await
}

/// Testable core of [`verify_responses_write_access`] with an explicit endpoint.
async fn verify_responses_write_access_at(
    endpoint: &str,
    access_token: &str,
) -> Result<(), AccountAuthError> {
    let client = match reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .build()
    {
        Ok(client) => client,
        Err(error) => {
            tracing::warn!("responses-write probe unavailable ({error}); keeping sign-in");
            return Ok(());
        }
    };
    let response = match client
        .post(endpoint)
        .header("Authorization", format!("Bearer {access_token}"))
        .header("Content-Type", "application/json")
        .body("{}")
        .send()
        .await
    {
        Ok(response) => response,
        Err(error) => {
            tracing::warn!("responses-write probe did not complete ({error}); keeping sign-in");
            return Ok(());
        }
    };
    if response.status() == reqwest::StatusCode::UNAUTHORIZED {
        let body = response.text().await.unwrap_or_default();
        let detail: String = body.chars().take(300).collect();
        return Err(AccountAuthError::OAuth(format!(
            "this account token cannot use the Responses API (HTTP 401 on the \
             authorization probe — missing scope `api.responses.write`?). \
             Sign-in was not saved; use an API key or an account with \
             Responses access. Provider answer: {detail}"
        )));
    }
    Ok(())
}

/// Load a provider's stored tokens, or [`AccountAuthError::NotSignedIn`].
///
/// An env var named after the Keychain account is checked first: that is the
/// injection channel used by tests and CI, which must not touch a real Keychain.
pub fn load_account_tokens(provider: ProviderKind) -> Result<AccountTokens, AccountAuthError> {
    ensure_provider_supported(provider)?;
    let account = token_account(provider)?;
    if std::env::var(account)
        .ok()
        .filter(|value| !value.trim().is_empty())
        .is_none()
    {
        ensure_bundle_loaded().map_err(|error| AccountAuthError::Storage(format!("{error:#}")))?;
    }
    cached_account_tokens(provider)
}

fn cached_account_tokens(provider: ProviderKind) -> Result<AccountTokens, AccountAuthError> {
    ensure_provider_supported(provider)?;
    let payload = cached_runtime_key(token_account(provider)?)
        .ok_or_else(|| AccountAuthError::NotSignedIn(provider.as_str().to_string()))?;
    serde_json::from_str(&payload).map_err(|error| AccountAuthError::Storage(error.to_string()))
}

/// Sign out of one provider: delete the Keychain entry and clear the env mirror.
///
/// Both are required — leaving the env override in place would let the next
/// [`load_account_tokens`] resurrect the session the operator just ended.
pub fn clear_account_tokens(provider: ProviderKind) -> Result<(), AccountAuthError> {
    ensure_provider_supported(provider)?;
    let account = token_account(provider)?;
    provider_auth_state(provider).replace_with(|| {
        delete_key(account).map_err(|error| AccountAuthError::Storage(error.to_string()))?;
        // SAFETY: preserve the sign-out contract for the process-env mirror.
        // The provider mutation boundary also excludes its refresh and login
        // writes until both deletion and env clearing have completed.
        unsafe { std::env::remove_var(account) };
        crate::config::keychain::advance_bundle_revision();
        Ok(())
    })
}

/// Ready-to-send `Authorization` value (`Bearer <token>`), refreshing if needed.
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
    access_token_with(
        provider,
        provider_auth_state(provider),
        move || load_account_tokens(provider),
        move |tokens| refresh_tokens(provider, tokens),
        move |tokens| persist_account_tokens(provider, tokens),
    )
    .await
}

/// The production transaction with injectable I/O boundaries. Tests can use
/// memory and barriers without a second refresh algorithm or a real Keychain.
async fn access_token_with<L, R, F, P>(
    provider: ProviderKind,
    state: Arc<ProviderAuthState>,
    load: L,
    refresh: R,
    persist: P,
) -> Result<String, AccountAuthError>
where
    L: Fn() -> Result<AccountTokens, AccountAuthError> + Send + Sync + 'static,
    R: FnOnce(AccountTokens) -> F + Send + 'static,
    F: std::future::Future<Output = Result<AccountTokens, AccountAuthError>> + Send + 'static,
    P: FnOnce(&AccountTokens) -> Result<(), AccountAuthError> + Send + 'static,
{
    let load = Arc::new(load);
    let first_state = Arc::clone(&state);
    let first_load = Arc::clone(&load);
    let (tokens, _) = credential_io(move || first_state.snapshot_with(|| first_load())).await?;
    if !tokens.expires_within(REFRESH_SKEW) {
        return Ok(tokens.access_token);
    }

    // This task, not its cancellable caller, owns serialization through rotation
    // AND persistence. Sign-out need not wait for the network: its synchronous
    // mutation invalidates the captured epoch before an old write can commit.
    let transaction = tokio::spawn(async move {
        let _refresh = state.refresh.lock().await;
        let snapshot_state = Arc::clone(&state);
        let (tokens, epoch) =
            credential_io(move || snapshot_state.snapshot_with(|| load())).await?;
        if !tokens.expires_within(REFRESH_SKEW) {
            return Ok(tokens.access_token);
        }
        let refreshed = refresh(tokens).await?;
        let commit_state = Arc::clone(&state);
        credential_io(move || {
            commit_state.commit_with(provider, &epoch, || persist(&refreshed))?;
            Ok(refreshed.access_token)
        })
        .await
    });
    transaction.await.map_err(|error| {
        AccountAuthError::Storage(format!("credential refresh task ended: {error}"))
    })?
}

/// Run a Keychain operation on the blocking pool. A Keychain call can wait on
/// a macOS authorization dialog for as long as the dialog stays open; on the
/// polling thread it froze every future polled beside it, and their timeouts
/// with them. A live take polls its formatter jobs inline, so one cold read
/// stopped the whole session until the dialog closed (replay cbf54ca9,
/// 2026-10-21). Off the polling thread, the caller's own deadline still fires.
async fn credential_io<T: Send + 'static>(
    operation: impl FnOnce() -> Result<T, AccountAuthError> + Send + 'static,
) -> Result<T, AccountAuthError> {
    tokio::task::spawn_blocking(operation)
        .await
        .map_err(|error| {
            AccountAuthError::Storage(format!("credential storage task ended: {error}"))
        })?
}

/// One owner for a provider's refresh serialization and account identity.
/// Epochs contain no credentials. A retained Arc identity cannot wrap or suffer
/// ABA: the old allocation stays alive until every in-flight snapshot drops it.
#[derive(Default)]
struct ProviderAuthState {
    refresh: tokio::sync::Mutex<()>,
    mutation: Mutex<Arc<()>>,
}

impl ProviderAuthState {
    fn snapshot_with(
        &self,
        load: impl FnOnce() -> Result<AccountTokens, AccountAuthError>,
    ) -> Result<(AccountTokens, Arc<()>), AccountAuthError> {
        let epoch = self
            .mutation
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        Ok((load()?, Arc::clone(&epoch)))
    }

    /// Login and logout supersede any old refresh, even if storage reports an
    /// error. Return that original error; invalidation is not a success receipt.
    fn replace_with<T>(
        &self,
        operation: impl FnOnce() -> Result<T, AccountAuthError>,
    ) -> Result<T, AccountAuthError> {
        let mut epoch = self
            .mutation
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        *epoch = Arc::new(());
        operation()
    }

    /// The identity check and existing storage operation are one critical
    /// section. Lock order is provider mutation -> Keychain BUNDLE_IO; nothing
    /// in the Keychain owner acquires a provider mutation lock in reverse.
    fn commit_with<T>(
        &self,
        provider: ProviderKind,
        expected: &Arc<()>,
        operation: impl FnOnce() -> Result<T, AccountAuthError>,
    ) -> Result<T, AccountAuthError> {
        let epoch = self
            .mutation
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if !Arc::ptr_eq(&epoch, expected) {
            return Err(AccountAuthError::NotSignedIn(provider.as_str().to_string()));
        }
        operation()
    }
}

/// Provider rows share this state across login, sign-out and every caller.
/// The table holds only synchronization and epoch identities, never tokens.
fn provider_auth_state(provider: ProviderKind) -> Arc<ProviderAuthState> {
    use std::collections::HashMap;
    use std::sync::OnceLock;

    static STATES: OnceLock<Mutex<HashMap<ProviderKind, Arc<ProviderAuthState>>>> = OnceLock::new();
    let mut table = STATES
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    Arc::clone(table.entry(provider).or_default())
}

/// Exchange only. The detached access-token transaction owns persistence and
/// checks its captured account epoch before invoking the existing storage owner.
async fn refresh_tokens(
    provider: ProviderKind,
    tokens: AccountTokens,
) -> Result<AccountTokens, AccountAuthError> {
    let config = provider_oauth_config(provider)?;
    let refresh_token = tokens.refresh_token.ok_or_else(|| {
        AccountAuthError::OAuth("stored account has no refresh token".to_string())
    })?;
    let client_id = client_id_for_provider(provider)?;
    let issuer = issuer_for(provider);
    refresh_provider_tokens(provider, config, &issuer, &client_id, &refresh_token).await
}

/// The refresh round trip itself: POST to the provider's token endpoint in its
/// own body encoding and parse the response. Stores nothing.
async fn refresh_provider_tokens(
    provider: ProviderKind,
    config: ProviderOAuthConfig,
    issuer: &str,
    client_id: &str,
    refresh_token: &str,
) -> Result<AccountTokens, AccountAuthError> {
    /// Token-endpoint response. Every field but `access_token` is optional —
    /// providers differ in what they return on refresh.
    #[derive(Deserialize)]
    struct RefreshResponse {
        access_token: String,
        refresh_token: Option<String>,
        id_token: Option<String>,
        token_type: Option<String>,
        expires_in: Option<u64>,
    }

    let endpoint = format!("{}{}", issuer.trim_end_matches('/'), config.token_path);
    let client = oauth_http_client(issuer)?;
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

/// Keychain account name for a provider's tokens.
fn token_account(provider: ProviderKind) -> Result<&'static str, AccountAuthError> {
    Ok(provider_oauth_config(provider)?.tokens_account)
}

/// Guard that a provider has a registry row, discarding the row itself.
fn ensure_provider_supported(provider: ProviderKind) -> Result<(), AccountAuthError> {
    provider_oauth_config(provider).map(|_| ())
}

/// Current Unix time in seconds. A clock before the epoch yields `0`, which
/// reads as "already expired" and triggers a refresh — the safe direction.
fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

/// Unit tests for client-id resolution, keychain isolation, and registry shape.
#[cfg(test)]
mod tests {
    #[test]
    #[serial]
    fn credential_projection_malformed_account_is_unknown_and_preserves_other_credentials() {
        let (_settings_guard, _directory) = isolated_settings_dir("credential_projection");
        let _key = EnvGuard::remove("LLM_OPENAI_API_KEY");
        let _stt = EnvGuard::remove("STT_FILE_API_KEY");
        let _tokens = EnvGuard::remove(OPENAI_ACCOUNT_TOKENS_ACCOUNT);
        let _cache = crate::config::keychain::test_support::install_bundle(&[
            (
                OPENAI_ACCOUNT_TOKENS_ACCOUNT,
                "{synthetic-private-token-json",
            ),
            ("LLM_OPENAI_API_KEY", "synthetic-independent-api-key"),
            ("STT_FILE_API_KEY", "synthetic-independent-stt-key"),
        ]);
        let probe = crate::config::keychain::CredentialAcquisitionProbe::forbid();
        let strict = account_status_snapshot(ProviderKind::OpenAiResponses);
        assert!(matches!(strict, Err(AccountAuthError::Storage(_))));
        let display = cached_account_status(ProviderKind::OpenAiResponses);
        assert!(!display.signed_in);
        assert_eq!(
            display.message,
            "Account access unavailable. Remove the stored account and sign in again."
        );
        assert!(!display.message.contains("synthetic-private-token-json"));
        assert!(crate::config::keychain::key_present("LLM_OPENAI_API_KEY"));
        assert!(crate::config::keychain::key_present("STT_FILE_API_KEY"));
        assert!(probe.attempts().is_empty());
    }
    use super::*;
    use serial_test::serial;

    // Exercise the real refresh transaction with memory-only I/O. None of these
    // witnesses reads environment credentials, opens the Keychain or uses HTTP.
    mod mutation_races {
        use super::*;
        use std::sync::atomic::{AtomicUsize, Ordering};

        const PROVIDER: ProviderKind = ProviderKind::OpenAiResponses;

        fn tokens(access: &str, expired: bool) -> AccountTokens {
            let mut tokens = AccountTokens::new(
                PROVIDER,
                access.into(),
                Some("synthetic-refresh".into()),
                None,
                None,
                None,
            );
            tokens.expires_at_unix = expired.then_some(0);
            tokens
        }

        type Store = Arc<Mutex<Option<AccountTokens>>>;

        fn load(store: &Store) -> Result<AccountTokens, AccountAuthError> {
            store
                .lock()
                .unwrap()
                .clone()
                .ok_or_else(|| AccountAuthError::NotSignedIn(PROVIDER.as_str().into()))
        }

        #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
        async fn caller_cancellation_keeps_refresh_serialized_until_blocking_persist_finishes() {
            let state = Arc::new(ProviderAuthState::default());
            let store = Arc::new(Mutex::new(Some(tokens("old", true))));
            let rotations = Arc::new(AtomicUsize::new(0));
            let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
            let (release_tx, release_rx) = std::sync::mpsc::channel();
            let first = tokio::spawn(access_token_with(
                PROVIDER,
                Arc::clone(&state),
                {
                    let store = Arc::clone(&store);
                    move || load(&store)
                },
                {
                    let rotations = Arc::clone(&rotations);
                    move |_| async move {
                        rotations.fetch_add(1, Ordering::SeqCst);
                        Ok(tokens("rotated", false))
                    }
                },
                {
                    let store = Arc::clone(&store);
                    move |tokens| {
                        let _ = entered_tx.send(());
                        release_rx.recv_timeout(Duration::from_secs(5)).unwrap();
                        *store.lock().unwrap() = Some(tokens.clone());
                        Ok(())
                    }
                },
            ));
            tokio::time::timeout(Duration::from_secs(5), entered_rx)
                .await
                .unwrap()
                .unwrap();
            first.abort();
            assert!(first.await.unwrap_err().is_cancelled());
            assert!(
                state.refresh.try_lock().is_err(),
                "rotation still owns serialization"
            );
            assert!(
                state.mutation.try_lock().is_err(),
                "persistence is inside mutation"
            );

            let (second_started, started) = tokio::sync::oneshot::channel();
            let second = tokio::spawn({
                let state = Arc::clone(&state);
                let store = Arc::clone(&store);
                let rotations = Arc::clone(&rotations);
                async move {
                    let _ = second_started.send(());
                    let read_store = Arc::clone(&store);
                    access_token_with(
                        PROVIDER,
                        state,
                        move || load(&read_store),
                        move |_| async move {
                            rotations.fetch_add(1, Ordering::SeqCst);
                            Ok(tokens("unexpected-second-rotation", false))
                        },
                        move |tokens| {
                            *store.lock().unwrap() = Some(tokens.clone());
                            Ok(())
                        },
                    )
                    .await
                }
            });
            started.await.unwrap();
            assert!(!second.is_finished());
            release_tx.send(()).unwrap();
            assert_eq!(
                tokio::time::timeout(Duration::from_secs(5), second)
                    .await
                    .unwrap()
                    .unwrap()
                    .unwrap(),
                "rotated"
            );
            assert_eq!(rotations.load(Ordering::SeqCst), 1);
            assert_eq!(load(&store).unwrap().access_token, "rotated");
        }

        #[tokio::test]
        async fn signout_during_network_rotation_cannot_be_undone_by_stale_persistence() {
            let state = Arc::new(ProviderAuthState::default());
            let store = Arc::new(Mutex::new(Some(tokens("old", true))));
            let writes = Arc::new(AtomicUsize::new(0));
            let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
            let (release_tx, release_rx) = tokio::sync::oneshot::channel();
            let refresh = tokio::spawn(access_token_with(
                PROVIDER,
                Arc::clone(&state),
                {
                    let store = Arc::clone(&store);
                    move || load(&store)
                },
                move |_| async move {
                    let _ = entered_tx.send(());
                    release_rx.await.unwrap();
                    Ok(tokens("stale-rotation", false))
                },
                {
                    let store = Arc::clone(&store);
                    let writes = Arc::clone(&writes);
                    move |tokens| {
                        writes.fetch_add(1, Ordering::SeqCst);
                        *store.lock().unwrap() = Some(tokens.clone());
                        Ok(())
                    }
                },
            ));
            tokio::time::timeout(Duration::from_secs(5), entered_rx)
                .await
                .unwrap()
                .unwrap();
            state
                .replace_with(|| {
                    *store.lock().unwrap() = None;
                    Ok(())
                })
                .unwrap();
            release_tx.send(()).unwrap();
            let result = tokio::time::timeout(Duration::from_secs(5), refresh)
                .await
                .unwrap()
                .unwrap();
            assert!(matches!(result, Err(AccountAuthError::NotSignedIn(_))));
            assert!(store.lock().unwrap().is_none());
            assert_eq!(writes.load(Ordering::SeqCst), 0);
        }

        #[test]
        fn signout_waits_for_an_already_committing_refresh_then_removes_it() {
            let state = Arc::new(ProviderAuthState::default());
            let store = Arc::new(Mutex::new(Some(tokens("old", true))));
            let (_, epoch) = state.snapshot_with(|| load(&store)).unwrap();
            let (entered_tx, entered_rx) = std::sync::mpsc::channel();
            let (release_tx, release_rx) = std::sync::mpsc::channel();
            let refresh = std::thread::spawn({
                let state = Arc::clone(&state);
                let store = Arc::clone(&store);
                move || {
                    state.commit_with(PROVIDER, &epoch, || {
                        entered_tx.send(()).unwrap();
                        release_rx.recv_timeout(Duration::from_secs(5)).unwrap();
                        *store.lock().unwrap() = Some(tokens("rotated", false));
                        Ok(())
                    })
                }
            });
            entered_rx.recv_timeout(Duration::from_secs(5)).unwrap();
            assert!(state.mutation.try_lock().is_err());
            let clear = std::thread::spawn({
                let state = Arc::clone(&state);
                let store = Arc::clone(&store);
                move || {
                    state.replace_with(|| {
                        *store.lock().unwrap() = None;
                        Ok(())
                    })
                }
            });
            release_tx.send(()).unwrap();
            refresh.join().unwrap().unwrap();
            clear.join().unwrap().unwrap();
            assert!(store.lock().unwrap().is_none());
        }

        #[test]
        fn identical_relogin_values_do_not_revive_the_previous_account_epoch() {
            let state = ProviderAuthState::default();
            let store = Arc::new(Mutex::new(Some(tokens("same-value", true))));
            let (old, epoch) = state.snapshot_with(|| load(&store)).unwrap();
            state
                .replace_with(|| {
                    *store.lock().unwrap() = None;
                    Ok(())
                })
                .unwrap();
            state
                .replace_with(|| {
                    *store.lock().unwrap() = Some(old.clone());
                    Ok(())
                })
                .unwrap();
            let stale = state.commit_with(PROVIDER, &epoch, || {
                *store.lock().unwrap() = Some(tokens("stale-rotation", false));
                Ok(())
            });
            assert!(matches!(stale, Err(AccountAuthError::NotSignedIn(_))));
            assert_eq!(load(&store).unwrap(), old);
        }

        #[test]
        fn failed_explicit_mutation_preserves_storage_error_but_invalidates_old_refresh() {
            let state = ProviderAuthState::default();
            let (_, epoch) = state.snapshot_with(|| Ok(tokens("old", true))).unwrap();
            let failure = state.replace_with::<()>(|| {
                Err(AccountAuthError::Storage("synthetic-delete-refusal".into()))
            });
            assert!(matches!(failure, Err(AccountAuthError::Storage(ref reason))
                if reason == "synthetic-delete-refusal"));
            let stale: Result<(), AccountAuthError> = state.commit_with(PROVIDER, &epoch, || {
                panic!("a failed explicit mutation still supersedes the old refresh")
            });
            assert!(matches!(stale, Err(AccountAuthError::NotSignedIn(_))));
        }
    }

    /// Point the settings store at an isolated scratch dir so these tests never
    /// read (or depend on) the operator's real settings.json.
    fn isolated_settings_dir(tag: &str) -> (EnvGuard, tempfile::TempDir) {
        let dir = tempfile::Builder::new()
            .prefix(&format!("cs_account_auth_{tag}_"))
            .tempdir()
            .expect("create scratch settings dir");
        (EnvGuard::set("CODESCRIBE_DATA_DIR", dir.path()), dir)
    }

    /// Capability probe only: 401 means no Responses write; 400 means the
    /// token authorized. Login persist does not call this.
    #[tokio::test]
    async fn responses_probe_rejects_401_and_passes_validation_400() {
        let mut starved_server = mockito::Server::new_async().await;
        let starved = starved_server
            .mock("POST", "/v1/responses")
            .with_status(401)
            .with_body(r#"{"error":"Missing scopes: api.responses.write"}"#)
            .create_async()
            .await;
        let starved_url = format!("{}/v1/responses", starved_server.url());
        let error = verify_responses_write_access_at(&starved_url, "starved-token")
            .await
            .expect_err("401 classifies as no Responses write");
        assert!(error.to_string().contains("api.responses.write"));
        starved.assert_async().await;

        let mut healthy_server = mockito::Server::new_async().await;
        let healthy = healthy_server
            .mock("POST", "/v1/responses")
            .with_status(400)
            .with_body(r#"{"error":"invalid input"}"#)
            .create_async()
            .await;
        let healthy_url = format!("{}/v1/responses", healthy_server.url());
        verify_responses_write_access_at(&healthy_url, "healthy-token")
            .await
            .expect("400 validation answer proves the scope");
        healthy.assert_async().await;
    }

    /// Missing client id must surface the shared registration-gate message and
    /// the failing provider's own setting/env keys (not another provider's).
    /// Anthropic is the remaining no-default row; OpenAI and xAI ship public ids.
    #[test]
    #[serial]
    fn no_client_id_reports_registration_gate() {
        let (_data_dir, _dir) = isolated_settings_dir("gate");
        // Pin the Anthropic env: the operator's dotenv is inherited by the
        // test process, so an unpinned var makes this pass or fail by machine.
        let _anthropic_guard = EnvGuard::remove(ANTHROPIC_CLIENT_ID_ENV);
        let anthropic = client_id_for_provider(ProviderKind::AnthropicMessages).unwrap_err();
        assert!(matches!(anthropic, AccountAuthError::NoClientId { .. }));
        assert!(anthropic.to_string().contains(NO_CLIENT_ID_MESSAGE));
        // The message must name *this* provider's fields, not OpenAI's by reflex.
        assert!(anthropic.to_string().contains(ANTHROPIC_CLIENT_ID_SETTING));
        assert!(anthropic.to_string().contains(ANTHROPIC_CLIENT_ID_ENV));
        // OpenAI is never gated when unset — Codex public app id is the default.
        assert_eq!(
            client_id_for_provider(ProviderKind::OpenAiResponses).unwrap(),
            OPENAI_CODEX_CLI_CLIENT_ID
        );
    }

    /// Settings.json client id wins over env and is re-read on every call so a
    /// Keys-panel save applies without restarting the process.
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

    /// Keys panel states for providers that ship a default client id (OpenAI):
    /// not signed in (login enabled via Codex default) → signed in as <email>.
    /// Anthropic still has the registration gate; that path is covered by
    /// `no_client_id_reports_registration_gate`.
    #[test]
    #[serial]
    fn account_status_maps_gate_then_not_signed_in_then_signed_in() {
        use base64::Engine;
        let (_data_dir, _dir) = isolated_settings_dir("status");
        let _disable = EnvGuard::set("CODESCRIBE_DISABLE_KEYCHAIN", "1");
        let _tokens = EnvGuard::remove(OPENAI_ACCOUNT_TOKENS_ACCOUNT);
        let _env = EnvGuard::remove(OPENAI_CLIENT_ID_ENV);

        // 1. OpenAI with no operator paste ⇒ Codex default is configured,
        //    sign-in is enabled, tokens absent.
        let status = account_status(ProviderKind::OpenAiResponses);
        assert!(status.client_id_configured);
        assert!(!status.signed_in);
        assert_eq!(status.message, "not signed in");
        assert_eq!(status.identity, None);

        // 2. Operator override still applies mid-process (settings win).
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
        assert_eq!(
            client_id_for_provider(ProviderKind::OpenAiResponses).unwrap(),
            "app_registered"
        );

        // 3. Stored tokens with an id_token email ⇒ signed in as <email>.
        let claims = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(r#"{"email":"user@example.com"}"#);
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
        assert_eq!(status.message, "signed in as user@example.com");
        assert_eq!(status.identity.as_deref(), Some("user@example.com"));
    }

    /// Identity for the Keys panel comes from the id_token email claim when present.
    #[test]
    fn signed_in_status_carries_the_id_token_email_when_present() {
        use base64::Engine;
        let claims = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(r#"{"email":"user@example.com","sub":"user-123"}"#);
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
            Some("user@example.com")
        );

        let no_id_token = AccountTokens {
            id_token: None,
            ..tokens
        };
        assert_eq!(id_token_identity(&no_id_token), None);
    }

    /// Mock keychain path stores and reloads JSON [`AccountTokens`] unchanged.
    #[test]
    #[serial]
    fn keychain_mock_round_trips_serialized_account_tokens() {
        let _disable = EnvGuard::set("CODESCRIBE_DISABLE_KEYCHAIN", "1");
        let _tokens = EnvGuard::remove(OPENAI_ACCOUNT_TOKENS_ACCOUNT);
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

    /// The thread polling `access_token` never performs the Keychain read:
    /// that read can wait on an authorization dialog, and a live take polls
    /// its formatter jobs inline (replay cbf54ca9 stopped for 13 minutes).
    #[tokio::test(flavor = "current_thread")]
    #[serial]
    async fn access_token_reads_credentials_off_the_polling_thread() {
        let _tokens = EnvGuard::remove(OPENAI_ACCOUNT_TOKENS_ACCOUNT);
        let _bundle = crate::config::keychain::test_support::install_bundle(&[]);
        let probe = crate::config::keychain::CredentialAcquisitionProbe::forbid();
        let _ = access_token(ProviderKind::OpenAiResponses).await;
        assert!(probe.attempts().is_empty(), "{:?}", probe.attempts());
    }

    /// Each provider uses a distinct keychain account so tokens never cross-wire.
    #[test]
    #[serial]
    fn provider_accounts_never_share_a_keychain_slot() {
        let _disable = EnvGuard::set("CODESCRIBE_DISABLE_KEYCHAIN", "1");
        let _openai = EnvGuard::remove(OPENAI_ACCOUNT_TOKENS_ACCOUNT);
        let _anthropic = EnvGuard::remove(ANTHROPIC_ACCOUNT_TOKENS_ACCOUNT);

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

    /// Client id and issuer resolution is per-provider; sibling env must not leak.
    #[test]
    #[serial]
    fn each_provider_reads_its_own_client_id_and_issuer() {
        let (_settings_guard, _dir) = isolated_settings_dir("provider_identity");
        let _openai_env = EnvGuard::remove(OPENAI_CLIENT_ID_ENV);
        let _anthropic_env = EnvGuard::set(ANTHROPIC_CLIENT_ID_ENV, "anthropic-from-env");
        let _issuer = EnvGuard::remove(ANTHROPIC_ISSUER_ENV);

        // OpenAI has neither setting nor env ⇒ Codex public app id, never
        // Anthropic's env value.
        assert_eq!(
            client_id_for_provider(ProviderKind::OpenAiResponses).unwrap(),
            OPENAI_CODEX_CLI_CLIENT_ID
        );
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
            assert_eq!(before, sorted.len(), "duplicate OAuth registry identifier");
        }
    }

    /// Every provider that can be selected in Settings must resolve to a row —
    /// or fail loudly. A silent `UnsupportedProvider` on a provider the picker
    /// offers is a dead sign-in button with no explanation.
    #[test]
    fn every_oauth_row_names_a_selectable_provider() {
        // OAuth is optional per vendor (Libraxis is key-only): every OAuth row must
        // point at a selectable vendor, and a vendor without a row still resolves.
        use crate::llm::provider::{ALL_PROVIDERS, ProviderKind, ProviderRef, ProviderRegistry};
        for row in PROVIDER_OAUTH_REGISTRY {
            assert!(
                ALL_PROVIDERS.contains(&row.provider),
                "{} has an OAuth row but is not selectable",
                row.provider
            );
        }
        let libraxis = ProviderRegistry::new(Vec::new())
            .resolve(&ProviderRef::Vendor(ProviderKind::LibraxisResponses))
            .expect("Libraxis resolves without an OAuth row");
        assert_eq!(libraxis.oauth_vendor, None);
        assert!(libraxis.key_required);
    }

    /// Client-id resolution reads the row's own accessor, so each provider sees
    /// only its own saved value — no cross-reads through a shared settings key.
    #[test]
    #[serial]
    fn each_row_reads_only_its_own_saved_client_id() {
        let (_data_dir, _dir) = isolated_settings_dir("row_accessor");
        let _openai_env = EnvGuard::remove(OPENAI_CLIENT_ID_ENV);
        let _anthropic_env = EnvGuard::remove(ANTHROPIC_CLIENT_ID_ENV);

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

    /// A shipped `default_client_id` is a deliberate, disclosed decision — never
    /// a silent private registration. This is the allowlist: any row that grows
    /// a default without being added here fails, which is exactly the copy-paste
    /// this guard exists to catch.
    ///
    /// OpenAI: Codex CLI public `app_…` id. xAI: Grok CLI public UUID. Both are
    /// disclosed in `NOTICE`. Anthropic has no shipped default.
    #[test]
    fn only_vendor_published_client_ids_ship_by_default() {
        for row in PROVIDER_OAUTH_REGISTRY {
            let expected = match row.provider {
                ProviderKind::OpenAiResponses => Some(OPENAI_CODEX_CLI_CLIENT_ID),
                ProviderKind::XaiResponses => Some(XAI_GROK_CLI_CLIENT_ID),
                _ => None,
            };
            assert_eq!(
                row.default_client_id, expected,
                "{} ships an undisclosed client id",
                row.provider
            );
        }
    }

    /// OpenAI sign-in must not require a Keys-panel paste when nothing is
    /// configured — the Codex public app id is the shipped default.
    #[test]
    #[serial]
    fn openai_resolves_codex_cli_client_id_without_settings() {
        let (_data_dir, _dir) = isolated_settings_dir("openai_default_client_id");
        let _openai_env = EnvGuard::remove(OPENAI_CLIENT_ID_ENV);
        UserSettings {
            openai_oauth_client_id: None,
            ..Default::default()
        }
        .save()
        .expect("persist empty openai client id");
        assert_eq!(
            client_id_for_provider(ProviderKind::OpenAiResponses).unwrap(),
            OPENAI_CODEX_CLI_CLIENT_ID
        );
    }

    /// xAI registered its published client against one exact redirect URI. A
    /// drifted host, port, or path is not a cosmetic difference — the authorize
    /// step rejects it, and the failure surfaces as a dead sign-in button.
    #[test]
    fn xai_row_matches_the_registered_grok_cli_redirect() {
        let row = provider_oauth_config(ProviderKind::XaiResponses).unwrap();
        assert_eq!(row.callback_host, "127.0.0.1");
        assert_eq!(row.preferred_port, 56121);
        assert_eq!(row.callback_path, "/callback");
        assert_eq!(row.default_issuer, "https://auth.x.ai");
        assert_eq!(row.authorize_path, "/oauth2/authorize");
        assert_eq!(row.token_path, "/oauth2/token");
        // Product sign-in is device-code (OpenCode SuperGrok); loopback metadata
        // is retained only as the published client's historical registration.
        assert_eq!(row.login_flow, LoginFlow::DeviceCode);
        assert_eq!(row.encoding, TokenRequestEncoding::Form);
        // The api:access scope is what makes the token usable for inference;
        // dropping it yields a token that signs in but cannot call the model.
        assert!(row.scope.contains("api:access"));
        assert!(row.scope.contains("grok-cli:access"));
        assert!(row.scope.contains("offline_access"));
        // Loopback extras retained for any future authorize URL; device path
        // attributes via referrer in the device-code form body.
        assert!(
            row.extra_authorize_params
                .iter()
                .any(|(k, v)| *k == "plan" && *v == "generic"),
            "xAI authorize extras still carry plan=generic (Grok-CLI contract)"
        );
        assert!(
            row.extra_authorize_params
                .iter()
                .any(|(k, v)| *k == "referrer" && *v == "codescribe"),
            "xAI authorize extras should attribute Codescribe as referrer"
        );
    }

    /// RAII env mutator for `#[serial]` tests; restores the prior value on drop.
    use crate::test_isolation::EnvGuard;

    /// The Codex backend needs the workspace id from the token claims; the
    /// codex-rs path wins, the opencode fallbacks follow, and a token with no
    /// claim yields `None` rather than an empty header.
    #[test]
    fn account_id_walks_the_codex_and_opencode_claim_paths() {
        use base64::Engine;
        let jwt = |payload: &str| {
            format!(
                "h.{}.s",
                base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(payload)
            )
        };
        let tokens = |access: &str, id: Option<&str>| {
            AccountTokens::new(
                ProviderKind::OpenAiResponses,
                access.to_string(),
                None,
                id.map(str::to_string),
                None,
                None,
            )
        };
        let codex = jwt(
            r#"{"email":"u@example.com","https://api.openai.com/auth":{"chatgpt_account_id":"acct_codex"}}"#,
        );
        assert_eq!(
            account_id_from_tokens(&tokens("opaque", Some(&codex))).as_deref(),
            Some("acct_codex")
        );
        let flat = jwt(r#"{"chatgpt_account_id":"acct_flat"}"#);
        assert_eq!(
            account_id_from_tokens(&tokens("opaque", Some(&flat))).as_deref(),
            Some("acct_flat")
        );
        let orgs = jwt(r#"{"organizations":[{"id":"org_first"},{"id":"org_second"}]}"#);
        assert_eq!(
            account_id_from_tokens(&tokens("opaque", Some(&orgs))).as_deref(),
            Some("org_first")
        );
        // No id_token: the access token's claims are walked the same way.
        let access = jwt(r#"{"https://api.openai.com/auth":{"chatgpt_account_id":"acct_access"}}"#);
        assert_eq!(
            account_id_from_tokens(&tokens(&access, None)).as_deref(),
            Some("acct_access")
        );
        assert_eq!(
            account_id_from_tokens(&tokens("opaque", Some(&jwt(r#"{"email":"x"}"#)))),
            None
        );
        assert_eq!(account_id_from_tokens(&tokens("not-a-jwt", None)), None);
    }
}
