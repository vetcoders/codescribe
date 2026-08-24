//! Live provider model discovery for the Settings model picker.
//!
//! Model dropdowns must come from the provider's own `/models` API for the
//! user's key. Static model catalogs go stale exactly when new releases matter,
//! so this module is the single discovery path plus a last-good cache for
//! offline/error states.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use chrono::Utc;
use reqwest::Client;
use reqwest::StatusCode;
use serde::{Deserialize, Serialize};
use tokio::sync::oneshot;
use tracing::warn;

use crate::config::RuntimeLlmLane;
use crate::llm::provider::{ProviderKind, WireFamily};

/// 5s client timeout for live /models discovery.
/// P2-09: short to keep Settings responsive. If provider is slow, we degrade to
/// cache rather than hang the picker. Justified as UX bound, not a knob (per
/// charter: no new Settings controls). P2-08: a newer discovery for the same
/// provider additionally aborts the in-flight request (per-provider generation
/// registry below), so this timeout is the worst case, not the supersede path.
const DISCOVERY_TIMEOUT: Duration = Duration::from_secs(5);
/// Last-good models cache filename under the Codescribe config dir.
const CACHE_FILE_NAME: &str = "model_discovery_cache.json";
/// Required `anthropic-version` header for the Models endpoint.
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
        env_key: String,
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
    /// A newer discovery request for the same provider superseded this one.
    /// The stale request is aborted and its result never touches cache/state;
    /// callers should drop this outcome silently (the newer request answers).
    Cancelled { provider: ProviderKind },
}

impl ModelDiscoveryError {
    /// Which provider failed. Needed because Settings refreshes several
    /// providers at once and must attribute each failure to its own row.
    pub const fn provider(&self) -> ProviderKind {
        match self {
            Self::NoKey { provider, .. }
            | Self::Network { provider, .. }
            | Self::HttpStatus { provider, .. }
            | Self::Parse { provider, .. }
            | Self::Cache { provider, .. }
            | Self::Cancelled { provider } => *provider,
        }
    }

    /// Stable machine-readable discriminant for the UI and tests. Distinct from
    /// [`Self::message`], which is prose the operator reads.
    pub const fn code(&self) -> &'static str {
        match self {
            Self::NoKey { .. } => "no_key",
            Self::Network { .. } => "network",
            Self::HttpStatus { .. } => "http_status",
            Self::Parse { .. } => "parse",
            Self::Cache { .. } => "cache",
            Self::Cancelled { .. } => "cancelled",
        }
    }

    /// Operator-facing explanation. Also the `reason` recorded when a failure
    /// degrades to [`ModelDiscoveryStatus::Cached`], which is why it never
    /// interpolates key material — only the env key's name.
    pub fn message(&self) -> String {
        match self {
            Self::NoKey { env_key, .. } => format!("{env_key} is not configured"),
            Self::Cancelled { .. } => "superseded by a newer discovery request".to_string(),
            Self::Network { message, .. }
            | Self::HttpStatus { message, .. }
            | Self::Parse { message, .. }
            | Self::Cache { message, .. } => message.clone(),
        }
    }
}

impl std::fmt::Display for ModelDiscoveryError {
    /// `provider: code: message` — never interpolates key material.
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
            Self::Cancelled { provider } => {
                write!(
                    f,
                    "{provider}: cancelled: superseded by a newer discovery request"
                )
            }
        }
    }
}

impl std::error::Error for ModelDiscoveryError {}

/// Last-good models for one provider. `fetched_at` is informational only —
/// the cache has no expiry, because a stale picker still beats an empty one.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct CachedProviderModels {
    fetched_at: String,
    models: Vec<DiscoveredModel>,
}

/// On-disk shape of the discovery cache. Keyed by provider so one provider's
/// failure can never invalidate another's last-good list; `BTreeMap` keeps the
/// serialized file diff-stable.
#[derive(Debug, Default, Serialize, Deserialize)]
struct DiscoveryCacheFile {
    providers: BTreeMap<String, CachedProviderModels>,
}

/// Envelope of `GET /v1/models` on the OpenAI wire family.
#[derive(Debug, Deserialize)]
struct OpenAiModelsResponse {
    data: Vec<OpenAiModel>,
}

