//! Live provider model discovery for the Settings model picker.
//!
//! Model dropdowns must come from the provider's own `/models` API for the
//! user's key. Static model catalogs go stale exactly when new releases matter,
//! so this module is the single discovery path plus a last-good cache for
//! offline/error states. It runs for **every** provider — vendor or Custom —
//! against the resolved row's endpoint, not against a sealed lane.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use chrono::Utc;
use reqwest::Client;
use reqwest::StatusCode;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::sync::oneshot;
use tracing::warn;

use crate::config::Config;
use crate::llm::account_auth::{self, GoogleCredential};
use crate::llm::provider::{ProviderKind, ProviderRef, ResolvedProvider, WireFamily};
use crate::llm::vendors;

/// 5s client timeout for live /models discovery.
/// P2-09: short to keep Settings responsive. If provider is slow, we degrade to
/// cache rather than hang the picker. Justified as UX bound, not a knob (per
/// charter: no new Settings controls). P2-08: a newer discovery for the same
/// provider additionally aborts the in-flight request (per-provider generation
/// registry below), so this timeout is the worst case, not the supersede path.
const DISCOVERY_TIMEOUT: Duration = Duration::from_secs(5);
/// Codex catalogue protocol version, pinned to the locally reviewed OMP donor
/// (`packages/catalog/src/wire/codex.ts`). This is not the app's version.
const CODEX_CATALOG_CLIENT_VERSION: &str = "0.159.0";
/// Last-good models cache filename under the Codescribe config dir.
const CACHE_FILE_NAME: &str = "model_discovery_cache.json";

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
    pub provider: ProviderRef,
    pub models: Vec<DiscoveredModel>,
    pub status: ModelDiscoveryStatus,
}

/// Discovery failure. These map cleanly to UI status strings; no variant carries
/// API key material.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModelDiscoveryError {
    /// A vendor row without a key. Custom rows never produce this: a
    /// key-optional host is asked without `Authorization` instead.
    NoKey {
        provider: ProviderRef,
        env_key: String,
    },
    Network {
        provider: ProviderRef,
        message: String,
    },
    HttpStatus {
        provider: ProviderRef,
        status: u16,
        message: String,
    },
    Parse {
        provider: ProviderRef,
        message: String,
    },
    Cache {
        provider: ProviderRef,
        message: String,
    },
    /// A newer discovery request for the same provider superseded this one.
    /// The stale request is aborted and its result never touches cache/state;
    /// callers should drop this outcome silently (the newer request answers).
    Cancelled { provider: ProviderRef },
}

