//! Live provider model discovery for the Settings model picker.
//!
//! Model dropdowns must come from the provider's own `/models` API for the
//! user's key. Static model catalogs go stale exactly when new releases matter,
//! so this module is the single discovery path plus a last-good cache for
//! offline/error states.

use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::time::Duration;

use chrono::Utc;
use reqwest::StatusCode;
use reqwest::blocking::Client;
use serde::{Deserialize, Serialize};
use tracing::warn;

use crate::config::Config;
use crate::config::DEFAULT_OPENAI_RESPONSES_ENDPOINT;
use crate::llm::provider::ProviderKind;

const DISCOVERY_TIMEOUT: Duration = Duration::from_secs(5);
const CACHE_FILE_NAME: &str = "model_discovery_cache.json";
const ANTHROPIC_MODELS_ENDPOINT: &str = "https://api.anthropic.com/v1/models";
const ANTHROPIC_VERSION: &str = "2023-06-01";

/// One discovered provider model. `id` is sent on the wire; `display_name` is
/// provider-provided when available and otherwise falls back to `id`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiscoveredModel {
    pub id: String,
    pub display_name: String,
}

/// Whether returned models came from the live provider or the last-good cache.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModelDiscoveryStatus {
    Fresh,
    Cached { reason: String },
}

/// Successful model discovery result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelDiscoveryResult {
    pub provider: ProviderKind,
    pub models: Vec<DiscoveredModel>,
    pub status: ModelDiscoveryStatus,
}

/// Discovery failure. These map cleanly to UI status strings; no variant carries
/// API key material.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModelDiscoveryError {
    NoKey {
        provider: ProviderKind,
        env_key: &'static str,
    },
    Network {
        provider: ProviderKind,
        message: String,
    },
    HttpStatus {
        provider: ProviderKind,
        status: u16,
        message: String,
    },
    Parse {
        provider: ProviderKind,
        message: String,
    },
    Cache {
        provider: ProviderKind,
        message: String,
    },
}

impl ModelDiscoveryError {
    pub const fn provider(&self) -> ProviderKind {
        match self {
            Self::NoKey { provider, .. }
            | Self::Network { provider, .. }
            | Self::HttpStatus { provider, .. }
            | Self::Parse { provider, .. }
            | Self::Cache { provider, .. } => *provider,
        }
    }

    pub const fn code(&self) -> &'static str {
        match self {
            Self::NoKey { .. } => "no_key",
            Self::Network { .. } => "network",
            Self::HttpStatus { .. } => "http_status",
            Self::Parse { .. } => "parse",
            Self::Cache { .. } => "cache",
        }
    }

    pub fn message(&self) -> String {
        match self {
            Self::NoKey { env_key, .. } => format!("{env_key} is not configured"),
            Self::Network { message, .. }
            | Self::HttpStatus { message, .. }
            | Self::Parse { message, .. }
            | Self::Cache { message, .. } => message.clone(),
        }
    }
}

impl std::fmt::Display for ModelDiscoveryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoKey { provider, env_key } => {
                write!(f, "{provider}: no_key: {env_key} is not configured")
            }
            Self::Network { provider, message } => {
                write!(f, "{provider}: network: {message}")
            }
            Self::HttpStatus {
                provider,
                status,
                message,
            } => write!(f, "{provider}: http_status {status}: {message}"),
            Self::Parse { provider, message } => write!(f, "{provider}: parse: {message}"),
            Self::Cache { provider, message } => write!(f, "{provider}: cache: {message}"),
        }
    }
}

