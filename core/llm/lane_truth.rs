//! Canonical truth resolution for LLM lane secrets, endpoints, and models.

use std::str::FromStr;

use crate::config::keychain;
use crate::config::{
    Config, DEFAULT_ASSISTIVE_MODEL, DEFAULT_FORMATTING_MODEL, DEFAULT_LLM_MODEL,
    DEFAULT_OPENAI_RESPONSES_ENDPOINT, UserSettings,
};
use crate::llm::account_auth;
use crate::llm::provider::{LlmMode, ProviderKind, resolve_provider};

/// Resolve a Keychain account without exposing the secret to callers that only
/// need presence. Explicit non-empty process env remains the highest-priority
/// source; an empty or missing env value falls back to the Keychain bundle.
pub fn secret(account: &str) -> Option<String> {
    secret_with_keychain(account, keychain::load_key)
}

fn secret_with_keychain(
    account: &str,
    load_key: impl FnOnce(&str) -> Option<String>,
) -> Option<String> {
    env_non_empty(account).or_else(|| load_key(account).and_then(non_empty))
}

/// Resolve and normalize the OpenAI Responses endpoint for one configured LLM
/// lane. Fresh settings are consulted before process env because config
/// bootstrap deliberately leaves seeded env values immutable after startup.
pub fn endpoint(lane: LlmMode, config: &Config) -> String {
    endpoint_with_settings(lane, config, &UserSettings::load())
}

fn endpoint_with_settings(lane: LlmMode, config: &Config, settings: &UserSettings) -> String {
    let (lane_key, lane_setting) = match lane {
        LlmMode::Formatting => (
            "LLM_FORMATTING_ENDPOINT",
            settings.llm_formatting_endpoint.clone(),
        ),
        LlmMode::Assistive => (
            "LLM_ASSISTIVE_ENDPOINT",
            settings.llm_assistive_endpoint.clone(),
        ),
    };

    let resolved = non_empty_option(lane_setting)
        .or_else(|| env_non_empty(lane_key))
        .or_else(|| non_empty_option(settings.llm_endpoint.clone()))
        .or_else(|| env_non_empty("LLM_ENDPOINT"))
        .or_else(|| non_empty_option(config.llm_endpoint.clone()))
        .unwrap_or_else(|| DEFAULT_OPENAI_RESPONSES_ENDPOINT.to_string());

    normalize_openai_responses_endpoint(&resolved)
}

/// Resolve the OpenAI model for one LLM lane from the same persisted snapshot
/// and env hierarchy as [`endpoint`]. Anthropic model ids are ignored on this
/// Responses-specific path, preserving the existing liveness-probe contract.
pub fn model(lane: LlmMode, config: &Config) -> String {
    model_with_settings(lane, config, &UserSettings::load())
}

/// Resolve the wire model for an explicit provider without making callers
/// reimplement the fresh-settings hierarchy. The OpenAI branch preserves the
/// Responses-only filtering in [`model`]; the Anthropic branch accepts only
/// Claude model ids and supplies the provider's lane-specific default.
pub fn model_for_provider(lane: LlmMode, provider: ProviderKind, config: &Config) -> String {
    model_for_provider_with_settings(lane, provider, config, &UserSettings::load())
}

fn model_for_provider_with_settings(
    lane: LlmMode,
    provider: ProviderKind,
    config: &Config,
    settings: &UserSettings,
) -> String {
    match provider {
        ProviderKind::OpenAiResponses => model_with_settings(lane, config, settings),
        ProviderKind::AnthropicMessages => anthropic_model_with_settings(lane, settings),
    }
}

fn model_with_settings(lane: LlmMode, _config: &Config, settings: &UserSettings) -> String {
    let (lane_key, lane_setting, lane_default) = match lane {
        LlmMode::Formatting => (
            "LLM_FORMATTING_MODEL",
            settings.llm_formatting_model.clone(),
            DEFAULT_FORMATTING_MODEL,
        ),
        LlmMode::Assistive => (
            "LLM_ASSISTIVE_MODEL",
            settings.llm_assistive_model.clone(),
            DEFAULT_ASSISTIVE_MODEL,
        ),
    };
    let openai_model = |candidate: String| (!candidate.starts_with("claude")).then_some(candidate);

    non_empty_option(lane_setting)
        .and_then(openai_model)
        .or_else(|| env_non_empty(lane_key).and_then(openai_model))
        .or_else(|| non_empty_option(settings.llm_model.clone()).and_then(openai_model))
        .or_else(|| env_non_empty("LLM_MODEL").and_then(openai_model))
        .unwrap_or_else(|| lane_default.to_string())
}

/// Resolve the provider identity for a lane from the same persisted-settings
/// truth as [`endpoint`] and [`model`]: a fresh settings value beats a stale
/// bootstrap env, env stays the fallback, and the canonical resolver keeps
/// ownership of parsing plus the protected OpenAI default.
pub fn provider(lane: LlmMode) -> ProviderKind {
    provider_with_settings(lane, &UserSettings::load())
}

fn provider_with_settings(lane: LlmMode, settings: &UserSettings) -> ProviderKind {
    let lane_setting = match lane {
        // No persisted formatting-provider setting exists yet; env remains the
        // only formatting-lane source.
        LlmMode::Formatting => None,
        LlmMode::Assistive => settings.llm_assistive_provider.clone(),
    };
    non_empty_option(lane_setting)
        .and_then(|raw| ProviderKind::from_str(&raw).ok())
        .unwrap_or_else(|| resolve_provider(lane))
}

