//! Provider-account authentication foundation for future "Sign in with ChatGPT".
//!
//! Tokens are stored as serialized JSON in the existing CodeScribe Keychain
//! bundle under a provider-specific account. No `auth.json` file is written.

use std::fmt;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::config::keychain::{delete_key, load_key, save_key};
use crate::llm::provider::ProviderKind;

pub mod device_code;
pub mod pkce;
pub mod server;

pub use device_code::{
    DeviceAuthConfig, DeviceCode, complete_device_code_login, request_device_code,
};
pub use pkce::{PkceCodes, challenge_for_verifier, generate_pkce};
pub use server::{LoginServer, ServerOptions, exchange_code_for_tokens, run_login_server};

pub const OPENAI_ACCOUNT_TOKENS_ACCOUNT: &str = "LLM_OPENAI_ACCOUNT_TOKENS";
pub const OPENAI_CLIENT_ID_ENV: &str = "CODESCRIBE_OPENAI_OAUTH_CLIENT_ID";
pub const OPENAI_ISSUER_ENV: &str = "CODESCRIBE_OPENAI_OAUTH_ISSUER";
pub const DEFAULT_ISSUER: &str = "https://auth.openai.com";
pub const NO_CLIENT_ID_MESSAGE: &str = "awaiting app registration";

const REFRESH_SKEW: Duration = Duration::from_secs(60);

#[derive(Debug)]
pub enum AccountAuthError {
    NoClientId,
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
            AccountAuthError::NoClientId => write!(
                f,
                "{NO_CLIENT_ID_MESSAGE}; set {OPENAI_CLIENT_ID_ENV} after app registration"
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
    let signed_in = load_account_tokens(provider).is_ok();
    let message = if !client_id_configured {
        NO_CLIENT_ID_MESSAGE.to_string()
    } else if signed_in {
        "signed in".to_string()
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
    ensure_provider_supported(provider)?;
    std::env::var(OPENAI_CLIENT_ID_ENV)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .ok_or(AccountAuthError::NoClientId)
}

pub fn issuer_from_env() -> String {
    std::env::var(OPENAI_ISSUER_ENV)
        .ok()
        .map(|value| value.trim().trim_end_matches('/').to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| DEFAULT_ISSUER.to_string())
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
    unsafe { std::env::remove_var(account) };
    Ok(())
}

pub async fn authorization_header(provider: ProviderKind) -> Result<String, AccountAuthError> {
    let mut tokens = load_account_tokens(provider)?;
    if tokens.expires_within(REFRESH_SKEW) {
        tokens = refresh_tokens(provider, tokens).await?;
    }
    Ok(format!("Bearer {}", tokens.access_token))
}

pub async fn refresh_tokens(
    provider: ProviderKind,
    tokens: AccountTokens,
) -> Result<AccountTokens, AccountAuthError> {
    ensure_provider_supported(provider)?;
    let refresh_token = tokens.refresh_token.ok_or_else(|| {
        AccountAuthError::OAuth("stored account has no refresh token".to_string())
    })?;
    let client_id = client_id_for_provider(provider)?;
    let issuer = issuer_from_env();
    let refreshed = refresh_openai_tokens(&issuer, &client_id, &refresh_token).await?;
    store_account_tokens(provider, &refreshed)?;
    Ok(refreshed)
}

async fn refresh_openai_tokens(
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

    let client = reqwest::Client::new();
    let response = client
        .post(format!("{}/oauth/token", issuer.trim_end_matches('/')))
        .form(&[
            ("grant_type", "refresh_token"),
            ("client_id", client_id),
            ("refresh_token", refresh_token),
        ])
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
        ProviderKind::OpenAiResponses,
        body.access_token,
        body.refresh_token.or(Some(refresh_token.to_string())),
        body.id_token,
        body.token_type,
        body.expires_in,
    ))
}

fn token_account(provider: ProviderKind) -> Result<&'static str, AccountAuthError> {
    match provider {
        ProviderKind::OpenAiResponses => Ok(OPENAI_ACCOUNT_TOKENS_ACCOUNT),
        ProviderKind::AnthropicMessages => Err(AccountAuthError::UnsupportedProvider(
            provider.as_str().to_string(),
        )),
    }
}

fn ensure_provider_supported(provider: ProviderKind) -> Result<(), AccountAuthError> {
    token_account(provider).map(|_| ())
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

    #[test]
    #[serial]
    fn no_client_id_reports_registration_gate() {
        let _guard = EnvGuard::unset(OPENAI_CLIENT_ID_ENV);
        let err = client_id_for_provider(ProviderKind::OpenAiResponses).unwrap_err();
        assert!(matches!(err, AccountAuthError::NoClientId));
        assert!(err.to_string().contains(NO_CLIENT_ID_MESSAGE));
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

    #[derive(Debug)]
    struct EnvGuard {
        key: &'static str,
        previous: Option<String>,
    }

    impl EnvGuard {
        fn set(key: &'static str, value: &str) -> Self {
            let previous = std::env::var(key).ok();
            unsafe { std::env::set_var(key, value) };
            Self { key, previous }
        }

        fn unset(key: &'static str) -> Self {
            let previous = std::env::var(key).ok();
            unsafe { std::env::remove_var(key) };
            Self { key, previous }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            match &self.previous {
                Some(value) => unsafe { std::env::set_var(self.key, value) },
                None => unsafe { std::env::remove_var(self.key) },
            }
        }
    }
}