impl std::error::Error for ModelDiscoveryError {}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CachedProviderModels {
    fetched_at: String,
    models: Vec<DiscoveredModel>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct DiscoveryCacheFile {
    providers: BTreeMap<String, CachedProviderModels>,
}

#[derive(Debug, Deserialize)]
struct OpenAiModelsResponse {
    data: Vec<OpenAiModel>,
}

#[derive(Debug, Deserialize)]
struct OpenAiModel {
    id: String,
}

#[derive(Debug, Deserialize)]
struct AnthropicModelsResponse {
    data: Vec<AnthropicModel>,
    #[serde(default)]
    has_more: bool,
    last_id: Option<String>,
}

#[derive(Debug, Deserialize)]
struct AnthropicModel {
    id: String,
    display_name: Option<String>,
}

/// Discover models for a provider using the already-supported config/key path.
///
/// `Config::load()` is intentionally the first operation: it applies settings,
/// optional `.env`, and Keychain population exactly like the provider runtime.
/// Missing keys are hard `no_key` failures and do not fall back to stale cache;
/// network/http/parse failures return last-good cache when available.
pub fn discover_models(
    provider: ProviderKind,
) -> Result<ModelDiscoveryResult, ModelDiscoveryError> {
    let config = Config::load();
    let key_name = provider.api_key_env_key();
    let api_key = env_non_empty(key_name).ok_or(ModelDiscoveryError::NoKey {
        provider,
        env_key: key_name,
    })?;

    let client = Client::builder()
        .timeout(DISCOVERY_TIMEOUT)
        .build()
        .map_err(|error| ModelDiscoveryError::Network {
            provider,
            message: format!("failed to create HTTP client: {error}"),
        })?;

    let fetched = match provider {
        ProviderKind::OpenAiResponses => fetch_openai_models(&client, &config, &api_key),
        ProviderKind::AnthropicMessages => fetch_anthropic_models(&client, &api_key),
    };

    match fetched {
        Ok(models) => {
            let models = normalize_models(models);
            if let Err(error) = write_cache(provider, &models) {
                warn!("{error}");
            }
            Ok(ModelDiscoveryResult {
                provider,
                models,
                status: ModelDiscoveryStatus::Fresh,
            })
        }
        Err(error) => match read_cached_models(provider) {
            Ok(models) if !models.is_empty() => Ok(ModelDiscoveryResult {
                provider,
                models,
                status: ModelDiscoveryStatus::Cached {
                    reason: error.message(),
                },
            }),
            _ => Err(error),
        },
    }
}

fn fetch_openai_models(
    client: &Client,
    config: &Config,
    api_key: &str,
) -> Result<Vec<DiscoveredModel>, ModelDiscoveryError> {
    let provider = ProviderKind::OpenAiResponses;
    let endpoint = env_non_empty("LLM_ASSISTIVE_ENDPOINT")
        .or_else(|| env_non_empty("LLM_ENDPOINT"))
        .or_else(|| config.llm_endpoint.clone())
        .unwrap_or_else(|| DEFAULT_OPENAI_RESPONSES_ENDPOINT.to_string());
    let endpoint = openai_models_endpoint(&endpoint)?;

    let response = client
        .get(endpoint)
        .bearer_auth(api_key)
        .send()
        .map_err(|error| network_error(provider, error))?;
    let body = response_body_or_error(provider, response)?;
    let parsed: OpenAiModelsResponse =
        serde_json::from_str(&body).map_err(|error| ModelDiscoveryError::Parse {
            provider,
            message: format!("failed to parse OpenAI models response: {error}"),
        })?;

    Ok(parsed
        .data
        .into_iter()
        .map(|model| DiscoveredModel {
            display_name: model.id.clone(),
            id: model.id,
        })
        .collect())
}

fn fetch_anthropic_models(
    client: &Client,
    api_key: &str,
) -> Result<Vec<DiscoveredModel>, ModelDiscoveryError> {
    let provider = ProviderKind::AnthropicMessages;
    let endpoint = anthropic_models_endpoint();
    let mut after_id: Option<String> = None;
    let mut models = Vec::new();

    loop {
        let mut request = client
            .get(&endpoint)
            .header("x-api-key", api_key)
            .header("anthropic-version", ANTHROPIC_VERSION);
        if let Some(after) = after_id.as_deref() {
            request = request.query(&[("after_id", after)]);
        }

        let response = request
            .send()
            .map_err(|error| network_error(provider, error))?;
        let body = response_body_or_error(provider, response)?;
        let parsed: AnthropicModelsResponse =
            serde_json::from_str(&body).map_err(|error| ModelDiscoveryError::Parse {
                provider,
                message: format!("failed to parse Anthropic models response: {error}"),
            })?;

        let next_after_id = parsed
            .last_id
            .clone()
            .or_else(|| parsed.data.last().map(|model| model.id.clone()));

        models.extend(parsed.data.into_iter().map(|model| {
            let display_name = model
                .display_name
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| model.id.clone());
            DiscoveredModel {
                id: model.id,
                display_name,
            }
        }));

        if !parsed.has_more {
            break;
        }

        after_id = Some(next_after_id.ok_or_else(|| ModelDiscoveryError::Parse {
            provider,
            message:
                "Anthropic models response has has_more=true without last_id or data".to_string(),
        })?);
    }