/// Suggested key-optional OpenAI-compatible endpoint (the LibraxisAI public
/// cloud) offered in guidance text when the assistive lane is unconfigured or
/// pointed at a key-requiring cloud without a key. Guidance only — code never
/// silently reroutes traffic here.
pub const SUGGESTED_KEY_OPTIONAL_ENDPOINT: &str = "https://api.libraxis.cloud/v1";

const DEFAULT_ANTHROPIC_MESSAGES_ENDPOINT: &str = "https://api.anthropic.com/v1/messages";
const DEFAULT_ANTHROPIC_FORMATTING_MODEL: &str = "claude-sonnet-4-6";
const DEFAULT_ANTHROPIC_MODEL: &str = "claude-opus-4-8";

/// Stable lane identity exposed by [`lane_truth_snapshot`]. `Main` is the
/// shared fallback configured by `LLM_ENDPOINT` / `LLM_MODEL`; the other two
/// variants are the concrete runtime lanes represented by [`LlmMode`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LaneTruthLane {
    Main,
    Formatting,
    Assistive,
}

impl LaneTruthLane {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Main => "main",
            Self::Formatting => "formatting",
            Self::Assistive => "assistive",
        }
    }
}

/// Secret-free, FFI-safe projection of the canonical truth for one LLM lane.
/// Resolution remains owned by the existing lane resolvers; this record only
/// gathers their outputs and reduces credentials to presence booleans.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaneTruthSnapshot {
    pub lane: LaneTruthLane,
    pub provider_id: String,
    pub endpoint: String,
    pub model: String,
    pub key_account: String,
    pub key_present: bool,
    pub account_auth: bool,
    pub available: bool,
    pub unavailable_reason: Option<String>,
}

/// Resolve one complete, secret-free lane snapshot from fresh persisted
/// settings, process env, and Keychain truth.
pub fn lane_truth_snapshot(lane: LaneTruthLane, config: &Config) -> LaneTruthSnapshot {
    lane_truth_snapshot_with(lane, config, &UserSettings::load(), keychain::load_key)
}

fn lane_truth_snapshot_with(
    lane: LaneTruthLane,
    config: &Config,
    settings: &UserSettings,
    load_key: impl Fn(&str) -> Option<String>,
) -> LaneTruthSnapshot {
    match lane {
        LaneTruthLane::Main => {
            let provider = ProviderKind::OpenAiResponses;
            let key_account = "LLM_API_KEY";
            let key_present = secret_with_keychain(key_account, &load_key).is_some();
            LaneTruthSnapshot {
                lane,
                provider_id: provider.as_str().to_string(),
                endpoint: shared_endpoint_with_settings(config, settings),
                model: shared_model_with_settings(settings),
                key_account: key_account.to_string(),
                key_present,
                account_auth: false,
                available: key_present,
                unavailable_reason: (!key_present)
                    .then(|| format!("The main lane has no stored credential ({key_account}).")),
            }
        }
        LaneTruthLane::Formatting => {
            let (provider, model) = formatting_identity_with(config, settings);
            let endpoint = match provider {
                ProviderKind::OpenAiResponses => {
                    endpoint_with_settings(LlmMode::Formatting, config, settings)
                }
                ProviderKind::AnthropicMessages => anthropic_messages_endpoint(),
            };
            // The formatting runtime intentionally owns a separate credential,
            // regardless of wire provider (see ai_formatting::get_llm_api_key).
            let key_account = "LLM_FORMATTING_API_KEY";
            let key_present = secret_with_keychain(key_account, &load_key).is_some();
            LaneTruthSnapshot {
                lane,
                provider_id: provider.as_str().to_string(),
                endpoint,
                model,
                key_account: key_account.to_string(),
                key_present,
                account_auth: false,
                available: key_present,
                unavailable_reason: (!key_present).then(|| {
                    format!("The formatting lane has no stored credential ({key_account}).")
                }),
            }
        }
        LaneTruthLane::Assistive => {
            let runtime = assistive_snapshot_with(config, settings, &load_key);
            let availability = availability_of(runtime.clone());
            let key_account = runtime.provider.api_key_env_key().to_string();
            LaneTruthSnapshot {
                lane,
                provider_id: runtime.provider.as_str().to_string(),
                endpoint: runtime.endpoint,
                model: runtime.model,
                key_account,
                key_present: runtime.api_key.is_some(),
                account_auth: runtime.account_auth,
                available: availability.is_ok(),
                unavailable_reason: availability.err(),
            }
        }
    }
}

/// Everything the agent send path needs to reach the assistive provider,
/// resolved from the same fresh settings → env → Keychain hierarchy as the
/// individual lane resolvers. `api_key: None` means "send without auth
/// headers" — valid for key-optional (self-hosted / LAN) endpoints.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssistiveLaneSnapshot {
    pub provider: ProviderKind,
    pub endpoint: String,
    pub model: String,
    pub api_key: Option<String>,
    /// True when the lane must authenticate with the stored ChatGPT account
    /// tokens instead of an API key: OpenAI provider, official (key-requiring)
    /// endpoint, no API key anywhere, but "Sign in with ChatGPT" tokens are
    /// stored. An explicit API key always wins; account tokens never ride to a
    /// non-official endpoint. The send path asks `account_auth` for a fresh
    /// bearer per request (auto-refresh), never a frozen token from here.
    pub account_auth: bool,
}

pub fn assistive_snapshot(config: &Config) -> AssistiveLaneSnapshot {
    assistive_snapshot_with(config, &UserSettings::load(), keychain::load_key)
}

