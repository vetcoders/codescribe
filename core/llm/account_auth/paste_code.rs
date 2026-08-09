//! Paste-code OAuth: the authorization-code variant where the provider shows
//! the code on its own page instead of redirecting to a loopback listener.
//!
//! Anthropic's console flow works this way — the redirect target is a page on
//! `console.anthropic.com` that renders `"<code>#<state>"` for the user to copy
//! back into the app. That makes it the one flow with no local HTTP server:
//! [`server::run_login_server`] handles the loopback providers instead.
//!
//! The `#` is a real separator, not a URL fragment marker: providers using this
//! shape return CSRF state glued to the code so a single paste carries both.
//! We verify the state here rather than trusting the paste, because a pasted
//! string is exactly the surface where a user can be socially engineered into
//! completing someone else's authorization.

use crate::llm::account_auth::{
    AccountAuthError, AccountTokens, PkceCodes, ProviderOAuthConfig, TokenRequestEncoding,
};
use crate::llm::provider::ProviderKind;

/// Authorization URL for a paste-code provider. `scope` is provider-defined;
/// `state` is verified against what comes back inside the pasted code.
pub fn build_authorize_url(
    authorize_base: &str,
    client_id: &str,
    redirect_uri: &str,
    scope: &str,
    pkce: &PkceCodes,
    state: &str,
) -> Result<String, AccountAuthError> {
    let mut url = reqwest::Url::parse(authorize_base)
        .map_err(|error| AccountAuthError::OAuth(format!("invalid authorize URL: {error}")))?;
    url.query_pairs_mut()
        .append_pair("code", "true")
        .append_pair("client_id", client_id)
        .append_pair("response_type", "code")
        .append_pair("redirect_uri", redirect_uri)
        .append_pair("scope", scope)
        .append_pair("code_challenge", &pkce.code_challenge)
        .append_pair("code_challenge_method", "S256")
        .append_pair("state", state);
    Ok(url.to_string())
}

/// Split a pasted `"<code>#<state>"` and verify the state matches the one this
/// app generated. A paste without `#` is accepted only when it is the bare code
/// and the caller has no state to check (`expected_state` empty) — otherwise a
/// missing state is a rejection, never a shrug.
pub fn split_pasted_code<'a>(
    pasted: &'a str,
    expected_state: &str,
) -> Result<&'a str, AccountAuthError> {
    let pasted = pasted.trim();
    if pasted.is_empty() {
        return Err(AccountAuthError::OAuth("pasted code is empty".to_string()));
    }
    match pasted.split_once('#') {
        Some((code, state)) => {
            let (code, state) = (code.trim(), state.trim());
            if code.is_empty() {
                return Err(AccountAuthError::OAuth(
                    "pasted value has no code before '#'".to_string(),
                ));
            }
            if state != expected_state {
                return Err(AccountAuthError::OAuth(
                    "pasted code carries a different state than this login started with; \
                     discard it and sign in again"
                        .to_string(),
                ));
            }
            Ok(code)
        }
        None if expected_state.is_empty() => Ok(pasted),
        None => Err(AccountAuthError::OAuth(
            "pasted code is missing its '#state' suffix; copy the whole value".to_string(),
        )),
    }
}

/// One paste-code exchange: the login attempt's identity (provider, endpoint,
/// client) plus the secrets that must match it (`pkce`, `expected_state`).
/// Grouped so the verifier and the state can never drift apart from the
/// authorize call that produced them.
pub struct PasteCodeExchange<'a> {
    /// Which provider the resulting tokens belong to.
    pub provider: ProviderKind,
    /// Token path and request encoding for this provider.
    pub config: ProviderOAuthConfig,
    /// Issuer base URL; the token path is appended to it.
    pub issuer: &'a str,
    /// OAuth client id used for the authorize call.
    pub client_id: &'a str,
    /// Redirect URI registered for the flow — must match the authorize call.
    pub redirect_uri: &'a str,
    /// PKCE pair whose verifier proves this app started the login.
    pub pkce: &'a PkceCodes,
    /// CSRF state to check against the one glued to the pasted code.
    pub expected_state: &'a str,
}

