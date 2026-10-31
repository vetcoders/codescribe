//! OAuth 2.1 for remote MCP servers (Streamable HTTP).
//!
//! A remote server that answers `401` with a `WWW-Authenticate: Bearer
//! resource_metadata="…"` challenge is asking the client to run the MCP
//! authorization flow (spec revision 2025-06-18). This module owns that flow end
//! to end, in the order the spec and the oh-my-pi reference implementation use:
//!
//! 1. **Challenge** — an unauthenticated `initialize` provokes the 401 and
//!    yields the protected-resource metadata URL plus any required `scope`.
//! 2. **Discovery** — RFC 9728 protected-resource metadata names the
//!    authorization server; RFC 8414 / OIDC metadata names its endpoints.
//! 3. **Client registration** — RFC 7591 dynamic registration of a public
//!    loopback client, unless a `client_id` was supplied.
//! 4. **Authorization code + PKCE (S256)** — the browser is opened by the
//!    caller on [`PendingLogin::authorization_url`]; a one-shot loopback
//!    listener on `127.0.0.1` receives the redirect and checks `state`.
//! 5. **Token exchange**, with the RFC 8707 `resource` indicator.
//! 6. **Storage and refresh** — one JSON credential per connector in the
//!    Keychain, refreshed shortly before expiry by [`bearer_for_account`].
//!
//! Network policy mirrors `llm::account_auth`: HTTPS only, no redirects, and a
//! numeric-loopback HTTP exception that exists solely under `cfg(test)`.
//!
//! The flow is split in two so the caller controls the browser:
//! [`begin_login`] does everything up to the authorization URL, and
//! [`PendingLogin::complete`] waits for the human and finishes the exchange.

use std::fmt;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, anyhow, bail};
use base64::Engine;
use rand::Rng;
use reqwest::Url;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tiny_http::{Header, Response, Server};
use tokio::sync::Mutex;

use crate::config::keychain::{runtime_key, save_key};
use crate::llm::account_auth::pkce::{PkceCodes, generate_pkce};

/// Keychain account prefix for stored MCP OAuth credentials.
const ACCOUNT_PREFIX: &str = "MCP_OAUTH_";
/// Keychain account suffix, kept distinct from `MCP_CONNECTOR_*_TOKEN` bearers.
const ACCOUNT_SUFFIX: &str = "_CREDENTIAL";
/// Per-request ceiling for discovery, registration, exchange and refresh.
const HTTP_TIMEOUT: Duration = Duration::from_secs(30);
/// A token that expires within this window is refreshed before use.
const REFRESH_SKEW_SECS: u64 = 60;
/// How long the loopback listener waits for the human to finish in the browser.
pub const DEFAULT_CALLBACK_TIMEOUT: Duration = Duration::from_secs(300);
/// Loopback callback path registered with the authorization server.
const CALLBACK_PATH: &str = "/callback";
/// Client name shown on the consent screen and sent in registration.
const CLIENT_NAME: &str = "Codescribe";
/// Cap on any metadata / token response body we are willing to buffer.
const MAX_BODY_BYTES: usize = 1024 * 1024;
/// MCP protocol revision announced in the unauthenticated probe.
const MCP_PROTOCOL_VERSION: &str = "2025-06-18";

/// Serializes refreshes: refresh tokens may rotate, so two concurrent refreshes
/// from the same stored credential would burn the token for one of them.
static REFRESH_LOCK: Mutex<()> = Mutex::const_new(());

/// Typed OAuth failures the connector maps to a user-facing hint.
#[derive(Debug)]
pub enum McpOAuthError {
    /// No stored credential for this connector — the human has not signed in.
    NotSignedIn,
    /// The stored grant can no longer be refreshed; sign in again.
    ReauthRequired(String),
}

impl fmt::Display for McpOAuthError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotSignedIn => f.write_str("MCP connector requires OAuth sign-in"),
            Self::ReauthRequired(reason) => {
                write!(f, "MCP connector OAuth grant needs a new sign-in: {reason}")
            }
        }
    }
}

impl std::error::Error for McpOAuthError {}

/// Keychain account that holds the OAuth credential for connector `name`.
///
/// Non-alphanumerics collapse to `_` and the name is upper-cased, matching the
/// `MCP_CONNECTOR_<NAME>_TOKEN` convention used for static bearers.
pub fn credential_account(server_name: &str) -> String {
    let slug: String = server_name
        .trim()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_uppercase()
            } else {
                '_'
            }
        })
        .collect();
    format!("{ACCOUNT_PREFIX}{slug}{ACCOUNT_SUFFIX}")
}

/// What the unauthenticated probe learned from the server's 401.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BearerChallenge {
    /// `resource_metadata` URL (RFC 9728 §5.1), if the server advertised one.
    pub resource_metadata: Option<String>,
    /// `scope` the server says it needs.
    pub scope: Option<String>,
}

/// Parse a `WWW-Authenticate` header value into the parameters MCP uses.
///
/// Tolerant by design: servers send quoted and bare parameters, several
/// challenges in one header, and arbitrary casing on the parameter names.
pub fn parse_bearer_challenge(header: &str) -> BearerChallenge {
    let mut challenge = BearerChallenge::default();
    let bytes = header.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        // Find the next `name=` token.
        let start = i;
        while i < bytes.len() && bytes[i] != b'=' && bytes[i] != b' ' && bytes[i] != b',' {
            i += 1;
        }
        if i >= bytes.len() || bytes[i] != b'=' {
            i += 1;
            continue;
        }
        let name = header[start..i].to_ascii_lowercase();
        i += 1; // '='
        let value = if i < bytes.len() && bytes[i] == b'"' {
            i += 1;
            let value_start = i;
            while i < bytes.len() && bytes[i] != b'"' {
                i += 1;
            }
            let value = &header[value_start..i];
            i = (i + 1).min(bytes.len());
            value
        } else {
            let value_start = i;
            while i < bytes.len() && bytes[i] != b',' && bytes[i] != b' ' {
                i += 1;
            }
            &header[value_start..i]
        };
        let value = value.trim();
        if value.is_empty() {
            continue;
        }
        match name.as_str() {
            "resource_metadata" => challenge.resource_metadata = Some(value.to_string()),
            "scope" => challenge.scope = Some(value.to_string()),
            _ => {}
        }
    }
    challenge
}

/// Authorization-server endpoints the flow needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthServerMetadata {
    pub issuer: Option<String>,
    pub authorization_endpoint: String,
    pub token_endpoint: String,
    pub registration_endpoint: Option<String>,
    pub scopes_supported: Vec<String>,
}

/// RFC 9728 protected-resource metadata, reduced to what we consume.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ResourceMetadata {
    pub resource: Option<String>,
    pub authorization_servers: Vec<String>,
    pub scopes_supported: Vec<String>,
}