fn assistive_snapshot_with(
    config: &Config,
    settings: &UserSettings,
    load_key: impl Fn(&str) -> Option<String>,
) -> AssistiveLaneSnapshot {
    let (provider, model) = assistive_identity_with(config, settings);
    let key_account = provider.api_key_env_key();
    let endpoint = match provider {
        ProviderKind::OpenAiResponses => {
            endpoint_with_settings(LlmMode::Assistive, config, settings)
        }
        ProviderKind::AnthropicMessages => anthropic_messages_endpoint(),
    };
    let api_key = secret_with_keychain(key_account, &load_key);
    let account_auth = provider == ProviderKind::OpenAiResponses
        && api_key.is_none()
        && endpoint_requires_api_key(&endpoint)
        && secret_with_keychain(account_auth::OPENAI_ACCOUNT_TOKENS_ACCOUNT, &load_key).is_some();
    AssistiveLaneSnapshot {
        provider,
        endpoint,
        model,
        api_key,
        account_auth,
    }
}

/// Provider identity + wire model for the formatting lane. OpenAI-compatible
/// providers retain the Responses-specific model guard, while Anthropic keeps
/// an explicitly configured Claude model instead of falling through to an
/// unrelated OpenAI default.
pub fn formatting_identity(config: &Config) -> (ProviderKind, String) {
    formatting_identity_with(config, &UserSettings::load())
}

fn formatting_identity_with(config: &Config, settings: &UserSettings) -> (ProviderKind, String) {
    let provider = provider_with_settings(LlmMode::Formatting, settings);
    let model = model_for_provider_with_settings(LlmMode::Formatting, provider, config, settings);
    (provider, model)
}

/// Provider identity + wire model for the assistive lane WITHOUT touching the
/// Keychain — safe for hot paths that only label metadata (thread persistence,
/// the vision gate).
pub fn assistive_identity(config: &Config) -> (ProviderKind, String) {
    assistive_identity_with(config, &UserSettings::load())
}

fn assistive_identity_with(config: &Config, settings: &UserSettings) -> (ProviderKind, String) {
    let provider = provider_with_settings(LlmMode::Assistive, settings);
    let model = model_for_provider_with_settings(LlmMode::Assistive, provider, config, settings);
    (provider, model)
}

fn anthropic_model_with_settings(lane: LlmMode, settings: &UserSettings) -> String {
    let (lane_key, lane_setting, lane_default) = match lane {
        LlmMode::Formatting => (
            "LLM_FORMATTING_MODEL",
            settings.llm_formatting_model.clone(),
            DEFAULT_ANTHROPIC_FORMATTING_MODEL,
        ),
        LlmMode::Assistive => (
            "LLM_ASSISTIVE_MODEL",
            settings.llm_assistive_model.clone(),
            DEFAULT_ANTHROPIC_MODEL,
        ),
    };
    let claude_model = |candidate: String| candidate.starts_with("claude").then_some(candidate);

    non_empty_option(lane_setting)
        .and_then(claude_model)
        .or_else(|| env_non_empty(lane_key).and_then(claude_model))
        .unwrap_or_else(|| lane_default.to_string())
}

/// Ready snapshot of the assistive lane, or the user-facing reason it cannot
/// reach a model. The `Err` string is actionable: it names the lane, the
/// resolved endpoint, and the exact missing piece — never a generic
/// "add an API key".
pub fn assistive_availability(config: &Config) -> Result<AssistiveLaneSnapshot, String> {
    availability_of(assistive_snapshot(config))
}

fn availability_of(snapshot: AssistiveLaneSnapshot) -> Result<AssistiveLaneSnapshot, String> {
    if snapshot.api_key.is_some() {
        return Ok(snapshot);
    }
    match snapshot.provider {
        ProviderKind::OpenAiResponses if !endpoint_requires_api_key(&snapshot.endpoint) => {
            Ok(snapshot)
        }
        // A signed-in ChatGPT account is a complete credential for the official
        // OpenAI endpoint — the agent must work with ONLY that login.
        ProviderKind::OpenAiResponses if snapshot.account_auth => Ok(snapshot),
        ProviderKind::OpenAiResponses => Err(format!(
            "The assistive lane points at {}, which requires an API key, and none is stored \
             (Keychain account LLM_ASSISTIVE_API_KEY). Add a key in Settings, sign in with \
             your ChatGPT account in Settings → Keys, or switch the assistive endpoint in \
             Settings → Engine to a key-optional server such as {}.",
            snapshot.endpoint, SUGGESTED_KEY_OPTIONAL_ENDPOINT
        )),
        ProviderKind::AnthropicMessages if !endpoint_requires_api_key(&snapshot.endpoint) => {
            Ok(snapshot)
        }
        ProviderKind::AnthropicMessages => Err(format!(
            "The assistive provider is Anthropic ({}), but no key is stored \
             (Keychain account LLM_ANTHROPIC_API_KEY). Add an Anthropic key in Settings, or \
             switch the assistive provider to an OpenAI-compatible endpoint such as {}.",
            snapshot.endpoint, SUGGESTED_KEY_OPTIONAL_ENDPOINT
        )),
    }
}

/// Official cloud APIs reject unauthenticated requests outright; every other
/// endpoint (self-hosted, LAN, Libraxis) may be key-optional and gets a clean
/// unauthenticated request instead of a hard refusal at the availability gate.
fn endpoint_requires_api_key(endpoint: &str) -> bool {
    let host = endpoint
        .split("://")
        .nth(1)
        .unwrap_or(endpoint)
        .split(['/', ':'])
        .next()
        .unwrap_or_default();
    host.eq_ignore_ascii_case("api.openai.com") || host.eq_ignore_ascii_case("api.anthropic.com")
}

pub(crate) fn endpoint_for_account(config: &Config, account: &str) -> String {
    let settings = UserSettings::load();
    match account {
        "LLM_FORMATTING_API_KEY" => endpoint_with_settings(LlmMode::Formatting, config, &settings),
        "LLM_ASSISTIVE_API_KEY" => endpoint_with_settings(LlmMode::Assistive, config, &settings),
        _ => shared_endpoint_with_settings(config, &settings),
    }
}