    Ok(models)
}

fn response_body_or_error(
    provider: ProviderKind,
    response: reqwest::blocking::Response,
) -> Result<String, ModelDiscoveryError> {
    let status = response.status();
    let body = response
        .text()
        .map_err(|error| network_error(provider, error))?;
    if status.is_success() {
        Ok(body)
    } else {
        Err(http_status_error(provider, status, &body))
    }
}

fn network_error(provider: ProviderKind, error: reqwest::Error) -> ModelDiscoveryError {
    ModelDiscoveryError::Network {
        provider,
        message: error.to_string(),
    }
}

fn http_status_error(
    provider: ProviderKind,
    status: StatusCode,
    body: &str,
) -> ModelDiscoveryError {
    let mut message = body.trim().replace('\n', " ");
    if message.len() > 240 {
        message.truncate(240);
        message.push_str("...");
    }
    if message.is_empty() {
        message = status
            .canonical_reason()
            .unwrap_or("provider returned an error")
            .to_string();
    }
    ModelDiscoveryError::HttpStatus {
        provider,
        status: status.as_u16(),
        message,
    }
}

fn openai_models_endpoint(endpoint: &str) -> Result<String, ModelDiscoveryError> {
    let provider = ProviderKind::OpenAiResponses;
    let mut url = reqwest::Url::parse(endpoint).map_err(|error| ModelDiscoveryError::Parse {
        provider,
        message: format!("invalid OpenAI endpoint '{endpoint}': {error}"),
    })?;
    url.set_query(None);
    url.set_fragment(None);

    let segments: Vec<String> = url
        .path_segments()
        .map(|parts| {
            parts
                .filter(|segment| !segment.is_empty())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    let mut next = segments;

    if next.last().is_some_and(|segment| segment == "models") {
        // already right
    } else if next
        .last()
        .is_some_and(|segment| segment == "responses" || segment == "completions")
    {
        next.pop();
        if next.last().is_some_and(|segment| segment == "chat") {
            next.pop();
        }
        next.push("models".to_string());
    } else {
        next.push("models".to_string());
    }

    url.path_segments_mut()
        .map_err(|_| ModelDiscoveryError::Parse {
            provider,
            message: format!("invalid OpenAI endpoint base '{endpoint}'"),
        })?
        .clear()
        .extend(next.iter().map(String::as_str));
    Ok(url.to_string())
}

fn normalize_models(models: Vec<DiscoveredModel>) -> Vec<DiscoveredModel> {
    let mut seen = HashSet::new();
    models
        .into_iter()
        .filter_map(|model| {
            let id = model.id.trim().to_string();
            if id.is_empty() || !seen.insert(id.clone()) {
                return None;
            }
            let display_name = model.display_name.trim().to_string();
            Some(DiscoveredModel {
                display_name: if display_name.is_empty() {
                    id.clone()
                } else {
                    display_name
                },
                id,
            })
        })
        .collect()
}

fn cache_path() -> std::path::PathBuf {
    Config::config_dir().join(CACHE_FILE_NAME)
}

fn read_cache_file() -> Result<DiscoveryCacheFile, ModelDiscoveryError> {
    let path = cache_path();
    match fs::read_to_string(&path) {
        Ok(raw) => serde_json::from_str(&raw).map_err(|error| ModelDiscoveryError::Cache {
            provider: ProviderKind::OpenAiResponses,
            message: format!("failed to parse {}: {error}", path.display()),
        }),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            Ok(DiscoveryCacheFile::default())
        }
        Err(error) => Err(ModelDiscoveryError::Cache {
            provider: ProviderKind::OpenAiResponses,
            message: format!("failed to read {}: {error}", path.display()),
        }),
    }
}

fn read_cached_models(provider: ProviderKind) -> Result<Vec<DiscoveredModel>, ModelDiscoveryError> {
    let cache = read_cache_file().map_err(|error| ModelDiscoveryError::Cache {
        provider,
        message: error.message(),
    })?;
    Ok(cache
        .providers
        .get(provider.as_str())
        .map(|entry| normalize_models(entry.models.clone()))
        .unwrap_or_default())
}

fn write_cache(
    provider: ProviderKind,
    models: &[DiscoveredModel],
) -> Result<(), ModelDiscoveryError> {
    let path = cache_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| ModelDiscoveryError::Cache {
            provider,
            message: format!("failed to create {}: {error}", parent.display()),
        })?;
    }

    let mut cache = read_cache_file().unwrap_or_default();
    cache.providers.insert(
        provider.as_str().to_string(),
        CachedProviderModels {
            fetched_at: Utc::now().to_rfc3339(),
            models: models.to_vec(),
        },
    );

    let raw = serde_json::to_string_pretty(&cache).map_err(|error| ModelDiscoveryError::Cache {
        provider,
        message: format!("failed to serialize model discovery cache: {error}"),
    })?;
    fs::write(&path, raw).map_err(|error| ModelDiscoveryError::Cache {
        provider,
        message: format!("failed to write {}: {error}", path.display()),
    })
}