fn string_list(value: Option<&Value>) -> Vec<String> {
    value
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

fn non_empty_str(value: Option<&Value>) -> Option<String> {
    value
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

/// Parse an RFC 9728 document.
pub fn parse_resource_metadata(document: &Value) -> ResourceMetadata {
    ResourceMetadata {
        resource: non_empty_str(document.get("resource")),
        authorization_servers: string_list(document.get("authorization_servers")),
        scopes_supported: string_list(document.get("scopes_supported")),
    }
}

/// Parse an RFC 8414 / OIDC discovery document. `None` when either endpoint the
/// code flow cannot work without is absent, or when the server lists
/// `code_challenge_methods_supported` and S256 is not among them.
pub fn parse_auth_server_metadata(document: &Value) -> Option<AuthServerMetadata> {
    let authorization_endpoint = non_empty_str(document.get("authorization_endpoint"))?;
    let token_endpoint = non_empty_str(document.get("token_endpoint"))?;
    let methods = string_list(document.get("code_challenge_methods_supported"));
    if !methods.is_empty() && !methods.iter().any(|m| m == "S256") {
        return None;
    }
    Some(AuthServerMetadata {
        issuer: non_empty_str(document.get("issuer")),
        authorization_endpoint,
        token_endpoint,
        registration_endpoint: non_empty_str(document.get("registration_endpoint")),
        scopes_supported: string_list(document.get("scopes_supported")),
    })
}

/// RFC 8414 §3.3: compare issuers ignoring a trailing slash on the path.
fn issuer_matches(document_issuer: &str, expected: &str) -> bool {
    fn norm(value: &str) -> Option<String> {
        let url = Url::parse(value).ok()?;
        Some(format!(
            "{}://{}{}",
            url.scheme(),
            url.host_str()?.to_ascii_lowercase()
                + &url.port().map(|p| format!(":{p}")).unwrap_or_default(),
            url.path().trim_end_matches('/')
        ))
    }
    match (norm(document_issuer), norm(expected)) {
        (Some(a), Some(b)) => a == b,
        _ => false,
    }
}

/// Candidate URLs for the protected-resource metadata document, most specific
/// first (RFC 9728 §3.1: well-known segment inserted before the resource path).
pub fn resource_metadata_candidates(server_url: &Url) -> Vec<Url> {
    let origin = server_url.origin().ascii_serialization();
    let path = server_url.path().trim_end_matches('/');
    let mut urls = Vec::new();
    if !path.is_empty() {
        urls.extend(Url::parse(&format!(
            "{origin}/.well-known/oauth-protected-resource{path}"
        )));
    }
    urls.extend(Url::parse(&format!(
        "{origin}/.well-known/oauth-protected-resource"
    )));
    urls
}

/// Candidate URLs for authorization-server metadata, in the order RFC 8414 §3.1
/// and OIDC Discovery prescribe for an issuer with or without a path.
pub fn auth_server_metadata_candidates(issuer: &Url) -> Vec<Url> {
    let origin = issuer.origin().ascii_serialization();
    let path = issuer.path().trim_end_matches('/');
    let mut urls = Vec::new();
    if path.is_empty() {
        urls.extend(Url::parse(&format!(
            "{origin}/.well-known/oauth-authorization-server"
        )));
        urls.extend(Url::parse(&format!(
            "{origin}/.well-known/openid-configuration"
        )));
    } else {
        urls.extend(Url::parse(&format!(
            "{origin}/.well-known/oauth-authorization-server{path}"
        )));
        urls.extend(Url::parse(&format!(
            "{origin}/.well-known/openid-configuration{path}"
        )));
        urls.extend(Url::parse(&format!(
            "{origin}{path}/.well-known/openid-configuration"
        )));
    }
    urls
}

/// HTTPS-only client without redirects. A numeric-loopback `http` exception
/// exists under `cfg(test)` only, so shipped builds cannot be pointed at
/// cleartext endpoints.
pub fn http_client() -> Result<reqwest::Client> {
    reqwest::Client::builder()
        .https_only(!cfg!(test))
        .redirect(reqwest::redirect::Policy::none())
        .timeout(HTTP_TIMEOUT)
        .build()
        .context("Failed to build MCP OAuth HTTP client")
}

/// Reject a non-HTTPS endpoint before any request is made. Under `cfg(test)`
/// numeric loopback `http` is accepted for local fixtures.
fn require_secure(url: &Url, what: &str) -> Result<()> {
    if url.scheme() == "https" || test_loopback_fixture(url) {
        return Ok(());
    }
    bail!("MCP OAuth {what} must use https: {url}")
}

/// Numeric-loopback `http` fixtures, honoured only in unit tests.
#[cfg(test)]
fn test_loopback_fixture(url: &Url) -> bool {
    url.scheme() == "http"
        && url.host_str().is_some_and(|h| {
            h.parse::<std::net::IpAddr>()
                .is_ok_and(|ip| ip.is_loopback())
        })
}

/// Shipped builds never accept cleartext endpoints.
#[cfg(not(test))]
fn test_loopback_fixture(_url: &Url) -> bool {
    false
}

/// Read a response body with a hard size cap.
async fn read_capped(mut response: reqwest::Response) -> Result<Vec<u8>> {
    let mut body = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .context("Failed reading OAuth body")?
    {
        if body.len() + chunk.len() > MAX_BODY_BYTES {
            bail!("MCP OAuth response exceeded {MAX_BODY_BYTES} bytes");
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

async fn get_json(client: &reqwest::Client, url: &Url) -> Option<Value> {
    require_secure(url, "metadata endpoint").ok()?;
    let response = client
        .get(url.clone())
        .header(reqwest::header::ACCEPT, "application/json")
        .send()
        .await
        .ok()?;
    if !response.status().is_success() {
        return None;
    }
    serde_json::from_slice(&read_capped(response).await.ok()?).ok()
}

/// Provoke the 401 challenge with an unauthenticated `initialize`.
///
/// Fails when the server does not demand authorization, because then there is
/// nothing to sign in to and a stored token would only add risk.
pub async fn probe_challenge(
    client: &reqwest::Client,
    server_url: &Url,
) -> Result<BearerChallenge> {
    require_secure(server_url, "server endpoint")?;
    let response = client
        .post(server_url.clone())
        .header(
            reqwest::header::ACCEPT,
            "application/json, text/event-stream",
        )
        .json(&json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": MCP_PROTOCOL_VERSION,
                "capabilities": {},
                "clientInfo": { "name": "codescribe", "version": env!("CARGO_PKG_VERSION") },
            },
        }))
        .send()
        .await
        .with_context(|| format!("MCP server '{server_url}' is unreachable"))?;
    if response.status().as_u16() != 401 {
        bail!(
            "MCP server '{server_url}' answered HTTP {} to an unauthenticated initialize; it does \
             not ask for OAuth",
            response.status().as_u16()
        );
    }
    Ok(response
        .headers()
        .get_all(reqwest::header::WWW_AUTHENTICATE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .map(parse_bearer_challenge)
        .fold(BearerChallenge::default(), |mut acc, next| {
            acc.resource_metadata = acc.resource_metadata.or(next.resource_metadata);
            acc.scope = acc.scope.or(next.scope);
            acc
        }))
}

/// Everything discovery established about how to authorize against a server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Discovery {
    pub server: AuthServerMetadata,
    /// Canonical resource for the RFC 8707 `resource` parameter.
    pub resource: String,
    /// Scope to request: the challenge's, else the resource's, else none.
    pub scope: Option<String>,
}

/// Run RFC 9728 → RFC 8414 discovery for `server_url`.
pub async fn discover(
    client: &reqwest::Client,
    server_url: &Url,
    challenge: &BearerChallenge,
) -> Result<Discovery> {
    // 1. Protected-resource metadata: the advertised URL first, then well-known.
    let mut resource_urls: Vec<Url> = challenge
        .resource_metadata
        .as_deref()
        .and_then(|raw| server_url.join(raw).ok())
        .into_iter()
        .collect();
    for candidate in resource_metadata_candidates(server_url) {
        if !resource_urls.contains(&candidate) {
            resource_urls.push(candidate);
        }
    }
    let mut resource_meta = ResourceMetadata::default();
    for url in &resource_urls {
        if let Some(document) = get_json(client, url).await {
            let parsed = parse_resource_metadata(&document);
            if !parsed.authorization_servers.is_empty() {
                resource_meta = parsed;
                break;
            }
        }
    }

    // 2. Authorization-server metadata. Without protected-resource metadata the
    //    spec's fallback is to treat the MCP server's own origin as the issuer.
    let issuers: Vec<Url> = if resource_meta.authorization_servers.is_empty() {
        vec![Url::parse(&server_url.origin().ascii_serialization())?]
    } else {
        resource_meta
            .authorization_servers
            .iter()
            .filter_map(|raw| Url::parse(raw).ok())
            .collect()
    };
    let mut found = None;
    'issuers: for issuer in &issuers {
        for url in auth_server_metadata_candidates(issuer) {
            let Some(document) = get_json(client, &url).await else {
                continue;
            };
            if let Some(declared) = document.get("issuer").and_then(Value::as_str)
                && !issuer_matches(declared, issuer.as_str())
            {
                continue;
            }
            if let Some(meta) = parse_auth_server_metadata(&document) {
                found = Some(meta);
                break 'issuers;
            }
        }
    }
    let server = found.ok_or_else(|| {
        anyhow!("No usable OAuth authorization-server metadata found for '{server_url}'")
    })?;
    require_secure(
        &Url::parse(&server.authorization_endpoint)?,
        "authorization endpoint",
    )?;
    require_secure(&Url::parse(&server.token_endpoint)?, "token endpoint")?;

    let resource = resource_meta
        .resource
        .clone()
        .unwrap_or_else(|| canonical_resource(server_url));
    let scope = challenge.scope.clone().or_else(|| {
        (!resource_meta.scopes_supported.is_empty())
            .then(|| resource_meta.scopes_supported.join(" "))
    });
    Ok(Discovery {
        server,
        resource,
        scope,
    })
}

/// RFC 8707 canonical resource: the server URL without fragment or trailing `/`.
fn canonical_resource(server_url: &Url) -> String {
    let mut url = server_url.clone();
    url.set_fragment(None);
    url.as_str().trim_end_matches('/').to_string()
}

/// A registered (or supplied) OAuth client.
#[derive(Clone, PartialEq, Eq)]
pub struct ClientRegistration {
    pub client_id: String,
    pub client_secret: Option<String>,
}

impl fmt::Debug for ClientRegistration {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ClientRegistration")
            .field("client_id", &self.client_id)
            .field(
                "client_secret",
                &self.client_secret.as_ref().map(|_| "<redacted>"),
            )
            .finish()
    }
}

