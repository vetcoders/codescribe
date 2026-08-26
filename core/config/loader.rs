//! Configuration loading and saving functionality.
//!
//! Handles loading from defaults, settings.json, optional .env, and runtime environment.
//!
//! Contract:
//! - `Config::default()` defines zero-state runtime truth.
//! - `settings.json` is the canonical persisted store for promoted/user-facing settings.
//! - `.env` is optional and only supplies env-managed / power-user overrides.
//! - explicit process env can still override for tests and developer runs.

use directories::BaseDirs;
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::env::VarError;
use std::fs;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tracing::{info, warn};

use super::defaults::{
    default_assistive_model, default_assistive_provider, default_formatting_model,
    default_formatting_provider, default_llm_endpoint, default_llm_model,
};
use super::energy_calibration::{SealedEnergyCalibration, energy_calibration_path};
use super::settings::{
    DEFAULT_AGENT_WORKSPACE_ROOT, FormattingPolicy, RuntimeAiExecution, RuntimeAiRequestTiming,
    RuntimeFormatterExecution, RuntimeLlmCredential, RuntimeLlmLane, RuntimeLlmLaneKind,
    RuntimeLlmLanes, RuntimeSettingsSnapshot, RuntimeSnapshotParts, SettingsLoaderInput,
    SettingsSnapshotDigest, SettingsSnapshotProvenance, SettingsSnapshotValidationError,
    UserSettings, normalize_agent_workspace_roots, parse_agent_workspace_roots,
};
use super::types::{
    Config, DeferredInsertShortcut, Language, OverlayPositionMode, TranscriptSendMode,
};
use crate::llm::account_auth;
use crate::llm::provider::{LlmMode, ProviderKind, WireFamily};

/// Has the process already seeded its environment from config? Seeding happens
/// once, at the first load; later loads read snapshots instead, so a background
/// thread never sees `set_var` racing under it.
static CONFIG_ENV_BOOTSTRAPPED: AtomicBool = AtomicBool::new(false);

/// Serializes the one bootstrap load, so two concurrent `Config::load()` calls
/// cannot both decide they are the first writer.
static CONFIG_ENV_BOOTSTRAP_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

/// Serialize the full settings read-modify-write transaction behind public
/// Config mutation APIs. Atomic renames prevent torn files, but without this
/// outer lock two distinct UI writes can both load the same snapshot and the
/// later rename silently erase the earlier field.
static CONFIG_PERSISTENCE_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

const AI_MAX_RETRIES_ENV: &str = "CODESCRIBE_AI_MAX_RETRIES";
const AI_RETRY_DELAY_MS_ENV: &str = "CODESCRIBE_AI_RETRY_DELAY_MS";
const AI_ATTEMPT_TIMEOUT_MS_ENV: &str = "CODESCRIBE_AI_ATTEMPT_TIMEOUT_MS";
const AI_INTER_CHUNK_TIMEOUT_MS_ENV: &str = "CODESCRIBE_AI_INTER_CHUNK_TIMEOUT_MS";
const DEFAULT_AI_MAX_RETRIES: u32 = 3;
const DEFAULT_AI_RETRY_DELAY_MS: u64 = 2_000;
const DEFAULT_AI_ATTEMPT_TIMEOUT_MS: u64 = 30_000;
const DEFAULT_AI_INTER_CHUNK_TIMEOUT_MS: u64 = 30_000;

fn config_persistence_guard() -> std::sync::MutexGuard<'static, ()> {
    CONFIG_PERSISTENCE_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Keys this process seeded itself. After bootstrap they are reported as absent
/// by [`Config::config_runtime_env_var`], so a later Settings write wins over
/// the value config planted at startup — that is what makes settings hot-apply
/// without a restart. Only a genuinely external env var keeps its priority.
static CONFIG_SEEDED_ENV_KEYS: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();

impl Config {
    /// Load configuration from disk or environment.
    ///
    /// Priority order:
    /// 1. Explicit process environment variables
    /// 2. `settings.json` for promoted/user-facing settings
    /// 3. Optional `.env` file for env-managed / power-user overrides
    /// 4. Default values
    ///
    /// If the .env file doesn't exist or is malformed, returns default configuration
    /// without raising an error.
    pub fn load() -> Self {
        Self::load_with_keychain_population(true)
    }

    /// Load runtime configuration without reading Keychain.
    ///
    /// This is for UI/runtime surfaces that must not trigger a macOS Keychain
    /// password prompt as a side effect of starting local dictation.
    pub fn load_without_keychain() -> Self {
        Self::load_with_keychain_population(false)
    }

    /// Run the one loader pass and seal the immutable settings truth used by a
    /// recording session. Consumers receive this value; they never re-read
    /// `settings.json` or process env during a take.
    pub fn load_runtime_snapshot(
    ) -> Result<RuntimeSettingsSnapshot, SettingsSnapshotValidationError> {
        Self::load_runtime_snapshot_with_keychain_population(true)
    }

    /// Keychain-free form of [`Self::load_runtime_snapshot`] for local capture.
    pub fn load_runtime_snapshot_without_keychain(
    ) -> Result<RuntimeSettingsSnapshot, SettingsSnapshotValidationError> {
        Self::load_runtime_snapshot_with_keychain_population(false)
    }