fn shared_endpoint_with_settings(config: &Config, settings: &UserSettings) -> String {
    let resolved = non_empty_option(settings.llm_endpoint.clone())
        .or_else(|| env_non_empty("LLM_ENDPOINT"))
        .or_else(|| non_empty_option(config.llm_endpoint.clone()))
        .unwrap_or_else(|| DEFAULT_OPENAI_RESPONSES_ENDPOINT.to_string());
    normalize_openai_responses_endpoint(&resolved)
}

pub(crate) fn model_for_account(config: &Config, account: &str) -> String {
    let settings = UserSettings::load();
    match account {
        "LLM_FORMATTING_API_KEY" => model_with_settings(LlmMode::Formatting, config, &settings),
        "LLM_ASSISTIVE_API_KEY" => model_with_settings(LlmMode::Assistive, config, &settings),
        _ => shared_model_with_settings(&settings),
    }
}

fn shared_model_with_settings(settings: &UserSettings) -> String {
    let openai_model = |candidate: String| (!candidate.starts_with("claude")).then_some(candidate);
    non_empty_option(settings.llm_model.clone())
        .and_then(openai_model)
        .or_else(|| env_non_empty("LLM_MODEL").and_then(openai_model))
        .unwrap_or_else(|| DEFAULT_LLM_MODEL.to_string())
}

pub fn normalize_openai_responses_endpoint(endpoint: &str) -> String {
    normalize_endpoint(
        endpoint,
        "/v1/responses",
        &["/v1/responses", "/v1/chat/completions", "/v1/completions"],
    )
}

pub(crate) fn normalize_anthropic_messages_endpoint(endpoint: &str) -> String {
    normalize_endpoint(endpoint, "/v1/messages", &["/v1/messages", "/v1/responses"])
}

pub(crate) fn anthropic_messages_endpoint() -> String {
    let endpoint = env_non_empty("LLM_ANTHROPIC_ENDPOINT")
        .unwrap_or_else(|| DEFAULT_ANTHROPIC_MESSAGES_ENDPOINT.to_string());
    normalize_anthropic_messages_endpoint(&endpoint)
}

fn normalize_endpoint(endpoint: &str, canonical_suffix: &str, known_suffixes: &[&str]) -> String {
    let mut base = endpoint.trim().trim_end_matches('/').to_string();
    for suffix in known_suffixes {
        if base.ends_with(suffix) {
            base.truncate(base.len() - suffix.len());
            return format!("{base}{canonical_suffix}");
        }
    }
    if base.ends_with("/v1") {
        base.truncate(base.len() - "/v1".len());
    }
    format!("{base}{canonical_suffix}")
}

fn env_non_empty(key: &str) -> Option<String> {
    std::env::var(key).ok().and_then(non_empty)
}

fn non_empty(value: String) -> Option<String> {
    let value = value.trim().to_string();
    (!value.is_empty()).then_some(value)
}

