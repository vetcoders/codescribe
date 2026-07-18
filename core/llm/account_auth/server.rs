// Portions derived from openai/codex (Apache-2.0).

use std::io::{self, Cursor, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use rand::RngCore;
use tiny_http::{Header, Request, Response, Server, StatusCode};

use crate::llm::account_auth::pkce::{PkceCodes, generate_pkce};
use crate::llm::account_auth::{
    AccountAuthError, AccountTokens, DEFAULT_ISSUER, store_account_tokens,
};
use crate::llm::provider::ProviderKind;

use base64::Engine;

const DEFAULT_PORT: u16 = 1455;
const SUCCESS_HTML: &str = r#"<!doctype html>
<html lang="en">
<head><meta charset="utf-8"><title>Codescribe signed in</title></head>
<body><h1>Codescribe signed in</h1><p>You can close this window.</p></body>
</html>"#;

#[derive(Debug, Clone)]
pub struct ServerOptions {
    pub client_id: String,
    pub issuer: String,
    pub port: u16,
    pub force_state: Option<String>,
}

impl ServerOptions {
    pub fn new(client_id: String) -> Self {
        Self {
            client_id,
            issuer: DEFAULT_ISSUER.to_string(),
            port: DEFAULT_PORT,
            force_state: None,
        }
    }
}

pub struct LoginServer {
    pub auth_url: String,
    pub actual_port: u16,
    server_handle: tokio::task::JoinHandle<Result<(), AccountAuthError>>,
    shutdown_handle: ShutdownHandle,
}

impl LoginServer {
    pub async fn block_until_done(self) -> Result<(), AccountAuthError> {
        self.server_handle
            .await
            .map_err(|error| AccountAuthError::Io(io::Error::other(error.to_string())))?
    }

    pub fn cancel(&self) {
        self.shutdown_handle.shutdown();
    }

    pub fn cancel_handle(&self) -> ShutdownHandle {
        self.shutdown_handle.clone()
    }
}

#[derive(Clone, Debug)]
pub struct ShutdownHandle {
    shutdown_notify: Arc<tokio::sync::Notify>,
}

impl ShutdownHandle {
    pub fn shutdown(&self) {
        // `notify_one` (not `notify_waiters`): it stores a permit when the
        // server task is momentarily outside its `select!` — e.g. mid-response
        // to a browser request — so a cancel landing in that window still stops
        // the server on its next loop instead of being silently lost.
        self.shutdown_notify.notify_one();
    }
}

pub async fn run_login_server(opts: ServerOptions) -> Result<LoginServer, AccountAuthError> {
    let pkce = generate_pkce();
    let state = opts.force_state.clone().unwrap_or_else(generate_state);
    let server = bind_server(opts.port)?;
    let actual_port = server
        .server_addr()
        .to_ip()
        .map(|addr| addr.port())
        .ok_or_else(|| AccountAuthError::Io(io::Error::other("unable to determine server port")))?;
    let server = Arc::new(server);
    let redirect_uri = format!("http://localhost:{actual_port}/auth/callback");
    let auth_url = build_authorize_url(&opts.issuer, &opts.client_id, &redirect_uri, &pkce, &state);

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

enum HandledRequest {
    Response(Response<Cursor<Vec<u8>>>),
    Redirect(Header),
    ResponseAndExit {
        headers: Vec<Header>,
        body: Vec<u8>,
        result: Result<(), AccountAuthError>,
    },
}

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

async fn process_request(
    url_raw: &str,
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

    match parsed_url.path() {
        "/auth/callback" => {
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

            match exchange_code_for_tokens(&opts.issuer, &opts.client_id, redirect_uri, pkce, code)
                .await
            {
                Ok(tokens) => {
                    if let Err(error) = store_account_tokens(ProviderKind::OpenAiResponses, &tokens)
                    {
                        return HandledRequest::Response(
                            Response::from_string(format!(
                                "Unable to persist account tokens: {error}"
                            ))
                            .with_status_code(500),
                        );
                    }
                    let success_url = format!("http://localhost:{actual_port}/success");
                    match Header::from_bytes(&b"Location"[..], success_url.as_bytes()) {
                        Ok(header) => HandledRequest::Redirect(header),
                        Err(_) => HandledRequest::Response(
                            Response::from_string("Internal Server Error").with_status_code(500),
                        ),
                    }
                }
                Err(error) => HandledRequest::Response(
                    Response::from_string(format!("Token exchange failed: {error}"))
                        .with_status_code(500),
                ),
            }
        }
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

pub async fn exchange_code_for_tokens(
    issuer: &str,
    client_id: &str,
    redirect_uri: &str,
    pkce: &PkceCodes,
    code: &str,
) -> Result<AccountTokens, AccountAuthError> {
    #[derive(serde::Deserialize)]
    struct TokenResponse {
        access_token: String,
        refresh_token: Option<String>,
        id_token: Option<String>,
        token_type: Option<String>,
        expires_in: Option<u64>,
    }

    let response = reqwest::Client::new()
        .post(format!("{}/oauth/token", issuer.trim_end_matches('/')))
        .form(&[
            ("grant_type", "authorization_code"),
            ("code", code),
            ("redirect_uri", redirect_uri),
            ("client_id", client_id),
            ("code_verifier", &pkce.code_verifier),
        ])
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
        ProviderKind::OpenAiResponses,
        tokens.access_token,
        tokens.refresh_token,
        tokens.id_token,
        tokens.token_type,
        tokens.expires_in,
    ))
}

fn build_authorize_url(
    issuer: &str,
    client_id: &str,
    redirect_uri: &str,
    pkce: &PkceCodes,
    state: &str,
) -> String {
    let mut url = reqwest::Url::parse(&format!("{}/oauth/authorize", issuer.trim_end_matches('/')))
        .expect("default issuer must parse");
    url.query_pairs_mut()
        .append_pair("response_type", "code")
        .append_pair("client_id", client_id)
        .append_pair("redirect_uri", redirect_uri)
        .append_pair("scope", "openid profile email offline_access")
        .append_pair("code_challenge", &pkce.code_challenge)
        .append_pair("code_challenge_method", "S256")
        .append_pair("id_token_add_organizations", "true")
        .append_pair("codescribe_account_flow", "true")
        .append_pair("state", state);
    url.to_string()
}

fn generate_state() -> String {
    let mut bytes = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut bytes);
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::account_auth::{OPENAI_ACCOUNT_TOKENS_ACCOUNT, load_account_tokens};
    use serial_test::serial;

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

        let mut opts = ServerOptions::new("client".to_string());
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
        let mut opts = ServerOptions::new("client".to_string());
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

    #[derive(Debug)]
    struct EnvGuard {
        key: &'static str,
        previous: Option<String>,
    }

    impl EnvGuard {
        fn set(key: &'static str, value: &str) -> Self {
            let previous = std::env::var(key).ok();
            // SAFETY: env-touching tests here are serialized with `serial`.
            unsafe { std::env::set_var(key, value) };
            Self { key, previous }
        }

        fn unset(key: &'static str) -> Self {
            let previous = std::env::var(key).ok();
            // SAFETY: env-touching tests here are serialized with `serial`.
            unsafe { std::env::remove_var(key) };
            Self { key, previous }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            match &self.previous {
                // SAFETY: env-touching tests here are serialized with `serial`.
                Some(value) => unsafe { std::env::set_var(self.key, value) },
                // SAFETY: env-touching tests here are serialized with `serial`.
                None => unsafe { std::env::remove_var(self.key) },
            }
        }
    }

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
}