    fn load_runtime_snapshot_with_keychain_population(
        populate_keychain: bool,
    ) -> Result<RuntimeSettingsSnapshot, SettingsSnapshotValidationError> {
        let input = SettingsLoaderInput {
            settings_path: UserSettings::settings_path(),
            allow_env_file: true,
            allow_process_env_overrides: true,
        };
        let values = Self::load_with_keychain_population(populate_keychain);
        let user_settings = UserSettings::load();
        let settings_bytes = fs::read(&input.settings_path).ok();
        let settings_json_sha256 = settings_bytes.as_deref().map(sha256_hex);
        let mut env_overlay_keys = Self::seeded_env_keys()
            .lock()
            .map(|keys| keys.iter().cloned().collect::<Vec<_>>())
            .unwrap_or_default();
        env_overlay_keys.sort_unstable();
        let loaded_at_unix_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_millis() as u64)
            .unwrap_or_default();
        // Same loader pass, same data dir: the measured acoustic calibration is
        // read once here and frozen with the snapshot. Absence/refusal seal as
        // explicit states — the admission gate names them, nothing repairs them.
        let energy_calibration_path = energy_calibration_path();
        let energy_calibration = SealedEnergyCalibration::load(&energy_calibration_path);
        let provenance = SettingsSnapshotProvenance {
            settings_json_path: settings_bytes.as_ref().map(|_| input.settings_path.clone()),
            settings_json_sha256,
            env_overlay_keys,
            defaults_applied: true,
            loaded_at_unix_ms,
            energy_calibration_path,
            energy_calibration_sha256: energy_calibration.sha256().map(str::to_owned),
        };
        let runtime_formatting_policy = Self::config_runtime_env_var("FORMATTING_LEVEL").ok();
        let formatting_policy = FormattingPolicy::resolve(
            runtime_formatting_policy.as_deref(),
            user_settings.formatting_level.as_deref(),
        )
        .map_err(|error| SettingsSnapshotValidationError::InvalidField {
            field: "formatting_policy",
            reason: error.to_string(),
        })?;
        let llm_lanes = Self::resolve_runtime_llm_lanes(&values, &user_settings);
        let ai_execution = Self::resolve_runtime_ai_execution(formatting_policy);
        let mut digest_values = values.clone();
        digest_values.llm_api_key = digest_values
            .llm_api_key
            .as_ref()
            .map(|_| "<redacted:present>".to_string());
        digest_values.stt_api_key = digest_values
            .stt_api_key
            .as_ref()
            .map(|_| "<redacted:present>".to_string());
        let digest_material = format!(
            "{digest_values:?}\n{user_settings:?}\n{provenance:?}\nformatting_policy={}\n{}\n{}\n{}",
            formatting_policy.as_str(),
            llm_lanes.digest_material(),
            ai_execution.digest_material(),
            energy_calibration.digest_material(),
        );
        let digest = SettingsSnapshotDigest::from_hex(sha256_hex(digest_material.as_bytes()));
        RuntimeSettingsSnapshot::seal_loaded(RuntimeSnapshotParts {
            values,
            user_settings,
            llm_lanes,
            formatting_policy,
            ai_execution,
            provenance,
            digest,
            energy_calibration,
        })
    }

    /// Resolve prompt, retry, and shared Agent/formatter timing once for the
    /// selected runtime generation. No consumer may reconstruct these facts.
    fn resolve_runtime_ai_execution(formatting_policy: FormattingPolicy) -> RuntimeAiExecution {
        let (formatting_prompt, assistive_prompt) =
            super::prompts::seal_runtime_prompts(formatting_policy);
        let max_retries = Self::runtime_env_or_default(
            AI_MAX_RETRIES_ENV,
            DEFAULT_AI_MAX_RETRIES,
        );
        let retry_delay_ms = Self::runtime_env_or_default(
            AI_RETRY_DELAY_MS_ENV,
            DEFAULT_AI_RETRY_DELAY_MS,
        );
        let attempt_timeout_ms = Self::runtime_env_or_default(
            AI_ATTEMPT_TIMEOUT_MS_ENV,
            DEFAULT_AI_ATTEMPT_TIMEOUT_MS,
        );
        let inter_chunk_timeout_ms = Self::runtime_env_or_default(
            AI_INTER_CHUNK_TIMEOUT_MS_ENV,
            DEFAULT_AI_INTER_CHUNK_TIMEOUT_MS,
        );

        RuntimeAiExecution::seal(
            RuntimeFormatterExecution::seal(
                formatting_prompt,
                assistive_prompt,
                max_retries,
                Duration::from_millis(retry_delay_ms),
            ),
            RuntimeAiRequestTiming::seal(
                Duration::from_millis(attempt_timeout_ms),
                Duration::from_millis(inter_chunk_timeout_ms),
            ),
        )
    }

    fn runtime_env_or_default<T>(key: &str, default: T) -> T
    where
        T: FromStr,
    {
        Self::config_runtime_env_var(key)
            .ok()
            .and_then(|value| {
                let value = value.trim();
                (!value.is_empty()).then(|| value.parse::<T>().ok()).flatten()
            })
            .unwrap_or(default)
    }

    /// Resolve the complete LLM organ during the one settings-loader pass.
    /// Consumers only receive the sealed result; none may repeat this work.
    fn resolve_runtime_llm_lanes(values: &Config, settings: &UserSettings) -> RuntimeLlmLanes {
        RuntimeLlmLanes::seal(
            Self::resolve_runtime_llm_lane(RuntimeLlmLaneKind::Main, values, settings),
            Self::resolve_runtime_llm_lane(RuntimeLlmLaneKind::Formatting, values, settings),
            Self::resolve_runtime_llm_lane(RuntimeLlmLaneKind::Assistive, values, settings),
        )
    }

    fn resolve_runtime_llm_lane(
        lane: RuntimeLlmLaneKind,
        values: &Config,
        settings: &UserSettings,
    ) -> RuntimeLlmLane {
        let provider = Self::resolve_runtime_llm_provider(lane, settings);
        let endpoint = Self::resolve_runtime_llm_endpoint(lane, provider, values, settings);
        let model = Self::resolve_runtime_llm_model(lane, provider, settings);
        let key_account = match lane {
            RuntimeLlmLaneKind::Main => "LLM_API_KEY",
            RuntimeLlmLaneKind::Formatting => "LLM_FORMATTING_API_KEY",
            RuntimeLlmLaneKind::Assistive => provider.api_key_env_key(),
        };
        let api_key = Self::runtime_env_non_empty(key_account);
        let endpoint_requires_key = ProviderKind::endpoint_requires_api_key(&endpoint);
        let account_auth = lane == RuntimeLlmLaneKind::Assistive
            && provider.wire_family() == WireFamily::OpenAiResponses
            && endpoint_requires_key
            && account_auth::provider_oauth_config(provider).is_ok_and(|row| {
                Self::runtime_env_non_empty(row.tokens_account).is_some()
            });
        let available = api_key.is_some() || !endpoint_requires_key || account_auth;
        let unavailable_reason = (!available).then(|| {
            format!(
                "The {} lane points at {}, which requires a credential, but neither Keychain account {} nor a supported signed-in provider account is available.",
                lane.as_str(),
                endpoint,
                key_account,
            )
        });
        let credential = RuntimeLlmCredential::seal(key_account, api_key, account_auth);
        RuntimeLlmLane::seal(
            lane,
            provider,
            endpoint,
            model,
            credential,
            available,
            unavailable_reason,
        )
    }

    fn resolve_runtime_llm_provider(
        lane: RuntimeLlmLaneKind,
        settings: &UserSettings,
    ) -> ProviderKind {
        let persisted = match lane {
            RuntimeLlmLaneKind::Assistive => settings.llm_assistive_provider.clone(),
            RuntimeLlmLaneKind::Main | RuntimeLlmLaneKind::Formatting => None,
        };
        let env_key = match lane {
            RuntimeLlmLaneKind::Formatting => Some("LLM_FORMATTING_PROVIDER"),
            RuntimeLlmLaneKind::Assistive => Some("LLM_ASSISTIVE_PROVIDER"),
            RuntimeLlmLaneKind::Main => None,
        };
        let fallback = match lane {
            RuntimeLlmLaneKind::Formatting => default_formatting_provider(),
            RuntimeLlmLaneKind::Main | RuntimeLlmLaneKind::Assistive => {
                default_assistive_provider()
            }
        };
        persisted
            .and_then(Self::non_empty_string)
            .and_then(|raw| ProviderKind::from_str(&raw).ok())
            .or_else(|| {
                env_key
                    .and_then(Self::runtime_env_non_empty)
                    .and_then(|raw| ProviderKind::from_str(&raw).ok())
            })
            .or_else(|| ProviderKind::from_str(&fallback).ok())
            .unwrap_or_default()
    }

    fn resolve_runtime_llm_model(
        lane: RuntimeLlmLaneKind,
        provider: ProviderKind,
        settings: &UserSettings,
    ) -> String {
        let (lane_setting, lane_env) = match lane {
            RuntimeLlmLaneKind::Main => (None, None),
            RuntimeLlmLaneKind::Formatting => (
                settings.llm_formatting_model.clone(),
                Some("LLM_FORMATTING_MODEL"),
            ),
            RuntimeLlmLaneKind::Assistive => (
                settings.llm_assistive_model.clone(),
                Some("LLM_ASSISTIVE_MODEL"),
            ),
        };
        let owned = |candidate: String| provider.owns_model(&candidate).then_some(candidate);
        let lane_model = lane_setting
            .and_then(Self::non_empty_string)
            .and_then(owned)
            .or_else(|| {
                lane_env
                    .and_then(Self::runtime_env_non_empty)
                    .and_then(owned)
            });
        let resolved = if lane == RuntimeLlmLaneKind::Main || provider.owns_generic_lane_config() {
            lane_model
                .or_else(|| {
                    settings
                        .llm_model
                        .clone()
                        .and_then(Self::non_empty_string)
                        .and_then(owned)
                })
                .or_else(|| Self::runtime_env_non_empty("LLM_MODEL").and_then(owned))
        } else {
            lane_model
        };
        resolved.unwrap_or_else(|| match lane {
            RuntimeLlmLaneKind::Main => default_llm_model(),
            RuntimeLlmLaneKind::Formatting => {
                provider.default_model(LlmMode::Formatting).to_string()
            }
            RuntimeLlmLaneKind::Assistive => {
                provider.default_model(LlmMode::Assistive).to_string()
            }
        })
    }

    fn resolve_runtime_llm_endpoint(
        lane: RuntimeLlmLaneKind,
        provider: ProviderKind,
        values: &Config,
        settings: &UserSettings,
    ) -> String {
        let resolved = if lane == RuntimeLlmLaneKind::Main
            || provider.owns_generic_lane_config()
        {
            let (lane_setting, lane_env) = match lane {
                RuntimeLlmLaneKind::Main => (None, None),
                RuntimeLlmLaneKind::Formatting => (
                    settings.llm_formatting_endpoint.clone(),
                    Some("LLM_FORMATTING_ENDPOINT"),
                ),
                RuntimeLlmLaneKind::Assistive => (
                    settings.llm_assistive_endpoint.clone(),
                    Some("LLM_ASSISTIVE_ENDPOINT"),
                ),
            };
            lane_setting
                .and_then(Self::non_empty_string)
                .or_else(|| lane_env.and_then(Self::runtime_env_non_empty))
                .or_else(|| {
                    settings
                        .llm_endpoint
                        .clone()
                        .and_then(Self::non_empty_string)
                })
                .or_else(|| Self::runtime_env_non_empty("LLM_ENDPOINT"))
                .or_else(|| values.llm_endpoint.clone().and_then(Self::non_empty_string))
                .unwrap_or_else(default_llm_endpoint)
        } else {
            Self::runtime_env_non_empty(provider.identity().endpoint_env)
                .unwrap_or_else(|| provider.identity().default_endpoint.to_string())
        };
        provider.normalize_endpoint(&resolved)
    }

    fn runtime_env_non_empty(key: &str) -> Option<String> {
        Self::config_runtime_env_var(key)
            .ok()
            .and_then(Self::non_empty_string)
    }

    fn non_empty_string(value: String) -> Option<String> {
        let value = value.trim().to_string();
        (!value.is_empty()).then_some(value)
    }

    /// The single load path behind both public entry points.
    ///
    /// Order matters throughout: legacy `.env` keys are migrated, then one-time
    /// imports into `settings.json` run, then non-promoted `.env` values are
    /// injected into the process env — promoted keys are deliberately skipped so
    /// a stale `~/.codescribe/.env` cannot shadow a choice made in the UI.
    /// Only after that are defaults, settings, and finally explicit env applied.
    fn load_with_keychain_population(populate_keychain: bool) -> Self {
        let _data_io = match super::storage_reset::begin_app_data_io() {
            Ok(guard) => guard,
            Err(error) => {
                warn!(%error, "Config load skipped while app-data reset owns the process");
                return Self::default();
            }
        };
        let _bootstrap_guard = Self::config_env_bootstrap_guard();
        let seed_process_env = Self::can_seed_process_env();
        let env_path = Self::env_path();
        let mut file_env_vars: Option<HashMap<String, String>> = None;

        // Load .env file if it exists. It is optional and never required for
        // normal runtime: we only use it for one-time migration and env-managed
        // keys that still intentionally live outside settings.json.
        if env_path.exists() {
            // Migrate legacy keys inside existing .env (power users only)
            Self::migrate_env_legacy_keys();

            if let Ok(vars) = Self::parse_env_file(&env_path) {
                file_env_vars = Some(vars);
            }
        }

        // One-time import from legacy .env-only installs into settings.json.
        super::migrate::migrate_if_needed(file_env_vars.as_ref());
        super::migrate::migrate_agent_workspace_roots_if_needed(file_env_vars.as_ref());

        // Optional .env remains available for env-managed / power-user keys, but
        // promoted settings are intentionally excluded so stale ~/.codescribe/.env
        // cannot shadow user choices persisted in settings.json.
        if let Some(vars) = file_env_vars.as_ref() {
            Self::inject_file_env_for_runtime(vars);
        }

        // Load API keys from Keychain (only if not already set by .env).
        if populate_keychain && seed_process_env {
            super::keychain::populate_env_from_keychain();
        }

        // Load user settings from JSON
        let user_settings = super::settings::UserSettings::load();

        let mut config = Self::default();

        // Apply user settings first (lowest priority after defaults)
        config.apply_user_settings(&user_settings);

        // Hold-indicator controls remain existing power-user `.env` keys (no
        // settings.json schema or migration). Re-read just these two values on
        // every snapshot so Settings/tray writes hot-apply after process-env
        // bootstrap; an explicit process env still wins in `load_from_env`.
        if let Some(file_env) = file_env_vars.as_ref() {
            if let Some(value) = file_env.get("HOLD_INDICATOR") {
                config.hold_indicator = matches!(value.as_str(), "1" | "true" | "yes" | "on");
            }
            if let Some(value) = file_env.get("HOLD_BADGE_SIZE")
                && let Ok(size) = value.parse()
            {
                config.hold_badge_size = size;
            }
        }

        // Override with environment variables (explicit runtime env + injected env-managed .env).
        config.load_from_env();
        config.apply_default_llm_runtime_env();
        config.sanitize();
        Self::mark_process_env_bootstrapped(seed_process_env);
        config
    }

    /// Hold the bootstrap lock for the duration of a load. Skipped under `cfg(test)`,
    /// where each test intentionally re-runs bootstrap against its own temp dir.
    fn config_env_bootstrap_guard() -> Option<std::sync::MutexGuard<'static, ()>> {
        if cfg!(test) {
            None
        } else {
            Some(
                CONFIG_ENV_BOOTSTRAP_LOCK
                    .get_or_init(|| Mutex::new(()))
                    .lock()
                    .expect("config env bootstrap lock poisoned"),
            )
        }
    }

    /// May this load still write to the process environment? True exactly once
    /// in production — afterwards, mutating env would race threads that are
    /// already running.
    fn can_seed_process_env() -> bool {
        cfg!(test) || !CONFIG_ENV_BOOTSTRAPPED.load(Ordering::SeqCst)
    }

    /// Close the seeding window after a successful bootstrap load.
    fn mark_process_env_bootstrapped(seed_process_env: bool) {
        if seed_process_env && !cfg!(test) {
            CONFIG_ENV_BOOTSTRAPPED.store(true, Ordering::SeqCst);
        }
    }

    /// Lazily-initialized set of self-seeded keys.
    fn seeded_env_keys() -> &'static Mutex<HashSet<String>> {
        CONFIG_SEEDED_ENV_KEYS.get_or_init(|| Mutex::new(HashSet::new()))
    }

    /// Record that this process — not the user's shell — set `key`. No-op under
    /// test, where each case manages its own environment.
    fn remember_seeded_env_key(key: &str) {
        if cfg!(test) {
            return;
        }
        if let Ok(mut keys) = Self::seeded_env_keys().lock() {
            keys.insert(key.to_string());
        }
    }

    /// Was this value planted by config itself? If so it must not outrank a
    /// fresh persisted setting.
    fn was_seeded_env_key(key: &str) -> bool {
        if cfg!(test) {
            return false;
        }
        Self::seeded_env_keys()
            .lock()
            .map(|keys| keys.contains(key))
            .unwrap_or(false)
    }

    /// Read runtime env truth, with two deliberate departures from
    /// `std::env::var`: Keychain-backed accounts are served from the cached
    /// secret rather than the environment, and a key this process seeded during
    /// bootstrap reads as absent afterwards — so persisted settings win over
    /// config's own startup copy.
    fn config_runtime_env_var(key: &str) -> Result<String, VarError> {
        if super::keychain::KEYCHAIN_ACCOUNTS.contains(&key) {
            return super::keychain::cached_runtime_key(key).ok_or(VarError::NotPresent);
        }
        if !Self::can_seed_process_env() && Self::was_seeded_env_key(key) {
            return Err(VarError::NotPresent);
        }
        std::env::var(key)
    }

    /// Resolve the effective formatting policy from fresh runtime truth.
    ///
    /// Explicit process env wins. Values seeded internally during bootstrap are
    /// ignored after bootstrap so a Settings write takes effect without restart.
    /// Loader-boundary helper for UI/tests that do not already hold a snapshot.
    /// Resolves policy only through the canonical sealed snapshot — never by a
    /// second `UserSettings::load` + process-env reconstruct.
    pub fn formatting_policy() -> anyhow::Result<FormattingPolicy> {
        Ok(Self::load_runtime_snapshot()?.formatting_policy())
    }

    /// Resolve the roots selected in Settings from fresh persisted truth.
    ///
    /// `settings.json` is authoritative. A legacy `.env`/process value is used
    /// only when the durable field is absent, so an old bootstrap value cannot
    /// mask a live Settings write. The migration pass copies legacy `.env`
    /// roots into `settings.json` before this resolver runs.
    pub fn effective_agent_workspace_roots() -> Vec<String> {
        let settings = super::settings::UserSettings::load();
        let persisted =
            normalize_agent_workspace_roots(settings.agent_workspace_roots.unwrap_or_default());
        if !persisted.is_empty() {
            return persisted;
        }

        let env_path = Self::env_path();
        if env_path.exists()
            && let Ok(vars) = Self::parse_env_file(&env_path)
            && let Some(value) = vars.get("AGENT_WORKSPACE_ROOTS")
        {
            let roots = parse_agent_workspace_roots(value);
            if !roots.is_empty() {
                return roots;
            }
        }

        if let Ok(value) = std::env::var("AGENT_WORKSPACE_ROOTS") {
            let roots = parse_agent_workspace_roots(&value);
            if !roots.is_empty() {
                return roots;
            }
        }

        vec![DEFAULT_AGENT_WORKSPACE_ROOT.to_string()]
    }

    /// Inject optional .env values into the process environment without allowing
    /// legacy file overrides to shadow promoted settings.json-backed keys.
    fn inject_file_env_for_runtime(file_env: &HashMap<String, String>) {
        for (key, value) in file_env {
            if super::settings::is_promoted_key(key) {
                debug_assert!(
                    !super::settings::is_promoted_key(key) || !key.is_empty(),
                    "promoted key bookkeeping should never see empty names"
                );
                continue;
            }
            if std::env::var_os(key).is_none() {
                Self::config_init_set_env(key, value);
            }
        }
    }

    /// Treat a whitespace-only value as unset — an empty env var is a common way
    /// to accidentally "configure" a key into a broken state.
    fn env_missing_or_empty(key: &str) -> bool {
        Self::config_runtime_env_var(key)
            .ok()
            .is_none_or(|value| value.trim().is_empty())
    }

    /// Seed a default without ever overwriting a value the user actually set.
    fn config_init_set_env_if_missing(key: &str, value: impl AsRef<str>) {
        if Self::env_missing_or_empty(key) {
            Self::config_init_set_env(key, value.as_ref());
        }
    }

    /// Give all three LLM lanes (base, formatting, assistive) a complete
    /// endpoint/model/provider triple, so a lane whose override is unset still
    /// resolves instead of failing at first use. The base endpoint is reused for
    /// every lane; only explicitly configured values differ.
    ///
    /// Deliberately seeds no API key: a missing credential must surface as an
    /// auth error the user can act on, not as a silent fallback to some other
    /// lane's key.
    fn apply_default_llm_runtime_env(&mut self) {
        let endpoint = self
            .llm_endpoint
            .as_deref()
            .filter(|value| !value.trim().is_empty())
            .map(str::to_string)
            .unwrap_or_else(default_llm_endpoint);

        self.llm_endpoint = Some(endpoint.clone());

        Self::config_init_set_env_if_missing("LLM_ENDPOINT", &endpoint);
        Self::config_init_set_env_if_missing("LLM_MODEL", default_llm_model());
        Self::config_init_set_env_if_missing("LLM_FORMATTING_ENDPOINT", &endpoint);
        Self::config_init_set_env_if_missing("LLM_FORMATTING_MODEL", default_formatting_model());
        Self::config_init_set_env_if_missing(
            "LLM_FORMATTING_PROVIDER",
            default_formatting_provider(),
        );
        Self::config_init_set_env_if_missing("LLM_ASSISTIVE_ENDPOINT", &endpoint);
        Self::config_init_set_env_if_missing("LLM_ASSISTIVE_MODEL", default_assistive_model());
        Self::config_init_set_env_if_missing(
            "LLM_ASSISTIVE_PROVIDER",
            default_assistive_provider(),
        );
    }

    /// Load configuration values from environment variables.
    pub fn load_from_env(&mut self) {
        // Hotkeys
        if let Ok(val) = Self::config_runtime_env_var("HOLD_EXCLUSIVE") {
            self.hold_exclusive = matches!(val.as_str(), "1" | "true" | "yes" | "on");
        }
        if let Ok(val) = Self::config_runtime_env_var("HOLD_ARM_MODIFIER")
            && let Ok(arm) = val.parse()
        {
            self.hold_arm_modifier = arm;
        }
        if let Ok(val) = Self::config_runtime_env_var("HOLD_START_DELAY_MS")
            && let Ok(ms) = val.parse()
        {
            self.hold_start_delay_ms = ms;
        }
        if let Ok(val) = Self::config_runtime_env_var("DOUBLE_TAP_INTERVAL_MS")
            && let Ok(ms) = val.parse()
        {
            self.double_tap_interval_ms = ms;
        }
        if let Ok(val) = Self::config_runtime_env_var("TOGGLE_SILENCE_SEC")
            && let Ok(sec) = val.parse()
        {
            self.toggle_silence_sec = sec;
        }
        if let Ok(val) = Self::config_runtime_env_var("CODESCRIBE_DEFERRED_INSERT_SHORTCUT")
            && let Ok(shortcut) = val.parse::<DeferredInsertShortcut>()
        {
            self.deferred_insert_shortcut = shortcut;
        }

        // Language
        if let Ok(val) = Self::config_runtime_env_var("WHISPER_LANGUAGE")
            && let Ok(lang) = val.parse::<Language>()
        {
            self.whisper_language = lang;
        }

        // AI Formatting
        if let Ok(val) = Self::config_runtime_env_var("AI_FORMATTING_ENABLED") {
            self.ai_formatting_enabled =
                matches!(val.as_str(), "1" | "true" | "yes" | "on" | "enabled");
        }
        if let Ok(val) = Self::config_runtime_env_var("AUTO_PASTE_ENABLED") {
            self.auto_paste_enabled =
                matches!(val.as_str(), "1" | "true" | "yes" | "on" | "enabled");
        }
        if let Ok(val) = Self::config_runtime_env_var("TRANSCRIPT_SEND_MODE")
            && let Ok(mode) = val.parse::<TranscriptSendMode>()
        {
            self.transcript_send_mode = mode;
        }
        if let Ok(val) = Self::config_runtime_env_var("CODESCRIBE_TRANSCRIPT_TAGGING") {
            self.transcript_tagging_enabled =
                matches!(val.as_str(), "1" | "true" | "yes" | "on" | "enabled");
        }
        if let Ok(val) = Self::config_runtime_env_var("CODESCRIBE_TRANSCRIPT_TAG_TEMPLATE") {
            self.transcript_tag_template = val;
        }
        if let Ok(val) = Self::config_runtime_env_var("AI_MAX_TOKENS")
            && let Ok(tokens) = val.parse()
        {
            self.ai_max_tokens = tokens;
        }
        if let Ok(val) = Self::config_runtime_env_var("AI_ASSISTIVE_MAX_TOKENS")
            && let Ok(tokens) = val.parse()
        {
            self.ai_assistive_max_tokens = tokens;
        }

        // UI
        if let Ok(val) = Self::config_runtime_env_var("SHOW_TRAY_GLYPH") {
            self.show_tray_glyph = val.parse().unwrap_or(true);
        }
        if let Ok(val) = Self::config_runtime_env_var("SHOW_DOCK_ICON") {
            self.show_dock_icon = matches!(val.as_str(), "1" | "true" | "yes" | "on");
        }
        if let Ok(val) = Self::config_runtime_env_var("TRANSCRIPTION_OVERLAY_ENABLED") {
            self.transcription_overlay_enabled =
                matches!(val.as_str(), "1" | "true" | "yes" | "on");
        }
        if let Ok(val) = Self::config_runtime_env_var("TRAY_START_ASSISTIVE") {
            self.tray_start_assistive = matches!(val.as_str(), "1" | "true" | "yes" | "on");
        }
        if let Ok(val) = Self::config_runtime_env_var("HOLD_INDICATOR") {
            self.hold_indicator = matches!(val.as_str(), "1" | "true" | "yes" | "on");
        }
        if let Ok(val) = Self::config_runtime_env_var("HOLD_BADGE_SIZE")
            && let Ok(size) = val.parse()
        {
            self.hold_badge_size = size;
        }
        if let Ok(val) = Self::config_runtime_env_var("HOLD_BADGE_OFFSET_X")
            && let Ok(offset) = val.parse()
        {
            self.hold_badge_offset_x = offset;
        }
        if let Ok(val) = Self::config_runtime_env_var("HOLD_BADGE_OFFSET_Y")
            && let Ok(offset) = val.parse()
        {
            self.hold_badge_offset_y = offset;
        }

        if let Ok(val) = Self::config_runtime_env_var("OVERLAY_POSITION_MODE")
            && let Ok(mode) = val.parse::<OverlayPositionMode>()
        {
            self.overlay_position_mode = mode;
        }
        if let Ok(val) = Self::config_runtime_env_var("OVERLAY_CUSTOM_X")
            && let Ok(x) = val.parse()
        {
            self.overlay_custom_x = Some(x);
        }
        if let Ok(val) = Self::config_runtime_env_var("OVERLAY_CUSTOM_Y")
            && let Ok(y) = val.parse()
        {
            self.overlay_custom_y = Some(y);
        }

        // Sound
        if let Ok(val) = Self::config_runtime_env_var("BEEP_ON_START") {
            self.beep_on_start = matches!(val.as_str(), "1" | "true" | "yes" | "on");
        }
        if let Ok(val) = Self::config_runtime_env_var("AGENT_ENTER_SENDS") {
            self.agent_enter_sends = matches!(val.as_str(), "1" | "true" | "yes" | "on");
        }
        if let Ok(val) = Self::config_runtime_env_var("SOUND_NAME") {
            self.sound_name = val;
        }
        if let Ok(val) = Self::config_runtime_env_var("SOUND_VOLUME")
            && let Ok(volume) = val.parse()
        {
            self.sound_volume = volume;
        }

        // Audio
        if let Ok(val) = Self::config_runtime_env_var("AUDIO_INPUT_DEVICE") {
            self.audio_input_device = (!val.trim().is_empty()).then_some(val);
        }
        // VAD config lives in `core/vad/config.rs` with hardcoded defaults and
        // opt-in power-user env overrides (`CODESCRIBE_UTTERANCE_GAP_SEC`,
        // `CODESCRIBE_TAIL_SILENCE_SEC`).
        // No legacy SILENCE_* variables - single source of truth.

        // History (default: on to avoid data loss)
        if let Ok(val) = Self::config_runtime_env_var("HISTORY_ENABLED") {
            self.history_enabled = val.parse().unwrap_or(true);
        }

        // Quick Notes (default: off)
        if let Ok(val) = Self::config_runtime_env_var("QUICK_NOTES_ENABLED") {
            self.quick_notes_enabled = matches!(val.as_str(), "1" | "true" | "yes" | "on");
        }
        if let Ok(val) = Self::config_runtime_env_var("QUICK_NOTES_SAVE_ONLY") {
            self.quick_notes_save_only = matches!(val.as_str(), "1" | "true" | "yes" | "on");
        }

        // Backends - LLM
        // LLM_API_KEY for cloud providers
        if let Ok(val) = Self::config_runtime_env_var("LLM_API_KEY") {
            self.llm_api_key = Some(val);
        }
        if let Ok(val) = Self::config_runtime_env_var("LLM_ENDPOINT") {
            self.llm_endpoint = Some(val);
        }

        // Backends - STT
        if let Ok(val) = Self::config_runtime_env_var("STT_ENDPOINT") {
            self.stt_endpoint = Some(val);
        }
        if let Ok(val) = Self::config_runtime_env_var("CODESCRIBE_STT_INITIAL_PROMPT_ENABLED") {
            self.stt_initial_prompt_enabled =
                matches!(val.as_str(), "1" | "true" | "yes" | "on" | "enabled");
        }
        // STT_API_KEY for cloud STT
        if let Ok(val) = Self::config_runtime_env_var("STT_API_KEY") {
            self.stt_api_key = Some(val);
        }

        // Local STT (Pure Rust Whisper)
        if let Ok(val) = Self::config_runtime_env_var("USE_LOCAL_STT") {
            self.use_local_stt = matches!(val.as_str(), "1" | "true" | "yes" | "on");
        }
        if let Ok(val) = Self::config_runtime_env_var("LOCAL_MODEL") {
            self.local_model = val;
        }

        // Clipboard
        if let Ok(val) = Self::config_runtime_env_var("RESTORE_CLIPBOARD") {
            self.restore_clipboard = val.parse().unwrap_or(true);
        }
        if let Ok(val) = Self::config_runtime_env_var("RESTORE_CLIPBOARD_DELAY_MS")
            && let Ok(delay) = val.parse()
        {
            self.restore_clipboard_delay_ms = delay;
        }

        // System
        if let Ok(val) = Self::config_runtime_env_var("START_AT_LOGIN") {
            self.start_at_login = matches!(val.as_str(), "1" | "true" | "yes" | "on");
        }

        // Debugging (default: on to keep paired .wav with transcripts)
        if let Ok(val) = Self::config_runtime_env_var("DUMP_AUDIO_LOGS") {
            self.dump_audio_logs = matches!(val.as_str(), "1" | "true" | "yes" | "on");
        }
    }

    /// Set an env var from settings, with basic validation.
    /// Rejects empty strings and strings longer than 4096 chars.
    fn safe_set_env(key: &str, value: &str) {
        if value.is_empty() || value.len() > 4096 {
            warn!(
                "Ignoring invalid setting {key}: value length {}",
                value.len()
            );
            return;
        }
        Self::config_init_set_env(key, value);
    }

    /// Write to the process env during bootstrap only, and remember the key.
    /// After the window closes this is a no-op — the value has to reach the
    /// runtime through a settings snapshot instead.
    fn config_init_set_env(key: &str, value: impl AsRef<str>) {
        if !Self::can_seed_process_env() {
            return;
        }
        // SAFETY: a process-wide bootstrap lock confines config env mutation to
        // the one pre-runtime writer; later loads read settings snapshots instead.
        unsafe { std::env::set_var(key, value.as_ref()) };
        Self::remember_seeded_env_key(key);
    }

    /// Apply user settings from JSON (lower priority than .env).
    /// Only applies values that are Some AND not already overridden by env vars.
    fn apply_user_settings(&mut self, settings: &super::settings::UserSettings) {
        // Helper: only apply if the env var is NOT set
        macro_rules! apply_parsed_if_no_env {
            ($env_key:expr, $field:expr, $val:expr) => {
                if Self::config_runtime_env_var($env_key).is_err() {
                    if let Some(ref v) = $val {
                        if let Ok(parsed) = v.parse() {
                            $field = parsed;
                        }
                    }
                }
            };
        }

        // Language
        apply_parsed_if_no_env!(
            "WHISPER_LANGUAGE",
            self.whisper_language,
            settings.whisper_language
        );
        // Hotkeys
        if Self::config_runtime_env_var("HOLD_START_DELAY_MS").is_err()
            && let Some(v) = settings.hold_start_delay_ms
        {
            self.hold_start_delay_ms = v;
        }
        if Self::config_runtime_env_var("DOUBLE_TAP_INTERVAL_MS").is_err()
            && let Some(v) = settings.double_tap_interval_ms
        {
            self.double_tap_interval_ms = v;
        }
        if Self::config_runtime_env_var("TOGGLE_SILENCE_SEC").is_err()
            && let Some(v) = settings.toggle_silence_sec
        {
            self.toggle_silence_sec = v;
        }
        if Self::config_runtime_env_var("HOLD_EXCLUSIVE").is_err()
            && let Some(v) = settings.hold_exclusive
        {
            self.hold_exclusive = v;
        }
        if Self::config_runtime_env_var("HOLD_ARM_MODIFIER").is_err()
            && let Some(ref v) = settings.hold_arm_modifier
            && let Ok(arm) = v.parse()
        {
            self.hold_arm_modifier = arm;
        }
        // AI
        if Self::config_runtime_env_var("AI_FORMATTING_ENABLED").is_err()
            && let Some(v) = settings.ai_formatting_enabled
        {
            self.ai_formatting_enabled = v;
        }
        if Self::config_runtime_env_var("AUTO_PASTE_ENABLED").is_err()
            && let Some(v) = settings.auto_paste_enabled
        {
            self.auto_paste_enabled = v;
        }
        if Self::config_runtime_env_var("CODESCRIBE_TRANSCRIPT_TAGGING").is_err()
            && let Some(v) = settings.transcript_tagging_enabled
        {
            self.transcript_tagging_enabled = v;
        }
        if Self::config_runtime_env_var("CODESCRIBE_TRANSCRIPT_TAG_TEMPLATE").is_err()
            && let Some(ref v) = settings.transcript_tag_template
        {
            self.transcript_tag_template = v.clone();
        }
        if Self::config_runtime_env_var("FORMATTING_LEVEL").is_err()
            && let Some(ref v) = settings.formatting_level
        {
            match FormattingPolicy::parse(v) {
                Ok(policy) => Self::safe_set_env("FORMATTING_LEVEL", policy.as_str()),
                Err(error) => warn!("Ignoring invalid persisted formatting policy: {error}"),
            }
        }
        // Sound
        if Self::config_runtime_env_var("BEEP_ON_START").is_err()
            && let Some(v) = settings.beep_on_start
        {
            self.beep_on_start = v;
        }
        if Self::config_runtime_env_var("SHOW_DOCK_ICON").is_err()
            && let Some(v) = settings.show_dock_icon
        {
            self.show_dock_icon = v;
        }
        if Self::config_runtime_env_var("TRANSCRIPTION_OVERLAY_ENABLED").is_err()
            && let Some(v) = settings.transcription_overlay_enabled
        {
            self.transcription_overlay_enabled = v;
            Self::safe_set_env("TRANSCRIPTION_OVERLAY_ENABLED", if v { "1" } else { "0" });
        }
        if Self::config_runtime_env_var("HOLD_INDICATOR").is_err()
            && let Some(v) = settings.hold_indicator
        {
            self.hold_indicator = v;
        }
        if Self::config_runtime_env_var("HOLD_BADGE_SIZE").is_err()
            && let Some(v) = settings.hold_badge_size
        {
            self.hold_badge_size = v.min(u32::MAX as u64) as u32;
        }
        if Self::config_runtime_env_var("RESTORE_CLIPBOARD").is_err()
            && let Some(v) = settings.restore_clipboard
        {
            self.restore_clipboard = v;
        }
        if Self::config_runtime_env_var("RESTORE_CLIPBOARD_DELAY_MS").is_err()
            && let Some(v) = settings.restore_clipboard_delay_ms
        {
            self.restore_clipboard_delay_ms = v;
        }
        if Self::config_runtime_env_var("CODESCRIBE_DEFERRED_INSERT_SHORTCUT").is_err()
            && let Some(raw) = settings.deferred_insert_shortcut.as_deref()
            && let Ok(shortcut) = raw.parse::<DeferredInsertShortcut>()
        {
            self.deferred_insert_shortcut = shortcut;
        }
        if Self::config_runtime_env_var("TRAY_START_ASSISTIVE").is_err()
            && let Some(v) = settings.tray_start_assistive
        {
            // `tray_start_assistive` is a Config struct field; downstream reads it
            // directly (e.g. `tray_toggles`). Persistence lives in settings.json,
            // so no runtime env mutation is needed here - and `load_without_keychain`
            // runs on UI actions (tray/composer mic), where `set_var` would race
            // background threads.
            self.tray_start_assistive = v;
        }
        if Self::config_runtime_env_var("SOUND_VOLUME").is_err()
            && let Some(v) = settings.sound_volume
        {
            self.sound_volume = v;
        }
        // LLM endpoints (from JSON, lower priority than .env)
        if Self::config_runtime_env_var("LLM_ENDPOINT").is_err()
            && let Some(ref v) = settings.llm_endpoint
        {
            self.llm_endpoint = Some(v.clone());
        }
        if Self::config_runtime_env_var("LLM_MODEL").is_err()
            && let Some(ref v) = settings.llm_model
        {
            // LLM_MODEL is not in Config struct but read from env at runtime
            // Set env var so downstream code picks it up
            Self::safe_set_env("LLM_MODEL", v);
        }
        // Assistive LLM (not in Config struct, read from env at runtime)
        if Self::config_runtime_env_var("LLM_ASSISTIVE_ENDPOINT").is_err()
            && let Some(ref v) = settings.llm_assistive_endpoint
        {
            Self::safe_set_env("LLM_ASSISTIVE_ENDPOINT", v);
        }
        if Self::config_runtime_env_var("LLM_ASSISTIVE_MODEL").is_err()
            && let Some(ref v) = settings.llm_assistive_model
        {
            Self::safe_set_env("LLM_ASSISTIVE_MODEL", v);
        }
        if Self::config_runtime_env_var("LLM_ASSISTIVE_PROVIDER").is_err()
            && let Some(ref v) = settings.llm_assistive_provider
        {
            Self::safe_set_env("LLM_ASSISTIVE_PROVIDER", v);
        }
        // ── Promoted fields (previously .env only) ──

        // LLM formatting (not in Config struct, read from env at runtime)
        if Self::config_runtime_env_var("LLM_FORMATTING_ENDPOINT").is_err()
            && let Some(ref v) = settings.llm_formatting_endpoint
        {
            Self::safe_set_env("LLM_FORMATTING_ENDPOINT", v);
        }
        if Self::config_runtime_env_var("LLM_FORMATTING_MODEL").is_err()
            && let Some(ref v) = settings.llm_formatting_model
        {
            Self::safe_set_env("LLM_FORMATTING_MODEL", v);
        }

        // Local STT
        if Self::config_runtime_env_var("USE_LOCAL_STT").is_err()
            && let Some(v) = settings.use_local_stt
        {
            self.use_local_stt = v;
            Self::config_init_set_env("USE_LOCAL_STT", if v { "1" } else { "0" });
        }
        if Self::config_runtime_env_var("LOCAL_MODEL").is_err()
            && let Some(ref v) = settings.local_model
        {
            self.local_model = v.clone();
        }

        // STT endpoint
        if Self::config_runtime_env_var("STT_ENDPOINT").is_err()
            && let Some(ref v) = settings.stt_endpoint
        {
            self.stt_endpoint = Some(v.clone());
        }

        // Transcript send mode
        apply_parsed_if_no_env!(
            "TRANSCRIPT_SEND_MODE",
            self.transcript_send_mode,
            settings.transcript_send_mode
        );

        // Audio input device
        if Self::config_runtime_env_var("AUDIO_INPUT_DEVICE").is_err()
            && let Some(ref v) = settings.audio_input_device
        {
            self.audio_input_device = Some(v.clone());
        }

        // Sound name
        if Self::config_runtime_env_var("SOUND_NAME").is_err()
            && let Some(ref v) = settings.sound_name
        {
            self.sound_name = v.clone();
        }

        // History
        if Self::config_runtime_env_var("HISTORY_ENABLED").is_err()
            && let Some(v) = settings.history_enabled
        {
            self.history_enabled = v;
        }

        // Quick Notes
        if Self::config_runtime_env_var("QUICK_NOTES_ENABLED").is_err()
            && let Some(v) = settings.quick_notes_enabled
        {
            self.quick_notes_enabled = v;
        }
        if Self::config_runtime_env_var("QUICK_NOTES_SAVE_ONLY").is_err()
            && let Some(v) = settings.quick_notes_save_only
        {
            self.quick_notes_save_only = v;
        }

        // System
        if Self::config_runtime_env_var("START_AT_LOGIN").is_err()
            && let Some(v) = settings.start_at_login
        {
            self.start_at_login = v;
        }
        if Self::config_runtime_env_var("QUBE_DAEMON_AUTOSTART").is_err()
            && let Some(v) = settings.qube_daemon_autostart
        {
            Self::config_init_set_env("QUBE_DAEMON_AUTOSTART", if v { "1" } else { "0" });
        }
        if Self::config_runtime_env_var("CODESCRIBE_QUBE_DONOR").is_err()
            && let Some(ref v) = settings.qube_donor
        {
            Self::safe_set_env("CODESCRIBE_QUBE_DONOR", v);
        }
        if Self::config_runtime_env_var("AGENT_ENTER_SENDS").is_err()
            && let Some(v) = settings.agent_enter_sends
        {
            self.agent_enter_sends = v;
        }

        // ── Voice Lab survivors (runtime env vars, not Config struct fields) ──
        if Self::config_runtime_env_var("CODESCRIBE_BUFFER_DELAY_MS").is_err()
            && let Some(v) = settings.buffer_delay_ms
        {
            Self::config_init_set_env("CODESCRIBE_BUFFER_DELAY_MS", v.to_string());
        }
        if Self::config_runtime_env_var("CODESCRIBE_TYPING_CPS").is_err()
            && let Some(v) = settings.typing_cps
        {
            Self::config_init_set_env("CODESCRIBE_TYPING_CPS", v.to_string());
        }
        if Self::config_runtime_env_var("CODESCRIBE_EMIT_WORDS_MAX").is_err()
            && let Some(v) = settings.emit_words_max
        {
            Self::config_init_set_env("CODESCRIBE_EMIT_WORDS_MAX", v.to_string());
        }
        if Self::config_runtime_env_var("CODESCRIBE_BUFFERED_INTERIM_SEC").is_err()
            && let Some(v) = settings.buffered_interim_sec
        {
            Self::config_init_set_env("CODESCRIBE_BUFFERED_INTERIM_SEC", format!("{v:.1}"));
        }
        if Self::config_runtime_env_var("WHISPER_MODEL").is_err()
            && let Some(ref v) = settings.whisper_model
        {
            Self::safe_set_env("WHISPER_MODEL", v);
        }
        if Self::config_runtime_env_var("BACKEND_MAX_UPLOAD_MB").is_err()
            && let Some(v) = settings.backend_max_upload_mb
        {
            Self::config_init_set_env("BACKEND_MAX_UPLOAD_MB", v.to_string());
        }

        // ── STT engine / final-pass (STT_CONTRACT single brain) ──
        // Product rule: durable settings.json wins for live engine selection so a
        // leftover CODESCRIBE_STT_ENGINE=auto in .env cannot lottery Apple death.
        // CI/power users still override by writing settings or using setSttEngine.
        if let Some(ref v) = settings.stt_engine {
            Self::safe_set_env("CODESCRIBE_STT_ENGINE", v);
        }
        if let Some(ref v) = settings.final_pass_mode {
            Self::safe_set_env("FINAL_PASS_MODE", v);
            Self::safe_set_env("CODESCRIBE_FINAL_PASS_MODE", v);
        }
        // Promoted single-brain (2026-08-10): settings.json wins at boot, same
        // as CODESCRIBE_STT_ENGINE — a leftover .env line must not lottery the
        // Layered toggle back OFF.
        if let Some(ref v) = settings.layered_transcription {
            Self::safe_set_env("CODESCRIBE_LAYERED_TRANSCRIPTION", v);
        }
        if Self::config_runtime_env_var("CODESCRIBE_STT_INITIAL_PROMPT_ENABLED").is_err()
            && let Some(v) = settings.stt_initial_prompt_enabled
        {
            self.stt_initial_prompt_enabled = v;
            Self::config_init_set_env(
                "CODESCRIBE_STT_INITIAL_PROMPT_ENABLED",
                if v { "1" } else { "0" },
            );
        }

        // ── Agent workspace roots ──
        // Compatibility seed for older runtime readers. Agent tools no longer
        // consume this mutable process snapshot; they re-read settings.json via
        // `effective_agent_workspace_roots` on every call.
        if Self::config_runtime_env_var("AGENT_WORKSPACE_ROOTS").is_err()
            && let Some(ref roots) = settings.agent_workspace_roots
            && !roots.is_empty()
        {
            Self::safe_set_env("AGENT_WORKSPACE_ROOTS", &roots.join(":"));
        }
    }

    /// Save a configuration value, routing to the appropriate tier:
    /// - API keys → Keychain
    /// - Regular-user fields → settings.json
    /// - Everything else → .env
    ///
    /// This is a persistence write only. Process-env seeding is restricted to
    /// bootstrap loads; live readers must reload the config/settings snapshot.
    pub fn save_to_env(&self, key: &str, value: &str) -> anyhow::Result<()> {
        let _data_io = super::storage_reset::begin_app_data_io()?;
        let _persistence = config_persistence_guard();
        let normalized_formatting = (key == "FORMATTING_LEVEL")
            .then(|| FormattingPolicy::parse(value))
            .transpose()?
            .map(|policy| policy.as_str().to_string());
        let value = normalized_formatting.as_deref().unwrap_or(value);

        // API keys → Keychain
        if super::keychain::KEYCHAIN_ACCOUNTS.contains(&key) {
            super::keychain::save_key(key, value)?;
            return Ok(());
        }

        // Regular-user fields → settings.json
        let is_regular = super::settings::is_promoted_key(key);

        if is_regular {
            let mut settings = super::settings::UserSettings::load();
            if Self::apply_optional_override(&mut settings, key, value) {
                settings.save()?;
                return Ok(());
            }
            // Route to appropriate setter based on value type
            match key {
                "HOLD_START_DELAY_MS"
                | "DOUBLE_TAP_INTERVAL_MS"
                | "CODESCRIBE_BUFFER_DELAY_MS"
                | "CODESCRIBE_EMIT_WORDS_MAX"
                | "BACKEND_MAX_UPLOAD_MB"
                | "HOLD_BADGE_SIZE"
                | "RESTORE_CLIPBOARD_DELAY_MS" => {
                    if let Ok(v) = value.parse::<u64>() {
                        settings.set_u64(key, v);
                    }
                }
                "SOUND_VOLUME"
                | "TOGGLE_SILENCE_SEC"
                | "CODESCRIBE_TYPING_CPS"
                | "CODESCRIBE_BUFFERED_INTERIM_SEC" => {
                    if let Ok(v) = value.parse::<f32>() {
                        settings.set_f32(key, v);
                    }
                }
                "AI_FORMATTING_ENABLED"
                | "AUTO_PASTE_ENABLED"
                | "TRANSCRIPT_TAGGING_ENABLED"
                | "BEEP_ON_START"
                | "SHOW_DOCK_ICON"
                | "TRANSCRIPTION_OVERLAY_ENABLED"
                | "TRAY_START_ASSISTIVE"
                | "HOLD_EXCLUSIVE"
                | "USE_LOCAL_STT"
                | "HISTORY_ENABLED"
                | "QUICK_NOTES_ENABLED"
                | "QUICK_NOTES_SAVE_ONLY"
                | "START_AT_LOGIN"
                | "QUBE_DAEMON_AUTOSTART"
                | "AGENT_ENTER_SENDS"
                | "CODESCRIBE_STT_INITIAL_PROMPT_ENABLED"
                | "HOLD_INDICATOR"
                | "RESTORE_CLIPBOARD" => {
                    let bool_val = matches!(value, "1" | "true" | "yes" | "on");
                    settings.set_bool(key, bool_val);
                }
                "HOLD_ARM_MODIFIER" => {
                    settings.set_string(key, value);
                }
                _ => {
                    settings.set_string(key, value);
                }
            }
            // STT contract: settings write is product truth — pin process env +
            // .env so boot cannot re-lottery via a stale CODESCRIBE_STT_ENGINE.
            if matches!(
                key,
                "CODESCRIBE_STT_ENGINE"
                    | "FINAL_PASS_MODE"
                    | "CODESCRIBE_FINAL_PASS_MODE"
                    | "CODESCRIBE_LAYERED_TRANSCRIPTION"
            ) {
                Self::reconcile_stt_runtime_key(key, value);
            }
            return Ok(());
        }

        // Power-user fields → .env file (existing behavior)
        let env_path = Self::env_path();
        if let Some(parent) = env_path.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut env_vars = if env_path.exists() {
            Self::parse_env_file(&env_path)?
        } else {
            HashMap::new()
        };
        env_vars.insert(key.to_string(), value.to_string());
        Self::write_env_file(&env_path, &env_vars).inspect_err(|error| {
            // A power-user key that cannot persist is a dead UI control, and the
            // Swift callers swallow the error — this line is the only witness
            // (2026-08-10: an immutable .env killed the Pointer Indicator row
            // with zero log output).
            tracing::warn!(key, %error, "save_to_env: .env write failed; value NOT persisted");
        })?;
        Ok(())
    }

    /// Save multiple configuration values in a single batch.
    ///
    /// This reduces repeated settings.json writes and .env rewrites, and
    /// minimizes redundant work when updating several fields at once.
    pub fn save_to_env_many(&self, entries: &[(&str, &str)]) -> anyhow::Result<()> {
        if entries.is_empty() {
            return Ok(());
        }
        let _data_io = super::storage_reset::begin_app_data_io()?;
        let _persistence = config_persistence_guard();

        let mut settings: Option<super::settings::UserSettings> = None;
        let mut env_vars: Option<HashMap<String, String>> = None;
        let mut env_path: Option<PathBuf> = None;

        for (key, value) in entries {
            if *key == "FORMATTING_LEVEL" {
                FormattingPolicy::parse(value)?;
            }
        }

        for (key, value) in entries {
            // API keys → Keychain
            if super::keychain::KEYCHAIN_ACCOUNTS.contains(key) {
                super::keychain::save_key(key, value)?;
                continue;
            }

            // Regular-user fields → settings.json
            let is_regular = super::settings::is_promoted_key(key);

            if is_regular {
                let settings_ref = settings.get_or_insert_with(super::settings::UserSettings::load);
                if Self::apply_optional_override(settings_ref, key, value) {
                    continue;
                }
                match *key {
                    // ── Strings ──
                    "WHISPER_LANGUAGE" => {
                        settings_ref.whisper_language = Some((*value).to_string())
                    }
                    "FORMATTING_LEVEL" => {
                        settings_ref.formatting_level =
                            Some(FormattingPolicy::parse(value)?.as_str().to_string())
                    }
                    "LOCAL_MODEL" => settings_ref.local_model = Some((*value).to_string()),
                    "STT_ENDPOINT" => settings_ref.stt_endpoint = Some((*value).to_string()),
                    "TRANSCRIPT_SEND_MODE" => {
                        settings_ref.transcript_send_mode = Some((*value).to_string())
                    }
                    "TRANSCRIPT_TAG_TEMPLATE" => {
                        settings_ref.transcript_tag_template = Some((*value).to_string())
                    }
                    "AUDIO_INPUT_DEVICE" => {
                        settings_ref.audio_input_device = Some((*value).to_string())
                    }
                    "SOUND_NAME" => settings_ref.sound_name = Some((*value).to_string()),
                    "WHISPER_MODEL" => settings_ref.whisper_model = Some((*value).to_string()),
                    "AGENT_WORKSPACE_ROOTS" => {
                        let roots = parse_agent_workspace_roots(value);
                        settings_ref.agent_workspace_roots = (!roots.is_empty()).then_some(roots);
                    }
                    "HOLD_ARM_MODIFIER" => {
                        if let Ok(arm) = value.parse::<crate::config::HoldArmModifier>() {
                            settings_ref.hold_arm_modifier = Some(arm.as_str().to_string());
                        }
                    }
                    "CODESCRIBE_STT_ENGINE" => {
                        settings_ref.stt_engine = Some((*value).to_string());
                        Self::reconcile_stt_runtime_key(key, value);
                    }
                    "FINAL_PASS_MODE" | "CODESCRIBE_FINAL_PASS_MODE" => {
                        let normalized = value.trim().to_ascii_lowercase();
                        if matches!(normalized.as_str(), "always" | "smart" | "off") {
                            settings_ref.final_pass_mode = Some(normalized.clone());
                            Self::reconcile_stt_runtime_key(key, &normalized);
                        }
                    }
                    "CODESCRIBE_LAYERED_TRANSCRIPTION" => {
                        settings_ref.layered_transcription = Some((*value).to_string());
                        Self::reconcile_stt_runtime_key(key, value);
                    }
                    // C2: same validated writes as the single-key set_string
                    // path — a batch write must not bypass mode/consent/URL
                    // validation or silently drop these keys.
                    "CODESCRIBE_ASR_MODE"
                    | "CODESCRIBE_CLOUD_CONSENT"
                    | "CODESCRIBE_ASR_GATEWAY_URL" => {
                        settings_ref.set_string(key, value);
                    }
                    // ── u64 ──
                    "HOLD_START_DELAY_MS" => {
                        if let Ok(v) = value.parse::<u64>() {
                            settings_ref.hold_start_delay_ms = Some(v);
                        }
                    }
                    "DOUBLE_TAP_INTERVAL_MS" => {
                        if let Ok(v) = value.parse::<u64>() {
                            settings_ref.double_tap_interval_ms = Some(v);
                        }
                    }
                    "CODESCRIBE_BUFFER_DELAY_MS" => {
                        if let Ok(v) = value.parse::<u64>() {
                            settings_ref.buffer_delay_ms = Some(v);
                        }
                    }
                    "CODESCRIBE_EMIT_WORDS_MAX" => {
                        if let Ok(v) = value.parse::<u64>() {
                            settings_ref.emit_words_max = Some(v);
                        }
                    }
                    "BACKEND_MAX_UPLOAD_MB" => {
                        if let Ok(v) = value.parse::<u64>() {
                            settings_ref.backend_max_upload_mb = Some(v);
                        }
                    }
                    "HOLD_BADGE_SIZE" => {
                        if let Ok(v) = value.parse::<u64>() {
                            settings_ref.hold_badge_size = Some(v);
                        }
                    }
                    "RESTORE_CLIPBOARD_DELAY_MS" => {
                        if let Ok(v) = value.parse::<u64>() {
                            settings_ref.restore_clipboard_delay_ms = Some(v);
                        }
                    }
                    "CODESCRIBE_DEFERRED_INSERT_SHORTCUT" => {
                        if let Ok(shortcut) = value.parse::<DeferredInsertShortcut>() {
                            settings_ref.deferred_insert_shortcut =
                                Some(shortcut.wire_id().to_string());
                        }
                    }
                    // ── f32 ──
                    "TOGGLE_SILENCE_SEC" => {
                        if let Ok(v) = value.parse::<f32>() {
                            settings_ref.toggle_silence_sec = Some(v);
                        }
                    }
                    "CODESCRIBE_TYPING_CPS" => {
                        if let Ok(v) = value.parse::<f32>() {
                            settings_ref.typing_cps = Some(v);
                        }
                    }
                    "CODESCRIBE_BUFFERED_INTERIM_SEC" => {
                        if let Ok(v) = value.parse::<f32>() {
                            settings_ref.buffered_interim_sec = Some(v);
                        }
                    }
                    "SOUND_VOLUME" => {
                        if let Ok(v) = value.parse::<f32>() {
                            settings_ref.sound_volume = Some(v);
                        }
                    }
                    // ── Bools ──
                    "AI_FORMATTING_ENABLED"
                    | "AUTO_PASTE_ENABLED"
                    | "TRANSCRIPT_TAGGING_ENABLED"
                    | "BEEP_ON_START"
                    | "SHOW_DOCK_ICON"
                    | "TRANSCRIPTION_OVERLAY_ENABLED"
                    | "TRAY_START_ASSISTIVE"
                    | "HOLD_EXCLUSIVE"
                    | "USE_LOCAL_STT"
                    | "HISTORY_ENABLED"
                    | "QUICK_NOTES_ENABLED"
                    | "QUICK_NOTES_SAVE_ONLY"
                    | "START_AT_LOGIN"
                    | "QUBE_DAEMON_AUTOSTART"
                    | "AGENT_ENTER_SENDS"
                    | "CODESCRIBE_STT_INITIAL_PROMPT_ENABLED"
                    | "HOLD_INDICATOR"
                    | "RESTORE_CLIPBOARD" => {
                        let bv = matches!(*value, "1" | "true" | "yes" | "on");
                        match *key {
                            "AI_FORMATTING_ENABLED" => {
                                settings_ref.ai_formatting_enabled = Some(bv)
                            }
                            "AUTO_PASTE_ENABLED" => settings_ref.auto_paste_enabled = Some(bv),
                            "BEEP_ON_START" => settings_ref.beep_on_start = Some(bv),
                            "SHOW_DOCK_ICON" => settings_ref.show_dock_icon = Some(bv),
                            "TRANSCRIPTION_OVERLAY_ENABLED" => {
                                settings_ref.transcription_overlay_enabled = Some(bv)
                            }
                            "TRAY_START_ASSISTIVE" => settings_ref.tray_start_assistive = Some(bv),
                            "HOLD_EXCLUSIVE" => settings_ref.hold_exclusive = Some(bv),
                            "USE_LOCAL_STT" => settings_ref.use_local_stt = Some(bv),
                            "HISTORY_ENABLED" => settings_ref.history_enabled = Some(bv),
                            "QUICK_NOTES_ENABLED" => settings_ref.quick_notes_enabled = Some(bv),
                            "QUICK_NOTES_SAVE_ONLY" => {
                                settings_ref.quick_notes_save_only = Some(bv)
                            }
                            "START_AT_LOGIN" => settings_ref.start_at_login = Some(bv),
                            "QUBE_DAEMON_AUTOSTART" => {
                                settings_ref.qube_daemon_autostart = Some(bv)
                            }
                            "AGENT_ENTER_SENDS" => settings_ref.agent_enter_sends = Some(bv),
                            "CODESCRIBE_STT_INITIAL_PROMPT_ENABLED" => {
                                settings_ref.stt_initial_prompt_enabled = Some(bv)
                            }
                            "HOLD_INDICATOR" => settings_ref.hold_indicator = Some(bv),
                            "RESTORE_CLIPBOARD" => settings_ref.restore_clipboard = Some(bv),
                            _ => {}
                        }
                    }
                    _ => {}
                }
                continue;
            }

            // Power-user fields → .env file
            let path = env_path.get_or_insert_with(Self::env_path).clone();
            let vars_ref = env_vars.get_or_insert_with(|| {
                if path.exists() {
                    Self::parse_env_file(&path).unwrap_or_default()
                } else {
                    HashMap::new()
                }
            });
            vars_ref.insert((*key).to_string(), (*value).to_string());
        }

        if let Some(settings) = settings
            && let Err(e) = settings.save()
        {
            warn!("Failed to save settings batch: {e}");
        }
        if let (Some(path), Some(vars)) = (env_path, env_vars) {
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent)?;
            }
            Self::write_env_file(&path, &vars).inspect_err(|error| {
                // Same witness as the single-key path: Swift callers swallow
                // the error, so an unwritable .env must at least leave a trace.
                tracing::warn!(%error, "save_to_env_many: .env write failed; batch NOT persisted");
            })?;
        }

        Ok(())
    }

    /// Handle the LLM override keys, where blank means *clear* rather than
    /// "store an empty string". Removing the field lets the resolver fall back
    /// to the default; storing `""` would pin the lane to an unusable endpoint.
    ///
    /// Returns `false` for keys it does not own, so the caller continues with
    /// its normal typed routing.
    fn apply_optional_override(
        settings: &mut super::settings::UserSettings,
        key: &str,
        value: &str,
    ) -> bool {
        let normalized = (!value.trim().is_empty()).then(|| value.to_string());
        match key {
            "LLM_ENDPOINT" => settings.llm_endpoint = normalized,
            "LLM_MODEL" => settings.llm_model = normalized,
            "LLM_ASSISTIVE_ENDPOINT" => settings.llm_assistive_endpoint = normalized,
            "LLM_ASSISTIVE_MODEL" => settings.llm_assistive_model = normalized,
            "LLM_ASSISTIVE_PROVIDER" => settings.llm_assistive_provider = normalized,
            "LLM_FORMATTING_ENDPOINT" => settings.llm_formatting_endpoint = normalized,
            "LLM_FORMATTING_MODEL" => settings.llm_formatting_model = normalized,
            _ => return false,
        }
        true
    }

    /// Pin STT-related process env + ~/.codescribe/.env to the settings value.
    ///
    /// Product rule (STT_CONTRACT / W2-A): Settings UI is the single brain for
    /// live engine selection. A leftover `CODESCRIBE_STT_ENGINE=auto` in `.env`
    /// must not win over an explicit `speech.engine.stt_engine` write.
    pub fn reconcile_stt_runtime_key(key: &str, value: &str) {
        let value = value.trim();
        if value.is_empty() {
            return;
        }
        // Live process truth used by core/stt::selected_engine() on every call.
        // Must bypass the bootstrap lock: UI writes happen after Config::load
        // marked env seeding done. Intentional single-writer path (settings UI).
        // SAFETY: same keys as boot seed; only called from save_to_env* on STT knobs.
        unsafe {
            std::env::set_var(key, value);
            if key == "FINAL_PASS_MODE" {
                std::env::set_var("CODESCRIBE_FINAL_PASS_MODE", value);
            } else if key == "CODESCRIBE_FINAL_PASS_MODE" {
                std::env::set_var("FINAL_PASS_MODE", value);
            }
        }

        let env_path = Self::env_path();
        let mut vars = if env_path.exists() {
            Self::parse_env_file(&env_path).unwrap_or_default()
        } else {
            HashMap::new()
        };
        let before = vars.get(key).cloned();
        vars.insert(key.to_string(), value.to_string());
        if key == "FINAL_PASS_MODE" {
            vars.insert("CODESCRIBE_FINAL_PASS_MODE".to_string(), value.to_string());
        } else if key == "CODESCRIBE_FINAL_PASS_MODE" {
            vars.insert("FINAL_PASS_MODE".to_string(), value.to_string());
        }
        if before.as_deref() != Some(value) {
            if let Some(parent) = env_path.parent() {
                let _ = fs::create_dir_all(parent);
            }
            if let Err(e) = Self::write_env_file(&env_path, &vars) {
                warn!("Failed to reconcile STT key {key} in .env: {e}");
            } else {
                info!("STT runtime reconciled {key}={value} (settings + process env + .env)");
            }
        }
    }

    /// Parse .env file into HashMap.
    pub fn parse_env_file(path: &Path) -> anyhow::Result<HashMap<String, String>> {
        // `path` is always internally derived from `Config::env_path()`
        // (config_dir()/.env, or the `CODESCRIBE_ENV_PATH` override used by tests
        // and power users) — never raw request or end-user input. No external
        // path-traversal source reaches this read.
        let path = canonical_existing_file(path)?;
        let contents = fs::read_to_string(&path)?;
        let mut vars = HashMap::new();

        for line in contents.lines() {
            let line = line.trim();

            // Skip empty lines and comments
            if line.is_empty() || line.starts_with('#') {
                continue;
            }

            // Parse KEY=VALUE
            if let Some((key, value)) = line.split_once('=') {
                let key = key.trim().to_string();
                let value = value
                    .trim()
                    .trim_matches('"')
                    .trim_matches('\'')
                    .to_string();
                vars.insert(key, value);
            }
        }

        Ok(vars)
    }

    /// Write HashMap to .env file, preserving existing structure and comments.
    ///
    /// If the file exists, updates values in-place. If a key doesn't exist, appends it.
    /// Comments and formatting are preserved.
    ///
    /// Uses safe_path utilities to enforce that writes stay within config_dir().
    pub fn write_env_file(
        path: &std::path::Path,
        vars: &HashMap<String, String>,
    ) -> anyhow::Result<()> {
        use crate::safe_path::{safe_read_to_string_bounded, safe_write_bounded};

        let _data_io = super::storage_reset::begin_app_data_io()?;

        // Use path's parent as root to support CODESCRIBE_ENV_PATH override (tests)
        let root = path
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(Self::config_dir);
        let mut remaining_vars = vars.clone();
        let mut output_lines: Vec<String> = Vec::new();

        // If file exists, preserve its structure
        if path.exists() {
            let contents = safe_read_to_string_bounded(path, &root)?;
            for line in contents.lines() {
                let trimmed = line.trim();

                // Preserve comments and empty lines as-is
                if trimmed.is_empty() || trimmed.starts_with('#') {
                    output_lines.push(line.to_string());
                    continue;
                }

                // Check if this is a KEY=VALUE line we need to update
                if let Some((key, _)) = trimmed.split_once('=') {
                    let key = key.trim();
                    if let Some(new_value) = remaining_vars.remove(key) {
                        // Update this key with new value
                        output_lines.push(format!("{}={}", key, new_value));
                    } else {
                        // Keep original line (key not in our update set)
                        output_lines.push(line.to_string());
                    }
                } else {
                    // Preserve any other lines (malformed but user-written)
                    output_lines.push(line.to_string());
                }
            }
        }

        // Append any new keys that weren't in the original file
        if !remaining_vars.is_empty() {
            if !output_lines.is_empty()
                && !output_lines.last().map(|l| l.is_empty()).unwrap_or(true)
            {
                output_lines.push(String::new()); // blank line before new section
            }
            output_lines.push("# Added by Codescribe".to_string());

            let mut keys: Vec<_> = remaining_vars.keys().collect();
            keys.sort();
            for key in keys {
                if let Some(value) = remaining_vars.get(key) {
                    output_lines.push(format!("{}={}", key, value));
                }
            }
        }

        // Write back using safe bounded write
        let output = output_lines.join("\n");
        // Add trailing newline if content exists
        let output = if output.is_empty() {
            output
        } else {
            format!("{}\n", output)
        };
        safe_write_bounded(path, &root, &output)?;

        Ok(())
    }

    /// Remove a narrow set of persisted `.env` rows while preserving every
    /// unrelated user-written row, comment and ordering. Used by scoped reset
    /// flows; callers must name their owned keys explicitly.
    pub fn remove_env_keys(keys: &[&str]) -> anyhow::Result<()> {
        use crate::safe_path::{safe_read_to_string_bounded, safe_write_bounded};

        let _data_io = super::storage_reset::begin_app_data_io()?;
        let _persistence = config_persistence_guard();
        let path = Self::env_path();
        if !path.exists() {
            return Ok(());
        }
        let path = path.canonicalize()?;
        let root = path
            .parent()
            .map(|parent| parent.to_path_buf())
            .unwrap_or_else(Self::config_dir);
        let contents = safe_read_to_string_bounded(&path, &root)?;
        let owned: HashSet<&str> = keys.iter().copied().collect();
        let output = contents
            .lines()
            .filter(|line| {
                let key = line.trim().split_once('=').map(|(key, _)| key.trim());
                !key.is_some_and(|key| owned.contains(key))
            })
            .collect::<Vec<_>>()
            .join("\n");
        let output = if output.is_empty() {
            String::new()
        } else {
            format!("{output}\n")
        };
        safe_write_bounded(&path, &root, &output)
    }

    /// Migrate legacy keys inside .env to the current contract.
    fn migrate_env_legacy_keys() {
        let env_path = Self::env_path();
        if !env_path.exists() {
            return;
        }

        let mut vars = match Self::parse_env_file(&env_path) {
            Ok(vars) => vars,
            Err(e) => {
                warn!("Failed to parse .env for migration: {}", e);
                return;
            }
        };

        let mut changed = false;

        let put_if_missing = |key: &str, value: String, vars: &mut HashMap<String, String>| {
            if !vars.contains_key(key) {
                vars.insert(key.to_string(), value);
                true
            } else {
                false
            }
        };

        // Legacy STT endpoint → canonical STT_ENDPOINT
        if let Some(val) = vars.remove("WHISPER_SERVER_URL") {
            changed = true;
            if put_if_missing("STT_ENDPOINT", val, &mut vars) {
                changed = true;
            }
        }

        // Legacy LLM endpoint → canonical LLM_ENDPOINT
        if let Some(val) = vars.remove("LLM_SERVER_URL") {
            changed = true;
            if put_if_missing("LLM_ENDPOINT", val, &mut vars) {
                changed = true;
            }
        }

        // Legacy LLM host → canonical LLM_ENDPOINT (/api/chat)
        let legacy_host = vars
            .remove("LLM_HOST")
            .or_else(|| vars.remove("OLLAMA_HOST"));
        if let Some(host) = legacy_host {
            changed = true;
            if !vars.contains_key("LLM_ENDPOINT") {
                let trimmed = host.trim_end_matches('/');
                let endpoint = if trimmed.ends_with("/api/chat") {
                    trimmed.to_string()
                } else {
                    format!("{}/api/chat", trimmed)
                };
                vars.insert("LLM_ENDPOINT".to_string(), endpoint);
                changed = true;
            }
        }

        // Legacy model name → canonical LLM_MODEL (shared fallback)
        if let Some(model) = vars.remove("OLLAMA_MODEL") {
            changed = true;
            if put_if_missing("LLM_MODEL", model, &mut vars) {
                changed = true;
            }
        }

        // Remove deprecated provider flag
        if vars.remove("AI_PROVIDER").is_some() {
            changed = true;
        }

        if changed {
            if let Err(e) = Self::write_env_file(&env_path, &vars) {
                warn!("Failed to write migrated .env: {}", e);
            } else {
                info!("Migrated legacy keys inside .env to the current contract");
            }
        }
    }

    /// Get the configuration directory path (`$HOME/.codescribe`).
    ///
    /// Can be overridden with `CODESCRIBE_DATA_DIR` environment variable.
    pub fn config_dir() -> PathBuf {
        // Helper to canonicalize if path exists (resolves macOS /var → /private/var)
        let maybe_canonicalize = |p: PathBuf| -> PathBuf { p.canonicalize().unwrap_or(p) };

        // Check for environment variable overrides
        if let Ok(custom) = std::env::var("CODESCRIBE_DATA_DIR") {
            return maybe_canonicalize(PathBuf::from(shellexpand::tilde(&custom).into_owned()));
        }

        // Default to $HOME/.codescribe (lowercase - Unix convention)
        BaseDirs::new()
            .map(|dirs| dirs.home_dir().join(".codescribe"))
            .unwrap_or_else(|| PathBuf::from(".codescribe"))
    }

    /// Get the full path to the .env file.
    pub fn env_path() -> PathBuf {
        if let Ok(custom) = std::env::var("CODESCRIBE_ENV_PATH") {
            return PathBuf::from(shellexpand::tilde(&custom).into_owned());
        }

        Self::config_dir().join(".env")
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// Resolve a path and require it to be a regular file. Canonicalizing first
/// collapses symlinks and `..`, so a directory or dangling link is rejected
/// before anything reads through it.
fn canonical_existing_file(path: &Path) -> anyhow::Result<PathBuf> {
    let path = path.canonicalize()?;
    if !path.is_file() {
        anyhow::bail!("Config env path is not a file: {}", path.display());
    }
    Ok(path)
}

/// Guards for the three-tier precedence this module implements: explicit
/// process env, then `settings.json`, then optional `.env`.
///
/// The recurring failure being tested is *shadowing* — a value written in one
/// tier being silently masked by another, or a persistence write leaking into
/// the process env and pinning a stale value for the rest of the session. Cases
/// therefore assert on the real files and on `std::env` directly, not just on
/// the returned `Config`.
#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::UserSettings;
    use serial_test::serial;
    use std::fs;
    use tempfile::TempDir;

    /// Set a var for the current test case.
    fn set_env_for_test<V: AsRef<std::ffi::OsStr>>(key: &str, value: V) {
        // SAFETY: these tests are marked `serial` and do not start background workers,
        // so process-env mutation stays confined to the active test case.
        unsafe { std::env::set_var(key, value) };
    }

    /// Unset a var for the current test case.
    fn remove_env_for_test(key: &str) {
        // SAFETY: same invariant as `set_env_for_test` above.
        unsafe { std::env::remove_var(key) };
    }

    /// Put a var back the way it was, including the "was absent" case — the one
    /// most easily lost when restoring by hand.
    fn restore_env_for_test(key: &str, previous: Option<String>) {
        if let Some(value) = previous {
            set_env_for_test(key, value);
        } else {
            remove_env_for_test(key);
        }
    }

    /// RAII guard that clears one env var and restores it on drop. Tests here
    /// must start from "the operator has not set this", because a variable
    /// inherited from the developer's own shell would mask the tier under test.
    struct TestEnvGuard {
        key: &'static str,
        previous: Option<String>,
    }

    impl TestEnvGuard {
        /// Clear `key`, remembering whatever was there before.
        fn unset(key: &'static str) -> Self {
            let previous = std::env::var(key).ok();
            remove_env_for_test(key);
            Self { key, previous }
        }
    }

    impl Drop for TestEnvGuard {
        /// Restore the captured env value (or absence) when the guard leaves scope.
        fn drop(&mut self) {
            restore_env_for_test(self.key, self.previous.take());
        }
    }

    /// Point config at a fresh temp dir and clear the vars that would otherwise
    /// preempt what is being tested. The returned guard owns the directory both
    /// `settings.json` and `.env` live in.
    fn setup_isolated_data_dir() -> TempDir {
        let tmp = TempDir::new().expect("tempdir");
        set_env_for_test("CODESCRIBE_DATA_DIR", tmp.path());
        remove_env_for_test("CODESCRIBE_ENV_PATH");
        remove_env_for_test("USE_LOCAL_STT");
        remove_env_for_test("CODESCRIBE_STT_INITIAL_PROMPT_ENABLED");
        tmp
    }

    /// Distinct UI writes are one read-modify-write transaction each. Start two
    /// callers together and prove the later atomic rename cannot erase the
    /// field persisted by the other caller.
    #[test]
    #[serial]
    fn concurrent_config_updates_preserve_both_distinct_fields() {
        const CHILD_FLAG: &str = "CODESCRIBE_TEST_CONFIG_RMW_CHILD";
        const CHILD_WITNESS: &str = "CODESCRIBE_TEST_CONFIG_RMW_WITNESS";
        if std::env::var_os(CHILD_FLAG).is_none() {
            let witness_dir = TempDir::new().expect("config RMW witness dir");
            let witness = witness_dir.path().join("passed");
            let status = std::process::Command::new(
                std::env::current_exe().expect("current core test executable"),
            )
            .args([
                "--exact",
                "config::loader::tests::concurrent_config_updates_preserve_both_distinct_fields",
                "--nocapture",
            ])
            .env(CHILD_FLAG, "1")
            .env(CHILD_WITNESS, &witness)
            .status()
            .expect("spawn isolated config RMW regression");
            assert!(status.success(), "isolated config RMW regression failed");
            assert_eq!(
                fs::read(witness).expect("child executed exact config RMW test"),
                b"config-rmw-pass"
            );
            return;
        }

        let _tmp = setup_isolated_data_dir();
        let _auto_paste = TestEnvGuard::unset("AUTO_PASTE_ENABLED");
        let _dock = TestEnvGuard::unset("SHOW_DOCK_ICON");
        let start = std::sync::Arc::new(std::sync::Barrier::new(3));
        let first_start = start.clone();
        let first = std::thread::spawn(move || {
            first_start.wait();
            Config::default()
                .save_to_env("AUTO_PASTE_ENABLED", "0")
                .expect("persist auto paste")
        });
        let second_start = start.clone();
        let second = std::thread::spawn(move || {
            second_start.wait();
            Config::default()
                .save_to_env("SHOW_DOCK_ICON", "0")
                .expect("persist dock icon")
        });
        start.wait();
        first.join().expect("first config writer joins");
        second.join().expect("second config writer joins");

        let persisted = UserSettings::load();
        assert_eq!(persisted.auto_paste_enabled, Some(false));
        assert_eq!(persisted.show_dock_icon, Some(false));
        fs::write(
            std::env::var_os(CHILD_WITNESS).expect("config RMW child witness path"),
            b"config-rmw-pass",
        )
        .expect("write config RMW child witness");
    }

    /// Every LLM write key as `(key, sample value, JSON pointer)`. A `None`
    /// pointer marks a key with no durable `settings.json` home — it is still
    /// exercised, to prove writing it does not invent one.
    fn llm_write_key_cases() -> &'static [(&'static str, &'static str, Option<&'static str>)] {
        &[
            (
                "LLM_ENDPOINT",
                "https://main.example/v1",
                Some("/speech/llm_endpoint"),
            ),
            ("LLM_MODEL", "gpt-main-test", Some("/speech/llm_model")),
            ("LLM_PROVIDER", "openai-responses", None),
            (
                "LLM_ASSISTIVE_ENDPOINT",
                "https://assistive.example/v1",
                Some("/speech/assistive/llm_endpoint"),
            ),
            (
                "LLM_ASSISTIVE_MODEL",
                "gpt-assistive-test",
                Some("/speech/assistive/llm_model"),
            ),
            (
                "LLM_ASSISTIVE_PROVIDER",
                "anthropic-messages",
                Some("/speech/assistive/llm_provider"),
            ),
            (
                "LLM_FORMATTING_ENDPOINT",
                "https://formatting.example/v1",
                Some("/speech/formatting/llm_endpoint"),
            ),
            (
                "LLM_FORMATTING_MODEL",
                "gpt-formatting-test",
                Some("/speech/formatting/llm_model"),
            ),
            ("LLM_FORMATTING_PROVIDER", "openai-responses", None),
        ]
    }

    /// Write one key via single or batch path, then reload `UserSettings` for compare.
    fn save_snapshot(key: &str, value: &str, batch: bool) -> UserSettings {
        let _tmp = setup_isolated_data_dir();
        let config = Config::default();
        if batch {
            config
                .save_to_env_many(&[(key, value)])
                .expect("save batch");
        } else {
            config.save_to_env(key, value).expect("save single");
        }
        UserSettings::load()
    }

    /// Assert a promoted LLM optional field is absent in loaded settings.
    fn assert_optional_override_absent(settings: &UserSettings, key: &str) {
        let actual = match key {
            "LLM_ENDPOINT" => settings.llm_endpoint.as_deref(),
            "LLM_MODEL" => settings.llm_model.as_deref(),
            "LLM_ASSISTIVE_ENDPOINT" => settings.llm_assistive_endpoint.as_deref(),
            "LLM_ASSISTIVE_MODEL" => settings.llm_assistive_model.as_deref(),
            "LLM_ASSISTIVE_PROVIDER" => settings.llm_assistive_provider.as_deref(),
            "LLM_FORMATTING_ENDPOINT" => settings.llm_formatting_endpoint.as_deref(),
            "LLM_FORMATTING_MODEL" => settings.llm_formatting_model.as_deref(),
            _ => return,
        };
        assert_eq!(actual, None, "{key} must be unset, got {actual:?}");
    }

    /// Promoted save must land in settings.json and must not set process env.
    #[test]
    #[serial]
    fn save_to_env_persists_promoted_setting_without_process_env_mutation() {
        let _tmp = setup_isolated_data_dir();
        let _model = TestEnvGuard::unset("LLM_MODEL");

        Config::default()
            .save_to_env("LLM_MODEL", "runtime-model")
            .expect("save setting");

        assert!(std::env::var("LLM_MODEL").is_err());
        assert_eq!(
            UserSettings::load().llm_model.as_deref(),
            Some("runtime-model")
        );
    }

    /// HOLD_ARM_MODIFIER string values persist and resolve to the enum on load.
    #[test]
    #[serial]
    fn hold_arm_modifier_roundtrips_through_persistence_and_fresh_load() {
        let _tmp = setup_isolated_data_dir();
        let _modifier = TestEnvGuard::unset("HOLD_ARM_MODIFIER");
        let config = Config::default();

        for (stored, expected) in [
            ("cmd", crate::config::HoldArmModifier::Cmd),
            ("shift", crate::config::HoldArmModifier::Shift),
        ] {
            config
                .save_to_env("HOLD_ARM_MODIFIER", stored)
                .expect("persist arm modifier");
            assert_eq!(
                UserSettings::load().hold_arm_modifier.as_deref(),
                Some(stored)
            );
            assert_eq!(Config::load_without_keychain().hold_arm_modifier, expected);
        }
    }

    /// Badge/indicator keys are promoted (2026-08-11): tray writes land in
    /// settings.json — never `.env`, whose immutability killed the Pointer
    /// Indicator row — and reload live without process-env shadowing.
    #[test]
    #[serial]
    fn hold_indicator_ui_writes_are_promoted_to_settings_json() {
        let _tmp = setup_isolated_data_dir();
        let _indicator = TestEnvGuard::unset("HOLD_INDICATOR");
        let _size = TestEnvGuard::unset("HOLD_BADGE_SIZE");
        let config = Config::default();

        config
            .save_to_env("HOLD_BADGE_SIZE", "8")
            .expect("save stored badge size");
        config
            .save_to_env("HOLD_INDICATOR", "0")
            .expect("disable indicator");
        let stored = UserSettings::load();
        assert_eq!(stored.hold_indicator, Some(false));
        assert_eq!(stored.hold_badge_size, Some(8));
        let disabled_config = Config::load_without_keychain();
        assert!(!disabled_config.hold_indicator);
        assert_eq!(disabled_config.hold_badge_size, 8);

        for size in [4u64, 8, 12] {
            let size_str = size.to_string();
            config
                .save_to_env_many(&[("HOLD_INDICATOR", "1"), ("HOLD_BADGE_SIZE", &size_str)])
                .expect("save enabled badge size");
            let stored = UserSettings::load();
            assert_eq!(stored.hold_indicator, Some(true));
            assert_eq!(stored.hold_badge_size, Some(size));
            let live = Config::load_without_keychain();
            assert!(live.hold_indicator);
            assert_eq!(u64::from(live.hold_badge_size), size);
        }

        // The promoted keys must leave `.env` alone entirely.
        let env_path = Config::env_path();
        if env_path.exists() {
            let env = Config::parse_env_file(&env_path).expect("parse optional env");
            assert!(!env.contains_key("HOLD_INDICATOR"));
            assert!(!env.contains_key("HOLD_BADGE_SIZE"));
        }
    }

    /// Deferred-insert + clipboard-restore keys are promoted: valid writes land
    /// in settings.json and reload live; invalid chords are rejected without
    /// touching disk.
    #[test]
    #[serial]
    fn deferred_insert_and_restore_clipboard_writes_are_promoted() {
        let _tmp = setup_isolated_data_dir();
        let _shortcut = TestEnvGuard::unset("CODESCRIBE_DEFERRED_INSERT_SHORTCUT");
        let _restore = TestEnvGuard::unset("RESTORE_CLIPBOARD");
        let _delay = TestEnvGuard::unset("RESTORE_CLIPBOARD_DELAY_MS");
        let config = Config::default();

        config
            .save_to_env("CODESCRIBE_DEFERRED_INSERT_SHORTCUT", "cmd_alt_v")
            .expect("save shortcut alias");
        assert_eq!(
            UserSettings::load().deferred_insert_shortcut.as_deref(),
            Some("command_option_v"),
            "aliases must persist as the canonical wire id"
        );
        assert_eq!(
            Config::load_without_keychain().deferred_insert_shortcut,
            DeferredInsertShortcut::CommandOptionV
        );

        config
            .save_to_env("CODESCRIBE_DEFERRED_INSERT_SHORTCUT", "not_a_chord")
            .expect("invalid chord is a non-fatal no-op");
        assert_eq!(
            UserSettings::load().deferred_insert_shortcut.as_deref(),
            Some("command_option_v"),
            "invalid chord must not clobber the stored one"
        );

        config
            .save_to_env_many(&[
                ("RESTORE_CLIPBOARD", "0"),
                ("RESTORE_CLIPBOARD_DELAY_MS", "450"),
            ])
            .expect("save clipboard restore batch");
        let stored = UserSettings::load();
        assert_eq!(stored.restore_clipboard, Some(false));
        assert_eq!(stored.restore_clipboard_delay_ms, Some(450));
        let live = Config::load_without_keychain();
        assert!(!live.restore_clipboard);
        assert_eq!(live.restore_clipboard_delay_ms, 450);
    }

    /// AUTO_PASTE single/batch writes reload live without shadowing process env.
    #[test]
    #[serial]
    fn auto_paste_single_and_batch_writes_are_hot_reloadable_without_env_shadow() {
        let _tmp = setup_isolated_data_dir();
        let _runtime = TestEnvGuard::unset("AUTO_PASTE_ENABLED");
        let config = Config::default();

        config
            .save_to_env("AUTO_PASTE_ENABLED", "0")
            .expect("save auto paste off");
        assert_eq!(UserSettings::load().auto_paste_enabled, Some(false));
        assert!(!Config::load_without_keychain().auto_paste_enabled);

        config
            .save_to_env_many(&[("AUTO_PASTE_ENABLED", "1")])
            .expect("save auto paste on");
        assert_eq!(UserSettings::load().auto_paste_enabled, Some(true));
        assert!(Config::load_without_keychain().auto_paste_enabled);

        let env_path = Config::env_path();
        if env_path.exists() {
            let env = Config::parse_env_file(&env_path).expect("parse optional env");
            assert!(!env.contains_key("AUTO_PASTE_ENABLED"));
        }
        assert!(std::env::var("AUTO_PASTE_ENABLED").is_err());
    }

    /// FORMATTING_LEVEL aliases normalize identically on single and batch paths.
    #[test]
    #[serial]
    fn formatting_policy_single_and_batch_writes_normalize_every_alias() {
        let cases = [
            ("off", "off"),
            ("correction", "correction"),
            ("smart", "smart"),
            ("max", "max"),
            ("raw", "off"),
            ("medium", "correction"),
            ("creative", "max"),
        ];

        for (input, normalized) in cases {
            for batch in [false, true] {
                let _tmp = setup_isolated_data_dir();
                let config = Config::default();
                if batch {
                    config
                        .save_to_env_many(&[("FORMATTING_LEVEL", input)])
                        .expect("save policy batch");
                } else {
                    config
                        .save_to_env("FORMATTING_LEVEL", input)
                        .expect("save policy single");
                }
                assert_eq!(
                    UserSettings::load().formatting_level.as_deref(),
                    Some(normalized),
                    "input={input}, batch={batch}"
                );
            }
        }

        for batch in [false, true] {
            let _tmp = setup_isolated_data_dir();
            let config = Config::default();
            let result = if batch {
                config.save_to_env_many(&[("FORMATTING_LEVEL", "aggressive")])
            } else {
                config.save_to_env("FORMATTING_LEVEL", "aggressive")
            };
            assert!(
                result.is_err(),
                "unknown policy was accepted, batch={batch}"
            );
            assert!(!UserSettings::settings_path().exists());
        }
    }

    /// Blank LLM override removes the JSON path and restores the loader default.
    #[test]
    #[serial]
    fn empty_llm_override_unsets_json_path_and_restores_resolved_fallback() {
        let _tmp = setup_isolated_data_dir();
        let _lane_endpoint = TestEnvGuard::unset("LLM_ASSISTIVE_ENDPOINT");
        let _shared_endpoint = TestEnvGuard::unset("LLM_ENDPOINT");
        let config = Config::default();

        config
            .save_to_env(
                "LLM_ASSISTIVE_ENDPOINT",
                "https://api.libraxis.cloud/v1/responses",
            )
            .expect("set assistive endpoint override");

        let set_json: serde_json::Value = serde_json::from_str(
            &fs::read_to_string(UserSettings::settings_path()).expect("read settings after set"),
        )
        .expect("parse settings after set");
        assert_eq!(
            set_json
                .pointer("/speech/assistive/llm_endpoint")
                .and_then(serde_json::Value::as_str),
            Some("https://api.libraxis.cloud/v1/responses")
        );

        config
            .save_to_env("LLM_ASSISTIVE_ENDPOINT", "")
            .expect("reset assistive endpoint override");

        let reset_json: serde_json::Value = serde_json::from_str(
            &fs::read_to_string(UserSettings::settings_path()).expect("read settings after reset"),
        )
        .expect("parse settings after reset");
        assert!(
            reset_json
                .pointer("/speech/assistive/llm_endpoint")
                .is_none(),
            "reset must remove the override path, got {reset_json}"
        );
        assert_eq!(UserSettings::load().llm_assistive_endpoint, None);
        let runtime_settings = Config::load_runtime_snapshot().expect("runtime settings seal");
        assert_eq!(
            runtime_settings.llm_lanes().assistive().endpoint(),
            crate::config::DEFAULT_OPENAI_RESPONSES_ENDPOINT
        );
    }

    /// Blank assistive provider removes the JSON path and restores default provider.
    #[test]
    #[serial]
    fn empty_assistive_provider_unsets_json_path_and_restores_default() {
        let _tmp = setup_isolated_data_dir();
        let _provider = TestEnvGuard::unset("LLM_ASSISTIVE_PROVIDER");
        let config = Config::default();

        config
            .save_to_env("LLM_ASSISTIVE_PROVIDER", "anthropic-messages")
            .expect("set assistive provider override");
        assert_eq!(
            UserSettings::load().llm_assistive_provider.as_deref(),
            Some("anthropic-messages")
        );

        config
            .save_to_env("LLM_ASSISTIVE_PROVIDER", "")
            .expect("reset assistive provider override");

        let reset_json: serde_json::Value = serde_json::from_str(
            &fs::read_to_string(UserSettings::settings_path()).expect("read settings after reset"),
        )
        .expect("parse settings after reset");
        assert!(
            reset_json
                .pointer("/speech/assistive/llm_provider")
                .is_none(),
            "reset must remove the provider override path, got {reset_json}"
        );
        assert_eq!(UserSettings::load().llm_assistive_provider, None);
        let runtime_settings = Config::load_runtime_snapshot().expect("runtime settings seal");
        assert_eq!(
            runtime_settings.llm_lanes().assistive().provider(),
            crate::llm::provider::ProviderKind::OpenAiResponses
        );
    }

    /// Single and batch LLM writes must produce bit-identical UserSettings snapshots.
    #[test]
    #[serial]
    fn llm_key_single_and_batch_writes_produce_identical_settings_snapshots() {
        for (key, value, _) in llm_write_key_cases() {
            for input in [*value, "", "   \t  "] {
                let single = save_snapshot(key, input, false);
                let batch = save_snapshot(key, input, true);
                assert_eq!(single, batch, "snapshot mismatch for {key}={input:?}");
            }
        }
    }

    /// Batch blank LLM overrides clear every optional JSON path and restore defaults.
    #[test]
    #[serial]
    fn save_to_env_many_blank_llm_overrides_remove_json_paths_and_restore_fallbacks() {
        let _tmp = setup_isolated_data_dir();
        let _endpoint = TestEnvGuard::unset("LLM_ENDPOINT");
        let _model = TestEnvGuard::unset("LLM_MODEL");
        let _formatting_endpoint = TestEnvGuard::unset("LLM_FORMATTING_ENDPOINT");
        let _formatting_model = TestEnvGuard::unset("LLM_FORMATTING_MODEL");
        let _assistive_endpoint = TestEnvGuard::unset("LLM_ASSISTIVE_ENDPOINT");
        let _assistive_model = TestEnvGuard::unset("LLM_ASSISTIVE_MODEL");
        let _assistive_provider = TestEnvGuard::unset("LLM_ASSISTIVE_PROVIDER");
        let config = Config::default();

        let set_entries: Vec<(&str, &str)> = llm_write_key_cases()
            .iter()
            .filter_map(|(key, value, pointer)| pointer.map(|_| (*key, *value)))
            .collect();
        config
            .save_to_env_many(&set_entries)
            .expect("set optional LLM overrides");

        let reset_entries: Vec<(&str, &str)> = set_entries
            .iter()
            .enumerate()
            .map(|(index, (key, _))| (*key, if index % 2 == 0 { "" } else { "  \n\t " }))
            .collect();
        config
            .save_to_env_many(&reset_entries)
            .expect("reset optional LLM overrides");

        let reset_json: serde_json::Value = serde_json::from_str(
            &fs::read_to_string(UserSettings::settings_path()).expect("read settings after reset"),
        )
        .expect("parse settings after reset");
        let settings = UserSettings::load();
        for (key, _, pointer) in llm_write_key_cases() {
            if let Some(pointer) = pointer {
                assert!(
                    reset_json.pointer(pointer).is_none(),
                    "batch reset must remove {key} at {pointer}, got {reset_json}"
                );
                assert_optional_override_absent(&settings, key);
            }
        }
        let runtime_settings = Config::load_runtime_snapshot().expect("runtime settings seal");
        assert_eq!(
            runtime_settings.llm_lanes().assistive().endpoint(),
            crate::config::DEFAULT_OPENAI_RESPONSES_ENDPOINT
        );
    }

    /// Batch promoted settings persist without mutating process env or creating .env.
    #[test]
    #[serial]
    fn save_to_env_many_persists_batch_without_process_env_mutation() {
        let _tmp = setup_isolated_data_dir();
        let _model = TestEnvGuard::unset("LLM_MODEL");
        let _workspace_roots = TestEnvGuard::unset("AGENT_WORKSPACE_ROOTS");

        Config::default()
            .save_to_env_many(&[
                ("LLM_MODEL", "batch-model"),
                ("AGENT_WORKSPACE_ROOTS", "/tmp/a:/tmp/b"),
            ])
            .expect("save settings batch");

        assert!(std::env::var("LLM_MODEL").is_err());
        assert!(std::env::var("AGENT_WORKSPACE_ROOTS").is_err());
        assert_eq!(
            UserSettings::load().llm_model.as_deref(),
            Some("batch-model")
        );
        assert_eq!(
            UserSettings::load().agent_workspace_roots,
            Some(vec!["/tmp/a".to_string(), "/tmp/b".to_string()])
        );
        assert!(
            !Config::env_path().exists(),
            "a fully promoted settings batch must not create a legacy .env"
        );
    }

    /// Load seeds OpenAI Responses endpoint/model defaults without requiring API keys.
    #[test]
    #[serial]
    fn load_injects_openai_responses_defaults_without_api_key() {
        let _tmp = setup_isolated_data_dir();
        let _endpoint = TestEnvGuard::unset("LLM_ENDPOINT");
        let _model = TestEnvGuard::unset("LLM_MODEL");
        let _formatting_endpoint = TestEnvGuard::unset("LLM_FORMATTING_ENDPOINT");
        let _formatting_model = TestEnvGuard::unset("LLM_FORMATTING_MODEL");
        let _assistive_endpoint = TestEnvGuard::unset("LLM_ASSISTIVE_ENDPOINT");
        let _assistive_model = TestEnvGuard::unset("LLM_ASSISTIVE_MODEL");
        let _api_key = TestEnvGuard::unset("LLM_API_KEY");
        let _formatting_key = TestEnvGuard::unset("LLM_FORMATTING_API_KEY");
        let _assistive_key = TestEnvGuard::unset("LLM_ASSISTIVE_API_KEY");

        let config = Config::load();

        assert_eq!(
            config.llm_endpoint.as_deref(),
            Some(super::super::DEFAULT_OPENAI_RESPONSES_ENDPOINT)
        );
        assert_eq!(
            std::env::var("LLM_ENDPOINT").as_deref(),
            Ok(super::super::DEFAULT_OPENAI_RESPONSES_ENDPOINT)
        );
        assert_eq!(
            std::env::var("LLM_MODEL").as_deref(),
            Ok(super::super::DEFAULT_LLM_MODEL)
        );
        assert_eq!(
            std::env::var("LLM_FORMATTING_ENDPOINT").as_deref(),
            Ok(super::super::DEFAULT_OPENAI_RESPONSES_ENDPOINT)
        );
        assert_eq!(
            std::env::var("LLM_FORMATTING_MODEL").as_deref(),
            Ok(super::super::DEFAULT_FORMATTING_MODEL)
        );
        assert_eq!(
            std::env::var("LLM_ASSISTIVE_ENDPOINT").as_deref(),
            Ok(super::super::DEFAULT_OPENAI_RESPONSES_ENDPOINT)
        );
        assert_eq!(
            std::env::var("LLM_ASSISTIVE_MODEL").as_deref(),
            Ok(super::super::DEFAULT_ASSISTIVE_MODEL)
        );
        assert!(std::env::var("LLM_API_KEY").is_err());
        assert!(std::env::var("LLM_FORMATTING_API_KEY").is_err());
        assert!(std::env::var("LLM_ASSISTIVE_API_KEY").is_err());
    }

    /// apply_user_settings copies hold/double-tap/silence/exclusive timing into Config.
    #[test]
    #[serial]
    fn test_hotkey_timing_params_applied_from_settings() {
        let prev_hold_start_delay = std::env::var("HOLD_START_DELAY_MS").ok();
        let prev_double_tap = std::env::var("DOUBLE_TAP_INTERVAL_MS").ok();
        let prev_toggle_silence = std::env::var("TOGGLE_SILENCE_SEC").ok();
        let prev_hold_exclusive = std::env::var("HOLD_EXCLUSIVE").ok();

        remove_env_for_test("HOLD_START_DELAY_MS");
        remove_env_for_test("DOUBLE_TAP_INTERVAL_MS");
        remove_env_for_test("TOGGLE_SILENCE_SEC");
        remove_env_for_test("HOLD_EXCLUSIVE");

        let mut config = Config::default();
        let settings = super::super::settings::UserSettings {
            hold_start_delay_ms: Some(500),
            double_tap_interval_ms: Some(300),
            toggle_silence_sec: Some(3.0),
            hold_exclusive: Some(true),
            ..Default::default()
        };

        config.apply_user_settings(&settings);

        assert_eq!(config.hold_start_delay_ms, 500);
        assert_eq!(config.double_tap_interval_ms, 300);
        assert!((config.toggle_silence_sec - 3.0).abs() < f32::EPSILON);
        assert!(config.hold_exclusive);

        restore_env_for_test("HOLD_START_DELAY_MS", prev_hold_start_delay);
        restore_env_for_test("DOUBLE_TAP_INTERVAL_MS", prev_double_tap);
        restore_env_for_test("TOGGLE_SILENCE_SEC", prev_toggle_silence);
        restore_env_for_test("HOLD_EXCLUSIVE", prev_hold_exclusive);
    }

    /// settings.json can disable local STT; load must honor that flag.
    #[test]
    #[serial]
    fn test_load_respects_use_local_stt_from_settings_json() {
        let _tmp = setup_isolated_data_dir();

        let mut settings = UserSettings::load();
        settings.use_local_stt = Some(false);
        settings.save().expect("save settings");

        let config = Config::load();
        assert!(
            !config.use_local_stt,
            "settings.json should be able to disable local STT"
        );
    }

    /// Whisper initial_prompt stays off until settings explicitly opts in.
    #[test]
    #[serial]
    fn test_stt_initial_prompt_defaults_off_and_requires_opt_in() {
        let _tmp = setup_isolated_data_dir();
        let _prompt_env = TestEnvGuard::unset("CODESCRIBE_STT_INITIAL_PROMPT_ENABLED");

        let default_config = Config::load();
        assert!(
            !default_config.stt_initial_prompt_enabled,
            "fresh config must not enable Whisper initial_prompt"
        );

        let mut settings = UserSettings::load();
        settings.stt_initial_prompt_enabled = Some(true);
        settings.save().expect("save settings");

        let config = Config::load();
        assert!(
            config.stt_initial_prompt_enabled,
            "settings.json seed should be able to opt into Whisper initial_prompt"
        );
        assert_eq!(
            std::env::var("CODESCRIBE_STT_INITIAL_PROMPT_ENABLED").as_deref(),
            Ok("1"),
            "settings seed should publish the env-managed STT prompt knob"
        );
    }

    /// Explicit process env can force STT initial_prompt off over settings.json.
    #[test]
    #[serial]
    fn test_runtime_env_can_force_stt_initial_prompt_off_over_settings() {
        let _tmp = setup_isolated_data_dir();
        let _prompt_env = TestEnvGuard::unset("CODESCRIBE_STT_INITIAL_PROMPT_ENABLED");

        let mut settings = UserSettings::load();
        settings.stt_initial_prompt_enabled = Some(true);
        settings.save().expect("save settings");

        set_env_for_test("CODESCRIBE_STT_INITIAL_PROMPT_ENABLED", "0");
        let config = Config::load();
        assert!(
            !config.stt_initial_prompt_enabled,
            "explicit env must be able to keep Whisper initial_prompt disabled"
        );
    }

    /// settings.json can disable the live transcription overlay.
    #[test]
    #[serial]
    fn test_load_respects_transcription_overlay_enabled_from_settings_json() {
        let _tmp = setup_isolated_data_dir();
        let _overlay_env = TestEnvGuard::unset("TRANSCRIPTION_OVERLAY_ENABLED");

        let mut settings = UserSettings::load();
        settings.transcription_overlay_enabled = Some(false);
        settings.save().expect("save settings");

        let config = Config::load();
        assert!(
            !config.transcription_overlay_enabled,
            "settings.json should be able to disable transcription overlay"
        );
    }

    /// settings.json can switch UI-initiated recording from dictation to assistive.
    #[test]
    #[serial]
    fn test_load_respects_tray_start_assistive_from_settings_json() {
        let _tmp = setup_isolated_data_dir();
        let _tray_start_env = TestEnvGuard::unset("TRAY_START_ASSISTIVE");

        let default_config = Config::load();
        assert!(
            !default_config.tray_start_assistive,
            "UI-initiated recording should default to dictation"
        );

        let mut settings = UserSettings::load();
        settings.tray_start_assistive = Some(true);
        settings.save().expect("save settings");

        let config = Config::load();
        assert!(
            config.tray_start_assistive,
            "settings.json should be able to switch UI-initiated recording to assistive"
        );
    }

    /// Legacy .env USE_LOCAL_STT migrates into settings.json on first load.
    #[test]
    #[serial]
    fn test_load_migrates_use_local_stt_from_env_file_before_settings_json_exists() {
        let _tmp = setup_isolated_data_dir();

        let env_path = Config::env_path();
        fs::create_dir_all(env_path.parent().expect("env dir")).expect("create env dir");
        fs::write(&env_path, "USE_LOCAL_STT=0\n").expect("write .env");

        let config = Config::load();
        assert!(!config.use_local_stt, ".env should disable local STT");

        let settings = UserSettings::load();
        assert_eq!(settings.use_local_stt, Some(false));
        assert!(UserSettings::settings_path().exists());
    }

    /// Promoted settings.json keys beat stale .env values and are not re-injected.
    #[test]
    #[serial]
    fn test_load_prefers_settings_json_over_promoted_env_file_values() {
        let _tmp = setup_isolated_data_dir();
        let previous = std::env::var("AI_FORMATTING_ENABLED").ok();
        remove_env_for_test("AI_FORMATTING_ENABLED");

        let mut settings = UserSettings::load();
        settings.ai_formatting_enabled = Some(false);
        settings.save().expect("save settings");

        let env_path = Config::env_path();
        fs::create_dir_all(env_path.parent().expect("env dir")).expect("create env dir");
        fs::write(&env_path, "AI_FORMATTING_ENABLED=1\n").expect("write .env");

        let config = Config::load();
        assert!(
            !config.ai_formatting_enabled,
            ".env should not override promoted settings.json keys"
        );
        assert!(
            std::env::var("AI_FORMATTING_ENABLED").is_err(),
            "promoted .env key must not be injected into process env"
        );

        restore_env_for_test("AI_FORMATTING_ENABLED", previous);
    }

    /// Non-promoted env-managed keys (e.g. STT_API_KEY) still load from optional .env.
    #[test]
    #[serial]
    fn test_load_still_honors_env_managed_values_from_optional_env_file() {
        let _tmp = setup_isolated_data_dir();

        let env_path = Config::env_path();
        fs::create_dir_all(env_path.parent().expect("env dir")).expect("create env dir");
        fs::write(&env_path, "STT_API_KEY=test-from-env-file\n").expect("write .env");

        let config = Config::load();
        assert_eq!(config.stt_api_key.as_deref(), Some("test-from-env-file"));
    }

    /// Explicit runtime env must not synthesize or persist into settings.json.
    #[test]
    #[serial]
    fn test_runtime_env_does_not_persist_into_settings_during_migration() {
        let _tmp = setup_isolated_data_dir();
        let env_path = Config::env_path();
        if env_path.exists() {
            fs::remove_file(&env_path).expect("scrub stale .env");
        }

        set_env_for_test("AI_FORMATTING_ENABLED", "1");

        let config = Config::load();
        assert!(config.ai_formatting_enabled);
        assert!(
            !UserSettings::settings_path().exists(),
            "explicit runtime env should not synthesize settings.json"
        );
        let reloaded = UserSettings::load();
        assert_eq!(
            reloaded.ai_formatting_enabled, None,
            "runtime env must not be persisted into settings.json on subsequent load"
        );

        remove_env_for_test("AI_FORMATTING_ENABLED");
    }
}