/// RFC 7591 dynamic registration of a public loopback client.
pub async fn register_client(
    client: &reqwest::Client,
    registration_endpoint: &str,
    redirect_uri: &str,
    scope: Option<&str>,
) -> Result<ClientRegistration> {
    let endpoint = Url::parse(registration_endpoint)?;
    require_secure(&endpoint, "registration endpoint")?;
    let mut body = json!({
        "client_name": CLIENT_NAME,
        "redirect_uris": [redirect_uri],
        "grant_types": ["authorization_code", "refresh_token"],
        "response_types": ["code"],
        "token_endpoint_auth_method": "none",
        "application_type": "native",
    });
    if let Some(scope) = scope {
        body["scope"] = Value::String(scope.to_string());
    }
    let response = client
        .post(endpoint)
        .header(reqwest::header::ACCEPT, "application/json")
        .json(&body)
        .send()
        .await
        .context("MCP OAuth client registration is unreachable")?;
    let status = response.status();
    if !status.is_success() {
        bail!(
            "MCP OAuth client registration was rejected with HTTP {}; supply a pre-registered \
             client_id",
            status.as_u16()
        );
    }
    let document: Value = serde_json::from_slice(&read_capped(response).await?)
        .context("Malformed MCP OAuth registration response")?;
    let client_id = non_empty_str(document.get("client_id"))
        .ok_or_else(|| anyhow!("MCP OAuth registration returned no client_id"))?;
    Ok(ClientRegistration {
        client_id,
        client_secret: non_empty_str(document.get("client_secret")),
    })
}

/// 256 bits of OS entropy, base64url — used for `state`.
fn random_state() -> String {
    let mut bytes = [0u8; 32];
    rand::rng().fill_bytes(&mut bytes);
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

/// Build the browser URL for the authorization request.
pub fn build_authorization_url(
    discovery: &Discovery,
    client_id: &str,
    redirect_uri: &str,
    pkce: &PkceCodes,
    state: &str,
) -> Result<String> {
    let mut url = Url::parse(&discovery.server.authorization_endpoint)?;
    {
        let mut query = url.query_pairs_mut();
        query
            .append_pair("response_type", "code")
            .append_pair("client_id", client_id)
            .append_pair("redirect_uri", redirect_uri)
            .append_pair("code_challenge", &pkce.code_challenge)
            .append_pair("code_challenge_method", "S256")
            .append_pair("state", state)
            .append_pair("resource", &discovery.resource);
        if let Some(scope) = &discovery.scope {
            query.append_pair("scope", scope);
        }
    }
    Ok(url.to_string())
}

/// Stored grant for one connector. Secrets live only in the Keychain, as one
/// JSON blob under [`credential_account`].
#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct OAuthCredential {
    pub access_token: String,
    #[serde(default)]
    pub refresh_token: Option<String>,
    /// Unix seconds when `access_token` stops being valid, if the server said.
    #[serde(default)]
    pub expires_at: Option<u64>,
    pub token_endpoint: String,
    pub client_id: String,
    #[serde(default)]
    pub client_secret: Option<String>,
    #[serde(default)]
    pub resource: Option<String>,
    #[serde(default)]
    pub scope: Option<String>,
}

impl fmt::Debug for OAuthCredential {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OAuthCredential")
            .field("access_token", &"<redacted>")
            .field(
                "refresh_token",
                &self.refresh_token.as_ref().map(|_| "<redacted>"),
            )
            .field("expires_at", &self.expires_at)
            .field("token_endpoint", &self.token_endpoint)
            .field("client_id", &self.client_id)
            .field("resource", &self.resource)
            .field("scope", &self.scope)
            .finish_non_exhaustive()
    }
}