impl ModelDiscoveryError {
    /// Which provider failed. Needed because Settings refreshes several
    /// providers at once and must attribute each failure to its own row.
    pub const fn provider(&self) -> &ProviderRef {
        match self {
            Self::NoKey { provider, .. }
            | Self::Network { provider, .. }
            | Self::HttpStatus { provider, .. }
            | Self::Parse { provider, .. }
            | Self::Cache { provider, .. }
            | Self::Cancelled { provider } => provider,
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

    /// Whether the provider refused the credential itself, as opposed to the
    /// network, the body shape or the cache failing. Settings turns this into
    /// "the API key was rejected, check it under Providers" and keeps the raw
    /// body out of the main line; every other failure stays a generic fetch
    /// error so a bad key is never diagnosed from an unrelated problem.
    ///
    /// 401 and 403 are credential refusals by definition. xAI answers a bad key
    /// with 400 and a body that names the API key, so a 400 counts only when the
    /// body says so.
    pub fn rejects_credential(&self) -> bool {
        match self {
            Self::HttpStatus {
                status, message, ..
            } => match *status {
                401 | 403 => true,
                400 => {
                    let lower = message.to_ascii_lowercase();
                    lower.contains("api key")
                        || lower.contains("api_key")
                        || lower.contains("apikey")
                }
                _ => false,
            },
            Self::NoKey { .. }
            | Self::Network { .. }
            | Self::Parse { .. }
            | Self::Cache { .. }
            | Self::Cancelled { .. } => false,
        }
    }

    /// Operator-facing explanation. Also the `reason` recorded when a failure
    /// degrades to [`ModelDiscoveryStatus::Cached`], which is why it never
    /// interpolates key material — only the account's name.
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
            Self::HttpStatus {
                provider,
                status,
                message,
            } => write!(f, "{provider}: http_status {status}: {message}"),
            other => write!(
                f,
                "{}: {}: {}",
                other.provider(),
                other.code(),
                other.message()
            ),
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

/// On-disk shape of the discovery cache. Keyed by provider reference
/// (`openai-responses`, `custom:<id>`) so one provider's failure can never
/// invalidate another's last-good list; `BTreeMap` keeps the file diff-stable.
#[derive(Debug, Default, Serialize, Deserialize)]
struct DiscoveryCacheFile {
    providers: BTreeMap<String, CachedProviderModels>,
}

/// One page of Anthropic's `/v1/models`. Unlike the OpenAI family this endpoint
/// paginates, so `has_more` and `last_id` drive the fetch loop.
#[derive(Debug, Deserialize)]
struct AnthropicModelsPage {
    #[serde(default)]
    has_more: bool,
    last_id: Option<String>,
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
fn generation_registry() -> &'static Mutex<HashMap<ProviderRef, GenerationSlot>> {
    /// Process-wide per-provider discovery generation + cancel handles.
    static REGISTRY: OnceLock<Mutex<HashMap<ProviderRef, GenerationSlot>>> = OnceLock::new();
    REGISTRY.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Claim the next discovery generation for `provider`, firing the cancel
/// signal of the previous in-flight request (if any).
fn claim_generation(provider: &ProviderRef) -> (u64, oneshot::Receiver<()>) {
    let (cancel_tx, cancel_rx) = oneshot::channel();
    let mut registry = generation_registry()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let slot = registry.entry(provider.clone()).or_insert(GenerationSlot {
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
fn finish_generation(provider: &ProviderRef, generation: u64) -> bool {
    let mut registry = generation_registry()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    match registry.get_mut(provider) {
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
type AfterClaimHook = Box<dyn FnOnce(&ProviderRef) + Send>;

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
fn run_test_after_claim(provider: &ProviderRef) {
    let hook = test_after_claim_hook()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .take();
    if let Some(hook) = hook {
        hook(provider);
    }
}

/// Account authentication follows the same provider policy as runtime requests.
fn account_discovery_vendor(
    provider: &ResolvedProvider,
) -> Result<Option<ProviderKind>, ModelDiscoveryError> {
    let Some(vendor @ (ProviderKind::OpenAiResponses | ProviderKind::XaiResponses)) =
        provider.oauth_vendor
    else {
        return Ok(None);
    };
    if provider.wire != WireFamily::OpenAiResponses {
        return Ok(None);
    }
    match account_auth::load_account_tokens(vendor) {
        Ok(_) => Ok(Some(vendor)),
        Err(account_auth::AccountAuthError::NotSignedIn(_)) => Ok(None),
        Err(_) => Err(ModelDiscoveryError::Network {
            provider: provider.reference.clone(),
            message: "The signed-in account credentials could not be read. Sign in again.".into(),
        }),
    }
}

/// An opaque credential/endpoint scope keeps API-key catalogues and different
/// OAuth sessions apart. No credential bytes are written into the cache.
fn account_catalog_cache_key(provider: &ProviderRef, endpoint: &str, token: &str) -> String {
    let mut hash = Sha256::new();
    let provider_key = provider.as_string();
    for part in [provider_key.as_str(), endpoint, token] {
        hash.update(part.as_bytes());
        hash.update([0]);
    }
    format!(
        "{}:oauth:{}",
        provider.as_string(),
        hex::encode(hash.finalize())
    )
}

fn discovery_runtime(
    provider: &ProviderRef,
) -> Result<&'static tokio::runtime::Runtime, ModelDiscoveryError> {
    // Account auth owns detached refresh transactions through token rotation
    // and persistence. Cancelling a catalogue request must not drop their
    // runtime and kill the transaction after the provider rotated its token.
    static RUNTIME: OnceLock<Result<tokio::runtime::Runtime, std::io::Error>> = OnceLock::new();
    RUNTIME
        .get_or_init(|| {
            tokio::runtime::Builder::new_multi_thread()
                .worker_threads(1)
                .thread_name("codescribe-model-discovery")
                .enable_all()
                .build()
        })
        .as_ref()
        .map_err(|error| ModelDiscoveryError::Network {
            provider: provider.clone(),
            message: format!("failed to start discovery runtime: {error}"),
        })
}

/// Discover models with the provider's signed-in account, or its API key when
/// no account is signed in. Key-required rows fail closed without either;
/// key-optional custom rows are asked without Authorization. Failed requests
/// can use a last-good catalogue only from the same authentication scope.
///
/// Each call claims a provider generation. Superseded requests are cancelled
/// and cannot overwrite the cache with an older result.
pub fn discover_models(
    provider: &ResolvedProvider,
    api_key: Option<&str>,
) -> Result<ModelDiscoveryResult, ModelDiscoveryError> {
    let reference = &provider.reference;
    let api_key = api_key.map(str::trim).filter(|key| !key.is_empty());
    // Match the runtime owner: a signed-in Responses vendor uses its account,
    // even when an API key is also stored for that provider.
    let account_vendor = account_discovery_vendor(provider)?;
    if api_key.is_none()
        && account_vendor.is_none()
        && provider.key_required
        && provider.wire != WireFamily::GoogleGenerateContent
    {
        return Err(ModelDiscoveryError::NoKey {
            provider: reference.clone(),
            env_key: provider.key_account.clone(),
        });
    }

    let (generation, cancelled) = claim_generation(reference);
    #[cfg(test)]
    run_test_after_claim(reference);

    let client = Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(DISCOVERY_TIMEOUT)
        .build()
        .map_err(|error| ModelDiscoveryError::Network {
            provider: reference.clone(),
            message: format!("failed to create HTTP client: {error}"),
        })?;
    let runtime = discovery_runtime(reference)?;

    // A failed account refresh must never fall through to an API-key cache.
    // Once the credential is acquired, its actual request scopes the cache.
    let mut cache_key = account_vendor.is_none().then(|| reference.as_string());
    let fetched = runtime.block_on(async {
        let fetch = async {
            // Public Responses providers share /v1/models. Codex account auth
            // uses its account backend and catalogue format instead.
            match provider.wire {
                WireFamily::OpenAiResponses => {
                    fetch_openai_models(&client, provider, api_key, account_vendor, &mut cache_key).await
                }
                WireFamily::AnthropicMessages => {
                    fetch_anthropic_models(&client, provider, api_key.unwrap_or_default()).await
                }
                WireFamily::GoogleGenerateContent => {
                    fetch_google_models(&client, provider, api_key).await
                }
            }
        };
        // `biased` polls the cancel channel first: a pre-fired cancel aborts
        // before the request is even sent; mid-flight, dropping the fetch
        // future tears down the HTTP connection.
        tokio::select! {
            biased;
            _ = cancelled => None,
            result = tokio::time::timeout(DISCOVERY_TIMEOUT, fetch) => Some(result.unwrap_or_else(|_| {
                Err(ModelDiscoveryError::Network {
                    provider: reference.clone(),
                    message: "Model discovery timed out".into(),
                })
            })),
        }
    });

    let Some(fetched) = fetched else {
        return Err(ModelDiscoveryError::Cancelled {
            provider: reference.clone(),
        });
    };
    commit_fetch_outcome(reference, generation, fetched, cache_key.as_deref())
}

/// Apply a finished fetch to module state (cache write / cache fallback).
/// A generation superseded between fetch completion and commit must not leak:
/// no cache write, no cache fallback — plain `Cancelled`.
fn commit_fetch_outcome(
    provider: &ProviderRef,
    generation: u64,
    fetched: Result<Vec<DiscoveredModel>, ModelDiscoveryError>,
    cache_key: Option<&str>,
) -> Result<ModelDiscoveryResult, ModelDiscoveryError> {
    if !finish_generation(provider, generation) {
        return Err(ModelDiscoveryError::Cancelled {
            provider: provider.clone(),
        });
    }

    match fetched {
        Ok(models) => {
            let models = normalize_models(models);
            if let Some(key) = cache_key
                && let Err(error) = write_cache(provider, key, &models)
            {
                warn!("{error}");
            }
            Ok(ModelDiscoveryResult {
                provider: provider.clone(),
                models,
                status: ModelDiscoveryStatus::Fresh,
            })
        }
        Err(error) => match cache_key
            .map(|key| read_cached_models(provider, key))
            .transpose()
        {
            Ok(Some(models)) if !models.is_empty() => Ok(ModelDiscoveryResult {
                provider: provider.clone(),
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
/// the row's inference endpoint — so a Custom proxy or gateway is discovered
/// against the same host that will serve inference. No key ⇒ no
/// `Authorization` header (key-optional Custom host).
async fn fetch_openai_models(
    client: &Client,
    provider: &ResolvedProvider,
    api_key: Option<&str>,
    account_vendor: Option<ProviderKind>,
    cache_key: &mut Option<String>,
) -> Result<Vec<DiscoveredModel>, ModelDiscoveryError> {
    let reference = &provider.reference;
    let codex = account_vendor == Some(ProviderKind::OpenAiResponses);
    let mut headers = Vec::new();
    let endpoint = if codex {
        let route = account_auth::account_responses_route(ProviderKind::OpenAiResponses)
            .expect("OpenAI account route exists");
        headers = route.headers;
        let mut url =
            reqwest::Url::parse(&route.endpoint).map_err(|_| ModelDiscoveryError::Parse {
                provider: reference.clone(),
                message: "Invalid Codex account endpoint".into(),
            })?;
        let base = url
            .path()
            .trim_end_matches('/')
            .strip_suffix("/responses")
            .ok_or_else(|| ModelDiscoveryError::Parse {
                provider: reference.clone(),
                message: "Codex account endpoint must end in /responses".into(),
            })?
            .to_string();
        url.set_path(&format!("{base}/models"));
        url.query_pairs_mut()
            .append_pair("client_version", CODEX_CATALOG_CLIENT_VERSION);
        headers.push(("version".into(), CODEX_CATALOG_CLIENT_VERSION.into()));
        headers.push(("OpenAI-Beta".into(), "responses=experimental".into()));
        url.to_string()
    } else {
        provider_models_endpoint(provider)?
    };
    let secret = match account_vendor {
        Some(vendor) => Some(account_auth::access_token(vendor).await.map_err(|_| {
            ModelDiscoveryError::Network {
                provider: reference.clone(),
                message: "The signed-in account credentials could not be refreshed. Sign in again."
                    .into(),
            }
        })?),
        None => api_key.map(ToString::to_string),
    };
    if account_vendor.is_some() {
        *cache_key = secret
            .as_deref()
            .map(|token| account_catalog_cache_key(reference, &endpoint, token));
    }
    let mut request = client.get(endpoint);
    if let Some(secret) = &secret {
        request = request.bearer_auth(secret);
    }
    for (name, value) in headers {
        request = request.header(name, value);
    }
    let response = request
        .send()
        .await
        .map_err(|error| network_error(reference, error))?;
    if account_vendor.is_some() && !response.status().is_success() {
        // OAuth error bodies can echo any part of a bearer token. Do not send
        // them into UI messages or the persisted last-good cache reason.
        return Err(http_status_error(
            reference,
            response.status(),
            "Account model discovery rejected the credentialed request",
        ));
    }
    let body = response_body_or_error(reference, response).await?;
    let parsed: serde_json::Value =
        serde_json::from_str(&body).map_err(|error| ModelDiscoveryError::Parse {
            provider: reference.clone(),
            message: format!("failed to parse models response: {error}"),
        })?;
    if codex {
        return codex_models_from_response(reference, &parsed);
    }
    Ok(vendors::libraxis::models_from_response(&parsed)
        .into_iter()
        .map(|(id, _)| DiscoveredModel {
            display_name: id.clone(),
            id,
        })
        .collect())
}

fn codex_models_from_response(
    reference: &ProviderRef,
    body: &serde_json::Value,
) -> Result<Vec<DiscoveredModel>, ModelDiscoveryError> {
    let rows = body
        .get("models")
        .or_else(|| body.get("data"))
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| ModelDiscoveryError::Parse {
            provider: reference.clone(),
            message: "Codex catalogue has no model list".into(),
        })?;
    Ok(rows
        .iter()
        .filter_map(|row| {
            if row
                .get("visibility")
                .and_then(serde_json::Value::as_str)
                .is_some_and(|v| v.eq_ignore_ascii_case("hide") || v.eq_ignore_ascii_case("hidden"))
            {
                return None;
            }
            let id = row.get("slug").or_else(|| row.get("id"))?.as_str()?.trim();
            if id.is_empty() {
                return None;
            }
            let display_name = row
                .get("display_name")
                .and_then(serde_json::Value::as_str)
                .map(str::trim)
                .filter(|name| !name.is_empty())
                .unwrap_or(id);
            Some(DiscoveredModel {
                id: id.into(),
                display_name: display_name.into(),
            })
        })
        .collect())
}

/// Native Gemini model discovery. Google paginates by `nextPageToken`, and
/// model names on this wire carry a `models/` resource prefix.
async fn fetch_google_models(
    client: &Client,
    provider: &ResolvedProvider,
    api_key: Option<&str>,
) -> Result<Vec<DiscoveredModel>, ModelDiscoveryError> {
    let reference = &provider.reference;
    if vendors::google::is_cloud_code_assist(&provider.endpoint) {
        return Err(ModelDiscoveryError::Parse {
            provider: reference.clone(),
            message:
                "Cloud Code Assist model discovery is not available through the Gemini models API"
                    .to_string(),
        });
    }
    let credential = match api_key {
        Some(key) => GoogleCredential::ApiKey(key.to_string()),
        None => account_auth::resolve_google_credential(&provider.key_account)
            .await
            .map_err(|error| ModelDiscoveryError::Network {
                provider: reference.clone(),
                message: account_auth::redact_google_secrets(&error.to_string()),
            })?,
    };
    vendors::google::prepare_request_with_credential(
        &provider.endpoint,
        "discovery",
        serde_json::Value::Null,
        false,
        Some(&credential),
    )
    .map_err(|message| ModelDiscoveryError::Parse {
        provider: reference.clone(),
        message,
    })?;
    let effective_base =
        vendors::google::effective_endpoint_for_credential(&provider.endpoint, Some(&credential));
    if vendors::google::is_cloud_code_assist(&effective_base) {
        let endpoint = vendors::google::cca_models_endpoint(&effective_base);
        let payload = match credential.project_id() {
            Some(project_id) => serde_json::json!({ "project": project_id }),
            None => serde_json::json!({}),
        };
        let request = vendors::google::apply_credential_headers(
            client.post(&endpoint).json(&payload),
            &credential,
            true,
        );
        let response = request
            .send()
            .await
            .map_err(|error| network_error(reference, error))?;
        if !response.status().is_success() {
            return Err(http_status_error(
                reference,
                response.status(),
                "Google model discovery rejected the credentialed request",
            ));
        }
        let body = response_body_or_error(reference, response).await?;
        let parsed: serde_json::Value =
            serde_json::from_str(&body).map_err(|error| ModelDiscoveryError::Parse {
                provider: reference.clone(),
                message: format!("failed to parse Google models response: {error}"),
            })?;
        return Ok(vendors::google::models_from_cca_response(&parsed)
            .into_iter()
            .map(|(id, display_name)| DiscoveredModel {
                display_name: display_name
                    .filter(|name| !name.trim().is_empty())
                    .unwrap_or_else(|| id.clone()),
                id,
            })
            .collect());
    }
    let endpoint = vendors::google::models_endpoint(&provider.endpoint);
    let mut page_token = None;
    let mut seen_tokens = HashSet::new();
    let mut models = Vec::new();
    loop {
        let mut request =
            vendors::google::apply_credential_headers(client.get(&endpoint), &credential, false);
        if let Some(token) = page_token.as_deref() {
            request = request.query(&[("pageToken", token)]);
        }
        let response = request
            .send()
            .await
            .map_err(|error| network_error(reference, error))?;
        if !response.status().is_success() {
            // A custom endpoint may echo an arbitrary API key or bearer token;
            // never put its rejection body into Settings or the model cache.
            return Err(http_status_error(
                reference,
                response.status(),
                "Google model discovery rejected the credentialed request",
            ));
        }
        let body = response_body_or_error(reference, response).await?;
        let parsed: serde_json::Value =
            serde_json::from_str(&body).map_err(|error| ModelDiscoveryError::Parse {
                provider: reference.clone(),
                message: format!("failed to parse Google models response: {error}"),
            })?;
        models.extend(
            vendors::google::models_from_response(&parsed)
                .into_iter()
                .map(|(id, display_name)| DiscoveredModel {
                    display_name: display_name
                        .filter(|name| !name.trim().is_empty())
                        .unwrap_or_else(|| id.clone()),
                    id,
                }),
        );
        page_token = parsed
            .get("nextPageToken")
            .and_then(serde_json::Value::as_str)
            .filter(|token| !token.is_empty())
            .map(str::to_string);
        let Some(token) = page_token.as_ref() else {
            break;
        };
        if !seen_tokens.insert(token.clone()) {
            return Err(ModelDiscoveryError::Parse {
                provider: reference.clone(),
                message: "Google models response repeated a page token".to_string(),
            });
        }
    }
    Ok(models)
}

/// Fetch the full Anthropic model list, following `after_id` pagination until
/// the provider reports no more pages. A `has_more` page without a cursor is a
/// parse error rather than a silent truncation of the picker.
async fn fetch_anthropic_models(
    client: &Client,
    provider: &ResolvedProvider,
    api_key: &str,
) -> Result<Vec<DiscoveredModel>, ModelDiscoveryError> {
    let reference = &provider.reference;
    let endpoint = provider_models_endpoint(provider)?;
    let mut after_id: Option<String> = None;
    let mut models = Vec::new();

    loop {
        let mut request = client
            .get(&endpoint)
            .header(vendors::anthropic::AUTH_HEADER, api_key);
        for (name, value) in vendors::anthropic::EXTRA_HEADERS {
            request = request.header(*name, *value);
        }
        if let Some(after) = after_id.as_deref() {
            request = request.query(&[("after_id", after)]);
        }

        let response = request
            .send()
            .await
            .map_err(|error| network_error(reference, error))?;
        let body = response_body_or_error(reference, response).await?;
        let parsed: serde_json::Value =
            serde_json::from_str(&body).map_err(|error| ModelDiscoveryError::Parse {
                provider: reference.clone(),
                message: format!("failed to parse Anthropic models response: {error}"),
            })?;
        let page: AnthropicModelsPage =
            serde_json::from_value(parsed.clone()).map_err(|error| ModelDiscoveryError::Parse {
                provider: reference.clone(),
                message: format!("failed to parse Anthropic models page: {error}"),
            })?;
        let rows = vendors::anthropic::models_from_response(&parsed);
        let next_after_id = page
            .last_id
            .clone()
            .or_else(|| rows.last().map(|(id, _)| id.clone()));

        models.extend(rows.into_iter().map(|(id, display_name)| {
            let display_name = display_name
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| id.clone());
            DiscoveredModel { id, display_name }
        }));

        if !page.has_more {
            break;
        }

        after_id = Some(next_after_id.ok_or_else(|| ModelDiscoveryError::Parse {
            provider: reference.clone(),
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
    provider: &ProviderRef,
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
fn network_error(provider: &ProviderRef, error: reqwest::Error) -> ModelDiscoveryError {
    ModelDiscoveryError::Network {
        provider: provider.clone(),
        message: account_auth::redact_google_secrets(&error.without_url().to_string()),
    }
}

/// Turn a non-2xx response into an error carrying a single-line, length-capped
/// excerpt of the body — enough to diagnose, small enough for a status label,
/// and with the status reason as fallback when the body is empty.
fn http_status_error(
    provider: &ProviderRef,
    status: StatusCode,
    body: &str,
) -> ModelDiscoveryError {
    let mut message = account_auth::redact_google_secrets(body.trim()).replace('\n', " ");
    if message.chars().count() > 240 {
        message = message.chars().take(240).collect();
        message.push_str("...");
    }
    if message.is_empty() {
        message = status
            .canonical_reason()
            .unwrap_or("provider returned an error")
            .to_string();
    }
    ModelDiscoveryError::HttpStatus {
        provider: provider.clone(),
        status: status.as_u16(),
        message,
    }
}

/// Derive the `/models` URL from the row's inference endpoint.
///
/// Vendor endpoints are pinned on the wire path; Custom rows are normalized
/// onto it too, so the trailing inference segment is swapped for `models`
/// rather than assumed. Query and fragment are dropped: they belong to the
/// inference call, not to discovery.
fn provider_models_endpoint(provider: &ResolvedProvider) -> Result<String, ModelDiscoveryError> {
    let reference = &provider.reference;
    let endpoint = provider.endpoint.as_str();
    let mut url = reqwest::Url::parse(endpoint).map_err(|error| ModelDiscoveryError::Parse {
        provider: reference.clone(),
        message: format!(
            "invalid {} endpoint '{endpoint}': {error}",
            provider.display_name
        ),
    })?;
    url.set_query(None);
    url.set_fragment(None);

    let mut next: Vec<String> = url
        .path_segments()
        .map(|parts| {
            parts
                .filter(|segment| !segment.is_empty())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();

    if next.last().is_some_and(|segment| segment == "models") {
        // already right
    } else if next.last().is_some_and(|segment| {
        segment == "responses" || segment == "completions" || segment == "messages"
    }) {
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
            provider: reference.clone(),
            message: format!(
                "invalid {} endpoint base '{endpoint}'",
                provider.display_name
            ),
        })?
        .clear()
        .extend(next.iter().map(String::as_str));
    Ok(url.to_string())
}

/// Trim, drop blanks, filter non-LLM modalities and redundant dated snapshots,
/// de-duplicate by id while preserving provider order, and disambiguate
/// colliding display names — the picker shows this list verbatim. Applied on
/// both write and read, so a cache file that predates a normalization rule is
/// still cleaned up on load.
fn normalize_models(models: Vec<DiscoveredModel>) -> Vec<DiscoveredModel> {
    let mut seen = HashSet::new();
    let deduped: Vec<DiscoveredModel> = models
        .into_iter()
        .filter_map(|model| {
            let id = model.id.trim().to_string();
            if id.is_empty() || is_non_text_model_id(&id) || !seen.insert(id.clone()) {
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
        .collect();

    let mut filtered: Vec<DiscoveredModel> = deduped
        .into_iter()
        .filter(|model| {
            if let Some(base) = strip_dated_snapshot_suffix(&model.id)
                && seen.contains(base)
            {
                return false;
            }
            true
        })
        .collect();

    let mut display_counts: BTreeMap<String, usize> = BTreeMap::new();
    for model in &filtered {
        *display_counts
            .entry(model.display_name.to_lowercase())
            .or_insert(0) += 1;
    }

    for model in &mut filtered {
        let key = model.display_name.to_lowercase();
        if display_counts.get(&key).copied().unwrap_or(0) > 1
            && model.display_name.to_lowercase() != model.id.to_lowercase()
        {
            model.display_name = format!("{} ({})", model.display_name, model.id);
        }
    }

    filtered
}

/// True for non-chat/non-text models returned by vendor `/v1/models` catalogs
/// (TTS, STT/transcribe, embeddings, moderation, image/video, realtime audio).
fn is_non_text_model_id(id: &str) -> bool {
    let lower = id.to_ascii_lowercase();
    lower.starts_with("tts-")
        || lower.ends_with("-tts")
        || lower.contains("-tts-")
        || lower.starts_with("whisper-")
        || lower.ends_with("-whisper")
        || lower.ends_with("-transcribe")
        || lower.contains("-transcribe-")
        || lower.starts_with("text-embedding-")
        || lower.starts_with("embedding-")
        || lower.starts_with("omni-moderation-")
        || lower.starts_with("text-moderation-")
        || lower.starts_with("dall-e-")
        || lower.starts_with("sora-")
        || lower.starts_with("gpt-image-")
        || lower.starts_with("chatgpt-image-")
        || lower.starts_with("grok-imagine-")
        || lower.contains("-flash-image")
        || lower.starts_with("gpt-realtime")
        || lower.starts_with("gpt-audio")
        || lower == "gpt-live-1"
        || lower == "babbage-002"
        || lower == "davinci-002"
}

/// Returns the base model id when `id` ends with `-YYYY-MM-DD` (e.g.
/// `gpt-5.2-2025-12-11` -> `Some("gpt-5.2")`).
fn strip_dated_snapshot_suffix(id: &str) -> Option<&str> {
    if id.len() <= 11 {
        return None;
    }
    let split = id.len() - 11;
    if !id.is_char_boundary(split) {
        return None;
    }
    let (base, suffix) = id.split_at(split);
    let bytes = suffix.as_bytes();
    let is_dated = bytes[0] == b'-'
        && bytes[1..5].iter().all(u8::is_ascii_digit)
        && bytes[5] == b'-'
        && bytes[6..8].iter().all(u8::is_ascii_digit)
        && bytes[8] == b'-'
        && bytes[9..11].iter().all(u8::is_ascii_digit);
    is_dated.then_some(base)
}

/// Cache location, resolved through `Config` so an isolated data dir (tests,
/// portable installs) redirects it along with everything else.
fn cache_path() -> std::path::PathBuf {
    Config::config_dir().join(CACHE_FILE_NAME)
}

/// Read the whole cache file. A missing file is an empty cache, not an error —
/// first run must not surface as a discovery failure.
fn read_cache_file(provider: &ProviderRef) -> Result<DiscoveryCacheFile, ModelDiscoveryError> {
    let path = cache_path();
    match fs::read_to_string(&path) {
        Ok(raw) => serde_json::from_str(&raw).map_err(|error| ModelDiscoveryError::Cache {
            provider: provider.clone(),
            message: format!("failed to parse {}: {error}", path.display()),
        }),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            Ok(DiscoveryCacheFile::default())
        }
        Err(error) => Err(ModelDiscoveryError::Cache {
            provider: provider.clone(),
            message: format!("failed to read {}: {error}", path.display()),
        }),
    }
}

/// Last-good models for one provider, re-normalized on read. Absent entry
/// yields an empty list, which callers treat as "no cache to fall back to".
fn read_cached_models(
    provider: &ProviderRef,
    cache_key: &str,
) -> Result<Vec<DiscoveredModel>, ModelDiscoveryError> {
    Ok(read_cache_file(provider)?
        .providers
        .get(cache_key)
        .map(|entry| normalize_models(entry.models.clone()))
        .unwrap_or_default())
}

/// Replace this provider's cache entry, leaving other providers untouched.
/// A failure here is logged rather than propagated: losing the cache write
/// must not turn a successful live discovery into a failed one.
fn write_cache(
    provider: &ProviderRef,
    cache_key: &str,
    models: &[DiscoveredModel],
) -> Result<(), ModelDiscoveryError> {
    let path = cache_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| ModelDiscoveryError::Cache {
            provider: provider.clone(),
            message: format!("failed to create {}: {error}", parent.display()),
        })?;
    }

    let mut cache = read_cache_file(provider).unwrap_or_default();
    cache.providers.insert(
        cache_key.to_string(),
        CachedProviderModels {
            fetched_at: Utc::now().to_rfc3339(),
            models: models.to_vec(),
        },
    );

    let raw = serde_json::to_string_pretty(&cache).map_err(|error| ModelDiscoveryError::Cache {
        provider: provider.clone(),
        message: format!("failed to serialize model discovery cache: {error}"),
    })?;
    fs::write(&path, raw).map_err(|error| ModelDiscoveryError::Cache {
        provider: provider.clone(),
        message: format!("failed to write {}: {error}", path.display()),
    })
}

/// Discovery is exercised against a mock HTTP server and an isolated data dir,
/// with provider rows built directly — no env, no settings file, no lane seal.
///
/// Every test touching the module's global generation registry is `#[serial]`:
/// the counters are process-wide, and a parallel run would cancel generations
/// belonging to another test.
#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::provider::{CustomProvider, ProviderKind, ProviderRegistry};
    use crate::test_isolation::ScopedEnv;
    use mockito::Matcher;
    use serial_test::serial;
    use tempfile::TempDir;

    /// A Custom row on `wire` at the mock server's base URL.
    fn custom_row(server: &mockito::Server, wire: WireFamily) -> ResolvedProvider {
        let row = CustomProvider::new("Mock Box", wire, &server.url()).expect("valid custom row");
        let id = row.id.clone();
        ProviderRegistry::new(vec![row])
            .resolve(&ProviderRef::Custom(id))
            .expect("custom row resolves")
    }

    /// The Custom row shaped like a vendor (key required) so the vendor-only
    /// `no_key` and cache paths can be driven against a mock host.
    fn keyed_row(server: &mockito::Server, wire: WireFamily) -> ResolvedProvider {
        ResolvedProvider {
            key_required: true,
            ..custom_row(server, wire)
        }
    }

    fn openai_ref() -> ProviderRef {
        ProviderRef::Vendor(ProviderKind::OpenAiResponses)
    }

    fn oauth_row(server: &mockito::Server, vendor: ProviderKind) -> ResolvedProvider {
        ResolvedProvider {
            reference: ProviderRef::Vendor(vendor),
            oauth_vendor: Some(vendor),
            ..keyed_row(server, WireFamily::OpenAiResponses)
        }
    }

    fn seed_account(env: &mut ScopedEnv, vendor: ProviderKind, access: &str) {
        use base64::Engine;
        let claims = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(r#"{"chatgpt_account_id":"catalogue-workspace"}"#);
        let tokens = account_auth::AccountTokens::new(
            vendor,
            access.into(),
            None,
            Some(format!("header.{claims}.signature")),
            None,
            Some(3600),
        );
        let key = match vendor {
            ProviderKind::OpenAiResponses => account_auth::OPENAI_ACCOUNT_TOKENS_ACCOUNT,
            ProviderKind::XaiResponses => account_auth::XAI_ACCOUNT_TOKENS_ACCOUNT,
            _ => panic!("fixture supports Responses accounts"),
        };
        env.set("CODESCRIBE_DISABLE_KEYCHAIN", "1");
        env.set(key, serde_json::to_string(&tokens).unwrap());
    }

    #[test]
    #[serial]
    fn codex_account_catalogue_uses_backend_and_account_even_with_api_key() {
        let _data = IsolatedDataDir::new();
        let mut env = ScopedEnv::new();
        let mut server = mockito::Server::new();
        let provider = oauth_row(&server, ProviderKind::OpenAiResponses);
        seed_account(
            &mut env,
            ProviderKind::OpenAiResponses,
            "codex-catalogue-access",
        );
        env.set(
            account_auth::CODEX_BACKEND_ENDPOINT_ENV,
            format!("{}/backend-api/codex/responses", server.url()),
        );
        let public = server.mock("GET", "/v1/models").expect(0).create();
        let account = server
            .mock("GET", "/backend-api/codex/models")
            .match_query(Matcher::UrlEncoded("client_version".into(), "0.159.0".into()))
            .match_header("authorization", "Bearer codex-catalogue-access")
            .match_header("chatgpt-account-id", "catalogue-workspace")
            .match_header("originator", "codescribe")
            .match_header("version", "0.159.0")
            .match_header("openai-beta", "responses=experimental")
            .match_header("x-api-key", Matcher::Missing)
            .with_status(200)
            .with_body(r#"{"models":[{"slug":"gpt-account","display_name":"Account model"},{"slug":"gpt-hidden","visibility":"hide"},{"id":"gpt-secondary"}]}"#)
            .expect(1)
            .create();

        let result = discover_models(&provider, Some("api-key-must-not-be-sent")).unwrap();
        assert_eq!(result.status, ModelDiscoveryStatus::Fresh);
        assert_eq!(
            result
                .models
                .iter()
                .map(|m| m.id.as_str())
                .collect::<Vec<_>>(),
            ["gpt-account", "gpt-secondary"]
        );
        assert_eq!(result.models[0].display_name, "Account model");
        let cache = fs::read_to_string(cache_path()).unwrap();
        assert!(!cache.contains("codex-catalogue-access"));
        assert!(!cache.contains("api-key-must-not-be-sent"));
        public.assert();
        account.assert();
    }

    #[test]
    #[serial]
    fn xai_account_without_api_key_reads_public_models_with_account_bearer() {
        let _data = IsolatedDataDir::new();
        let mut env = ScopedEnv::new();
        let mut server = mockito::Server::new();
        let provider = oauth_row(&server, ProviderKind::XaiResponses);
        seed_account(&mut env, ProviderKind::XaiResponses, "xai-catalogue-access");
        let account = server
            .mock("GET", "/v1/models")
            .match_header("authorization", "Bearer xai-catalogue-access")
            .with_status(200)
            .with_body(r#"{"data":[{"id":"grok-account"}]}"#)
            .expect(1)
            .create();

        let result = discover_models(&provider, None).unwrap();
        assert_eq!(result.status, ModelDiscoveryStatus::Fresh);
        assert_eq!(result.models[0].id, "grok-account");
        account.assert();
    }

    #[test]
    #[serial]
    fn oauth_failure_cannot_use_api_key_catalogue() {
        let _data = IsolatedDataDir::new();
        let mut env = ScopedEnv::new();
        let mut server = mockito::Server::new();
        let provider = oauth_row(&server, ProviderKind::XaiResponses);
        env.set("CODESCRIBE_DISABLE_KEYCHAIN", "1");
        env.remove(account_auth::XAI_ACCOUNT_TOKENS_ACCOUNT);
        let _api = server
            .mock("GET", "/v1/models")
            .match_header("authorization", "Bearer api-only")
            .with_status(200)
            .with_body(r#"{"data":[{"id":"grok-api-only"}]}"#)
            .create();
        assert_eq!(
            discover_models(&provider, Some("api-only")).unwrap().status,
            ModelDiscoveryStatus::Fresh
        );
        seed_account(&mut env, ProviderKind::XaiResponses, "account-only");
        let account = server
            .mock("GET", "/v1/models")
            .match_header("authorization", "Bearer account-only")
            .with_status(503)
            .with_body("temporarily unavailable")
            .expect(1)
            .create();

        assert_eq!(
            discover_models(&provider, Some("api-only"))
                .unwrap_err()
                .code(),
            "http_status"
        );
        account.assert();
    }

    #[test]
    #[serial]
    fn oauth_last_good_cache_isolated_when_account_credential_changes() {
        let _data = IsolatedDataDir::new();
        let mut env = ScopedEnv::new();
        let mut server = mockito::Server::new();
        let provider = oauth_row(&server, ProviderKind::XaiResponses);
        seed_account(&mut env, ProviderKind::XaiResponses, "account-a");
        let _ok = server
            .mock("GET", "/v1/models")
            .with_status(200)
            .with_body(r#"{"data":[{"id":"grok-account-a"}]}"#)
            .create();
        let fresh = discover_models(&provider, None).unwrap();
        let _fail = server
            .mock("GET", "/v1/models")
            .with_status(503)
            .with_body("outage")
            .create();
        let cached = discover_models(&provider, None).unwrap();
        assert!(matches!(cached.status, ModelDiscoveryStatus::Cached { .. }));
        assert_eq!(cached.models, fresh.models);

        seed_account(&mut env, ProviderKind::XaiResponses, "account-b");
        assert_eq!(
            discover_models(&provider, None).unwrap_err().code(),
            "http_status"
        );
    }

    #[test]
    #[serial]
    fn oauth_error_body_never_exposes_full_or_truncated_bearer() {
        let _data = IsolatedDataDir::new();
        let mut env = ScopedEnv::new();
        let mut server = mockito::Server::new();
        let provider = oauth_row(&server, ProviderKind::XaiResponses);
        let token = "private-token-fragment".repeat(20);
        seed_account(&mut env, ProviderKind::XaiResponses, &token);
        let _fail = server
            .mock("GET", "/v1/models")
            .with_status(401)
            .with_body(token)
            .create();

        let error = discover_models(&provider, None).unwrap_err();
        assert!(error.rejects_credential());
        assert!(!error.message().contains("private-token-fragment"));
    }

    #[test]
    #[serial]
    fn corrupt_account_does_not_silently_switch_to_api_key() {
        let _data = IsolatedDataDir::new();
        let mut env = ScopedEnv::new();
        let mut server = mockito::Server::new();
        let provider = oauth_row(&server, ProviderKind::XaiResponses);
        env.set(account_auth::XAI_ACCOUNT_TOKENS_ACCOUNT, "invalid-json");
        let mock = server.mock("GET", "/v1/models").expect(0).create();
        assert_eq!(
            discover_models(&provider, Some("api-key"))
                .unwrap_err()
                .code(),
            "network"
        );
        mock.assert();
    }

    #[test]
    fn codex_parser_accepts_data_but_rejects_an_unrecognized_catalogue() {
        let models = codex_models_from_response(
            &openai_ref(),
            &serde_json::json!({
                "data": [{"id": "gpt-live"}, {"slug": "hidden", "visibility": "hidden"}]
            }),
        )
        .unwrap();
        assert_eq!(models.len(), 1);
        assert_eq!(models[0].id, "gpt-live");
        assert_eq!(
            codex_models_from_response(
                &openai_ref(),
                &serde_json::json!({"error": "not a catalogue"})
            )
            .unwrap_err()
            .code(),
            "parse"
        );
    }

    #[test]
    fn discovery_runtime_keeps_detached_auth_work_alive_between_calls() {
        let runtime = discovery_runtime(&openai_ref()).unwrap();
        let (done, received) = std::sync::mpsc::channel();
        let (release, waiting) = tokio::sync::oneshot::channel();
        runtime.block_on(async {
            tokio::spawn(async move {
                waiting.await.unwrap();
                done.send(()).unwrap();
            });
        });
        // The synchronous discovery caller has left block_on. Its cancelled
        // fetch must still let the auth owner's detached transaction finish.
        release.send(()).unwrap();
        received.recv_timeout(Duration::from_secs(1)).unwrap();
    }

    /// Live Responses-family discovery reports `Fresh` and lands in the cache
    /// verbatim, keyed by the reference, so the next offline refresh has
    /// something to fall back to.
    #[test]
    #[serial]
    fn responses_models_parse_and_cache_round_trips() {
        let _env = IsolatedDataDir::new();
        let mut server = mockito::Server::new();
        let _mock = server
            .mock("GET", "/v1/models")
            .match_header("authorization", "Bearer sk-test")
            .with_status(200)
            .with_body(r#"{"object":"list","data":[{"id":"gpt-live"},{"id":"gpt-other"}]}"#)
            .create();
        let provider = custom_row(&server, WireFamily::OpenAiResponses);

        let result = discover_models(&provider, Some("sk-test")).expect("fresh discovery");

        assert_eq!(result.status, ModelDiscoveryStatus::Fresh);
        assert_eq!(result.provider, provider.reference);
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
        let cached = read_cached_models(&provider.reference, provider.reference.as_str().as_ref())
            .expect("cache read");
        assert_eq!(cached, result.models);
    }

    /// Effect witness (§B): a Custom row without a key is asked without
    /// `Authorization` instead of failing `no_key` — a key-optional local host
    /// is a first-class provider.
    #[test]
    #[serial]
    fn custom_provider_without_key_discovers_without_authorization() {
        let _env = IsolatedDataDir::new();
        let mut server = mockito::Server::new();
        let mock = server
            .mock("GET", "/v1/models")
            .match_header("authorization", Matcher::Missing)
            .with_status(200)
            .with_body(r#"{"data":[{"id":"qwen-local"}]}"#)
            .expect(1)
            .create();
        let provider = custom_row(&server, WireFamily::OpenAiResponses);

        let result = discover_models(&provider, None).expect("key-optional discovery");

        assert_eq!(result.models[0].id, "qwen-local");
        mock.assert();
    }

    /// Pagination is followed to the end and display names survive, including
    /// the fallback to `id` for a model that ships without one. A regression
    /// here silently truncates the picker to page one.
    #[test]
    #[serial]
    fn anthropic_models_parse_display_names_and_pagination() {
        let _env = IsolatedDataDir::new();
        let mut server = mockito::Server::new();
        let version = vendors::anthropic::EXTRA_HEADERS[0].1;
        let _page_1 = server
            .mock("GET", "/v1/models")
            .match_header("x-api-key", "anthropic-test")
            .match_header("anthropic-version", version)
            .with_status(200)
            .with_body(
                r#"{"data":[{"id":"claude-a","display_name":"Claude A"}],"has_more":true,"last_id":"claude-a"}"#,
            )
            .create();
        let _page_2 = server
            .mock("GET", "/v1/models")
            .match_query(Matcher::UrlEncoded("after_id".to_string(), "claude-a".to_string()))
            .match_header("x-api-key", "anthropic-test")
            .match_header("anthropic-version", version)
            .with_status(200)
            .with_body(
                r#"{"data":[{"id":"claude-b","display_name":"Claude B"},{"id":"claude-c"}],"has_more":false}"#,
            )
            .create();
        let provider = custom_row(&server, WireFamily::AnthropicMessages);

        let result = discover_models(&provider, Some("anthropic-test")).expect("paged discovery");

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
    }

    /// A vendor (key required) without a key fails before anything reaches the
    /// wire — asserted by the mock's `expect(0)`, not merely by the error code.
    #[test]
    #[serial]
    fn vendor_without_key_returns_error_without_request() {
        let _env = IsolatedDataDir::new();
        let mut server = mockito::Server::new();
        let _mock = server.mock("GET", "/v1/models").expect(0).create();
        let provider = keyed_row(&server, WireFamily::AnthropicMessages);

        let err = discover_models(&provider, Some("  ")).expect_err("no key must fail closed");

        assert_eq!(err.code(), "no_key");
        assert_eq!(err.provider(), &provider.reference);
        assert_eq!(
            err.message(),
            "LLM_CUSTOM_MOCK_BOX_API_KEY is not configured"
        );
    }

    /// Only a credential refusal reads as a rejected key: 401/403 outright, 400
    /// when the body names the API key (xAI). Everything else stays a plain
    /// fetch error, so Settings never blames the key for an outage or a parse
    /// failure.
    #[test]
    fn rejects_credential_only_for_credential_refusals() {
        let provider = openai_ref();
        let http = |status: u16, message: &str| ModelDiscoveryError::HttpStatus {
            provider: provider.clone(),
            status,
            message: message.to_string(),
        };

        assert!(http(401, "Unauthorized").rejects_credential());
        assert!(http(403, "{\"error\":\"forbidden\"}").rejects_credential());
        assert!(http(400, "Incorrect API key provided: xai-***").rejects_credential());
        assert!(http(400, "{\"error\":\"invalid api_key\"}").rejects_credential());

        assert!(!http(400, "model parameter is required").rejects_credential());
        assert!(!http(404, "Not Found").rejects_credential());
        assert!(!http(429, "rate limited").rejects_credential());
        assert!(!http(500, "Incorrect API key provided").rejects_credential());
        assert!(
            !ModelDiscoveryError::Network {
                provider: provider.clone(),
                message: "connection refused".into(),
            }
            .rejects_credential()
        );
        assert!(
            !ModelDiscoveryError::Parse {
                provider: provider.clone(),
                message: "expected `data`".into(),
            }
            .rejects_credential()
        );
        assert!(
            !ModelDiscoveryError::NoKey {
                provider: provider.clone(),
                env_key: "LLM_OPENAI_API_KEY".into(),
            }
            .rejects_credential()
        );
        assert!(!ModelDiscoveryError::Cancelled { provider }.rejects_credential());
    }

    /// A provider outage degrades the picker to the last-good list with the
    /// failure carried as `reason`, instead of emptying it.
    #[test]
    #[serial]
    fn network_error_uses_last_good_cache() {
        let _env = IsolatedDataDir::new();
        let mut server = mockito::Server::new();
        let provider = custom_row(&server, WireFamily::OpenAiResponses);
        let _ok = server
            .mock("GET", "/v1/models")
            .with_status(200)
            .with_body(r#"{"data":[{"id":"gpt-cached"}]}"#)
            .create();
        let fresh = discover_models(&provider, Some("sk-test")).expect("seed cache");
        assert_eq!(fresh.status, ModelDiscoveryStatus::Fresh);

        let _fail = server
            .mock("GET", "/v1/models")
            .with_status(503)
            .with_body("temporarily unavailable")
            .create();
        let cached = discover_models(&provider, Some("sk-test")).expect("cache fallback");

        assert_eq!(
            cached.status,
            ModelDiscoveryStatus::Cached {
                reason: "temporarily unavailable".to_string(),
            }
        );
        assert_eq!(cached.models, fresh.models);
    }

    /// A superseded request is abandoned before a single byte goes out — the
    /// `biased` select must see the pre-fired cancel first. Proven by the mock
    /// asserting zero requests, and by the cache staying empty.
    #[test]
    #[serial]
    fn superseding_generation_cancels_inflight_discovery() {
        let _env = IsolatedDataDir::new();
        let mut server = mockito::Server::new();
        let mock = server
            .mock("GET", "/v1/models")
            .with_status(200)
            .with_body(r#"{"data":[{"id":"gpt-should-never-land"}]}"#)
            .expect(0)
            .create();
        let provider = custom_row(&server, WireFamily::OpenAiResponses);

        // Supersede the claimed generation before the fetch starts — the
        // biased select must abort without a single request on the wire.
        *test_after_claim_hook()
            .lock()
            .expect("test hook lock should not be poisoned") = Some(Box::new(|provider| {
            let _ = claim_generation(provider);
        }));

        let err = discover_models(&provider, Some("sk-test"))
            .expect_err("superseded discovery must return cancelled");

        assert_eq!(err.code(), "cancelled");
        assert_eq!(err.provider(), &provider.reference);
        mock.assert();
        let cached = read_cached_models(&provider.reference, provider.reference.as_str().as_ref())
            .expect("cache read");
        assert!(
            cached.is_empty(),
            "cancelled discovery must not write cache"
        );
    }

    /// The other half of the cancel race: a fetch that was superseded *after*
    /// completing must commit nothing, or a slow earlier request would end up
    /// overwriting the newer answer.
    #[test]
    #[serial]
    fn stale_generation_result_does_not_overwrite_cache() {
        let _env = IsolatedDataDir::new();
        let mut server = mockito::Server::new();
        let provider = custom_row(&server, WireFamily::OpenAiResponses);
        let _ok = server
            .mock("GET", "/v1/models")
            .with_status(200)
            .with_body(r#"{"data":[{"id":"gpt-last-good"}]}"#)
            .create();
        let fresh = discover_models(&provider, Some("sk-test")).expect("seed cache");

        let (stale_generation, _stale_cancel) = claim_generation(&provider.reference);
        let (_newer_generation, _newer_cancel) = claim_generation(&provider.reference);
        let err = commit_fetch_outcome(
            &provider.reference,
            stale_generation,
            Ok(vec![DiscoveredModel {
                id: "gpt-stale-arrival".to_string(),
                display_name: "gpt-stale-arrival".to_string(),
            }]),
            Some(provider.reference.as_str().as_ref()),
        )
        .expect_err("stale generation must not commit its result");

        assert_eq!(err.code(), "cancelled");
        let cached = read_cached_models(&provider.reference, provider.reference.as_str().as_ref())
            .expect("cache read");
        assert_eq!(cached, fresh.models, "stale result must not mutate cache");
    }

    /// The cancel channel stays silent while a request is the newest one, and
    /// fires exactly when a newer claim arrives.
    #[test]
    #[serial]
    fn newer_generation_fires_cancel_signal() {
        let (_first, mut first_cancel) = claim_generation(&openai_ref());
        assert!(
            matches!(
                first_cancel.try_recv(),
                Err(tokio::sync::oneshot::error::TryRecvError::Empty)
            ),
            "cancel must stay silent until a newer generation claims"
        );

        let (_second, _second_cancel) = claim_generation(&openai_ref());
        assert!(
            first_cancel.try_recv().is_ok(),
            "newer generation must fire the previous cancel signal"
        );
    }

    /// Generations are per-provider: Settings refreshes several providers in
    /// one batch, and a Custom claim must not abort an in-flight OpenAI fetch
    /// (the reason the registry is a map, not a single counter).
    #[test]
    #[serial]
    fn cross_provider_generations_are_independent() {
        let (openai_generation, mut openai_cancel) = claim_generation(&openai_ref());
        let (_custom_generation, _custom_cancel) =
            claim_generation(&ProviderRef::Custom("my-box".to_string()));

        assert!(
            matches!(
                openai_cancel.try_recv(),
                Err(tokio::sync::oneshot::error::TryRecvError::Empty)
            ),
            "another provider's discovery must not cancel this one"
        );
        assert!(
            finish_generation(&openai_ref(), openai_generation),
            "OpenAI generation must stay current across Custom claims"
        );
    }

    /// Vendor rows derive `/models` from their pinned wire endpoint; a Custom
    /// row pasted as a bare proxy base gets `/models` appended.
    #[test]
    fn models_endpoint_derives_from_the_resolved_endpoint() {
        let registry = ProviderRegistry::default();
        let openai = registry.resolve(&openai_ref()).expect("openai row");
        assert_eq!(
            provider_models_endpoint(&openai).unwrap(),
            "https://api.openai.com/v1/models"
        );
        let anthropic = registry
            .resolve(&ProviderRef::Vendor(ProviderKind::AnthropicMessages))
            .expect("anthropic row");
        assert_eq!(
            provider_models_endpoint(&anthropic).unwrap(),
            vendors::anthropic::MODELS_ENDPOINT
        );
        let proxy = ResolvedProvider {
            endpoint: "https://proxy.example/openai".to_string(),
            ..anthropic
        };
        assert_eq!(
            provider_models_endpoint(&proxy).unwrap(),
            "https://proxy.example/openai/models"
        );
    }

    #[test]
    fn normalize_models_disambiguates_colliding_display_names_and_filters_noise() {
        let raw = vec![
            DiscoveredModel {
                id: "gemini-2.5-flash".to_string(),
                display_name: "Gemini 3.5 Flash Lite".to_string(),
            },
            DiscoveredModel {
                id: "gemini-2.5-flash-lite".to_string(),
                display_name: "Gemini 3.5 Flash Lite".to_string(),
            },
            DiscoveredModel {
                id: "gemini-3.5-flash-lite".to_string(),
                display_name: "Gemini 3.5 Flash Lite".to_string(),
            },
            DiscoveredModel {
                id: "gemini-3.1-flash-image".to_string(),
                display_name: "Gemini 3.1 Flash Image".to_string(),
            },
            DiscoveredModel {
                id: "gpt-5.2".to_string(),
                display_name: "gpt-5.2".to_string(),
            },
            DiscoveredModel {
                id: "gpt-5.2-2025-12-11".to_string(),
                display_name: "gpt-5.2-2025-12-11".to_string(),
            },
            DiscoveredModel {
                id: "claude-opus-4-5-20251101".to_string(),
                display_name: "Claude Opus 4.5".to_string(),
            },
            DiscoveredModel {
                id: "tts-1".to_string(),
                display_name: "tts-1".to_string(),
            },
            DiscoveredModel {
                id: "whisper-1".to_string(),
                display_name: "whisper-1".to_string(),
            },
            DiscoveredModel {
                id: "gpt-4o-transcribe".to_string(),
                display_name: "gpt-4o-transcribe".to_string(),
            },
            DiscoveredModel {
                id: "text-embedding-3-small".to_string(),
                display_name: "text-embedding-3-small".to_string(),
            },
            DiscoveredModel {
                id: "grok-imagine-video".to_string(),
                display_name: "grok-imagine-video".to_string(),
            },
        ];

        let normalized = normalize_models(raw);
        assert_eq!(
            normalized,
            vec![
                DiscoveredModel {
                    id: "gemini-2.5-flash".to_string(),
                    display_name: "Gemini 3.5 Flash Lite (gemini-2.5-flash)".to_string(),
                },
                DiscoveredModel {
                    id: "gemini-2.5-flash-lite".to_string(),
                    display_name: "Gemini 3.5 Flash Lite (gemini-2.5-flash-lite)".to_string(),
                },
                DiscoveredModel {
                    id: "gemini-3.5-flash-lite".to_string(),
                    display_name: "Gemini 3.5 Flash Lite (gemini-3.5-flash-lite)".to_string(),
                },
                DiscoveredModel {
                    id: "gpt-5.2".to_string(),
                    display_name: "gpt-5.2".to_string(),
                },
                DiscoveredModel {
                    id: "claude-opus-4-5-20251101".to_string(),
                    display_name: "Claude Opus 4.5".to_string(),
                },
            ]
        );
    }

    /// A fresh temp data dir for the cache file, restored on drop. Held for the
    /// whole test so it outlives every cache read and write.
    struct IsolatedDataDir {
        _tmp: TempDir,
        previous: Option<String>,
    }

    impl IsolatedDataDir {
        fn new() -> Self {
            let tmp = tempfile::tempdir().unwrap();
            let previous = std::env::var("CODESCRIBE_DATA_DIR").ok();
            // SAFETY: serial tests; restored on drop.
            unsafe { std::env::set_var("CODESCRIBE_DATA_DIR", tmp.path()) };
            Self {
                _tmp: tmp,
                previous,
            }
        }
    }

    impl Drop for IsolatedDataDir {
        fn drop(&mut self) {
            // SAFETY: serial tests; puts the previous value back.
            unsafe {
                match self.previous.as_deref() {
                    Some(value) => std::env::set_var("CODESCRIBE_DATA_DIR", value),
                    None => std::env::remove_var("CODESCRIBE_DATA_DIR"),
                }
            }
        }
    }
}