/// One OpenAI-family model. Only `id` is read: the protocol carries no display
/// name, so the picker falls back to the id itself.
#[derive(Debug, Deserialize)]
struct OpenAiModel {
    id: String,
}

/// One page of Anthropic's `/v1/models`. Unlike the OpenAI family this endpoint
/// paginates, so `has_more` and `last_id` drive the fetch loop.
#[derive(Debug, Deserialize)]
struct AnthropicModelsResponse {
    data: Vec<AnthropicModel>,
    #[serde(default)]
    has_more: bool,
    last_id: Option<String>,
}

/// One Anthropic model. `display_name` is optional on the wire, hence the
/// fallback to `id` when it is absent or blank.
#[derive(Debug, Deserialize)]
struct AnthropicModel {
    id: String,
    display_name: Option<String>,
}

/// Per-provider discovery generation: `current` is the newest claimed request,
/// `cancel` aborts the in-flight fetch of that generation when superseded.
struct GenerationSlot {
    current: u64,
    cancel: Option<oneshot::Sender<()>>,
}

/// P2-08: generations are per-provider because Settings discovers several
/// providers in one refresh batch — an Anthropic refresh must never abort an
/// in-flight OpenAI fetch (mirrors the per-provider counters in Swift).
fn generation_registry() -> &'static Mutex<HashMap<ProviderKind, GenerationSlot>> {
    /// Process-wide per-provider discovery generation + cancel handles.
    static REGISTRY: OnceLock<Mutex<HashMap<ProviderKind, GenerationSlot>>> = OnceLock::new();
    REGISTRY.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Claim the next discovery generation for `provider`, firing the cancel
/// signal of the previous in-flight request (if any).
fn claim_generation(provider: ProviderKind) -> (u64, oneshot::Receiver<()>) {
    let (cancel_tx, cancel_rx) = oneshot::channel();
    let mut registry = generation_registry()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let slot = registry.entry(provider).or_insert(GenerationSlot {
        current: 0,
        cancel: None,
    });
    slot.current += 1;
    if let Some(previous) = slot.cancel.take() {
        let _ = previous.send(());
    }
    slot.cancel = Some(cancel_tx);
    (slot.current, cancel_rx)
}

/// Mark `generation` as finished. Returns false when a newer generation
/// superseded it mid-flight — the caller must then discard its result.
fn finish_generation(provider: ProviderKind, generation: u64) -> bool {
    let mut registry = generation_registry()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    match registry.get_mut(&provider) {
        Some(slot) if slot.current == generation => {
            slot.cancel = None;
            true
        }
        _ => false,
    }
}

/// One-shot callback fired right after a generation is claimed. `FnOnce` so a
/// test cannot accidentally arm the same interference twice.
#[cfg(test)]
type AfterClaimHook = Box<dyn FnOnce(ProviderKind) + Send>;

/// Process-wide slot holding the armed hook. Tests using it run `#[serial]`,
/// since the slot — like the generation registry — is global state.
#[cfg(test)]
fn test_after_claim_hook() -> &'static Mutex<Option<AfterClaimHook>> {
    /// Armed cancel-interference hook; tests must be `#[serial]`.
    static HOOK: OnceLock<Mutex<Option<AfterClaimHook>>> = OnceLock::new();
    HOOK.get_or_init(|| Mutex::new(None))
}

/// Test seam: lets a test supersede the just-claimed generation before the
/// fetch starts, making the cancel path deterministic without real network.
#[cfg(test)]
fn run_test_after_claim(provider: ProviderKind) {
    let hook = test_after_claim_hook()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .take();
    if let Some(hook) = hook {
        hook(provider);
    }
}