impl OAuthCredential {
    /// Whether the access token is expired or about to be, at `now` (unix secs).
    pub fn needs_refresh(&self, now: u64) -> bool {
        self.expires_at
            .is_some_and(|expires_at| expires_at <= now.saturating_add(REFRESH_SKEW_SECS))
    }
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Token endpoint answer, both for code exchange and refresh.
#[derive(Deserialize)]
struct TokenResponse {
    access_token: Option<String>,
    refresh_token: Option<String>,
    expires_in: Option<u64>,
    scope: Option<String>,
    token_type: Option<String>,
    error: Option<String>,
    error_description: Option<String>,
}

async fn post_token(
    client: &reqwest::Client,
    token_endpoint: &str,
    form: &[(&str, &str)],
) -> Result<(u16, TokenResponse)> {
    let endpoint = Url::parse(token_endpoint)?;
    require_secure(&endpoint, "token endpoint")?;
    let response = client
        .post(endpoint)
        .header(reqwest::header::ACCEPT, "application/json")
        .form(form)
        .send()
        .await
        .context("MCP OAuth token endpoint is unreachable")?;
    let status = response.status().as_u16();
    let parsed: TokenResponse = serde_json::from_slice(&read_capped(response).await?)
        .with_context(|| format!("Malformed MCP OAuth token response (HTTP {status})"))?;
    Ok((status, parsed))
}

fn credential_from_token(
    token: TokenResponse,
    previous: Option<&OAuthCredential>,
    token_endpoint: &str,
    registration: &ClientRegistration,
    resource: Option<&str>,
    now: u64,
) -> Result<OAuthCredential> {
    let access_token = token
        .access_token
        .filter(|t| !t.is_empty())
        .ok_or_else(|| {
            anyhow!(
                "MCP OAuth token response had no access_token{}",
                token
                    .error_description
                    .or(token.error)
                    .map(|e| format!(": {e}"))
                    .unwrap_or_default()
            )
        })?;
    if let Some(kind) = &token.token_type
        && !kind.eq_ignore_ascii_case("bearer")
    {
        bail!("MCP OAuth token type '{kind}' is not supported");
    }
    Ok(OAuthCredential {
        access_token,
        // A refresh that does not rotate keeps the previous refresh token.
        refresh_token: token
            .refresh_token
            .filter(|t| !t.is_empty())
            .or_else(|| previous.and_then(|p| p.refresh_token.clone())),
        expires_at: token.expires_in.map(|secs| now.saturating_add(secs)),
        token_endpoint: token_endpoint.to_string(),
        client_id: registration.client_id.clone(),
        client_secret: registration.client_secret.clone(),
        resource: resource.map(str::to_string),
        scope: token
            .scope
            .or_else(|| previous.and_then(|p| p.scope.clone())),
    })
}

/// Redeem an authorization code (RFC 6749 §4.1.3 + PKCE verifier + RFC 8707).
async fn exchange_code(
    client: &reqwest::Client,
    discovery: &Discovery,
    registration: &ClientRegistration,
    redirect_uri: &str,
    pkce: &PkceCodes,
    code: &str,
) -> Result<OAuthCredential> {
    let mut form = vec![
        ("grant_type", "authorization_code"),
        ("code", code),
        ("redirect_uri", redirect_uri),
        ("client_id", registration.client_id.as_str()),
        ("code_verifier", pkce.code_verifier.as_str()),
        ("resource", discovery.resource.as_str()),
    ];
    if let Some(secret) = &registration.client_secret {
        form.push(("client_secret", secret.as_str()));
    }
    let (status, token) = post_token(client, &discovery.server.token_endpoint, &form).await?;
    if !(200..300).contains(&status) {
        bail!(
            "MCP OAuth code exchange failed with HTTP {status}{}",
            token.error.map(|e| format!(" ({e})")).unwrap_or_default()
        );
    }
    credential_from_token(
        token,
        None,
        &discovery.server.token_endpoint,
        registration,
        Some(&discovery.resource),
        unix_now(),
    )
}

/// Refresh `credential` with its refresh token (RFC 6749 §6).
pub async fn refresh_credential(
    client: &reqwest::Client,
    credential: &OAuthCredential,
    now: u64,
) -> Result<OAuthCredential> {
    let refresh_token = credential
        .refresh_token
        .as_deref()
        .ok_or_else(|| McpOAuthError::ReauthRequired("no refresh token was issued".into()))?;
    let mut form = vec![
        ("grant_type", "refresh_token"),
        ("refresh_token", refresh_token),
        ("client_id", credential.client_id.as_str()),
    ];
    if let Some(resource) = &credential.resource {
        form.push(("resource", resource.as_str()));
    }
    if let Some(secret) = &credential.client_secret {
        form.push(("client_secret", secret.as_str()));
    }
    let (status, token) = post_token(client, &credential.token_endpoint, &form).await?;
    if !(200..300).contains(&status) {
        let reason = token.error.unwrap_or_else(|| format!("HTTP {status}"));
        // A 400/401 on refresh means the grant is gone (revoked, rotated away,
        // expired). Anything else may be transient and must not demand a login.
        if matches!(status, 400 | 401) {
            return Err(McpOAuthError::ReauthRequired(reason).into());
        }
        bail!("MCP OAuth refresh failed with HTTP {status} ({reason})");
    }
    credential_from_token(
        token,
        Some(credential),
        &credential.token_endpoint,
        &ClientRegistration {
            client_id: credential.client_id.clone(),
            client_secret: credential.client_secret.clone(),
        },
        credential.resource.as_deref(),
        now,
    )
}

/// Load the stored credential for `account`, if any.
pub fn load_credential(account: &str) -> Result<Option<OAuthCredential>> {
    let Some(raw) = runtime_key(account) else {
        return Ok(None);
    };
    serde_json::from_str(&raw)
        .map(Some)
        .with_context(|| format!("Stored MCP OAuth credential '{account}' is not valid JSON"))
}

/// Persist `credential` under `account`.
pub fn store_credential(account: &str, credential: &OAuthCredential) -> Result<()> {
    save_key(account, &serde_json::to_string(credential)?)
}

/// The bearer to send for `account`, refreshing first when it is (nearly)
/// expired. `force_refresh` is the retry path after the server answered 401 to a
/// token we believed valid.
pub async fn bearer_for_account(account: &str, force_refresh: bool) -> Result<String> {
    let credential = load_credential(account)?.ok_or(McpOAuthError::NotSignedIn)?;
    if !force_refresh && !credential.needs_refresh(unix_now()) {
        return Ok(credential.access_token);
    }
    let _guard = REFRESH_LOCK.lock().await;
    // Another task may have refreshed while we waited for the lock.
    let current = load_credential(account)?.ok_or(McpOAuthError::NotSignedIn)?;
    if current != credential && !current.needs_refresh(unix_now()) {
        return Ok(current.access_token);
    }
    let refreshed = refresh_credential(&http_client()?, &current, unix_now()).await?;
    store_credential(account, &refreshed)?;
    Ok(refreshed.access_token)
}

/// Optional inputs to [`begin_login`].
#[derive(Debug, Clone)]
pub struct LoginOptions {
    /// Pre-registered client; skips dynamic registration when set.
    pub client_id: Option<String>,
    pub client_secret: Option<String>,
    /// Overrides the scope discovery would request.
    pub scope: Option<String>,
    /// How long [`PendingLogin::complete`] waits for the browser.
    pub callback_timeout: Duration,
}

impl Default for LoginOptions {
    fn default() -> Self {
        Self {
            client_id: None,
            client_secret: None,
            scope: None,
            callback_timeout: DEFAULT_CALLBACK_TIMEOUT,
        }
    }
}

/// A login that has reached the browser step. Discovery and registration are
/// done and the loopback listener is bound; nothing is stored until
/// [`Self::complete`] succeeds.
pub struct PendingLogin {
    /// Open this in the human's browser.
    pub authorization_url: String,
    /// Loopback redirect URI registered for this attempt.
    pub redirect_uri: String,
    /// Discovery result, for diagnostics.
    pub discovery: Discovery,
    registration: ClientRegistration,
    pkce: PkceCodes,
    state: String,
    listener: Server,
    timeout: Duration,
    client: reqwest::Client,
}

impl fmt::Debug for PendingLogin {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PendingLogin")
            .field("authorization_url", &self.authorization_url)
            .field("redirect_uri", &self.redirect_uri)
            .finish_non_exhaustive()
    }
}