fn env_non_empty(key: &str) -> Option<String> {
    std::env::var(key)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn anthropic_models_endpoint() -> String {
    #[cfg(test)]
    if let Some(endpoint) = env_non_empty("CODESCRIBE_TEST_ANTHROPIC_MODELS_ENDPOINT") {
        return endpoint;
    }

    ANTHROPIC_MODELS_ENDPOINT.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use mockito::Matcher;
    use serial_test::serial;
    use tempfile::TempDir;

    #[test]
    #[serial]
    fn openai_models_parse_and_cache_round_trips() {
        let mut env = TestEnv::new();
        let mut server = mockito::Server::new();
        env.set("LLM_ASSISTIVE_API_KEY", "sk-test");
        env.set(
            "LLM_ASSISTIVE_ENDPOINT",
            &format!("{}/v1/responses", server.url()),
        );

        let _mock = server
            .mock("GET", "/v1/models")
            .match_header("authorization", "Bearer sk-test")
            .with_status(200)
            .with_body(r#"{"object":"list","data":[{"id":"gpt-live"},{"id":"gpt-other"}]}"#)
            .create();

        let result = discover_models(ProviderKind::OpenAiResponses).unwrap();

        assert_eq!(result.status, ModelDiscoveryStatus::Fresh);
        assert_eq!(
            result.models,
            vec![
                DiscoveredModel {
                    id: "gpt-live".to_string(),
                    display_name: "gpt-live".to_string(),
                },
                DiscoveredModel {
                    id: "gpt-other".to_string(),
                    display_name: "gpt-other".to_string(),
                },
            ]
        );

        let cached = read_cached_models(ProviderKind::OpenAiResponses).unwrap();
        assert_eq!(cached, result.models);
        env.keepalive();
    }

    #[test]
    #[serial]
    fn anthropic_models_parse_display_names_and_pagination() {
        let mut env = TestEnv::new();
        let mut server = mockito::Server::new();
        env.set("LLM_ANTHROPIC_API_KEY", "anthropic-test");
        env.set(
            "CODESCRIBE_TEST_ANTHROPIC_MODELS_ENDPOINT",
            &format!("{}/v1/models", server.url()),
        );

        let _page_1 = server
            .mock("GET", "/v1/models")
            .match_header("x-api-key", "anthropic-test")
            .match_header("anthropic-version", ANTHROPIC_VERSION)
            .with_status(200)
            .with_body(
                r#"{"data":[{"id":"claude-a","display_name":"Claude A"}],"has_more":true,"last_id":"claude-a"}"#,
            )
            .create();
        let _page_2 = server
            .mock("GET", "/v1/models")
            .match_query(Matcher::UrlEncoded("after_id".to_string(), "claude-a".to_string()))
            .match_header("x-api-key", "anthropic-test")
            .match_header("anthropic-version", ANTHROPIC_VERSION)
            .with_status(200)
            .with_body(
                r#"{"data":[{"id":"claude-b","display_name":"Claude B"},{"id":"claude-c"}],"has_more":false}"#,
            )
            .create();

        let result = discover_models(ProviderKind::AnthropicMessages).unwrap();

        assert_eq!(result.status, ModelDiscoveryStatus::Fresh);
        assert_eq!(
            result.models,
            vec![
                DiscoveredModel {
                    id: "claude-a".to_string(),
                    display_name: "Claude A".to_string(),
                },
                DiscoveredModel {
                    id: "claude-b".to_string(),
                    display_name: "Claude B".to_string(),
                },
                DiscoveredModel {
                    id: "claude-c".to_string(),
                    display_name: "claude-c".to_string(),
                },
            ]
        );
        env.keepalive();
    }

    #[test]
    #[serial]
    fn no_key_returns_error_without_request() {
        let mut env = TestEnv::new();
        let mut server = mockito::Server::new();
        env.remove("LLM_ANTHROPIC_API_KEY");
        env.set(
            "CODESCRIBE_TEST_ANTHROPIC_MODELS_ENDPOINT",
            &format!("{}/v1/models", server.url()),
        );
        let _mock = server.mock("GET", "/v1/models").expect(0).create();

        let err = discover_models(ProviderKind::AnthropicMessages).unwrap_err();

        assert_eq!(err.code(), "no_key");
        assert_eq!(err.provider(), ProviderKind::AnthropicMessages);
        env.keepalive();
    }

    #[test]
    #[serial]
    fn network_error_uses_last_good_cache() {
        let mut env = TestEnv::new();
        let mut server = mockito::Server::new();
        env.set("LLM_ASSISTIVE_API_KEY", "sk-test");
        env.set(
            "LLM_ASSISTIVE_ENDPOINT",
            &format!("{}/v1/responses", server.url()),
        );

        let _ok = server
            .mock("GET", "/v1/models")
            .with_status(200)
            .with_body(r#"{"data":[{"id":"gpt-cached"}]}"#)
            .create();
        let fresh = discover_models(ProviderKind::OpenAiResponses).unwrap();
        assert_eq!(fresh.status, ModelDiscoveryStatus::Fresh);

        let _fail = server
            .mock("GET", "/v1/models")
            .with_status(503)
            .with_body("temporarily unavailable")
            .create();
        let cached = discover_models(ProviderKind::OpenAiResponses).unwrap();

        assert_eq!(
            cached.status,
            ModelDiscoveryStatus::Cached {
                reason: "temporarily unavailable".to_string(),
            }
        );
        assert_eq!(cached.models, fresh.models);
        env.keepalive();
    }

    #[test]
    fn openai_endpoint_normalizes_common_api_paths() {
        assert_eq!(
            openai_models_endpoint("https://api.openai.com/v1/responses").unwrap(),
            "https://api.openai.com/v1/models"
        );
        assert_eq!(
            openai_models_endpoint("https://api.openai.com/v1/chat/completions").unwrap(),
            "https://api.openai.com/v1/models"
        );
        assert_eq!(
            openai_models_endpoint("https://proxy.example/openai").unwrap(),
            "https://proxy.example/openai/models"
        );
    }

    struct TestEnv {
        _tmp: TempDir,
        guards: Vec<EnvGuard>,
    }

    impl TestEnv {
        fn new() -> Self {
            let tmp = tempfile::tempdir().unwrap();
            let mut this = Self {
                _tmp: tmp,
                guards: Vec::new(),
            };
            let data_dir = this._tmp.path().to_string_lossy().to_string();
            this.set("CODESCRIBE_DATA_DIR", &data_dir);
            this.set("CODESCRIBE_DISABLE_KEYCHAIN", "1");
            this.remove("LLM_ASSISTIVE_API_KEY");
            this.remove("LLM_ANTHROPIC_API_KEY");
            this.remove("CODESCRIBE_TEST_ANTHROPIC_MODELS_ENDPOINT");
            this.remove("LLM_ASSISTIVE_ENDPOINT");
            this.remove("LLM_ENDPOINT");
            this
        }

        fn set(&mut self, key: &'static str, value: &str) {
            self.guards.push(EnvGuard::set(key, value));
        }

        fn remove(&mut self, key: &'static str) {
            self.guards.push(EnvGuard::remove(key));
        }

        fn keepalive(&self) {}
    }

    struct EnvGuard {
        key: &'static str,
        prev: Option<String>,
    }

    impl EnvGuard {
        fn set(key: &'static str, value: &str) -> Self {
            let prev = std::env::var(key).ok();
            unsafe { std::env::set_var(key, value) };
            Self { key, prev }
        }

        fn remove(key: &'static str) -> Self {
            let prev = std::env::var(key).ok();
            unsafe { std::env::remove_var(key) };
            Self { key, prev }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            match self.prev.as_deref() {
                Some(value) => unsafe { std::env::set_var(self.key, value) },
                None => unsafe { std::env::remove_var(self.key) },
            }
        }
    }
}