/// Discover models through one already-resolved runtime lane.
///
/// The endpoint, provider, protocol, and credential all come from the same
/// immutable settings snapshot as the request path.
/// Missing keys are hard `no_key` failures and do not fall back to stale cache;
/// network/http/parse failures return last-good cache when available.
///
/// P2-08: each call claims a per-provider generation; a newer call for the same
/// provider aborts the in-flight fetch (`tokio::select!` on the cancel channel)
/// and a superseded result never writes cache — it surfaces as `Cancelled`.
pub fn discover_models(lane: &RuntimeLlmLane) -> Result<ModelDiscoveryResult, ModelDiscoveryError> {
    let provider = lane.provider();
    let key_name = lane.credential().key_account();
    let api_key = lane
        .credential()
        .api_key()
        .map(str::to_string)
        .ok_or(ModelDiscoveryError::NoKey {
            provider,
            env_key: key_name.to_string(),
        })?;

    let (generation, cancelled) = claim_generation(provider);
    #[cfg(test)]
    run_test_after_claim(provider);

    let client = Client::builder()
        .timeout(DISCOVERY_TIMEOUT)
        .build()
        .map_err(|error| ModelDiscoveryError::Network {
            provider,
            message: format!("failed to create HTTP client: {error}"),
        })?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|error| ModelDiscoveryError::Network {
            provider,
            message: format!("failed to start discovery runtime: {error}"),
        })?;

    let fetched = runtime.block_on(async {
        let fetch = async {
            // Discovery follows the protocol, not the vendor: every Responses
            // provider serves the same OpenAI-compatible `/v1/models`.
            match provider.wire_family() {
                WireFamily::OpenAiResponses => {
                    fetch_openai_models(&client, lane.endpoint(), provider, &api_key).await
                }
                WireFamily::AnthropicMessages => {
                    fetch_anthropic_models(&client, lane.endpoint(), &api_key).await
                }
            }
        };
        // `biased` polls the cancel channel first: a pre-fired cancel aborts
        // before the request is even sent; mid-flight, dropping the fetch
        // future tears down the HTTP connection.
        tokio::select! {
            biased;
            _ = cancelled => None,
            result = fetch => Some(result),
        }
    });

    let Some(fetched) = fetched else {
        return Err(ModelDiscoveryError::Cancelled { provider });
    };
    commit_fetch_outcome(provider, generation, fetched)
}