/// Probe, discover, register and bind: everything short of the human.
pub async fn begin_login(server_url: &str, options: LoginOptions) -> Result<PendingLogin> {
    let server_url = Url::parse(server_url.trim())
        .with_context(|| format!("Invalid remote MCP endpoint: {server_url}"))?;
    let client = http_client()?;
    let challenge = probe_challenge(&client, &server_url).await?;
    let mut discovery = discover(&client, &server_url, &challenge).await?;
    if let Some(scope) = options.scope.filter(|s| !s.trim().is_empty()) {
        discovery.scope = Some(scope);
    }

    let listener = Server::http("127.0.0.1:0")
        .map_err(|error| anyhow!("Cannot bind OAuth loopback listener: {error}"))?;
    let port = listener
        .server_addr()
        .to_ip()
        .map(|addr| addr.port())
        .ok_or_else(|| anyhow!("OAuth loopback listener has no port"))?;
    let redirect_uri = format!("http://127.0.0.1:{port}{CALLBACK_PATH}");

    let registration =
        match options.client_id.filter(|id| !id.trim().is_empty()) {
            Some(client_id) => ClientRegistration {
                client_id,
                client_secret: options.client_secret,
            },
            None => {
                let endpoint = discovery.server.registration_endpoint.as_deref().ok_or_else(|| {
                anyhow!(
                    "Authorization server advertises no dynamic client registration; supply a \
                     pre-registered client_id"
                )
            })?;
                register_client(&client, endpoint, &redirect_uri, discovery.scope.as_deref())
                    .await?
            }
        };

    let pkce = generate_pkce();
    let state = random_state();
    let authorization_url = build_authorization_url(
        &discovery,
        &registration.client_id,
        &redirect_uri,
        &pkce,
        &state,
    )?;
    Ok(PendingLogin {
        authorization_url,
        redirect_uri,
        discovery,
        registration,
        pkce,
        state,
        listener,
        timeout: options.callback_timeout,
        client,
    })
}

impl PendingLogin {
    /// Wait for the browser redirect, redeem the code and store the credential
    /// under `account`. Consumes the login: the PKCE verifier is single-use.
    pub async fn complete(self, account: &str) -> Result<OAuthCredential> {
        let Self {
            redirect_uri,
            discovery,
            registration,
            pkce,
            state,
            listener,
            timeout,
            client,
            ..
        } = self;
        let code =
            tokio::task::spawn_blocking(move || wait_for_callback(&listener, &state, timeout))
                .await
                .context("OAuth callback task failed")??;
        let credential = exchange_code(
            &client,
            &discovery,
            &registration,
            &redirect_uri,
            &pkce,
            &code,
        )
        .await?;
        store_credential(account, &credential)?;
        Ok(credential)
    }
}

const CALLBACK_SUCCESS_HTML: &str = "<!doctype html><meta charset=\"utf-8\"><title>Codescribe</title>\
<body style=\"font:15px -apple-system,sans-serif;text-align:center;margin-top:20vh\">\
<h1>Signed in.</h1><p>You can close this window and return to Codescribe.</p>\
<script>try{history.replaceState(null,'',location.pathname)}catch(_){}</script></body>";

fn html_response(status: u16, body: &str) -> Response<std::io::Cursor<Vec<u8>>> {
    let mut response = Response::from_string(body).with_status_code(status);
    if let Ok(header) = Header::from_bytes(&b"Content-Type"[..], &b"text/html; charset=utf-8"[..]) {
        response.add_header(header);
    }
    response
}

