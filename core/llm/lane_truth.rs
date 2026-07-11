//! Canonical truth resolution for LLM lane secrets, endpoints, and models.

use crate::config::keychain;
use crate::config::{
    Config, DEFAULT_ASSISTIVE_MODEL, DEFAULT_FORMATTING_MODEL, DEFAULT_LLM_MODEL,
    DEFAULT_OPENAI_RESPONSES_ENDPOINT, UserSettings,
};
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

/// Keep provider identity owned by the existing provider resolver while giving
/// readiness and discovery one shared import surface.
pub fn provider(lane: LlmMode) -> ProviderKind {
    resolve_provider(lane)
}

pub(crate) fn endpoint_for_account(config: &Config, account: &str) -> String {
    let settings = UserSettings::load();
    match account {
        "LLM_FORMATTING_API_KEY" => endpoint_with_settings(LlmMode::Formatting, config, &settings),
        "LLM_ASSISTIVE_API_KEY" => endpoint_with_settings(LlmMode::Assistive, config, &settings),
        _ => {
            let resolved = non_empty_option(settings.llm_endpoint)
                .or_else(|| env_non_empty("LLM_ENDPOINT"))
                .or_else(|| non_empty_option(config.llm_endpoint.clone()))
                .unwrap_or_else(|| DEFAULT_OPENAI_RESPONSES_ENDPOINT.to_string());
            normalize_openai_responses_endpoint(&resolved)
        }
    }
}

pub(crate) fn model_for_account(config: &Config, account: &str) -> String {
    let settings = UserSettings::load();
    match account {
        "LLM_FORMATTING_API_KEY" => model_with_settings(LlmMode::Formatting, config, &settings),
        "LLM_ASSISTIVE_API_KEY" => model_with_settings(LlmMode::Assistive, config, &settings),
        _ => {
            let openai_model =
                |candidate: String| (!candidate.starts_with("claude")).then_some(candidate);
            non_empty_option(settings.llm_model)
                .and_then(openai_model)
                .or_else(|| env_non_empty("LLM_MODEL").and_then(openai_model))
                .unwrap_or_else(|| DEFAULT_LLM_MODEL.to_string())
        }
    }
}

pub fn normalize_openai_responses_endpoint(endpoint: &str) -> String {
    normalize_endpoint(
        endpoint,
        "/v1/responses",
        &["/v1/responses", "/v1/chat/completions", "/v1/completions"],
    )
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
            provider(LlmMode::Assistive),
            ProviderKind::AnthropicMessages
        );
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
