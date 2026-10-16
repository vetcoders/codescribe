//! Exercise the shipped TLS boundary (the library is built without cfg(test)).
use std::time::Duration;

use codescribe_core::llm::account_auth::{
    AccountAuthError, DeviceAuthConfig, PkceCodes, exchange_code_for_tokens, request_device_code,
};
use codescribe_core::llm::provider::ProviderKind;

#[tokio::test]
async fn shipped_code_exchange_rejects_even_numeric_loopback_http_before_sending() {
    let mut server = mockito::Server::new_async().await;
    let endpoint = server
        .mock("POST", "/oauth/token")
        .expect(0)
        .create_async()
        .await;
    let result = exchange_code_for_tokens(
        ProviderKind::OpenAiResponses,
        &server.url(),
        "fixture-client",
        "http://localhost:1455/auth/callback",
        &PkceCodes {
            code_verifier: "fixture-verifier".into(),
            code_challenge: "fixture-challenge".into(),
        },
        "fixture-code",
    )
    .await;
    assert!(matches!(result, Err(AccountAuthError::Http(_))));
    endpoint.assert_async().await;
}

#[tokio::test]
async fn shipped_device_login_rejects_cleartext_issuer_before_sending() {
    let mut server = mockito::Server::new_async().await;
    let endpoint = server
        .mock("POST", "/api/accounts/deviceauth/usercode")
        .expect(0)
        .create_async()
        .await;
    let result = request_device_code(&DeviceAuthConfig {
        provider: ProviderKind::OpenAiResponses,
        issuer: server.url(),
        client_id: "fixture-client".into(),
        max_wait: Duration::from_secs(1),
    })
    .await;
    assert!(matches!(result, Err(AccountAuthError::Http(_))));
    endpoint.assert_async().await;
}
