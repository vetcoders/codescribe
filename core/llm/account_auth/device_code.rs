// Portions derived from openai/codex (Apache-2.0).

use std::time::{Duration, Instant};

use reqwest::StatusCode;
use serde::{Deserialize, Deserializer, Serialize, de};

use crate::llm::account_auth::pkce::PkceCodes;
use crate::llm::account_auth::server::exchange_code_for_tokens;
use crate::llm::account_auth::{AccountAuthError, DEFAULT_ISSUER, store_account_tokens};
use crate::llm::provider::ProviderKind;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceAuthConfig {
    pub issuer: String,
    pub client_id: String,
    pub max_wait: Duration,
}

impl DeviceAuthConfig {
    pub fn new(client_id: String) -> Self {
        Self {
            issuer: DEFAULT_ISSUER.to_string(),
            client_id,
            max_wait: Duration::from_secs(15 * 60),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceCode {
    pub verification_url: String,
    pub user_code: String,
    pub device_auth_id: String,
    pub interval: u64,
}

#[derive(Debug, Deserialize)]
struct UserCodeResp {
    device_auth_id: String,
    #[serde(alias = "user_code", alias = "usercode")]
    user_code: String,
    #[serde(default, deserialize_with = "deserialize_interval")]
    interval: u64,
}

#[derive(Debug, Serialize)]
struct UserCodeReq {
    client_id: String,
}

#[derive(Debug, Serialize)]
struct TokenPollReq {
    device_auth_id: String,
    user_code: String,
}

#[derive(Debug, Deserialize)]
struct CodeSuccessResp {
    authorization_code: String,
    code_challenge: String,
    code_verifier: String,
}

fn deserialize_interval<'de, D>(deserializer: D) -> Result<u64, D::Error>
where
    D: Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum StringOrU64 {
        String(String),
        U64(u64),
    }

    match StringOrU64::deserialize(deserializer)? {
        StringOrU64::String(value) => value
            .trim()
            .parse::<u64>()
            .map_err(|error| de::Error::custom(format!("invalid u64 string: {error}"))),
        StringOrU64::U64(value) => Ok(value),
    }
}

pub async fn request_device_code(
    config: &DeviceAuthConfig,
) -> Result<DeviceCode, AccountAuthError> {
    let client = reqwest::Client::new();
    request_device_code_with_client(&client, config).await
}

async fn request_device_code_with_client(
    client: &reqwest::Client,
    config: &DeviceAuthConfig,
) -> Result<DeviceCode, AccountAuthError> {
    let base_url = api_accounts_base(&config.issuer);
    let response = client
        .post(format!("{base_url}/deviceauth/usercode"))
        .json(&UserCodeReq {
            client_id: config.client_id.clone(),
        })
        .send()
        .await
        .map_err(|error| AccountAuthError::Http(error.to_string()))?;

    if !response.status().is_success() {
        return Err(AccountAuthError::OAuth(format!(
            "device code request failed with status {}",
            response.status()
        )));
    }

    let body: UserCodeResp = response
        .json()
        .await
        .map_err(|error| AccountAuthError::OAuth(error.to_string()))?;
    Ok(DeviceCode {
        verification_url: format!("{}/codex/device", config.issuer.trim_end_matches('/')),
        user_code: body.user_code,
        device_auth_id: body.device_auth_id,
        interval: body.interval,
    })
}

pub async fn complete_device_code_login(
    config: &DeviceAuthConfig,
    device_code: &DeviceCode,
) -> Result<(), AccountAuthError> {
    let client = reqwest::Client::new();
    let code_resp = poll_for_authorization_code(
        &client,
        &api_accounts_base(&config.issuer),
        &device_code.device_auth_id,
        &device_code.user_code,
        device_code.interval,
        config.max_wait,
    )
    .await?;
    let pkce = PkceCodes {
        code_verifier: code_resp.code_verifier,
        code_challenge: code_resp.code_challenge,
    };
    let redirect_uri = format!(
        "{}/deviceauth/callback",
        config.issuer.trim_end_matches('/')
    );
    let tokens = exchange_code_for_tokens(
        &config.issuer,
        &config.client_id,
        &redirect_uri,
        &pkce,
        &code_resp.authorization_code,
    )
    .await?;
    store_account_tokens(ProviderKind::OpenAiResponses, &tokens)
}

async fn poll_for_authorization_code(
    client: &reqwest::Client,
    api_base_url: &str,
    device_auth_id: &str,
    user_code: &str,
    interval: u64,
    max_wait: Duration,
) -> Result<CodeSuccessResp, AccountAuthError> {
    let url = format!("{api_base_url}/deviceauth/token");
    let started = Instant::now();

    loop {
        let response = client
            .post(&url)
            .json(&TokenPollReq {
                device_auth_id: device_auth_id.to_string(),
                user_code: user_code.to_string(),
            })
            .send()
            .await
            .map_err(|error| AccountAuthError::Http(error.to_string()))?;
        let status = response.status();

        if status.is_success() {
            return response
                .json()
                .await
                .map_err(|error| AccountAuthError::OAuth(error.to_string()));
        }

        if status == StatusCode::FORBIDDEN || status == StatusCode::NOT_FOUND {
            if started.elapsed() >= max_wait {
                return Err(AccountAuthError::OAuth(
                    "device auth timed out after 15 minutes".to_string(),
                ));
            }
            let remaining = max_wait.saturating_sub(started.elapsed());
            tokio::time::sleep(Duration::from_secs(interval).min(remaining)).await;
            continue;
        }

        return Err(AccountAuthError::OAuth(format!(
            "device auth failed with status {status}"
        )));
    }
}

fn api_accounts_base(issuer: &str) -> String {
    format!("{}/api/accounts", issuer.trim_end_matches('/'))
}

#[cfg(test)]
mod tests {
    use super::*;
    use mockito::Matcher;

    #[tokio::test]
    async fn device_code_polling_waits_through_pending_then_returns_code() {
        let mut server = mockito::Server::new_async().await;
        let _pending = server
            .mock("POST", "/api/accounts/deviceauth/token")
            .match_body(Matcher::JsonString(
                r#"{"device_auth_id":"dev-1","user_code":"USER-1"}"#.to_string(),
            ))
            .with_status(403)
            .expect(1)
            .create_async()
            .await;
        let _ok = server
            .mock("POST", "/api/accounts/deviceauth/token")
            .match_body(Matcher::JsonString(
                r#"{"device_auth_id":"dev-1","user_code":"USER-1"}"#.to_string(),
            ))
            .with_status(200)
            .with_body(
                r#"{"authorization_code":"auth-code","code_challenge":"challenge","code_verifier":"verifier"}"#,
            )
            .expect(1)
            .create_async()
            .await;

        let client = reqwest::Client::new();
        let response = poll_for_authorization_code(
            &client,
            &format!("{}/api/accounts", server.url()),
            "dev-1",
            "USER-1",
            0,
            Duration::from_secs(2),
        )
        .await
        .unwrap();

        assert_eq!(response.authorization_code, "auth-code");
        assert_eq!(response.code_verifier, "verifier");
    }

    #[tokio::test]
    async fn request_device_code_maps_usercode_response() {
        let mut server = mockito::Server::new_async().await;
        let _mock = server
            .mock("POST", "/api/accounts/deviceauth/usercode")
            .with_status(200)
            .with_body(r#"{"device_auth_id":"dev-1","user_code":"USER-1","interval":"0"}"#)
            .expect(1)
            .create_async()
            .await;
        let config = DeviceAuthConfig {
            issuer: server.url(),
            client_id: "client".to_string(),
            max_wait: Duration::from_secs(1),
        };

        let code = request_device_code(&config).await.unwrap();

        assert_eq!(code.device_auth_id, "dev-1");
        assert_eq!(code.user_code, "USER-1");
        assert_eq!(code.interval, 0);
        assert!(code.verification_url.ends_with("/codex/device"));
    }
}