/// Apply a finished fetch to module state (cache write / cache fallback).
/// A generation superseded between fetch completion and commit must not leak:
/// no cache write, no cache fallback — plain `Cancelled`.
fn commit_fetch_outcome(
    provider: ProviderKind,
    generation: u64,
    fetched: Result<Vec<DiscoveredModel>, ModelDiscoveryError>,
) -> Result<ModelDiscoveryResult, ModelDiscoveryError> {
    if !finish_generation(provider, generation) {
        return Err(ModelDiscoveryError::Cancelled { provider });
    }

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

/// Fetch models from an OpenAI-family provider, deriving the `/models` URL from
/// the endpoint the runtime already uses for this lane — so a custom proxy or
/// gateway is discovered against the same host that will serve inference.
async fn fetch_openai_models(
    client: &Client,
    lane_endpoint: &str,
    provider: ProviderKind,
    api_key: &str,
) -> Result<Vec<DiscoveredModel>, ModelDiscoveryError> {
    let endpoint = provider_models_endpoint(lane_endpoint, provider)?;

    let response = client
        .get(endpoint)
        .bearer_auth(api_key)
        .send()
        .await
        .map_err(|error| network_error(provider, error))?;
    let body = response_body_or_error(provider, response).await?;
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

/// Fetch the full Anthropic model list, following `after_id` pagination until
/// the provider reports no more pages. A `has_more` page without a cursor is a
/// parse error rather than a silent truncation of the picker.
async fn fetch_anthropic_models(
    client: &Client,
    lane_endpoint: &str,
    api_key: &str,
) -> Result<Vec<DiscoveredModel>, ModelDiscoveryError> {
    let provider = ProviderKind::AnthropicMessages;
    let endpoint = provider_models_endpoint(lane_endpoint, provider)?;
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
            .await
            .map_err(|error| network_error(provider, error))?;
        let body = response_body_or_error(provider, response).await?;
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

/// Read the body, then classify by status. Reading first is deliberate: an
/// error body usually holds the provider's own explanation, which is more use
/// to the operator than a bare status line.
async fn response_body_or_error(
    provider: ProviderKind,
    response: reqwest::Response,
) -> Result<String, ModelDiscoveryError> {
    let status = response.status();
    let body = response
        .text()
        .await
        .map_err(|error| network_error(provider, error))?;
    if status.is_success() {
        Ok(body)
    } else {
        Err(http_status_error(provider, status, &body))
    }
}

/// Wrap a transport failure (DNS, TLS, timeout) as a `Network` error — the
/// class that is allowed to fall back to last-good cache.
fn network_error(provider: ProviderKind, error: reqwest::Error) -> ModelDiscoveryError {
    ModelDiscoveryError::Network {
        provider,
        message: error.to_string(),
    }
}

/// Turn a non-2xx response into an error carrying a single-line, length-capped
/// excerpt of the body — enough to diagnose, small enough for a status label,
/// and with the status reason as fallback when the body is empty.
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

/// Derive the `/models` URL from a configured inference endpoint.
///
/// Operators paste whatever their provider documents — `/v1/responses`,
/// `/v1/chat/completions`, or a bare proxy base — so the trailing inference
/// segment is swapped for `models` rather than assumed. Query and fragment are
/// dropped: they belong to the inference call, not to discovery.
fn provider_models_endpoint(
    endpoint: &str,
    provider: ProviderKind,
) -> Result<String, ModelDiscoveryError> {
    let mut url = reqwest::Url::parse(endpoint).map_err(|error| ModelDiscoveryError::Parse {
        provider,
            message: format!("invalid {} endpoint '{endpoint}': {error}", provider.display_name()),
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
        .is_some_and(|segment| {
            segment == "responses" || segment == "completions" || segment == "messages"
        })
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
            message: format!("invalid {} endpoint base '{endpoint}'", provider.display_name()),
        })?
        .clear()
        .extend(next.iter().map(String::as_str));
    Ok(url.to_string())
}

/// Trim, drop blanks, and de-duplicate by id while preserving provider order —
/// the picker shows this list verbatim. Applied on both write and read, so a
/// cache file that predates a normalization rule is still cleaned up on load.
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

/// Cache location, resolved through `Config` so an isolated data dir (tests,
/// portable installs) redirects it along with everything else.
fn cache_path() -> std::path::PathBuf {
    Config::config_dir().join(CACHE_FILE_NAME)
}

/// Read the whole cache file. A missing file is an empty cache, not an error —
/// first run must not surface as a discovery failure.
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

/// Last-good models for one provider, re-normalized on read. Absent entry
/// yields an empty list, which callers treat as "no cache to fall back to".
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

/// Replace this provider's cache entry, leaving other providers untouched.
/// A failure here is logged rather than propagated: losing the cache write
/// must not turn a successful live discovery into a failed one.
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

/// Discovery is exercised against a mock HTTP server and an isolated data dir,
/// so no case depends on a real provider or on the operator's own config.
///
/// Every test touching the module's global generation registry is `#[serial]`:
/// the counters are process-wide, and a parallel run would cancel generations
/// belonging to another test.
#[cfg(test)]
mod tests {
    use super::*;
    use mockito::Matcher;
    use serial_test::serial;
    use tempfile::TempDir;

    fn discover_assistive_models() -> Result<ModelDiscoveryResult, ModelDiscoveryError> {
        let runtime_settings = Config::load_runtime_snapshot().expect("runtime settings seal");
        discover_models(runtime_settings.llm_lanes().assistive())
    }

    /// Live OpenAI discovery reports `Fresh` and lands in the cache verbatim,
    /// so the next offline refresh has something to fall back to.
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

        let result = discover_assistive_models()
            .expect("discover_models should succeed in OpenAI test path");

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

        let cached = read_cached_models(ProviderKind::OpenAiResponses)
            .expect("read_cached_models should succeed after fresh discovery write");
        assert_eq!(cached, result.models);
        env.keepalive();
    }

    /// Pagination is followed to the end and display names survive, including
    /// the fallback to `id` for a model that ships without one. A regression
    /// here silently truncates the picker to page one.
    #[test]
    #[serial]
    fn anthropic_models_parse_display_names_and_pagination() {
        let mut env = TestEnv::new();
        let mut server = mockito::Server::new();
        env.set("LLM_ASSISTIVE_PROVIDER", "anthropic-messages");
        env.set("LLM_ANTHROPIC_API_KEY", "anthropic-test");
        env.set(
            "LLM_ANTHROPIC_ENDPOINT",
            &format!("{}/v1/messages", server.url()),
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

        let result = discover_assistive_models()
            .expect("discover_models should succeed in Anthropic test path");

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

    /// A missing key fails before anything reaches the wire — asserted by the
    /// mock's `expect(0)`, not merely by the returned error code.
    #[test]
    #[serial]
    fn no_key_returns_error_without_request() {
        let mut env = TestEnv::new();
        let mut server = mockito::Server::new();
        env.set("LLM_ASSISTIVE_PROVIDER", "anthropic-messages");
        env.remove("LLM_ANTHROPIC_API_KEY");
        env.set(
            "LLM_ANTHROPIC_ENDPOINT",
            &format!("{}/v1/messages", server.url()),
        );
        let _mock = server.mock("GET", "/v1/models").expect(0).create();

        let err = discover_assistive_models()
            .expect_err("discover_models should fail without key in this Anthropic error test");

        assert_eq!(err.code(), "no_key");
        assert_eq!(err.provider(), ProviderKind::AnthropicMessages);
        env.keepalive();
    }

    /// A provider outage degrades the picker to the last-good list with the
    /// failure carried as `reason`, instead of emptying it.
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
        let fresh = discover_assistive_models()
            .expect("discover_models should succeed for cache freshness test");
        assert_eq!(fresh.status, ModelDiscoveryStatus::Fresh);

        let _fail = server
            .mock("GET", "/v1/models")
            .with_status(503)
            .with_body("temporarily unavailable")
            .create();
        let cached = discover_assistive_models()
            .expect("discover_models should return cached result without network");

        assert_eq!(
            cached.status,
            ModelDiscoveryStatus::Cached {
                reason: "temporarily unavailable".to_string(),
            }
        );
        assert_eq!(cached.models, fresh.models);
        env.keepalive();
    }

    /// A superseded request is abandoned before a single byte goes out — the
    /// `biased` select must see the pre-fired cancel first. Proven by the mock
    /// asserting zero requests, and by the cache staying empty.
    #[test]
    #[serial]
    fn superseding_generation_cancels_inflight_discovery() {
        let mut env = TestEnv::new();
        let mut server = mockito::Server::new();
        env.set("LLM_ASSISTIVE_API_KEY", "sk-test");
        env.set(
            "LLM_ASSISTIVE_ENDPOINT",
            &format!("{}/v1/responses", server.url()),
        );
        let mock = server
            .mock("GET", "/v1/models")
            .with_status(200)
            .with_body(r#"{"data":[{"id":"gpt-should-never-land"}]}"#)
            .expect(0)
            .create();

        // Supersede the claimed generation before the fetch starts — the
        // biased select must abort without a single request on the wire.
        *test_after_claim_hook()
            .lock()
            .expect("test hook lock should not be poisoned") = Some(Box::new(|provider| {
            let _ = claim_generation(provider);
        }));

        let err = discover_assistive_models()
            .expect_err("superseded discovery must return cancelled");

        assert_eq!(err.code(), "cancelled");
        assert_eq!(err.provider(), ProviderKind::OpenAiResponses);
        mock.assert();
        let cached = read_cached_models(ProviderKind::OpenAiResponses)
            .expect("cache read should succeed after cancelled discovery");
        assert!(
            cached.is_empty(),
            "cancelled discovery must not write cache"
        );
        env.keepalive();
    }

    /// The other half of the cancel race: a fetch that was superseded *after*
    /// completing must commit nothing, or a slow earlier request would end up
    /// overwriting the newer answer.
    #[test]
    #[serial]
    fn stale_generation_result_does_not_overwrite_cache() {
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
            .with_body(r#"{"data":[{"id":"gpt-last-good"}]}"#)
            .create();
        let fresh = discover_assistive_models()
            .expect("seed discovery should succeed before staleness test");

        // A fetch that completes after being superseded must commit nothing.
        let (stale_generation, _stale_cancel) = claim_generation(ProviderKind::OpenAiResponses);
        let (_newer_generation, _newer_cancel) = claim_generation(ProviderKind::OpenAiResponses);
        let err = commit_fetch_outcome(
            ProviderKind::OpenAiResponses,
            stale_generation,
            Ok(vec![DiscoveredModel {
                id: "gpt-stale-arrival".to_string(),
                display_name: "gpt-stale-arrival".to_string(),
            }]),
        )
        .expect_err("stale generation must not commit its result");

        assert_eq!(err.code(), "cancelled");
        let cached = read_cached_models(ProviderKind::OpenAiResponses)
            .expect("cache read should succeed after stale commit attempt");
        assert_eq!(cached, fresh.models, "stale result must not mutate cache");
        env.keepalive();
    }

    /// The cancel channel stays silent while a request is the newest one, and
    /// fires exactly when a newer claim arrives.
    #[test]
    #[serial]
    fn newer_generation_fires_cancel_signal() {
        let (_first, mut first_cancel) = claim_generation(ProviderKind::OpenAiResponses);
        assert!(
            matches!(
                first_cancel.try_recv(),
                Err(tokio::sync::oneshot::error::TryRecvError::Empty)
            ),
            "cancel must stay silent until a newer generation claims"
        );

        let (_second, _second_cancel) = claim_generation(ProviderKind::OpenAiResponses);
        assert!(
            first_cancel.try_recv().is_ok(),
            "newer generation must fire the previous cancel signal"
        );
    }

    /// Generations are per-provider: Settings refreshes several providers in
    /// one batch, and an Anthropic claim must not abort an in-flight OpenAI
    /// fetch (the reason the registry is a map, not a single counter).
    #[test]
    #[serial]
    fn cross_provider_generations_are_independent() {
        let (openai_generation, mut openai_cancel) =
            claim_generation(ProviderKind::OpenAiResponses);
        let (_anthropic_generation, _anthropic_cancel) =
            claim_generation(ProviderKind::AnthropicMessages);

        assert!(
            matches!(
                openai_cancel.try_recv(),
                Err(tokio::sync::oneshot::error::TryRecvError::Empty)
            ),
            "another provider's discovery must not cancel this one"
        );
        assert!(
            finish_generation(ProviderKind::OpenAiResponses, openai_generation),
            "OpenAI generation must stay current across Anthropic claims"
        );
    }

    /// The three endpoint shapes operators actually paste — Responses, legacy
    /// chat completions, and a bare proxy base — all resolve to `/models`.
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

    /// Isolated environment for one test: a temp data dir plus the env vars
    /// discovery reads, each restored on drop. The temp dir is held so it
    /// outlives the cache reads and writes performed during the test.
    struct TestEnv {
        _tmp: TempDir,
        guards: Vec<EnvGuard>,
    }

    impl TestEnv {
        /// Point the data dir at a fresh temp directory, disable the keychain,
        /// and clear every key/endpoint var so the test starts from a known
        /// state rather than inheriting the operator's own configuration.
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
            this.remove("LLM_ASSISTIVE_PROVIDER");
            this.remove("LLM_ANTHROPIC_API_KEY");
            this.remove("LLM_ANTHROPIC_ENDPOINT");
            this.remove("LLM_ASSISTIVE_ENDPOINT");
            this.remove("LLM_ENDPOINT");
            this
        }

        /// Set a var for the duration of the test.
        fn set(&mut self, key: &'static str, value: &str) {
            self.guards.push(EnvGuard::set(key, value));
        }

        /// Unset a var for the duration of the test.
        fn remove(&mut self, key: &'static str) {
            self.guards.push(EnvGuard::remove(key));
        }

        /// Drop-order pin, not dead code: called at the end of a test so the
        /// guards cannot be dropped — and the environment restored — before the
        /// assertions above have run.
        fn keepalive(&self) {}
    }

    /// One env var borrowed and given back. Restoring the previous value on
    /// drop is what keeps `#[serial]` tests from leaking state into each other.
    struct EnvGuard {
        key: &'static str,
        prev: Option<String>,
    }

    impl EnvGuard {
        /// Remember the current value, then set the new one.
        fn set(key: &'static str, value: &str) -> Self {
            let prev = std::env::var(key).ok();
            unsafe { std::env::set_var(key, value) };
            Self { key, prev }
        }

        /// Remember the current value, then unset the var.
        fn remove(key: &'static str) -> Self {
            let prev = std::env::var(key).ok();
            unsafe { std::env::remove_var(key) };
            Self { key, prev }
        }
    }

    impl Drop for EnvGuard {
        /// Restores or clears the env var so serial tests stay isolated.
        fn drop(&mut self) {
            match self.prev.as_deref() {
                Some(value) => unsafe { std::env::set_var(self.key, value) },
                None => unsafe { std::env::remove_var(self.key) },
            }
        }
    }
}
