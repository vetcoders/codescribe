// Portions derived from openai/codex (Apache-2.0).

//! Loopback OAuth login server.
//!
//! Runs a throwaway HTTP server on localhost, hands the caller an authorize URL
//! to open in a browser, and completes when the provider redirects back with an
//! authorization code.
//!
//! Everything vendor-specific — issuer, callback path, token path, body
//! encoding, scope, extra authorize params, preferred port — comes from the
//! provider's registry row rather than being hardcoded to one vendor's shape.
//! The provider identity is carried through the entire flow, from
//! `ServerOptions` to the token store, which is what prevents the failure this
//! module was reshaped to fix: a code obtained for one vendor being filed under
//! another vendor's account slot.
//!
//! Only providers whose login flow is a loopback callback are served here.
//! Paste-code providers are refused outright — binding a port nothing will call
//! back on is worse than an honest error.

use std::io::{self, Cursor, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use rand::RngCore;
use tiny_http::{Header, Request, Response, Server, StatusCode};

use crate::llm::account_auth::pkce::{PkceCodes, generate_pkce};
use crate::llm::account_auth::{
    AccountAuthError, AccountTokens, LoginFlow, ProviderOAuthConfig, TokenRequestEncoding,
    issuer_for, provider_oauth_config, store_account_tokens,
};
use crate::llm::provider::ProviderKind;

use base64::Engine;

/// Browser page after a successful OAuth callback — tells the user to close it.
const SUCCESS_HTML: &str = r#"<!doctype html>
<html lang="en">
<head><meta charset="utf-8"><title>Codescribe signed in</title></head>
<body><h1>Codescribe signed in</h1><p>You can close this window.</p></body>
</html>"#;

/// One loopback login attempt. `provider` is the identity the resulting tokens
/// are stored under — it is carried through the whole flow so the callback can
/// never persist a token to a provider the caller did not ask for.
#[derive(Debug, Clone)]
pub struct ServerOptions {
    pub provider: ProviderKind,
    pub client_id: String,
    pub issuer: String,
    pub port: u16,
    pub force_state: Option<String>,
}

impl ServerOptions {
    /// Registry-derived defaults for `provider`: its issuer (env override
    /// honoured) and the loopback port its registered redirect URI expects.
    /// Callers override only for tests.
    pub fn new(provider: ProviderKind, client_id: String) -> Result<Self, AccountAuthError> {
        let config = provider_oauth_config(provider)?;
        Ok(Self {
            provider,
            client_id,
            issuer: issuer_for(provider),
            port: config.preferred_port,
            force_state: None,
        })
    }
}

/// A login attempt already listening on localhost.
///
/// Returned as soon as the port is bound, before the user has done anything —
/// the caller needs `auth_url` to open a browser, and `actual_port` because the
/// preferred port may have been taken. The flow itself completes later, through
/// [`block_until_done`] or a cancel.
///
/// [`block_until_done`]: Self::block_until_done
pub struct LoginServer {
    pub auth_url: String,
    pub actual_port: u16,
    server_handle: tokio::task::JoinHandle<Result<(), AccountAuthError>>,
    shutdown_handle: ShutdownHandle,
}

impl LoginServer {
    /// Wait for the login to settle, consuming the server.
    ///
    /// `Ok` means tokens were exchanged and stored. Every other ending —
    /// cancelled in the browser, cancelled by the caller, request channel
    /// closed — is an error, because none of them produced a usable account.
    pub async fn block_until_done(self) -> Result<(), AccountAuthError> {
        self.server_handle
            .await
            .map_err(|error| AccountAuthError::Io(io::Error::other(error.to_string())))?
    }

    /// Abandon the login. The pending [`block_until_done`] resolves to an
    /// error.
    ///
    /// [`block_until_done`]: Self::block_until_done
    pub fn cancel(&self) {
        self.shutdown_handle.shutdown();
    }

    /// A detachable cancel trigger, for cancelling from somewhere that does not
    /// own the server — a UI button, a timeout.
    pub fn cancel_handle(&self) -> ShutdownHandle {
        self.shutdown_handle.clone()
    }
}

/// Cheap clonable handle that can stop a running [`LoginServer`].
#[derive(Clone, Debug)]
pub struct ShutdownHandle {
    shutdown_notify: Arc<tokio::sync::Notify>,
}

impl ShutdownHandle {
    /// Ask the server to stop. Idempotent and safe to call after it already
    /// finished.
    pub fn shutdown(&self) {
        // `notify_one` (not `notify_waiters`): it stores a permit when the
        // server task is momentarily outside its `select!` — e.g. mid-response
        // to a browser request — so a cancel landing in that window still stops
        // the server on its next loop instead of being silently lost.
        self.shutdown_notify.notify_one();
    }
}

/// Start a loopback login for `opts.provider`.
///
/// Refuses any provider that does not sign in through a local callback. On
/// success the port is bound and the authorize URL — carrying PKCE, `state`,
/// and the redirect URI built from the provider's own callback host and path —
/// is ready to open; the returned handle is how the caller waits or cancels.
///
/// The blocking `tiny_http` accept loop lives on its own thread and feeds
/// requests to an async task over a channel, so the async side can await both a
/// request and a cancel in one `select!`.
pub async fn run_login_server(opts: ServerOptions) -> Result<LoginServer, AccountAuthError> {
    let config = provider_oauth_config(opts.provider)?;
    // A paste-code provider has no loopback listener to bind. Binding one anyway
    // would open a port nothing will ever call back on — and, historically, then
    // file the tokens under whichever provider the code happened to name.
    if config.login_flow != LoginFlow::Loopback {
        return Err(AccountAuthError::UnsupportedProvider(format!(
            "{} does not use a local callback server (login flow: {:?})",
            opts.provider, config.login_flow
        )));
    }
    let pkce = generate_pkce();
    let state = opts.force_state.clone().unwrap_or_else(generate_state);
    let server = bind_server(opts.port)?;
    let actual_port = server
        .server_addr()
        .to_ip()
        .map(|addr| addr.port())
        .ok_or_else(|| AccountAuthError::Io(io::Error::other("unable to determine server port")))?;
    let server = Arc::new(server);
    let redirect_uri = format!(
        "http://{}:{actual_port}{}",
        config.callback_host, config.callback_path
    );
    let auth_url = build_authorize_url(config, &opts, &redirect_uri, &pkce, &state);

    let (tx, mut rx) = tokio::sync::mpsc::channel::<Request>(16);
    let request_server = server.clone();
    let _request_thread = thread::spawn(move || {
        while let Ok(request) = request_server.recv() {
            if tx.blocking_send(request).is_err() {
                break;
            }
        }
    });

    let shutdown_notify = Arc::new(tokio::sync::Notify::new());
    let server_handle = {
        let shutdown_notify = shutdown_notify.clone();
        let server = server.clone();
        tokio::spawn(async move {
            let result = loop {
                tokio::select! {
                    _ = shutdown_notify.notified() => {
                        break Err(AccountAuthError::OAuth("login was not completed".to_string()));
                    }
                    maybe_request = rx.recv() => {
                        let Some(request) = maybe_request else {
                            break Err(AccountAuthError::OAuth("login was not completed".to_string()));
                        };
                        let url_raw = request.url().to_string();
                        let handled = process_request(
                            &url_raw,
                            config,
                            &opts,
                            &redirect_uri,
                            &pkce,
                            actual_port,
                            &state,
                        )
                        .await;
                        let exit = respond(request, handled).await;
                        if let Some(result) = exit {
                            break result;
                        }
                    }
                }
            };
            server.unblock();
            result
        })
    };

    Ok(LoginServer {
        auth_url,
        actual_port,
        server_handle,
        shutdown_handle: ShutdownHandle { shutdown_notify },
    })
}

/// What to do with one incoming request, decided before any I/O.
///
/// Separating the decision from the reply is what lets a single variant —
/// [`ResponseAndExit`] — express "answer the browser, then stop the server"
/// without the handler needing to reach the server loop.
///
/// [`ResponseAndExit`]: HandledRequest::ResponseAndExit
enum HandledRequest {
    Response(Response<Cursor<Vec<u8>>>),
    /// Kept for rare 302 paths (e.g. future multi-hop success pages). Callback
    /// completion now uses [`ResponseAndExit`] so login does not depend on a
    /// second hop to `localhost` (IPv6 trap on `127.0.0.1`-bound listeners).
    #[allow(dead_code)]
    Redirect(Header),
    ResponseAndExit {
        headers: Vec<Header>,
        body: Vec<u8>,
        result: Result<(), AccountAuthError>,
    },
}

/// Write the decided reply, off the async executor.
///
/// Returns `Some` when this request ends the login — the value is the flow's
/// result — and `None` when the server should keep listening.
async fn respond(
    request: Request,
    handled: HandledRequest,
) -> Option<Result<(), AccountAuthError>> {
    match handled {
        HandledRequest::Response(response) => {
            let _ = tokio::task::spawn_blocking(move || request.respond(response)).await;
            None
        }
        HandledRequest::Redirect(header) => {
            let response = Response::empty(302).with_header(header);
            let _ = tokio::task::spawn_blocking(move || request.respond(response)).await;
            None
        }
        HandledRequest::ResponseAndExit {
            headers,
            body,
            result,
        } => {
            let _ = tokio::task::spawn_blocking(move || {
                send_response_with_disconnect(request, headers, body)
            })
            .await;
            Some(result)
        }
    }
}

/// Route one request: the provider's callback path, or our own `/success` and
/// `/cancel`.
///
/// On the callback path the checks are ordered so nothing irreversible happens
/// on an unverified request — `state` must match before the code is even read,
/// and the code is exchanged before anything is persisted. A `state` mismatch
/// is answered 400 and the server keeps listening; it is not a reason to end a
/// login the real browser may still complete.
///
/// After a successful exchange the browser is redirected to `/success`, so the
/// user lands on a page whose URL no longer contains their authorization code.
async fn process_request(
    url_raw: &str,
    config: ProviderOAuthConfig,
    opts: &ServerOptions,
    redirect_uri: &str,
    pkce: &PkceCodes,
    actual_port: u16,
    state: &str,
) -> HandledRequest {
    let parsed_url = match reqwest::Url::parse(&format!("http://localhost{url_raw}")) {
        Ok(url) => url,
        Err(error) => {
            return HandledRequest::Response(
                Response::from_string(format!("Bad Request: {error}")).with_status_code(400),
            );
        }
    };

    // The callback path is the provider's, not a literal: a vendor registers a
    // specific redirect URI and will call back on exactly that path. `/success`
    // and `/cancel` are ours, so they stay fixed.
    let path = parsed_url.path();
    if path == config.callback_path {
        let params: std::collections::HashMap<String, String> =
            parsed_url.query_pairs().into_owned().collect();
        if params.get("state").map(String::as_str) != Some(state) {
            return HandledRequest::Response(
                Response::from_string("State mismatch").with_status_code(400),
            );
        }
        let Some(code) = params.get("code").filter(|value| !value.is_empty()) else {
            return HandledRequest::Response(
                Response::from_string("Missing authorization code").with_status_code(400),
            );
        };

        return match exchange_code_for_tokens(
            opts.provider,
            &opts.issuer,
            &opts.client_id,
            redirect_uri,
            pkce,
            code,
        )
        .await
        {
            Ok(tokens) => {
                if let Err(error) = store_account_tokens(opts.provider, &tokens) {
                    return HandledRequest::Response(
                        Response::from_string(format!("Unable to persist account tokens: {error}"))
                            .with_status_code(500),
                    );
                }
                // Serve success HTML on the callback response itself. Do NOT
                // 302 to `http://localhost:{port}/success`: on macOS
                // `localhost` often resolves to `::1` while we bind only
                // `127.0.0.1`, so the second hop fails and `block_until_done`
                // never sees `/success` even though tokens are already stored.
                // Completing here also keeps the auth code out of a second URL.
                let _ = actual_port; // port still used by bind/authorize URL
                let headers = match Header::from_bytes(
                    &b"Content-Type"[..],
                    &b"text/html; charset=utf-8"[..],
                ) {
                    Ok(header) => vec![header],
                    Err(_) => Vec::new(),
                };
                HandledRequest::ResponseAndExit {
                    headers,
                    body: SUCCESS_HTML.as_bytes().to_vec(),
                    result: Ok(()),
                }
            }
            Err(error) => HandledRequest::Response(
                Response::from_string(format!("Token exchange failed: {error}"))
                    .with_status_code(500),
            ),
        };
    }

    match path {
        "/success" => {
            let headers =
                match Header::from_bytes(&b"Content-Type"[..], &b"text/html; charset=utf-8"[..]) {
                    Ok(header) => vec![header],
                    Err(_) => Vec::new(),
                };
            HandledRequest::ResponseAndExit {
                headers,
                body: SUCCESS_HTML.as_bytes().to_vec(),
                result: Ok(()),
            }
        }
        "/cancel" => HandledRequest::ResponseAndExit {
            headers: Vec::new(),
            body: b"Login cancelled".to_vec(),
            result: Err(AccountAuthError::OAuth("login cancelled".to_string())),
        },
        _ => HandledRequest::Response(Response::from_string("Not Found").with_status_code(404)),
    }
}

/// Redeem an authorization code at `provider`'s token endpoint.
///
/// The endpoint path, body encoding, and the identity the tokens are tagged
/// with all come from the provider's registry row — `provider` is the single
/// input that decides all three, so a code obtained for one vendor cannot be
/// redeemed into another vendor's account slot.
pub async fn exchange_code_for_tokens(
    provider: ProviderKind,
    issuer: &str,
    client_id: &str,
    redirect_uri: &str,
    pkce: &PkceCodes,
    code: &str,
) -> Result<AccountTokens, AccountAuthError> {
    /// Minimal issuer JSON body; only fields we persist into `AccountTokens`.
    #[derive(serde::Deserialize)]
    struct TokenResponse {
        access_token: String,
        refresh_token: Option<String>,
        id_token: Option<String>,
        token_type: Option<String>,
        expires_in: Option<u64>,
    }

    let config = provider_oauth_config(provider)?;
    let endpoint = format!("{}{}", issuer.trim_end_matches('/'), config.token_path);
    let request = reqwest::Client::new().post(endpoint);
    let request = match config.encoding {
        TokenRequestEncoding::Form => request.form(&[
            ("grant_type", "authorization_code"),
            ("code", code),
            ("redirect_uri", redirect_uri),
            ("client_id", client_id),
            ("code_verifier", pkce.code_verifier.as_str()),
        ]),
        TokenRequestEncoding::Json => request.json(&serde_json::json!({
            "grant_type": "authorization_code",
            "code": code,
            "redirect_uri": redirect_uri,
            "client_id": client_id,
            "code_verifier": pkce.code_verifier,
        })),
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

/// Build the authorize URL the user's browser opens.
///
/// Assembled entirely from the provider's row plus this attempt's PKCE
/// challenge and `state`, so a new vendor needs a registry entry rather than a
/// branch here.
fn build_authorize_url(
    config: ProviderOAuthConfig,
    opts: &ServerOptions,
    redirect_uri: &str,
    pkce: &PkceCodes,
    state: &str,
) -> String {
    let mut url = reqwest::Url::parse(&format!(
        "{}{}",
        opts.issuer.trim_end_matches('/'),
        config.authorize_path
    ))
    .expect("issuer must parse");
    // OpenCode XaiAuthPlugin always sends OpenID `nonce` with the xAI authorize
    // request (alongside PKCE + state). Harmless for other loopback providers
    // that ignore unknown params; required for parity with auth.x.ai.
    let nonce = generate_state();
    url.query_pairs_mut()
        .append_pair("response_type", "code")
        .append_pair("client_id", &opts.client_id)
        .append_pair("redirect_uri", redirect_uri)
        .append_pair("scope", config.scope)
        .append_pair("code_challenge", &pkce.code_challenge)
        .append_pair("code_challenge_method", "S256")
        .append_pair("nonce", &nonce)
        .extend_pairs(config.extra_authorize_params.iter().copied())
        .append_pair("state", state);
    url.to_string()
}

/// 256 bits of randomness, URL-safe base64. Used for both `state` and the
/// OpenID `nonce` — same unguessability requirement, same generator.
fn generate_state() -> String {
    let mut bytes = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut bytes);
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

/// Bind the loopback listener, evicting a stale login if the port is taken.
///
/// The port is not arbitrary — it is the one the provider's registered redirect
/// URI points at, so falling back to a different one would break the callback.
/// An abandoned earlier attempt is therefore asked to release it (`/cancel`)
/// and the bind is retried once.
fn bind_server(port: u16) -> Result<Server, AccountAuthError> {
    let bind_address = format!("127.0.0.1:{port}");
    match Server::http(&bind_address) {
        Ok(server) => Ok(server),
        Err(error) => {
            let message = error.to_string();
            if message.contains("Address already in use") {
                let _ = send_cancel_request(port);
            }
            Server::http(&bind_address).map_err(|retry_error| {
                AccountAuthError::Io(io::Error::other(retry_error.to_string()))
            })
        }
    }
}

/// Hand-write a `GET /cancel` at whatever holds `port`.
///
/// Raw TCP with short timeouts rather than an HTTP client: this runs on a
/// blocked bind path, the peer may not be one of ours, and the reply is
/// irrelevant — only that the port gets released.
fn send_cancel_request(port: u16) -> io::Result<()> {
    let addr: SocketAddr = format!("127.0.0.1:{port}")
        .parse()
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))?;
    let mut stream = TcpStream::connect_timeout(&addr, Duration::from_secs(2))?;
    stream.set_read_timeout(Some(Duration::from_secs(2)))?;
    stream.set_write_timeout(Some(Duration::from_secs(2)))?;
    stream.write_all(b"GET /cancel HTTP/1.1\r\n")?;
    stream.write_all(format!("Host: 127.0.0.1:{port}\r\n").as_bytes())?;
    stream.write_all(b"Connection: close\r\n\r\n")?;
    let mut buf = [0u8; 64];
    let _ = stream.read(&mut buf);
    Ok(())
}

/// Write a terminal response by hand and close the connection.
///
/// The server is about to shut down, so the reply must not leave the browser on
/// a keep-alive connection waiting for a socket that will never answer again:
/// any inherited `Connection` header is dropped in favour of `close`, and
/// `Content-Length` is set so the browser knows the body is complete.
fn send_response_with_disconnect(
    request: Request,
    mut headers: Vec<Header>,
    body: Vec<u8>,
) -> io::Result<()> {
    let status = StatusCode(200);
    let mut writer = request.into_writer();
    let reason = status.default_reason_phrase();
    write!(writer, "HTTP/1.1 {} {}\r\n", status.0, reason)?;
    headers.retain(|header| !header.field.equiv("Connection"));
    if let Ok(header) = Header::from_bytes(&b"Connection"[..], &b"close"[..]) {
        headers.push(header);
    }
    if let Ok(header) =
        Header::from_bytes(&b"Content-Length"[..], body.len().to_string().as_bytes())
    {
        headers.push(header);
    }
    for header in headers {
        write!(
            writer,
            "{}: {}\r\n",
            header.field.as_str(),
            header.value.as_str()
        )?;
    }
    writer.write_all(b"\r\n")?;
    writer.write_all(&body)?;
    writer.flush()
}

/// Loopback OAuth server: roundtrip, state reject, env guards, code exchange.
#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::account_auth::{
        ANTHROPIC_ACCOUNT_TOKENS_ACCOUNT, ANTHROPIC_DEFAULT_ISSUER, DEFAULT_ISSUER,
        OPENAI_ACCOUNT_TOKENS_ACCOUNT, load_account_tokens,
    };
    use serial_test::serial;

    /// OpenAI Responses login options for tests (registry row must exist).
    fn openai_opts(client_id: &str) -> ServerOptions {
        ServerOptions::new(ProviderKind::OpenAiResponses, client_id.to_string())
            .expect("OpenAI has an OAuth registry row")
    }

    /// Full "Sign in with ChatGPT" roundtrip against a mock issuer: the login
    /// server hands out the authorize URL, the browser redirect lands on
    /// `/auth/callback`, the code is exchanged at the issuer, tokens persist
    /// through the (test-env) Keychain path, and `block_until_done` resolves.
    /// `#[serial]`: the test-env token store is the process env var.
    #[tokio::test]
    #[serial]
    async fn login_roundtrip_callback_exchanges_code_and_stores_tokens() {
        let _disable = EnvGuard::set("CODESCRIBE_DISABLE_KEYCHAIN", "1");
        let _tokens = EnvGuard::unset(OPENAI_ACCOUNT_TOKENS_ACCOUNT);

        let mut issuer = mockito::Server::new_async().await;
        let _mock = issuer
            .mock("POST", "/oauth/token")
            .match_body(mockito::Matcher::AllOf(vec![
                mockito::Matcher::UrlEncoded(
                    "grant_type".to_string(),
                    "authorization_code".to_string(),
                ),
                mockito::Matcher::UrlEncoded("code".to_string(), "auth-code".to_string()),
                mockito::Matcher::UrlEncoded("client_id".to_string(), "client".to_string()),
            ]))
            .with_status(200)
            .with_body(
                r#"{"access_token":"account-access","refresh_token":"account-refresh","expires_in":3600}"#,
            )
            .expect(1)
            .create_async()
            .await;

        let mut opts = openai_opts("client");
        opts.issuer = issuer.url();
        opts.port = 0;
        opts.force_state = Some("roundtrip-state".to_string());
        let login = run_login_server(opts).await.expect("bind login server");
        assert!(login.auth_url.contains("state=roundtrip-state"));
        assert!(login.auth_url.starts_with(&issuer.url()));

        let callback = format!(
            "http://127.0.0.1:{}/auth/callback?code=auth-code&state=roundtrip-state",
            login.actual_port
        );
        // reqwest follows the 302 to /success, which completes the server loop.
        let response = reqwest::get(&callback).await.expect("callback request");
        assert!(response.status().is_success());
        login.block_until_done().await.expect("login completes");

        let stored =
            load_account_tokens(ProviderKind::OpenAiResponses).expect("tokens were stored");
        assert_eq!(stored.access_token, "account-access");
        assert_eq!(stored.refresh_token.as_deref(), Some("account-refresh"));
    }

    /// A forged `state` must be rejected before any token exchange, and the
    /// pending login stays cancellable (cancel ⇒ honest "not completed" error).
    #[tokio::test]
    async fn callback_with_wrong_state_is_rejected_without_token_exchange() {
        let mut opts = openai_opts("client");
        opts.port = 0;
        opts.force_state = Some("expected-state".to_string());
        let login = run_login_server(opts).await.expect("bind login server");

        let callback = format!(
            "http://127.0.0.1:{}/auth/callback?code=auth-code&state=forged",
            login.actual_port
        );
        let response = reqwest::get(&callback).await.expect("callback request");
        assert_eq!(response.status().as_u16(), 400);

        login.cancel();
        assert!(login.block_until_done().await.is_err());
    }

    /// RAII env override for serial tests; restores prior value on drop.
    #[derive(Debug)]
    struct EnvGuard {
        key: &'static str,
        previous: Option<String>,
    }

    impl EnvGuard {
        /// Set `key=value`, capturing the previous state for restore.
        fn set(key: &'static str, value: &str) -> Self {
            let previous = std::env::var(key).ok();
            // SAFETY: env-touching tests here are serialized with `serial`.
            unsafe { std::env::set_var(key, value) };
            Self { key, previous }
        }

        /// Remove `key`, capturing the previous state for restore.
        fn unset(key: &'static str) -> Self {
            let previous = std::env::var(key).ok();
            // SAFETY: env-touching tests here are serialized with `serial`.
            unsafe { std::env::remove_var(key) };
            Self { key, previous }
        }
    }

    impl Drop for EnvGuard {
        /// Restore the exact process env captured at set/unset.
        fn drop(&mut self) {
            match &self.previous {
                // SAFETY: env-touching tests here are serialized with `serial`.
                Some(value) => unsafe { std::env::set_var(self.key, value) },
                // SAFETY: env-touching tests here are serialized with `serial`.
                None => unsafe { std::env::remove_var(self.key) },
            }
        }
    }

    /// Authorization-code grant posts form body and maps access/refresh/id.
    #[tokio::test]
    async fn exchange_code_posts_authorization_grant_and_maps_tokens() {
        let mut server = mockito::Server::new_async().await;
        let _mock = server
            .mock("POST", "/oauth/token")
            .match_body(mockito::Matcher::AllOf(vec![
                mockito::Matcher::UrlEncoded(
                    "grant_type".to_string(),
                    "authorization_code".to_string(),
                ),
                mockito::Matcher::UrlEncoded("code".to_string(), "auth-code".to_string()),
                mockito::Matcher::UrlEncoded("client_id".to_string(), "client".to_string()),
            ]))
            .with_status(200)
            .with_body(
                r#"{"access_token":"access","refresh_token":"refresh","id_token":"id","expires_in":3600}"#,
            )
            .expect(1)
            .create_async()
            .await;

        let tokens = exchange_code_for_tokens(
            ProviderKind::OpenAiResponses,
            &server.url(),
            "client",
            "http://localhost:1455/auth/callback",
            &PkceCodes {
                code_verifier: "verifier".to_string(),
                code_challenge: "challenge".to_string(),
            },
            "auth-code",
        )
        .await
        .unwrap();

        assert_eq!(tokens.access_token, "access");
        assert_eq!(tokens.refresh_token.as_deref(), Some("refresh"));
        assert_eq!(tokens.id_token.as_deref(), Some("id"));
    }

    /// The exchange is driven entirely by the registry row: a non-OpenAI
    /// provider must hit *its* token path, with *its* body encoding, and come
    /// back tagged as itself. Before the registry, this call posted a
    /// form-encoded body to `/oauth/token` and stamped every token
    /// `openai-responses` no matter who was signing in.
    #[tokio::test]
    async fn exchange_uses_the_requested_providers_token_path_and_encoding() {
        let mut server = mockito::Server::new_async().await;
        let _openai_path_must_not_be_used = server
            .mock("POST", "/oauth/token")
            .expect(0)
            .create_async()
            .await;
        let _mock = server
            .mock("POST", "/v1/oauth/token")
            .match_header("content-type", "application/json")
            .match_body(mockito::Matcher::JsonString(
                r#"{"grant_type":"authorization_code","code":"auth-code",
                    "redirect_uri":"https://console.anthropic.com/oauth/code/callback",
                    "client_id":"client","code_verifier":"verifier"}"#
                    .to_string(),
            ))
            .with_status(200)
            .with_body(r#"{"access_token":"anthropic-access","expires_in":3600}"#)
            .expect(1)
            .create_async()
            .await;

        let tokens = exchange_code_for_tokens(
            ProviderKind::AnthropicMessages,
            &server.url(),
            "client",
            "https://console.anthropic.com/oauth/code/callback",
            &PkceCodes {
                code_verifier: "verifier".to_string(),
                code_challenge: "challenge".to_string(),
            },
            "auth-code",
        )
        .await
        .unwrap();

        assert_eq!(tokens.access_token, "anthropic-access");
        assert_eq!(
            tokens.provider,
            ProviderKind::AnthropicMessages.as_str(),
            "tokens must be tagged with the provider they were issued for"
        );
    }

    /// Anthropic signs in by pasting a code; there is no loopback callback to
    /// listen for. Refusing is the honest answer — the alternative (bind a port
    /// and open OpenAI's authorize page) is exactly how tokens used to land in
    /// the wrong keychain slot.
    #[tokio::test]
    #[serial]
    async fn paste_code_provider_is_refused_a_loopback_server() {
        let _disable = EnvGuard::set("CODESCRIBE_DISABLE_KEYCHAIN", "1");
        let _openai = EnvGuard::unset(OPENAI_ACCOUNT_TOKENS_ACCOUNT);
        let _anthropic = EnvGuard::unset(ANTHROPIC_ACCOUNT_TOKENS_ACCOUNT);

        let opts = ServerOptions::new(ProviderKind::AnthropicMessages, "client".to_string())
            .expect("Anthropic has an OAuth registry row");
        let Err(error) = run_login_server(opts).await else {
            panic!("paste-code provider must not get a callback server");
        };

        assert!(matches!(
            error,
            AccountAuthError::UnsupportedProvider(ref message)
                if message.contains("does not use a local callback server")
        ));
        // Nothing was stored for anyone — least of all under OpenAI.
        assert!(load_account_tokens(ProviderKind::OpenAiResponses).is_err());
        assert!(load_account_tokens(ProviderKind::AnthropicMessages).is_err());
    }

    /// `ServerOptions::new` is the only place the bridge gets its endpoints, so
    /// the issuer and callback port must come from the row, never from an
    /// OpenAI-shaped default applied to every provider.
    #[test]
    #[serial]
    fn server_options_take_issuer_and_port_from_the_providers_row() {
        let _openai_issuer = EnvGuard::unset("CODESCRIBE_OPENAI_OAUTH_ISSUER");
        let _anthropic_issuer = EnvGuard::unset("CODESCRIBE_ANTHROPIC_OAUTH_ISSUER");

        let openai = openai_opts("client");
        assert_eq!(openai.provider, ProviderKind::OpenAiResponses);
        assert_eq!(openai.issuer, DEFAULT_ISSUER);
        assert_eq!(openai.port, 1455);

        let anthropic = ServerOptions::new(ProviderKind::AnthropicMessages, "client".to_string())
            .expect("Anthropic has an OAuth registry row");
        assert_eq!(anthropic.issuer, ANTHROPIC_DEFAULT_ISSUER);
        assert_ne!(
            anthropic.issuer, openai.issuer,
            "a second provider must not inherit OpenAI's issuer"
        );
    }

    /// Scope and the OpenAI-only query pairs are row data now; this pins that
    /// the generated URL still carries exactly what OpenAI expects.
    #[test]
    fn authorize_url_is_built_from_the_registry_row() {
        let opts = openai_opts("client");
        let config = provider_oauth_config(ProviderKind::OpenAiResponses).unwrap();
        let pkce = PkceCodes {
            code_verifier: "verifier".to_string(),
            code_challenge: "challenge".to_string(),
        };

        let url = build_authorize_url(
            config,
            &opts,
            "http://localhost:1455/auth/callback",
            &pkce,
            "st4te",
        );
        let parsed = reqwest::Url::parse(&url).expect("authorize url parses");
        let pairs: std::collections::HashMap<_, _> = parsed.query_pairs().into_owned().collect();

        assert_eq!(parsed.path(), config.authorize_path);
        assert_eq!(pairs["scope"], config.scope);
        assert_eq!(pairs["code_challenge_method"], "S256");
        assert_eq!(pairs["state"], "st4te");
        for (key, value) in config.extra_authorize_params {
            assert_eq!(pairs[*key], *value);
        }
    }
}