/// Exchange a pasted authorization code for tokens.
pub async fn exchange_pasted_code(
    exchange: PasteCodeExchange<'_>,
    pasted: &str,
) -> Result<AccountTokens, AccountAuthError> {
    let PasteCodeExchange {
        provider,
        config,
        issuer,
        client_id,
        redirect_uri,
        pkce,
        expected_state,
    } = exchange;
    /// The subset of the token-endpoint payload this flow consumes.
    #[derive(serde::Deserialize)]
    struct TokenResponse {
        access_token: String,
        refresh_token: Option<String>,
        id_token: Option<String>,
        token_type: Option<String>,
        expires_in: Option<u64>,
    }

    let code = split_pasted_code(pasted, expected_state)?;
    let endpoint = format!("{}{}", issuer.trim_end_matches('/'), config.token_path);
    let request = reqwest::Client::new().post(endpoint);
    let request = match config.encoding {
        TokenRequestEncoding::Json => request.json(&serde_json::json!({
            "grant_type": "authorization_code",
            "code": code,
            "state": expected_state,
            "redirect_uri": redirect_uri,
            "client_id": client_id,
            "code_verifier": pkce.code_verifier,
        })),
        TokenRequestEncoding::Form => request.form(&[
            ("grant_type", "authorization_code"),
            ("code", code),
            ("state", expected_state),
            ("redirect_uri", redirect_uri),
            ("client_id", client_id),
            ("code_verifier", pkce.code_verifier.as_str()),
        ]),
    };

    let response = request
        .send()
        .await
        .map_err(|error| AccountAuthError::Http(error.to_string()))?;
    if !response.status().is_success() {
        return Err(AccountAuthError::OAuth(format!(
            "token endpoint returned status {}",
            response.status()
        )));
    }
    let tokens: TokenResponse = response
        .json()
        .await
        .map_err(|error| AccountAuthError::OAuth(error.to_string()))?;
    Ok(AccountTokens::new(
        provider,
        tokens.access_token,
        tokens.refresh_token,
        tokens.id_token,
        tokens.token_type,
        tokens.expires_in,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::account_auth::generate_pkce;

    #[test]
    fn split_accepts_matching_state_and_rejects_everything_else() {
        assert_eq!(split_pasted_code("abc#st4te", "st4te").unwrap(), "abc");
        assert_eq!(split_pasted_code("  abc#st4te \n", "st4te").unwrap(), "abc");
        // Wrong state = someone else's authorization.
        assert!(split_pasted_code("abc#other", "st4te").is_err());
        // Missing state when one was expected is a rejection, not a fallback.
        assert!(split_pasted_code("abc", "st4te").is_err());
        assert!(split_pasted_code("#st4te", "st4te").is_err());
        assert!(split_pasted_code("   ", "st4te").is_err());
        // Only a caller with no state may accept a bare code.
        assert_eq!(split_pasted_code("abc", "").unwrap(), "abc");
    }

    #[test]
    fn authorize_url_carries_pkce_challenge_and_state() {
        let pkce = generate_pkce();
        let url = build_authorize_url(
            "https://claude.ai/oauth/authorize",
            "client-x",
            "https://console.anthropic.com/oauth/code/callback",
            "user:profile user:inference",
            &pkce,
            "st4te",
        )
        .expect("authorize url");
        let parsed = reqwest::Url::parse(&url).expect("parse");
        let pairs: std::collections::HashMap<_, _> = parsed.query_pairs().into_owned().collect();
        assert_eq!(pairs["client_id"], "client-x");
        assert_eq!(pairs["response_type"], "code");
        assert_eq!(pairs["code_challenge_method"], "S256");
        assert_eq!(pairs["code_challenge"], pkce.code_challenge);
        assert_eq!(pairs["state"], "st4te");
        assert_eq!(pairs["code"], "true");
    }
}