fn non_empty_option(value: Option<String>) -> Option<String> {
    value.and_then(non_empty)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Config, DEFAULT_ASSISTIVE_MODEL, DEFAULT_FORMATTING_MODEL, UserSettings};
    use crate::llm::provider::{LlmMode, ProviderKind};
    use serial_test::serial;
    use tempfile::TempDir;

    #[test]
    #[serial]
    fn secret_prefers_a_non_empty_env_value() {
        let _key = EnvGuard::set("LLM_ASSISTIVE_API_KEY", "  env-secret  ");

        assert_eq!(
            secret_with_keychain("LLM_ASSISTIVE_API_KEY", |_| {
                Some("keychain-secret".to_string())
            }),
            Some("env-secret".to_string())
        );
    }

    #[test]
    #[serial]
    fn secret_falls_back_to_keychain_when_env_is_empty_or_unset() {
        let empty = EnvGuard::set("LLM_ASSISTIVE_API_KEY", "   ");
        assert_eq!(
            secret_with_keychain("LLM_ASSISTIVE_API_KEY", |_| {
                Some("  keychain-secret  ".to_string())
            }),
            Some("keychain-secret".to_string())
        );
        drop(empty);

        let _unset = EnvGuard::remove("LLM_ASSISTIVE_API_KEY");
        assert_eq!(
            secret_with_keychain("LLM_ASSISTIVE_API_KEY", |_| {
                Some("keychain-only".to_string())
            }),
            Some("keychain-only".to_string())
        );
    }

    #[test]
    #[serial]
    fn assistive_endpoint_uses_lane_then_shared_then_config_then_default() {
        let config = Config {
            llm_endpoint: Some("https://config.example/v1".to_string()),
            ..Config::default()
        };
        let settings = UserSettings::default();
        let lane = EnvGuard::set("LLM_ASSISTIVE_ENDPOINT", "https://lane.example/custom/v1");
        let shared = EnvGuard::set("LLM_ENDPOINT", "https://shared.example/v1");

        assert_eq!(
            endpoint_with_settings(LlmMode::Assistive, &config, &settings),
            "https://lane.example/custom/v1/responses"
        );
        drop(lane);
        let _lane_unset = EnvGuard::remove("LLM_ASSISTIVE_ENDPOINT");
        assert_eq!(
            endpoint_with_settings(LlmMode::Assistive, &config, &settings),
            "https://shared.example/v1/responses"
        );
        drop(shared);
        let _shared_unset = EnvGuard::remove("LLM_ENDPOINT");
        assert_eq!(
            endpoint_with_settings(LlmMode::Assistive, &config, &settings),
            "https://config.example/v1/responses"
        );

        let no_config = Config {
            llm_endpoint: None,
            ..Config::default()
        };
        assert_eq!(
            endpoint_with_settings(LlmMode::Assistive, &no_config, &settings),
            DEFAULT_OPENAI_RESPONSES_ENDPOINT
        );
    }

    #[test]
    #[serial]
    fn persisted_lane_endpoint_beats_a_stale_bootstrap_env_value() {
        let _lane = EnvGuard::set(
            "LLM_ASSISTIVE_ENDPOINT",
            "https://stale-bootstrap.example/v1",
        );
        let _shared = EnvGuard::remove("LLM_ENDPOINT");
        let settings = UserSettings {
            llm_assistive_endpoint: Some("https://fresh-settings.example/v1".to_string()),
            ..UserSettings::default()
        };

        assert_eq!(
            endpoint_with_settings(LlmMode::Assistive, &Config::default(), &settings),
            "https://fresh-settings.example/v1/responses"
        );
    }

    #[test]
    fn responses_endpoint_normalizes_openrouter_and_libraxis_bases() {
        assert_eq!(
            normalize_openai_responses_endpoint("https://openrouter.ai/api/v1"),
            "https://openrouter.ai/api/v1/responses"
        );
        assert_eq!(
            normalize_openai_responses_endpoint("https://api.libraxis.cloud/v1"),
            "https://api.libraxis.cloud/v1/responses"
        );
    }

    #[test]
    #[serial]
    fn lane_models_use_fresh_settings_and_lane_defaults() {
        let _lane = EnvGuard::set("LLM_ASSISTIVE_MODEL", "stale-bootstrap-model");
        let _shared = EnvGuard::remove("LLM_MODEL");
        let settings = UserSettings {
            llm_assistive_model: Some("fresh-assistive-model".to_string()),
            ..UserSettings::default()
        };

        assert_eq!(
            model_with_settings(LlmMode::Assistive, &Config::default(), &settings),
            "fresh-assistive-model"
        );
        assert_eq!(
            model_with_settings(
                LlmMode::Formatting,
                &Config::default(),
                &UserSettings::default()
            ),
            DEFAULT_FORMATTING_MODEL
        );

        let _assistive_unset = EnvGuard::remove("LLM_ASSISTIVE_MODEL");
        assert_eq!(
            model_with_settings(
                LlmMode::Assistive,
                &Config::default(),
                &UserSettings::default()
            ),
            DEFAULT_ASSISTIVE_MODEL
        );
    }

    #[test]
    #[serial]
    fn provider_delegates_to_the_canonical_provider_resolver() {
        let _provider = EnvGuard::set("LLM_ASSISTIVE_PROVIDER", "anthropic-messages");

        assert_eq!(
            provider_with_settings(LlmMode::Assistive, &UserSettings::default()),
            ProviderKind::AnthropicMessages
        );
    }

    #[test]
    #[serial]
    fn formatting_identity_keeps_a_fresh_claude_model_for_anthropic() {
        let _provider = EnvGuard::set("LLM_FORMATTING_PROVIDER", "anthropic-messages");
        let _model = EnvGuard::set("LLM_FORMATTING_MODEL", "claude-stale-bootstrap");
        let settings = UserSettings {
            llm_formatting_model: Some("claude-sonnet-4-6".to_string()),
            ..UserSettings::default()
        };

        assert_eq!(
            formatting_identity_with(&Config::default(), &settings),
            (
                ProviderKind::AnthropicMessages,
                "claude-sonnet-4-6".to_string()
            )
        );
    }

    /// Clear every env var the assistive-lane resolution consults, so the
    /// availability tests below are hermetic on any host.
    fn lane_env_guards() -> Vec<EnvGuard> {
        vec![
            EnvGuard::remove("LLM_ASSISTIVE_PROVIDER"),
            EnvGuard::remove("LLM_ASSISTIVE_ENDPOINT"),
            EnvGuard::remove("LLM_ASSISTIVE_MODEL"),
            EnvGuard::remove("LLM_ENDPOINT"),
            EnvGuard::remove("LLM_MODEL"),
            EnvGuard::remove("LLM_ASSISTIVE_API_KEY"),
            EnvGuard::remove("LLM_ANTHROPIC_API_KEY"),
            EnvGuard::remove("LLM_ANTHROPIC_ENDPOINT"),
            EnvGuard::remove(account_auth::OPENAI_ACCOUNT_TOKENS_ACCOUNT),
        ]
    }

    #[test]
    #[serial]
    fn signed_in_chatgpt_account_alone_makes_the_official_endpoint_available() {
        let _env = lane_env_guards();

        let snapshot =
            assistive_snapshot_with(&Config::default(), &UserSettings::default(), |account| {
                (account == account_auth::OPENAI_ACCOUNT_TOKENS_ACCOUNT)
                    .then(|| r#"{"provider":"openai-responses"}"#.to_string())
            });
        assert!(snapshot.account_auth, "stored tokens must arm account auth");
        assert_eq!(snapshot.api_key, None);

        let ready = availability_of(snapshot).expect("ChatGPT login alone must be enough");
        assert!(ready.account_auth);
    }

    #[test]
    #[serial]
    fn explicit_api_key_wins_over_stored_account_tokens() {
        let _env = lane_env_guards();

        let snapshot =
            assistive_snapshot_with(&Config::default(), &UserSettings::default(), |account| {
                match account {
                    "LLM_ASSISTIVE_API_KEY" => Some("kc-secret".to_string()),
                    account_auth::OPENAI_ACCOUNT_TOKENS_ACCOUNT => {
                        Some(r#"{"provider":"openai-responses"}"#.to_string())
                    }
                    _ => None,
                }
            });

        assert_eq!(snapshot.api_key.as_deref(), Some("kc-secret"));
        assert!(
            !snapshot.account_auth,
            "explicit API key must win over account tokens"
        );
    }

    #[test]
    #[serial]
    fn account_tokens_never_ride_to_a_key_optional_endpoint() {
        let _env = lane_env_guards();
        let settings = UserSettings {
            llm_assistive_endpoint: Some("https://api.libraxis.cloud/v1".to_string()),
            ..UserSettings::default()
        };

        let snapshot = assistive_snapshot_with(&Config::default(), &settings, |account| {
            (account == account_auth::OPENAI_ACCOUNT_TOKENS_ACCOUNT)
                .then(|| r#"{"provider":"openai-responses"}"#.to_string())
        });

        assert!(
            !snapshot.account_auth,
            "account bearer must not leak to non-official endpoints"
        );
        // The lane stays available through the key-optional arm, unauthenticated.
        let ready = availability_of(snapshot).expect("key-optional endpoint works keyless");
        assert_eq!(ready.api_key, None);
    }

    #[test]
    #[serial]
    fn unconfigured_lane_is_unavailable_with_an_actionable_reason() {
        let _env = lane_env_guards();

        let snapshot =
            assistive_snapshot_with(&Config::default(), &UserSettings::default(), |_| None);
        let reason = availability_of(snapshot).expect_err("default lane needs a key");

        assert!(
            reason.contains(DEFAULT_OPENAI_RESPONSES_ENDPOINT),
            "{reason}"
        );
        assert!(reason.contains("LLM_ASSISTIVE_API_KEY"), "{reason}");
        assert!(reason.contains(SUGGESTED_KEY_OPTIONAL_ENDPOINT), "{reason}");
    }

    #[test]
    #[serial]
    fn key_optional_endpoint_is_available_without_any_api_key() {
        let _env = lane_env_guards();
        let settings = UserSettings {
            llm_assistive_endpoint: Some("https://api.libraxis.cloud/v1".to_string()),
            ..UserSettings::default()
        };

        let snapshot = assistive_snapshot_with(&Config::default(), &settings, |_| None);
        let ready = availability_of(snapshot).expect("local-first lane must work keyless");

        assert_eq!(ready.endpoint, "https://api.libraxis.cloud/v1/responses");
        assert_eq!(ready.api_key, None);
    }

    #[test]
    #[serial]
    fn keychain_only_key_makes_the_official_endpoint_available() {
        let _env = lane_env_guards();

        let snapshot =
            assistive_snapshot_with(&Config::default(), &UserSettings::default(), |account| {
                (account == "LLM_ASSISTIVE_API_KEY").then(|| "kc-secret".to_string())
            });
        let ready = availability_of(snapshot).expect("keychain key alone must be enough");

        assert_eq!(ready.api_key.as_deref(), Some("kc-secret"));
        assert_eq!(ready.endpoint, DEFAULT_OPENAI_RESPONSES_ENDPOINT);
    }

    #[test]
    #[serial]
    fn anthropic_lane_without_its_key_names_the_anthropic_account() {
        let _env = lane_env_guards();
        let settings = UserSettings {
            llm_assistive_provider: Some("anthropic-messages".to_string()),
            ..UserSettings::default()
        };

        let snapshot = assistive_snapshot_with(&Config::default(), &settings, |_| None);
        let reason = availability_of(snapshot).expect_err("anthropic lane requires its key");

        assert!(reason.contains("LLM_ANTHROPIC_API_KEY"), "{reason}");
        assert!(reason.contains("Anthropic"), "{reason}");
    }

    #[test]
    #[serial]
    fn self_hosted_anthropic_lane_is_available_without_an_api_key() {
        let _env = lane_env_guards();
        let _endpoint = EnvGuard::set("LLM_ANTHROPIC_ENDPOINT", "http://127.0.0.1:8080/v1");
        let settings = UserSettings {
            llm_assistive_provider: Some("anthropic-messages".to_string()),
            ..UserSettings::default()
        };

        let snapshot = assistive_snapshot_with(&Config::default(), &settings, |_| None);
        let ready = availability_of(snapshot).expect("self-hosted Anthropic may be key-optional");

        assert_eq!(ready.endpoint, "http://127.0.0.1:8080/v1/messages");
        assert_eq!(ready.api_key, None);
    }

    #[test]
    fn official_api_hosts_require_keys_case_insensitively() {
        assert!(endpoint_requires_api_key("https://API.OPENAI.COM/v1"));
        assert!(endpoint_requires_api_key(
            "https://Api.Anthropic.Com/v1/messages"
        ));
        assert!(!endpoint_requires_api_key(
            "https://openai-compatible.example/v1"
        ));
    }

    #[test]
    #[serial]
    fn fresh_settings_endpoint_flips_availability_without_a_restart() {
        let _env = lane_env_guards();
        // Stale bootstrap env points at the official cloud; no key anywhere.
        let _stale = EnvGuard::set("LLM_ASSISTIVE_ENDPOINT", "https://api.openai.com/v1");

        let before =
            assistive_snapshot_with(&Config::default(), &UserSettings::default(), |_| None);
        assert!(
            availability_of(before).is_err(),
            "official cloud without a key"
        );

        // The user saves a key-optional endpoint in Settings — the very next
        // resolution must see it, no restart, env untouched.
        let fresh = UserSettings {
            llm_assistive_endpoint: Some("https://api.libraxis.cloud/v1".to_string()),
            ..UserSettings::default()
        };
        let after = availability_of(assistive_snapshot_with(&Config::default(), &fresh, |_| {
            None
        }))
        .expect("fresh settings must flip availability immediately");
        assert_eq!(after.endpoint, "https://api.libraxis.cloud/v1/responses");
    }

    #[test]
    #[serial]
    fn anthropic_identity_uses_a_claude_model_and_openai_identity_never_does() {
        let _env = lane_env_guards();

        let anthropic = UserSettings {
            llm_assistive_provider: Some("anthropic-messages".to_string()),
            llm_assistive_model: Some("claude-opus-4-8".to_string()),
            ..UserSettings::default()
        };
        assert_eq!(
            assistive_identity_with(&Config::default(), &anthropic),
            (
                ProviderKind::AnthropicMessages,
                "claude-opus-4-8".to_string()
            )
        );

        // A leftover claude model id never leaks onto the Responses wire path.
        let openai = UserSettings {
            llm_assistive_model: Some("claude-opus-4-8".to_string()),
            ..UserSettings::default()
        };
        assert_eq!(
            assistive_identity_with(&Config::default(), &openai),
            (
                ProviderKind::OpenAiResponses,
                DEFAULT_ASSISTIVE_MODEL.to_string()
            )
        );
    }

    #[test]
    #[serial]
    fn persisted_assistive_provider_beats_a_stale_bootstrap_env_after_reload() {
        let data_dir = TempDir::new().expect("isolated data dir");
        let _data_dir = EnvGuard::set(
            "CODESCRIBE_DATA_DIR",
            data_dir.path().to_string_lossy().as_ref(),
        );
        let _provider = EnvGuard::set("LLM_ASSISTIVE_PROVIDER", "openai-responses");

        UserSettings {
            llm_assistive_provider: Some("anthropic-messages".to_string()),
            ..Default::default()
        }
        .save()
        .expect("persist assistive provider");

        assert_eq!(
            provider(LlmMode::Assistive),
            ProviderKind::AnthropicMessages
        );
    }

    #[test]
    #[serial]
    fn lane_truth_snapshot_matches_individual_resolvers_across_truth_table() {
        struct SnapshotCase {
            name: &'static str,
            lane: LaneTruthLane,
            settings: UserSettings,
            env: Vec<(&'static str, &'static str)>,
            keys: Vec<(&'static str, &'static str)>,
        }

        let cases = vec![
            SnapshotCase {
                name: "main env overrides",
                lane: LaneTruthLane::Main,
                settings: UserSettings::default(),
                env: vec![
                    ("LLM_ENDPOINT", "https://main-env.example/v1"),
                    ("LLM_MODEL", "main-env-model"),
                    ("LLM_API_KEY", "main-env-key"),
                ],
                keys: vec![],
            },
            SnapshotCase {
                name: "main fresh settings and keychain",
                lane: LaneTruthLane::Main,
                settings: UserSettings {
                    llm_endpoint: Some("https://main-settings.example/v1".to_string()),
                    llm_model: Some("main-settings-model".to_string()),
                    ..UserSettings::default()
                },
                env: vec![],
                keys: vec![("LLM_API_KEY", "main-keychain-key")],
            },
            SnapshotCase {
                name: "formatting defaults and env key",
                lane: LaneTruthLane::Formatting,
                settings: UserSettings::default(),
                env: vec![("LLM_FORMATTING_API_KEY", "formatting-env-key")],
                keys: vec![],
            },
            SnapshotCase {
                name: "formatting fresh settings and keychain",
                lane: LaneTruthLane::Formatting,
                settings: UserSettings {
                    llm_formatting_endpoint: Some(
                        "https://formatting-settings.example/v1".to_string(),
                    ),
                    llm_formatting_model: Some("formatting-settings-model".to_string()),
                    ..UserSettings::default()
                },
                env: vec![],
                keys: vec![("LLM_FORMATTING_API_KEY", "formatting-keychain-key")],
            },
            SnapshotCase {
                name: "formatting anthropic env identity",
                lane: LaneTruthLane::Formatting,
                settings: UserSettings::default(),
                env: vec![
                    ("LLM_FORMATTING_PROVIDER", "anthropic-messages"),
                    ("LLM_FORMATTING_MODEL", "claude-sonnet-test"),
                    ("LLM_ANTHROPIC_ENDPOINT", "http://127.0.0.1:18080/v1"),
                ],
                keys: vec![("LLM_FORMATTING_API_KEY", "formatting-anthropic-key")],
            },
            SnapshotCase {
                name: "assistive official endpoint unavailable",
                lane: LaneTruthLane::Assistive,
                settings: UserSettings::default(),
                env: vec![],
                keys: vec![],
            },
            SnapshotCase {
                name: "assistive fresh settings and keychain",
                lane: LaneTruthLane::Assistive,
                settings: UserSettings {
                    llm_assistive_endpoint: Some(
                        "https://assistive-settings.example/v1".to_string(),
                    ),
                    llm_assistive_model: Some("assistive-settings-model".to_string()),
                    ..UserSettings::default()
                },
                env: vec![],
                keys: vec![("LLM_ASSISTIVE_API_KEY", "assistive-keychain-key")],
            },
            SnapshotCase {
                name: "assistive anthropic env and env key",
                lane: LaneTruthLane::Assistive,
                settings: UserSettings {
                    llm_assistive_provider: Some("anthropic-messages".to_string()),
                    llm_assistive_model: Some("claude-opus-test".to_string()),
                    ..UserSettings::default()
                },
                env: vec![
                    (
                        "LLM_ANTHROPIC_ENDPOINT",
                        "https://anthropic-proxy.example/v1",
                    ),
                    ("LLM_ANTHROPIC_API_KEY", "anthropic-env-key"),
                ],
                keys: vec![],
            },
            SnapshotCase {
                name: "assistive key-optional endpoint",
                lane: LaneTruthLane::Assistive,
                settings: UserSettings {
                    llm_assistive_endpoint: Some("https://api.libraxis.cloud/v1".to_string()),
                    ..UserSettings::default()
                },
                env: vec![],
                keys: vec![],
            },
            SnapshotCase {
                name: "assistive account auth",
                lane: LaneTruthLane::Assistive,
                settings: UserSettings::default(),
                env: vec![],
                keys: vec![(
                    account_auth::OPENAI_ACCOUNT_TOKENS_ACCOUNT,
                    r#"{"provider":"openai-responses"}"#,
                )],
            },
        ];
        assert!(
            cases.len() >= 8,
            "parity table must retain at least 8 cases"
        );

        for case in cases {
            let _clean_env = snapshot_env_guards();
            let _case_env: Vec<_> = case
                .env
                .iter()
                .map(|(key, value)| EnvGuard::set(key, value))
                .collect();
            let load_key = |account: &str| {
                case.keys
                    .iter()
                    .find(|(candidate, _)| *candidate == account)
                    .map(|(_, value)| (*value).to_string())
            };
            let config = Config::default();
            let snapshot = lane_truth_snapshot_with(case.lane, &config, &case.settings, &load_key);

            assert_eq!(snapshot.lane, case.lane, "{}: lane", case.name);
            match case.lane {
                LaneTruthLane::Main => {
                    assert_eq!(
                        snapshot.provider_id,
                        ProviderKind::OpenAiResponses.as_str(),
                        "{}: provider",
                        case.name
                    );
                    assert_eq!(
                        snapshot.endpoint,
                        shared_endpoint_with_settings(&config, &case.settings),
                        "{}: endpoint",
                        case.name
                    );
                    assert_eq!(
                        snapshot.model,
                        shared_model_with_settings(&case.settings),
                        "{}: model",
                        case.name
                    );
                    let key_present = secret_with_keychain("LLM_API_KEY", &load_key).is_some();
                    assert_eq!(snapshot.key_present, key_present, "{}: key", case.name);
                    assert_eq!(snapshot.available, key_present, "{}: available", case.name);
                }
                LaneTruthLane::Formatting => {
                    let (provider, model) = formatting_identity_with(&config, &case.settings);
                    let endpoint = match provider {
                        ProviderKind::OpenAiResponses => {
                            endpoint_with_settings(LlmMode::Formatting, &config, &case.settings)
                        }
                        ProviderKind::AnthropicMessages => anthropic_messages_endpoint(),
                    };
                    assert_eq!(
                        snapshot.provider_id,
                        provider.as_str(),
                        "{}: provider",
                        case.name
                    );
                    assert_eq!(snapshot.endpoint, endpoint, "{}: endpoint", case.name);
                    assert_eq!(snapshot.model, model, "{}: model", case.name);
                    let key_present =
                        secret_with_keychain("LLM_FORMATTING_API_KEY", &load_key).is_some();
                    assert_eq!(snapshot.key_present, key_present, "{}: key", case.name);
                    assert_eq!(snapshot.available, key_present, "{}: available", case.name);
                }
                LaneTruthLane::Assistive => {
                    let runtime = assistive_snapshot_with(&config, &case.settings, &load_key);
                    let availability = availability_of(runtime.clone());
                    assert_eq!(
                        snapshot.provider_id,
                        runtime.provider.as_str(),
                        "{}: provider",
                        case.name
                    );
                    assert_eq!(
                        snapshot.endpoint, runtime.endpoint,
                        "{}: endpoint",
                        case.name
                    );
                    assert_eq!(snapshot.model, runtime.model, "{}: model", case.name);
                    assert_eq!(
                        snapshot.key_present,
                        runtime.api_key.is_some(),
                        "{}: key",
                        case.name
                    );
                    assert_eq!(
                        snapshot.account_auth, runtime.account_auth,
                        "{}: account auth",
                        case.name
                    );
                    assert_eq!(
                        snapshot.available,
                        availability.is_ok(),
                        "{}: available",
                        case.name
                    );
                    assert_eq!(
                        snapshot.unavailable_reason,
                        availability.err(),
                        "{}: reason",
                        case.name
                    );
                }
            }
        }
    }

    fn snapshot_env_guards() -> Vec<EnvGuard> {
        let mut guards = lane_env_guards();
        guards.extend([
            EnvGuard::remove("LLM_API_KEY"),
            EnvGuard::remove("LLM_FORMATTING_API_KEY"),
            EnvGuard::remove("LLM_FORMATTING_PROVIDER"),
            EnvGuard::remove("LLM_FORMATTING_ENDPOINT"),
            EnvGuard::remove("LLM_FORMATTING_MODEL"),
        ]);
        guards
    }

    struct EnvGuard {
        key: &'static str,
        previous: Option<String>,
    }

    impl EnvGuard {
        fn set(key: &'static str, value: &str) -> Self {
            let previous = std::env::var(key).ok();
            // SAFETY: these process-env tests are serialized with `serial`.
            unsafe { std::env::set_var(key, value) };
            Self { key, previous }
        }

        fn remove(key: &'static str) -> Self {
            let previous = std::env::var(key).ok();
            // SAFETY: these process-env tests are serialized with `serial`.
            unsafe { std::env::remove_var(key) };
            Self { key, previous }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            match self.previous.as_deref() {
                Some(value) => {
                    // SAFETY: these process-env tests are serialized with `serial`.
                    unsafe { std::env::set_var(self.key, value) };
                }
                None => {
                    // SAFETY: these process-env tests are serialized with `serial`.
                    unsafe { std::env::remove_var(self.key) };
                }
            }
        }
    }
}