/// Block until the browser redirect carries a valid `code`.
///
/// A request with the wrong `state` is answered 400 and ignored — it must not
/// end a login the real browser may still complete. An `error` parameter from
/// the authorization server ends the wait with that error.
fn wait_for_callback(listener: &Server, expected_state: &str, timeout: Duration) -> Result<String> {
    let deadline = std::time::Instant::now() + timeout;
    loop {
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        if remaining.is_zero() {
            bail!("Timed out waiting for the browser to finish MCP sign-in");
        }
        let Some(request) = listener
            .recv_timeout(remaining)
            .context("OAuth loopback listener failed")?
        else {
            continue;
        };
        let Ok(url) = Url::parse(&format!("http://127.0.0.1{}", request.url())) else {
            let _ = request.respond(html_response(400, "Bad request"));
            continue;
        };
        if url.path() != CALLBACK_PATH {
            let _ = request.respond(html_response(404, "Not found"));
            continue;
        }
        let params: std::collections::HashMap<String, String> =
            url.query_pairs().into_owned().collect();
        if params.get("state").map(String::as_str) != Some(expected_state) {
            let _ = request.respond(html_response(400, "State mismatch"));
            continue;
        }
        if let Some(error) = params.get("error") {
            let detail = params
                .get("error_description")
                .map(|d| format!(": {d}"))
                .unwrap_or_default();
            let _ = request.respond(html_response(400, "Sign-in was not completed."));
            bail!("MCP OAuth authorization was denied ({error}{detail})");
        }
        let Some(code) = params.get("code").filter(|c| !c.is_empty()).cloned() else {
            let _ = request.respond(html_response(400, "Missing authorization code"));
            continue;
        };
        let _ = request.respond(html_response(200, CALLBACK_SUCCESS_HTML));
        return Ok(code);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mockito::Matcher;

    fn url(raw: &str) -> Url {
        Url::parse(raw).unwrap()
    }

    // ---- challenge parsing -------------------------------------------------

    /// The MCP 401 shape: quoted `resource_metadata` and `scope`.
    #[test]
    fn challenge_extracts_resource_metadata_and_scope() {
        let parsed = parse_bearer_challenge(
            "Bearer realm=\"mcp\", resource_metadata=\"https://hq.example/.well-known/oauth-protected-resource/mcp\", scope=\"files:read files:write\"",
        );
        assert_eq!(
            parsed.resource_metadata.as_deref(),
            Some("https://hq.example/.well-known/oauth-protected-resource/mcp")
        );
        assert_eq!(parsed.scope.as_deref(), Some("files:read files:write"));
    }

    /// Bare (unquoted) parameters and odd casing still parse; absent ones stay `None`.
    #[test]
    fn challenge_tolerates_bare_values_and_missing_fields() {
        let parsed = parse_bearer_challenge("Bearer Resource_Metadata=https://a.example/meta");
        assert_eq!(
            parsed.resource_metadata.as_deref(),
            Some("https://a.example/meta")
        );
        assert_eq!(parsed.scope, None);
        assert_eq!(parse_bearer_challenge("Bearer"), BearerChallenge::default());
    }

    // ---- discovery URL candidates -------------------------------------------

    /// RFC 9728: the path-inserted document is tried before the origin root.
    #[test]
    fn resource_candidates_insert_well_known_before_path() {
        let urls = resource_metadata_candidates(&url("https://hq.mega.dev/mcp"));
        assert_eq!(
            urls.iter().map(Url::as_str).collect::<Vec<_>>(),
            vec![
                "https://hq.mega.dev/.well-known/oauth-protected-resource/mcp",
                "https://hq.mega.dev/.well-known/oauth-protected-resource",
            ]
        );
    }

    /// RFC 8414 vs OIDC ordering for an issuer without and with a path.
    #[test]
    fn auth_server_candidates_follow_rfc8414_then_oidc() {
        let bare = auth_server_metadata_candidates(&url("https://auth.example"));
        assert_eq!(
            bare.iter().map(Url::as_str).collect::<Vec<_>>(),
            vec![
                "https://auth.example/.well-known/oauth-authorization-server",
                "https://auth.example/.well-known/openid-configuration",
            ]
        );
        let pathed = auth_server_metadata_candidates(&url("https://auth.example/tenant1"));
        assert_eq!(
            pathed.iter().map(Url::as_str).collect::<Vec<_>>(),
            vec![
                "https://auth.example/.well-known/oauth-authorization-server/tenant1",
                "https://auth.example/.well-known/openid-configuration/tenant1",
                "https://auth.example/tenant1/.well-known/openid-configuration",
            ]
        );
    }

    // ---- metadata parsing ---------------------------------------------------

    /// A server that lists PKCE methods without S256 cannot be used safely.
    #[test]
    fn auth_metadata_requires_s256_when_methods_are_listed() {
        let base = json!({
            "authorization_endpoint": "https://a.example/authorize",
            "token_endpoint": "https://a.example/token",
        });
        assert!(parse_auth_server_metadata(&base).is_some());
        let mut plain_only = base.clone();
        plain_only["code_challenge_methods_supported"] = json!(["plain"]);
        assert!(parse_auth_server_metadata(&plain_only).is_none());
        let mut with_s256 = base;
        with_s256["code_challenge_methods_supported"] = json!(["plain", "S256"]);
        assert!(parse_auth_server_metadata(&with_s256).is_some());
    }

    /// Trailing slash and host case do not make an issuer differ (RFC 8414 §3.3).
    #[test]
    fn issuer_comparison_normalises_trailing_slash_and_host_case() {
        assert!(issuer_matches(
            "https://Auth.Example/",
            "https://auth.example"
        ));
        assert!(!issuer_matches(
            "https://evil.example",
            "https://auth.example"
        ));
    }

    // ---- full discovery against a fixture server ----------------------------

    /// Challenge → protected-resource metadata → AS metadata, scope from the challenge.
    #[tokio::test]
    async fn discovery_walks_challenge_to_authorization_server() {
        let mut server = mockito::Server::new_async().await;
        let base = server.url();
        let prm = server
            .mock("GET", "/.well-known/oauth-protected-resource/mcp")
            .with_body(
                json!({
                    "resource": format!("{base}/mcp"),
                    "authorization_servers": [base],
                    "scopes_supported": ["course:read"],
                })
                .to_string(),
            )
            .create_async()
            .await;
        let asm = server
            .mock("GET", "/.well-known/oauth-authorization-server")
            .with_body(
                json!({
                    "issuer": base,
                    "authorization_endpoint": format!("{base}/authorize"),
                    "token_endpoint": format!("{base}/token"),
                    "registration_endpoint": format!("{base}/register"),
                    "code_challenge_methods_supported": ["S256"],
                })
                .to_string(),
            )
            .create_async()
            .await;
        let server_url = url(&format!("{base}/mcp"));
        let challenge = BearerChallenge {
            resource_metadata: Some(format!("{base}/.well-known/oauth-protected-resource/mcp")),
            scope: Some("course:write".into()),
        };
        let found = discover(&http_client().unwrap(), &server_url, &challenge)
            .await
            .unwrap();
        prm.assert_async().await;
        asm.assert_async().await;
        assert_eq!(found.resource, format!("{base}/mcp"));
        assert_eq!(found.scope.as_deref(), Some("course:write"));
        assert_eq!(found.server.token_endpoint, format!("{base}/token"));
        assert_eq!(
            found.server.registration_endpoint.as_deref(),
            Some(format!("{base}/register").as_str())
        );
    }

    /// A metadata document whose `issuer` names a different server is not trusted.
    #[tokio::test]
    async fn discovery_rejects_issuer_mismatch() {
        let mut server = mockito::Server::new_async().await;
        let base = server.url();
        let _asm = server
            .mock("GET", Matcher::Regex("oauth-authorization-server".into()))
            .with_body(
                json!({
                    "issuer": "https://evil.example",
                    "authorization_endpoint": "https://evil.example/authorize",
                    "token_endpoint": "https://evil.example/token",
                })
                .to_string(),
            )
            .create_async()
            .await;
        let result = discover(
            &http_client().unwrap(),
            &url(&format!("{base}/mcp")),
            &BearerChallenge::default(),
        )
        .await;
        assert!(result.is_err());
    }

    // ---- registration -------------------------------------------------------

    /// DCR posts a public native client with the exact loopback redirect.
    #[tokio::test]
    async fn registration_posts_public_native_client() {
        let mut server = mockito::Server::new_async().await;
        let register = server
            .mock("POST", "/register")
            .match_body(Matcher::PartialJson(json!({
                "redirect_uris": ["http://127.0.0.1:4242/callback"],
                "token_endpoint_auth_method": "none",
                "grant_types": ["authorization_code", "refresh_token"],
                "scope": "a b",
            })))
            .with_body(json!({"client_id": "client-1"}).to_string())
            .create_async()
            .await;
        let registered = register_client(
            &http_client().unwrap(),
            &format!("{}/register", server.url()),
            "http://127.0.0.1:4242/callback",
            Some("a b"),
        )
        .await
        .unwrap();
        register.assert_async().await;
        assert_eq!(registered.client_id, "client-1");
        assert_eq!(registered.client_secret, None);
    }

    /// A rejected registration points at the pre-registered `client_id` escape hatch.
    #[tokio::test]
    async fn registration_rejection_is_actionable() {
        let mut server = mockito::Server::new_async().await;
        let _register = server
            .mock("POST", "/register")
            .with_status(403)
            .create_async()
            .await;
        let error = register_client(
            &http_client().unwrap(),
            &format!("{}/register", server.url()),
            "http://127.0.0.1:1/callback",
            None,
        )
        .await
        .unwrap_err();
        assert!(error.to_string().contains("pre-registered client_id"));
    }

    // ---- authorization URL + PKCE ------------------------------------------

    fn fixture_discovery(base: &str) -> Discovery {
        Discovery {
            server: AuthServerMetadata {
                issuer: Some(base.into()),
                authorization_endpoint: format!("{base}/authorize"),
                token_endpoint: format!("{base}/token"),
                registration_endpoint: None,
                scopes_supported: vec![],
            },
            resource: format!("{base}/mcp"),
            scope: Some("course:read".into()),
        }
    }

    /// The authorize URL carries S256 PKCE, state, resource and scope.
    #[test]
    fn authorization_url_carries_pkce_state_resource_and_scope() {
        let pkce = generate_pkce();
        let built = build_authorization_url(
            &fixture_discovery("https://a.example"),
            "client-1",
            "http://127.0.0.1:9/callback",
            &pkce,
            "state-xyz",
        )
        .unwrap();
        let parsed = url(&built);
        let query: std::collections::HashMap<_, _> = parsed.query_pairs().into_owned().collect();
        assert_eq!(parsed.path(), "/authorize");
        assert_eq!(query["response_type"], "code");
        assert_eq!(query["client_id"], "client-1");
        assert_eq!(query["redirect_uri"], "http://127.0.0.1:9/callback");
        assert_eq!(query["code_challenge"], pkce.code_challenge);
        assert_eq!(query["code_challenge_method"], "S256");
        assert_eq!(query["state"], "state-xyz");
        assert_eq!(query["resource"], "https://a.example/mcp");
        assert_eq!(query["scope"], "course:read");
        assert!(
            !built.contains(&pkce.code_verifier),
            "verifier must never travel in the URL"
        );
    }

    // ---- token exchange + refresh ------------------------------------------

    /// The exchange sends verifier, resource and client id; expiry is stamped from `expires_in`.
    #[tokio::test]
    async fn code_exchange_sends_verifier_and_stores_expiry() {
        let mut server = mockito::Server::new_async().await;
        let base = server.url();
        let pkce = generate_pkce();
        let token = server
            .mock("POST", "/token")
            .match_body(Matcher::AllOf(vec![
                Matcher::UrlEncoded("grant_type".into(), "authorization_code".into()),
                Matcher::UrlEncoded("code".into(), "the-code".into()),
                Matcher::UrlEncoded("client_id".into(), "client-1".into()),
                Matcher::UrlEncoded("code_verifier".into(), pkce.code_verifier.clone()),
                Matcher::UrlEncoded("resource".into(), format!("{base}/mcp")),
                Matcher::UrlEncoded("redirect_uri".into(), "http://127.0.0.1:9/callback".into()),
            ]))
            .with_body(
                json!({
                    "access_token": "at-1",
                    "refresh_token": "rt-1",
                    "expires_in": 3600,
                    "token_type": "Bearer",
                })
                .to_string(),
            )
            .create_async()
            .await;
        let credential = exchange_code(
            &http_client().unwrap(),
            &fixture_discovery(&base),
            &ClientRegistration {
                client_id: "client-1".into(),
                client_secret: None,
            },
            "http://127.0.0.1:9/callback",
            &pkce,
            "the-code",
        )
        .await
        .unwrap();
        token.assert_async().await;
        assert_eq!(credential.access_token, "at-1");
        assert_eq!(credential.refresh_token.as_deref(), Some("rt-1"));
        assert_eq!(credential.token_endpoint, format!("{base}/token"));
        assert!(credential.expires_at.unwrap() > unix_now());
    }

    /// A 200 body with an `error` and no token must not be stored as a credential.
    #[tokio::test]
    async fn exchange_without_access_token_is_an_error() {
        let mut server = mockito::Server::new_async().await;
        let base = server.url();
        let _token = server
            .mock("POST", "/token")
            .with_body(json!({"error": "invalid_grant"}).to_string())
            .create_async()
            .await;
        let result = exchange_code(
            &http_client().unwrap(),
            &fixture_discovery(&base),
            &ClientRegistration {
                client_id: "c".into(),
                client_secret: None,
            },
            "http://127.0.0.1:9/callback",
            &generate_pkce(),
            "code",
        )
        .await;
        assert!(result.unwrap_err().to_string().contains("no access_token"));
    }

    fn stored(base: &str) -> OAuthCredential {
        OAuthCredential {
            access_token: "old-at".into(),
            refresh_token: Some("old-rt".into()),
            expires_at: Some(1_000),
            token_endpoint: format!("{base}/token"),
            client_id: "client-1".into(),
            client_secret: None,
            resource: Some(format!("{base}/mcp")),
            scope: Some("course:read".into()),
        }
    }

    /// Refresh rotates the refresh token when the server issues one.
    #[tokio::test]
    async fn refresh_rotates_tokens() {
        let mut server = mockito::Server::new_async().await;
        let base = server.url();
        let refresh = server
            .mock("POST", "/token")
            .match_body(Matcher::AllOf(vec![
                Matcher::UrlEncoded("grant_type".into(), "refresh_token".into()),
                Matcher::UrlEncoded("refresh_token".into(), "old-rt".into()),
                Matcher::UrlEncoded("client_id".into(), "client-1".into()),
            ]))
            .with_body(
                json!({"access_token": "new-at", "refresh_token": "new-rt", "expires_in": 60})
                    .to_string(),
            )
            .create_async()
            .await;
        let next = refresh_credential(&http_client().unwrap(), &stored(&base), 5_000)
            .await
            .unwrap();
        refresh.assert_async().await;
        assert_eq!(next.access_token, "new-at");
        assert_eq!(next.refresh_token.as_deref(), Some("new-rt"));
        assert_eq!(next.expires_at, Some(5_060));
        assert_eq!(next.scope.as_deref(), Some("course:read"));
    }

    /// A non-rotating server keeps the existing refresh token alive.
    #[tokio::test]
    async fn refresh_keeps_old_refresh_token_when_not_rotated() {
        let mut server = mockito::Server::new_async().await;
        let base = server.url();
        let _refresh = server
            .mock("POST", "/token")
            .with_body(json!({"access_token": "new-at"}).to_string())
            .create_async()
            .await;
        let next = refresh_credential(&http_client().unwrap(), &stored(&base), 5_000)
            .await
            .unwrap();
        assert_eq!(next.refresh_token.as_deref(), Some("old-rt"));
        assert_eq!(next.expires_at, None);
    }

    /// `invalid_grant` on refresh is a re-login, not a transient failure.
    #[tokio::test]
    async fn refresh_rejection_demands_a_new_login() {
        let mut server = mockito::Server::new_async().await;
        let base = server.url();
        let _refresh = server
            .mock("POST", "/token")
            .with_status(400)
            .with_body(json!({"error": "invalid_grant"}).to_string())
            .create_async()
            .await;
        let error = refresh_credential(&http_client().unwrap(), &stored(&base), 5_000)
            .await
            .unwrap_err();
        assert!(matches!(
            error.downcast_ref::<McpOAuthError>(),
            Some(McpOAuthError::ReauthRequired(reason)) if reason == "invalid_grant"
        ));
    }

    /// A 5xx on refresh is surfaced as an ordinary error — the grant may still be good.
    #[tokio::test]
    async fn refresh_server_error_is_not_a_reauth_demand() {
        let mut server = mockito::Server::new_async().await;
        let base = server.url();
        let _refresh = server
            .mock("POST", "/token")
            .with_status(503)
            .with_body("{}")
            .create_async()
            .await;
        let error = refresh_credential(&http_client().unwrap(), &stored(&base), 5_000)
            .await
            .unwrap_err();
        assert!(error.downcast_ref::<McpOAuthError>().is_none());
    }

    // ---- credential bookkeeping --------------------------------------------

    /// Refresh is due inside the skew window and not before; no expiry means never.
    #[test]
    fn needs_refresh_respects_skew() {
        let mut credential = stored("https://a.example");
        credential.expires_at = Some(1_000);
        assert!(credential.needs_refresh(1_000 - REFRESH_SKEW_SECS));
        assert!(!credential.needs_refresh(1_000 - REFRESH_SKEW_SECS - 1));
        credential.expires_at = None;
        assert!(!credential.needs_refresh(u64::MAX / 2));
    }

    /// Tokens never reach logs through `Debug`.
    #[test]
    fn debug_output_redacts_secrets() {
        let credential = stored("https://a.example");
        let rendered = format!("{credential:?}");
        assert!(!rendered.contains("old-at"));
        assert!(!rendered.contains("old-rt"));
        let registration = ClientRegistration {
            client_id: "c".into(),
            client_secret: Some("shh".into()),
        };
        assert!(!format!("{registration:?}").contains("shh"));
    }

    /// Account names follow the connector convention and cannot smuggle separators.
    #[test]
    fn credential_account_is_slugged() {
        assert_eq!(
            credential_account("mega-challenge"),
            "MCP_OAUTH_MEGA_CHALLENGE_CREDENTIAL"
        );
        assert_eq!(credential_account(" a b/c "), "MCP_OAUTH_A_B_C_CREDENTIAL");
    }

    // ---- loopback callback --------------------------------------------------

    fn spawn_get(port: u16, path_and_query: String) -> std::thread::JoinHandle<u16> {
        std::thread::spawn(move || {
            reqwest::blocking::get(format!("http://127.0.0.1:{port}{path_and_query}"))
                .map(|r| r.status().as_u16())
                .unwrap_or(0)
        })
    }

    fn listener_port(listener: &Server) -> u16 {
        listener.server_addr().to_ip().unwrap().port()
    }

    /// A forged-state request is refused and the genuine redirect still completes.
    #[test]
    fn callback_ignores_wrong_state_then_accepts_the_real_redirect() {
        let listener = Server::http("127.0.0.1:0").unwrap();
        let port = listener_port(&listener);
        // The browser retries after the forged request is refused; sequencing
        // the two keeps the test independent of accept order.
        let browser = std::thread::spawn(move || {
            let forged = spawn_get(port, "/callback?code=evil&state=nope".into())
                .join()
                .unwrap();
            let real = spawn_get(port, "/callback?code=good&state=s1".into())
                .join()
                .unwrap();
            (forged, real)
        });
        let code = wait_for_callback(&listener, "s1", Duration::from_secs(5)).unwrap();
        assert_eq!(code, "good");
        assert_eq!(browser.join().unwrap(), (400, 200));
    }

    /// An `error` redirect from the authorization server ends the wait with that error.
    #[test]
    fn callback_surfaces_authorization_denial() {
        let listener = Server::http("127.0.0.1:0").unwrap();
        let port = listener_port(&listener);
        let denied = spawn_get(
            port,
            "/callback?error=access_denied&error_description=nope&state=s1".into(),
        );
        let error = wait_for_callback(&listener, "s1", Duration::from_secs(5)).unwrap_err();
        assert!(error.to_string().contains("access_denied"));
        denied.join().unwrap();
    }

    /// No browser, no hang: the wait is bounded.
    #[test]
    fn callback_times_out() {
        let listener = Server::http("127.0.0.1:0").unwrap();
        let error = wait_for_callback(&listener, "s1", Duration::from_millis(150)).unwrap_err();
        assert!(error.to_string().contains("Timed out"));
    }

    // ---- end to end through begin_login -------------------------------------

    /// Whole flow against fixtures: 401 probe → discovery → DCR → URL → callback → token → store.
    #[tokio::test]
    async fn login_roundtrip_stores_a_credential() {
        let mut server = mockito::Server::new_async().await;
        let base = server.url();
        let _probe = server
            .mock("POST", "/mcp")
            .with_status(401)
            .with_header(
                "www-authenticate",
                &format!(
                    "Bearer resource_metadata=\"{base}/.well-known/oauth-protected-resource/mcp\""
                ),
            )
            .create_async()
            .await;
        let _prm = server
            .mock("GET", "/.well-known/oauth-protected-resource/mcp")
            .with_body(
                json!({"resource": format!("{base}/mcp"), "authorization_servers": [base]})
                    .to_string(),
            )
            .create_async()
            .await;
        let _asm = server
            .mock("GET", "/.well-known/oauth-authorization-server")
            .with_body(
                json!({
                    "issuer": base,
                    "authorization_endpoint": format!("{base}/authorize"),
                    "token_endpoint": format!("{base}/token"),
                    "registration_endpoint": format!("{base}/register"),
                })
                .to_string(),
            )
            .create_async()
            .await;
        let _register = server
            .mock("POST", "/register")
            .with_body(json!({"client_id": "dcr-client"}).to_string())
            .create_async()
            .await;
        let token = server
            .mock("POST", "/token")
            .match_body(Matcher::UrlEncoded("code".into(), "browser-code".into()))
            .with_body(
                json!({"access_token": "live-at", "refresh_token": "live-rt", "expires_in": 3600})
                    .to_string(),
            )
            .create_async()
            .await;

        let pending = begin_login(&format!("{base}/mcp"), LoginOptions::default())
            .await
            .unwrap();
        assert!(
            pending
                .authorization_url
                .starts_with(&format!("{base}/authorize?"))
        );
        let parsed = url(&pending.authorization_url);
        let query: std::collections::HashMap<_, _> = parsed.query_pairs().into_owned().collect();
        assert_eq!(query["client_id"], "dcr-client");
        let redirect = url(&query["redirect_uri"]);
        // The "browser": hit the registered redirect with the issued state.
        let hit = spawn_get(
            redirect.port().unwrap(),
            format!(
                "{}?code=browser-code&state={}",
                redirect.path(),
                query["state"]
            ),
        );
        let account = "MCP_OAUTH_TEST_LOGIN_ROUNDTRIP_CREDENTIAL";
        let credential = pending.complete(account).await.unwrap();
        assert_eq!(hit.join().unwrap(), 200);
        token.assert_async().await;
        assert_eq!(credential.access_token, "live-at");
        assert_eq!(load_credential(account).unwrap().unwrap(), credential);
        // Fresh token → the stored bearer is used as is, no refresh call.
        assert_eq!(bearer_for_account(account, false).await.unwrap(), "live-at");
    }

    /// A server that does not challenge is not an OAuth server.
    #[tokio::test]
    async fn probe_refuses_servers_that_do_not_ask_for_auth() {
        let mut server = mockito::Server::new_async().await;
        let _open = server
            .mock("POST", "/mcp")
            .with_status(200)
            .create_async()
            .await;
        let error = probe_challenge(
            &http_client().unwrap(),
            &url(&format!("{}/mcp", server.url())),
        )
        .await
        .unwrap_err();
        assert!(error.to_string().contains("does not ask for OAuth"));
    }

    /// Without a stored credential the connector reports sign-in, not a 401 mystery.
    #[tokio::test]
    async fn missing_credential_is_not_signed_in() {
        let error = bearer_for_account("MCP_OAUTH_TEST_NEVER_STORED_CREDENTIAL", false)
            .await
            .unwrap_err();
        assert!(matches!(
            error.downcast_ref::<McpOAuthError>(),
            Some(McpOAuthError::NotSignedIn)
        ));
    }
}
