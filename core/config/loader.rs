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

use super::energy_calibration::{SealedEnergyCalibration, energy_calibration_path};
use super::settings::{
    DEFAULT_AGENT_WORKSPACE_ROOT, DEFAULT_SEAL_LANE_ARMED, FormattingPolicy, RuntimeAiExecution,
    RuntimeAiRequestTiming, RuntimeFormatterExecution, RuntimeLlmCredential, RuntimeLlmLane,
    RuntimeLlmLaneKind, RuntimeLlmLanes, RuntimeSettingsSnapshot, RuntimeSnapshotParts,
    SILERO_FUSION_ENV, SettingsSnapshotDigest, SettingsSnapshotProvenance,
    SettingsSnapshotValidationError, UserSettings, normalize_agent_workspace_roots,
    normalize_stt_engine, parse_agent_workspace_roots,
};
use super::types::{
    Config, DeferredInsertShortcut, Language, OverlayPositionMode, TranscriptSendMode,
};
use crate::llm::account_auth;
use crate::llm::provider::{LlmMode, ProviderKind, ProviderRef, ProviderRegistry, WireFamily};

/// Has the process already seeded its environment from config? Seeding happens
/// once, at the first load; later loads read snapshots instead, so a background
/// thread never sees `set_var` racing under it.
static CONFIG_ENV_BOOTSTRAPPED: AtomicBool = AtomicBool::new(false);
/// Retired per-lane key names still accepted as `LLM_OPENAI_API_KEY` aliases.
/// REMOVE AFTER 2026-10-15.
const LEGACY_LLM_KEY_ENV: [&str; 3] = [
    "LLM_API_KEY",
    "LLM_FORMATTING_API_KEY",
    "LLM_ASSISTIVE_API_KEY",
];
/// Retired endpoint/model env names; present ⇒ one warning, never honored.
const LEGACY_LLM_ENDPOINT_ENV: [&str; 6] = [
    "LLM_ENDPOINT",
    "LLM_FORMATTING_ENDPOINT",
    "LLM_ASSISTIVE_ENDPOINT",
    "LLM_XAI_ENDPOINT",
    "LLM_ANTHROPIC_ENDPOINT",
    "LLM_MODEL",
];
/// Legacy LLM env names this process has already warned about.
static LEGACY_LLM_ENV_WARNED: OnceLock<Mutex<HashSet<&'static str>>> = OnceLock::new();

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

/// Source facts for the sole snapshot resolver. Values are captured after the
/// existing Config loader precedence; no digest or resolved lane is caller-owned.
/// Deliberately not Debug: credentials and prompt content must not enter logs.
#[derive(Clone)]
pub struct CapturedRuntimeInputs {
    pub values: Config,
    pub user_settings: UserSettings,
    pub settings_path: PathBuf,
    pub settings_bytes: Option<Vec<u8>>,
    pub env_overlay_keys: Vec<String>,
    pub loaded_at_unix_ms: u64,
    pub energy_calibration_path: PathBuf,
    pub energy_calibration: SealedEnergyCalibration,
    /// Only explicit runtime overrides; absent and non-Unicode remain distinct.
    pub overrides: HashMap<String, Result<String, VarError>>,
    /// Captured key/account availability, keyed by provider key account.
    pub credentials: HashMap<String, CapturedLaneCredential>,
    pub prompts: super::prompts::CapturedRuntimePrompts,
    pub repair_receipt: super::repair::RepairReceipt,
}

/// Captured credential facts. Secret values deliberately have no Debug impl.
#[derive(Clone, Default)]
pub struct CapturedLaneCredential {
    pub api_key: Option<String>,
    pub signed_in: bool,
}

impl CapturedRuntimeInputs {
    /// Explicit no-host inputs: compiled defaults and missing measured evidence.
    /// The caller supplies both the root and time; neither HOME nor a clock is read.
    pub fn defaults_at(data_root: PathBuf, loaded_at_unix_ms: u64) -> Self {
        let energy_calibration_path =
            data_root.join(super::energy_calibration::ENERGY_CALIBRATION_FILE_NAME);
        Self {
            values: Config::default(),
            user_settings: UserSettings::default(),
            settings_path: data_root.join("settings.json"),
            settings_bytes: None,
            env_overlay_keys: Vec::new(),
            loaded_at_unix_ms,
            energy_calibration: SealedEnergyCalibration::from_captured(
                &energy_calibration_path,
                Ok(None),
            ),
            energy_calibration_path,
            overrides: HashMap::new(),
            credentials: HashMap::new(),
            prompts: super::prompts::CapturedRuntimePrompts::default(),
            repair_receipt: super::repair::RepairReceipt::default(),
        }
    }

    fn env(&self, key: &str) -> Result<String, VarError> {
        self.overrides
            .get(key)
            .cloned()
            .unwrap_or(Err(VarError::NotPresent))
    }

    fn non_empty(&self, key: &str) -> Option<String> {
        self.env(key).ok().and_then(Config::non_empty_string)
    }
}

type StartupAcquisitionAttempts = std::rc::Rc<std::cell::RefCell<Vec<&'static str>>>;

thread_local! {
    static STARTUP_ACQUISITION_PROBE: std::cell::RefCell<Option<StartupAcquisitionAttempts>> = const { std::cell::RefCell::new(None) };
}

/// Synchronous, thread-bound acquisition tripwire for dependency-mode witnesses.
/// Every observed entry panics BEFORE host access, never silently suppresses it.
/// No environment changes and no cross-thread/process test configuration.
#[doc(hidden)]
pub struct StartupAcquisitionProbe {
    attempts: StartupAcquisitionAttempts,
}

impl StartupAcquisitionProbe {
    pub fn forbid() -> Self {
        let attempts = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        STARTUP_ACQUISITION_PROBE.with(|slot| {
            let mut slot = slot.borrow_mut();
            assert!(
                slot.is_none(),
                "startup acquisition probe already installed"
            );
            *slot = Some(attempts.clone());
        });
        Self { attempts }
    }

    pub fn attempts(&self) -> Vec<&'static str> {
        self.attempts.borrow().clone()
    }
}

impl Drop for StartupAcquisitionProbe {
    fn drop(&mut self) {
        STARTUP_ACQUISITION_PROBE.with(|slot| *slot.borrow_mut() = None);
    }
}

/// Instrument acquisition adapters, including the app crate's startup adapter.
#[doc(hidden)]
pub fn note_startup_acquisition(source: &'static str) {
    STARTUP_ACQUISITION_PROBE.with(|slot| {
        if let Some(attempts) = slot.borrow().as_ref() {
            attempts.borrow_mut().push(source);
            panic!("forbidden startup acquisition: {source}");
        }
    });
}

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

    /// Load runtime configuration using only files, env, and cached credentials.
    /// Credential imports remain pending until an authorized load can commit them.
    pub fn load_without_keychain() -> Self {
        Self::load_with_keychain_population(false)
    }

    /// Run the one loader pass and seal the immutable settings truth used by a
    /// recording session. Consumers receive this value; they never re-read
    /// `settings.json` or process env during a take.
    pub fn load_runtime_snapshot()
    -> Result<RuntimeSettingsSnapshot, SettingsSnapshotValidationError> {
        Ok(Self::load_runtime_snapshot_with_keychain_population(true))
    }

    /// Keychain-free form of [`Self::load_runtime_snapshot`] for local capture.
    pub fn load_runtime_snapshot_without_keychain()
    -> Result<RuntimeSettingsSnapshot, SettingsSnapshotValidationError> {
        Ok(Self::load_runtime_snapshot_with_keychain_population(false))
    }

    /// Infallible launch path: config refusals remain typed in the receipt and
    /// disarm capture while leaving Settings available for recovery.
    pub fn load_startup_runtime_snapshot(populate_keychain: bool) -> RuntimeSettingsSnapshot {
        Self::load_runtime_snapshot_with_keychain_population(populate_keychain)
    }

    fn load_runtime_snapshot_with_keychain_population(
        populate_keychain: bool,
    ) -> RuntimeSettingsSnapshot {
        let input = Self::capture_runtime_inputs(populate_keychain);
        let prior_repairs = input.repair_receipt.clone();
        let snapshot = Self::resolve_runtime_snapshot_with_capture(|| input);
        // Recording/logging are host concerns, never part of resolution/sealing.
        let receipt = snapshot.repair_receipt();
        super::repair::record(super::repair::RepairReceipt {
            actions: receipt
                .actions
                .iter()
                .filter(|r| !prior_repairs.actions.contains(r))
                .cloned()
                .collect(),
            backups: receipt
                .backups
                .iter()
                .filter(|r| !prior_repairs.backups.contains(r))
                .cloned()
                .collect(),
            unrepairable: receipt
                .unrepairable
                .iter()
                .filter(|r| !prior_repairs.unrepairable.contains(r))
                .cloned()
                .collect(),
        });
        super::repair::log_launch_once();
        snapshot
    }

    /// Shared capture-to-resolution handoff. A replay adapter can supply the
    /// exact captured facts without running any host acquisition in a witness.
    fn resolve_runtime_snapshot_with_capture(
        capture: impl FnOnce() -> CapturedRuntimeInputs,
    ) -> RuntimeSettingsSnapshot {
        Self::runtime_snapshot_from_captured(capture())
    }

    /// Production acquisition adapter. The shared resolver below sees values only.
    fn capture_runtime_inputs(populate_keychain: bool) -> CapturedRuntimeInputs {
        note_startup_acquisition("settings capture");
        let (values, user_settings) = Self::capture_config_and_settings(populate_keychain);
        let settings_path = UserSettings::settings_path();
        let settings_bytes = fs::read(&settings_path).ok();
        let env_overlay_keys = Self::seeded_env_keys()
            .lock()
            .map(|keys| keys.iter().cloned().collect::<Vec<_>>())
            .unwrap_or_default();
        let loaded_at_unix_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_millis() as u64)
            .unwrap_or_default();
        let energy_calibration_path = energy_calibration_path();
        let energy_calibration = SealedEnergyCalibration::load(&energy_calibration_path);
        let mut overrides = HashMap::new();
        for key in [
            "CODESCRIBE_LAYERED_TRANSCRIPTION",
            "FORMATTING_LEVEL",
            crate::stt::tail_provider::STT_TAIL_PROVIDER_ENV,
            AI_MAX_RETRIES_ENV,
            AI_RETRY_DELAY_MS_ENV,
            AI_ATTEMPT_TIMEOUT_MS_ENV,
            AI_INTER_CHUNK_TIMEOUT_MS_ENV,
            LlmMode::Formatting.provider_env_key(),
            LlmMode::Formatting.model_env_key(),
            LlmMode::Assistive.provider_env_key(),
            LlmMode::Assistive.model_env_key(),
        ] {
            overrides.insert(key.to_string(), Self::config_runtime_env_var(key));
        }
        // This single documented key also honors the seeded .env value.
        overrides.insert(
            SILERO_FUSION_ENV.to_string(),
            std::env::var(SILERO_FUSION_ENV),
        );
        let mut input = CapturedRuntimeInputs {
            values,
            user_settings,
            settings_path,
            settings_bytes,
            env_overlay_keys,
            loaded_at_unix_ms,
            energy_calibration_path,
            energy_calibration,
            overrides,
            credentials: HashMap::new(),
            prompts: super::prompts::CapturedRuntimePrompts::default(),
            repair_receipt: super::repair::launch_receipt(),
        };
        Self::warn_legacy_llm_endpoint_env();
        let registry = ProviderRegistry::from_settings(&input.user_settings);
        for lane in [
            RuntimeLlmLaneKind::Formatting,
            RuntimeLlmLaneKind::Assistive,
        ] {
            let reference = Self::runtime_provider_reference(lane, &input);
            let provider = registry
                .resolve(&reference)
                .or_else(|| registry.resolve(&ProviderRef::default()))
                .expect("the default vendor is always registered");
            let api_key = Self::runtime_lane_api_key(&provider.key_account);
            let signed_in = lane == RuntimeLlmLaneKind::Assistive
                && provider.wire == WireFamily::OpenAiResponses
                && provider.oauth_vendor.is_some_and(|vendor| {
                    account_auth::provider_oauth_config(vendor)
                        .is_ok_and(|row| Self::signed_in_provider_account(row.tokens_account))
                });
            input.credentials.insert(
                provider.key_account,
                CapturedLaneCredential { api_key, signed_in },
            );
        }
        let policy = FormattingPolicy::resolve(
            input.env("FORMATTING_LEVEL").ok().as_deref(),
            input.user_settings.formatting_level.as_deref(),
        )
        .unwrap_or(FormattingPolicy::Off);
        input.prompts = super::prompts::CapturedRuntimePrompts::capture(policy);
        input
    }

    /// Resolve and seal captured facts through the same core path as production.
    /// This entry does no host acquisition and accepts no caller-made digest.
    pub fn runtime_snapshot_from_captured(
        mut input: CapturedRuntimeInputs,
    ) -> RuntimeSettingsSnapshot {
        let (seal_lane_armed, seal_lane_env_override) = Self::resolve_seal_lane_armed(&input);
        if seal_lane_env_override {
            input.env_overlay_keys.push(SILERO_FUSION_ENV.to_string());
        }
        input.env_overlay_keys.sort_unstable();
        input.env_overlay_keys.dedup();
        let provenance = SettingsSnapshotProvenance {
            settings_json_path: input
                .settings_bytes
                .as_ref()
                .map(|_| input.settings_path.clone()),
            settings_json_sha256: input.settings_bytes.as_deref().map(sha256_hex),
            env_overlay_keys: input.env_overlay_keys.clone(),
            defaults_applied: true,
            loaded_at_unix_ms: input.loaded_at_unix_ms,
            energy_calibration_path: input.energy_calibration_path.clone(),
            energy_calibration_sha256: input.energy_calibration.sha256().map(str::to_owned),
        };
        let user_settings = &input.user_settings;
        let phase_override = input.env("CODESCRIBE_LAYERED_TRANSCRIPTION").ok();
        let mut local_tail_patch = resolve_local_tail_patch(
            phase_override
                .as_deref()
                .or(user_settings.layered_transcription.as_deref()),
        );
        let tail_provider = match input.env(crate::stt::tail_provider::STT_TAIL_PROVIDER_ENV) {
            Ok(value) => crate::stt::tail_provider::TailProviderId::parse(&value).ok(),
            Err(VarError::NotPresent) => Some(crate::stt::tail_provider::TailProviderId::InProcess),
            Err(_) => None,
        };
        if tail_provider.is_none() {
            local_tail_patch =
                crate::asr_session::recorder::LocalTailPatchDisposition::DegradedInvalidOverride;
        }
        let runtime_formatting_policy = input.env("FORMATTING_LEVEL").ok();
        let formatting_policy = FormattingPolicy::resolve(
            runtime_formatting_policy.as_deref(),
            user_settings.formatting_level.as_deref(),
        )
        .unwrap_or_else(|_| {
            let refusal = super::repair::ConfigUnrepairable {
                path: input.settings_path.clone(),
                reason: "invalid FORMATTING_LEVEL override; formatting disabled for this launch; fix the override".into(),
            };
            if !input.repair_receipt.unrepairable.contains(&refusal) {
                input.repair_receipt.unrepairable.push(refusal);
            }
            FormattingPolicy::Off
        });
        if let (Some(runtime), Some(persisted)) = (
            runtime_formatting_policy.as_deref(),
            user_settings.formatting_level.as_deref(),
        ) && let (Ok(runtime), Ok(persisted)) = (
            FormattingPolicy::parse(runtime),
            FormattingPolicy::parse(persisted),
        ) && runtime != persisted
        {
            let note = super::repair::RepairAction::PrecedenceNote {
                key: "FORMATTING_LEVEL".into(),
            };
            if !input.repair_receipt.actions.contains(&note) {
                input.repair_receipt.actions.push(note);
            }
        }
        let seal_lane_armed = seal_lane_armed && input.repair_receipt.unrepairable.is_empty();
        let llm_lanes = Self::resolve_runtime_llm_lanes(&input);
        let ai_execution = Self::resolve_runtime_ai_execution(formatting_policy, &input);
        let mut digest_values = input.values.clone();
        for key in [
            &mut digest_values.stt_file_api_key,
            &mut digest_values.stt_live_api_key,
        ] {
            *key = key.as_ref().map(|_| "<redacted:present>".to_string());
        }
        let repair_sha256 = sha256_hex(
            serde_json::to_string(&input.repair_receipt)
                .expect("repair receipt serializes")
                .as_bytes(),
        );
        let digest_material = format!(
            "repair_sha256={repair_sha256}\n{digest_values:?}\n{user_settings:?}\n{provenance:?}\nformatting_policy={}\nseal_lane_armed={seal_lane_armed}\nlocal_tail_patch={local_tail_patch:?}\ntail_provider={tail_provider:?}\n{}\n{}\n{}",
            formatting_policy.as_str(),
            llm_lanes.digest_material(),
            ai_execution.digest_material(),
            input.energy_calibration.digest_material(),
        );
        let digest = SettingsSnapshotDigest::from_hex(sha256_hex(digest_material.as_bytes()));
        let parts = RuntimeSnapshotParts {
            repair_receipt: input.repair_receipt,
            values: input.values,
            user_settings: input.user_settings,
            llm_lanes,
            formatting_policy,
            ai_execution,
            provenance,
            digest,
            energy_calibration: input.energy_calibration,
            seal_lane_armed,
            local_tail_patch,
            tail_provider,
        };
        let recovery = parts.clone();
        match RuntimeSettingsSnapshot::seal_loaded(RuntimeSnapshotParts {
            repair_receipt: parts.repair_receipt,
            values: parts.values,
            user_settings: parts.user_settings,
            llm_lanes: parts.llm_lanes,
            formatting_policy: parts.formatting_policy,
            ai_execution: parts.ai_execution,
            provenance: parts.provenance,
            digest: parts.digest,
            energy_calibration: parts.energy_calibration,
            seal_lane_armed: parts.seal_lane_armed,
            local_tail_patch: parts.local_tail_patch,
            tail_provider: parts.tail_provider,
        }) {
            Ok(snapshot) => snapshot,
            Err(error) => {
                RuntimeSettingsSnapshot::refused_startup(recovery, error, input.settings_path)
            }
        }
    }

    /// Resolve the one product-owned arming value for this immutable settings
    /// generation. The process value may be either an explicit shell override
    /// or the optional `.env` value injected during bootstrap; for this one
    /// documented power-user key both forms deliberately outrank Settings.
    fn resolve_seal_lane_armed(input: &CapturedRuntimeInputs) -> (bool, bool) {
        let configured = input
            .user_settings
            .seal_lane_armed
            .unwrap_or(DEFAULT_SEAL_LANE_ARMED);
        match input.env(SILERO_FUSION_ENV) {
            Ok(raw) => (
                matches!(
                    raw.trim().to_ascii_lowercase().as_str(),
                    "1" | "true" | "yes" | "on"
                ),
                true,
            ),
            Err(_) => (configured, false),
        }
    }

    /// Resolve prompt, retry, and shared Agent/formatter timing once for the
    /// selected runtime generation. No consumer may reconstruct these facts.
    fn resolve_runtime_ai_execution(
        formatting_policy: FormattingPolicy,
        input: &CapturedRuntimeInputs,
    ) -> RuntimeAiExecution {
        let (formatting_prompt, assistive_prompt) = input.prompts.seal(formatting_policy);
        let max_retries =
            Self::runtime_env_or_default(input, AI_MAX_RETRIES_ENV, DEFAULT_AI_MAX_RETRIES);
        let retry_delay_ms =
            Self::runtime_env_or_default(input, AI_RETRY_DELAY_MS_ENV, DEFAULT_AI_RETRY_DELAY_MS);
        let attempt_timeout_ms = Self::runtime_env_or_default(
            input,
            AI_ATTEMPT_TIMEOUT_MS_ENV,
            DEFAULT_AI_ATTEMPT_TIMEOUT_MS,
        );
        let inter_chunk_timeout_ms = Self::runtime_env_or_default(
            input,
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

    fn runtime_env_or_default<T>(input: &CapturedRuntimeInputs, key: &str, default: T) -> T
    where
        T: FromStr,
    {
        input
            .env(key)
            .ok()
            .and_then(|value| {
                let value = value.trim();
                (!value.is_empty())
                    .then(|| value.parse::<T>().ok())
                    .flatten()
            })
            .unwrap_or(default)
    }

    /// Resolve both LLM lanes during the one settings-loader pass. Consumers
    /// only receive the sealed result; none may repeat this work.
    fn resolve_runtime_llm_lanes(input: &CapturedRuntimeInputs) -> RuntimeLlmLanes {
        let registry = ProviderRegistry::from_settings(&input.user_settings);
        RuntimeLlmLanes::seal(
            Self::resolve_runtime_llm_lane(RuntimeLlmLaneKind::Formatting, &registry, input),
            Self::resolve_runtime_llm_lane(RuntimeLlmLaneKind::Assistive, &registry, input),
        )
    }

    /// The single resolution path (`00_ATLAS.md §B`): provider reference from
    /// env, then settings, then the default vendor; the registry turns it into
    /// endpoint + wire + key account; the model comes from env, settings, or
    /// the vendor default (a Custom provider has no default and seals as
    /// unavailable until one is chosen). Credentials are read through the
    /// Keychain corridor only.
    fn resolve_runtime_llm_lane(
        lane: RuntimeLlmLaneKind,
        registry: &ProviderRegistry,
        input: &CapturedRuntimeInputs,
    ) -> RuntimeLlmLane {
        let settings = &input.user_settings;
        let (mode, persisted_model) = match lane {
            RuntimeLlmLaneKind::Formatting => (
                LlmMode::Formatting,
                settings.llm_formatting_model.as_deref(),
            ),
            RuntimeLlmLaneKind::Assistive => {
                (LlmMode::Assistive, settings.llm_assistive_model.as_deref())
            }
        };
        let reference = Self::runtime_provider_reference(lane, input);
        let (provider, mut unavailable_reason) = match registry.resolve(&reference) {
            Some(provider) => (provider, None),
            None => (
                registry
                    .resolve(&ProviderRef::default())
                    .expect("the default vendor is always registered"),
                Some(format!(
                    "custom provider `{}` no longer exists",
                    reference.custom_id().unwrap_or_default()
                )),
            ),
        };
        let model = input
            .non_empty(mode.model_env_key())
            .or_else(|| {
                persisted_model
                    .map(str::to_string)
                    .and_then(Self::non_empty_string)
            })
            .or_else(|| {
                provider
                    .reference
                    .vendor()
                    .map(|vendor| vendor.default_model(mode).to_string())
            })
            .unwrap_or_default();
        if model.is_empty() && unavailable_reason.is_none() {
            unavailable_reason = Some(format!(
                "no model selected for provider {}",
                provider.display_name
            ));
        }
        let captured = input
            .credentials
            .get(&provider.key_account)
            .cloned()
            .unwrap_or_default();
        let api_key = captured.api_key;
        let account_auth = lane == RuntimeLlmLaneKind::Assistive
            && provider.wire == WireFamily::OpenAiResponses
            && provider.oauth_vendor.is_some()
            && captured.signed_in;
        let credentialed = api_key.is_some() || account_auth || !provider.key_required;
        if !credentialed && unavailable_reason.is_none() {
            unavailable_reason = Some(format!(
                "The {} lane points at {} ({}), which requires a credential, but neither Keychain account {} nor a supported signed-in provider account is available.",
                lane.as_str(),
                provider.display_name,
                provider.endpoint,
                provider.key_account,
            ));
        }
        let available = unavailable_reason.is_none();
        let credential =
            RuntimeLlmCredential::seal(provider.key_account.clone(), api_key, account_auth);
        RuntimeLlmLane::seal(
            lane,
            provider,
            model,
            credential,
            available,
            unavailable_reason,
        )
    }

    fn runtime_provider_reference(
        lane: RuntimeLlmLaneKind,
        input: &CapturedRuntimeInputs,
    ) -> ProviderRef {
        let (mode, persisted) = match lane {
            RuntimeLlmLaneKind::Formatting => (
                LlmMode::Formatting,
                input.user_settings.llm_formatting_provider.as_deref(),
            ),
            RuntimeLlmLaneKind::Assistive => (
                LlmMode::Assistive,
                input.user_settings.llm_assistive_provider.as_deref(),
            ),
        };
        input
            .non_empty(mode.provider_env_key())
            .as_deref()
            .and_then(ProviderRef::parse)
            .or_else(|| persisted.and_then(ProviderRef::parse))
            .unwrap_or_default()
    }

    /// The API key for a provider account through the Keychain corridor.
    ///
    /// Env alias, REMOVE AFTER 2026-10-15: the retired `LLM_API_KEY` /
    /// `LLM_FORMATTING_API_KEY` / `LLM_ASSISTIVE_API_KEY` process-env names
    /// still feed the OpenAI account (and only that one), with a single warn.
    fn runtime_lane_api_key(account: &str) -> Option<String> {
        note_startup_acquisition("credential cache");
        let key = super::keychain::cached_runtime_key(account);
        if key.is_some() || account != ProviderKind::OpenAiResponses.api_key_account() {
            return key;
        }
        LEGACY_LLM_KEY_ENV.iter().find_map(|legacy| {
            let value = std::env::var(legacy).ok().and_then(Self::non_empty_string)?;
            Self::warn_once_legacy_llm_env(
                legacy,
                "is a retired key name; it feeds LLM_OPENAI_API_KEY until 2026-10-15 — move it to Settings › Providers",
            );
            Some(value)
        })
    }

    /// Legacy endpoint/model env is not honored anywhere anymore: vendor
    /// endpoints are pinned, a different host is a Custom provider. Say so once.
    fn warn_legacy_llm_endpoint_env() {
        for legacy in LEGACY_LLM_ENDPOINT_ENV {
            if std::env::var_os(legacy).is_some() {
                Self::warn_once_legacy_llm_env(
                    legacy,
                    "is no longer honored; add a Custom provider in Settings › Providers",
                );
            }
        }
    }

    /// One warning per legacy variable per process.
    fn warn_once_legacy_llm_env(key: &'static str, what: &str) {
        let warned = LEGACY_LLM_ENV_WARNED.get_or_init(|| Mutex::new(HashSet::new()));
        let first = warned
            .lock()
            .map(|mut seen| seen.insert(key))
            .unwrap_or(true);
        if first {
            warn!("{key} {what}");
        }
    }

    /// Seal-time truth of "a provider account is signed in": the serialized
    /// token record under `tokens_account`, read exactly where sign-in put it —
    /// the Keychain bundle, through the process cache (no Keychain I/O at seal
    /// time, same corridor as every API key). An explicit process env value
    /// still wins, as for every other secret. The record must parse: a corrupt
    /// blob seals as "not signed in" instead of a lane that fails at first use.
    ///
    /// Deliberately not [`Self::config_runtime_env_var`]: token accounts are
    /// kept out of `KEYCHAIN_ACCOUNTS` so OAuth tokens are never mirrored into
    /// the process environment (child processes inherit it). That helper could
    /// therefore only see env, which the app never seeds with tokens — from
    /// 6517d4f6a until this change account auth was unreachable in production:
    /// Settings showed "signed in as …" while every sealed lane carried
    /// `account_auth=false`, and removing the API key refused the lane.
    fn signed_in_provider_account(tokens_account: &str) -> bool {
        note_startup_acquisition("account cache");
        super::keychain::cached_runtime_key(tokens_account)
            .is_some_and(|raw| serde_json::from_str::<account_auth::AccountTokens>(&raw).is_ok())
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
        Self::capture_config_and_settings(populate_keychain).0
    }

    fn capture_config_and_settings(populate_keychain: bool) -> (Self, UserSettings) {
        note_startup_acquisition("config files/env/keychain");
        let _data_io = match super::storage_reset::begin_app_data_io() {
            Ok(guard) => guard,
            Err(error) => {
                warn!(%error, "Config load skipped while app-data reset owns the process");
                return (Self::default(), UserSettings::default());
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
        let deferred_settings =
            super::migrate::migrate_if_needed(file_env_vars.as_ref(), populate_keychain);
        if deferred_settings.is_none() {
            super::migrate::migrate_agent_workspace_roots_if_needed(file_env_vars.as_ref());
        }

        // Optional .env remains available for env-managed / power-user keys, but
        // promoted settings are intentionally excluded so stale ~/.codescribe/.env
        // cannot shadow user choices persisted in settings.json.
        if let Some(vars) = file_env_vars.as_ref() {
            Self::inject_file_env_for_runtime(vars);
        }

        // Load API keys from Keychain (only if not already set by .env).
        if populate_keychain {
            super::keychain::populate_env_from_keychain(seed_process_env);
        }

        // Load user settings from JSON. A legacy LLM lane layout migrates
        // inside `load`; its Keychain key moves are applied here, the only
        // place allowed to touch the bundle during a load.
        let mut user_settings = deferred_settings.unwrap_or_else(UserSettings::load);
        if populate_keychain && !user_settings.pending_key_moves.is_empty() {
            match super::settings::UserSettings::settle_pending_key_moves() {
                Ok(changed) => {
                    info!("Applied legacy LLM key relocation ({changed} bundle changes)");
                    user_settings = UserSettings::load();
                }
                Err(error) => warn!("Legacy LLM key relocation remains pending: {error}"),
            }
        }

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

        // STT rows retain explicit .env overrides, including on subsequent loads.
        if let Some(file_env) = file_env_vars.as_ref() {
            if let Some(raw) = file_env.get("STT_ENDPOINT") {
                config.apply_stt_endpoint_alias(raw);
            }
            for (name, target) in [
                ("STT_FILE_ENDPOINT", &mut config.stt_file_endpoint),
                ("STT_LIVE_ENDPOINT", &mut config.stt_live_endpoint),
            ] {
                if let Some(value) = file_env.get(name) {
                    *target = Some(value.clone());
                }
            }
        }

        // Override with environment variables (explicit runtime env + injected env-managed .env).
        config.load_from_env();
        config.sanitize();
        Self::mark_process_env_bootstrapped(seed_process_env);
        (config, user_settings)
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
        note_startup_acquisition("runtime env/cache");
        if super::keychain::is_known_account(key) {
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
    /// `CODESCRIBE_SILERO_FUSION` is the deliberate exception: it remains a
    /// documented power-user override of the product-owned settings field.
    fn inject_file_env_for_runtime(file_env: &HashMap<String, String>) {
        for (key, value) in file_env {
            if super::settings::is_promoted_key(key) && key != SILERO_FUSION_ENV {
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

    fn apply_stt_endpoint_alias(&mut self, raw: &str) {
        let (file, live) = super::stt_migration::split_retired_stt_endpoint(raw);
        self.stt_file_endpoint = file.or(self.stt_file_endpoint.take());
        self.stt_live_endpoint = live.or(self.stt_live_endpoint.take());
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
        if let Ok(val) = Self::config_runtime_env_var("WHISPER_CONTEXT_WINDOW_SEC")
            && let Ok(sec) = val.parse::<f32>()
        {
            self.whisper_context_window_sec = sec;
        }
        if let Ok(val) = Self::config_runtime_env_var("LIGHT_PLUS_SENTENCE_PAUSE_SEC")
            && let Ok(sec) = val.parse::<f32>()
        {
            self.light_plus_sentence_pause_sec = sec;
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

        // Current lane rows override the warned legacy aliases through 2026-10-15.
        if let Ok(raw) = Self::config_runtime_env_var("STT_ENDPOINT") {
            self.apply_stt_endpoint_alias(&raw);
        }
        for (name, target) in [
            ("STT_FILE_ENDPOINT", &mut self.stt_file_endpoint),
            ("STT_LIVE_ENDPOINT", &mut self.stt_live_endpoint),
            ("STT_FILE_API_KEY", &mut self.stt_file_api_key),
            ("STT_LIVE_API_KEY", &mut self.stt_live_api_key),
        ] {
            if let Ok(value) = Self::config_runtime_env_var(name) {
                *target = Some(value);
            }
        }
        // The retired account is deliberately absent from KEYCHAIN_ACCOUNTS.
        if let Some(key) = std::env::var("STT_API_KEY")
            .ok()
            .filter(|v| !v.trim().is_empty())
            .or_else(|| super::keychain::cached_runtime_key("STT_API_KEY"))
        {
            static WARN: std::sync::Once = std::sync::Once::new();
            WARN.call_once(|| warn!("STT_API_KEY is retired; use STT_FILE_API_KEY / STT_LIVE_API_KEY (removed after 2026-10-15)"));
            for target in [&mut self.stt_file_api_key, &mut self.stt_live_api_key] {
                if target.as_deref().is_none_or(|v| v.trim().is_empty()) {
                    *target = Some(key.clone());
                }
            }
        }
        if let Ok(val) = Self::config_runtime_env_var("CODESCRIBE_STT_INITIAL_PROMPT_ENABLED") {
            self.stt_initial_prompt_enabled =
                matches!(val.as_str(), "1" | "true" | "yes" | "on" | "enabled");
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
        if Self::config_runtime_env_var("WHISPER_CONTEXT_WINDOW_SEC").is_err()
            && let Some(v) = settings.whisper_context_window_sec
        {
            self.whisper_context_window_sec = v;
        }
        if Self::config_runtime_env_var("LIGHT_PLUS_SENTENCE_PAUSE_SEC").is_err()
            && let Some(v) = settings.light_plus_sentence_pause_sec
        {
            self.light_plus_sentence_pause_sec = v;
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
        // LLM lanes are not seeded into process env: the loader resolves them
        // from settings.json + explicit env through one path (`resolve_runtime_llm_lane`).
        // ── Promoted fields (previously .env only) ──

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

        for (name, target, value) in [
            (
                "STT_FILE_ENDPOINT",
                &mut self.stt_file_endpoint,
                &settings.stt_file_endpoint,
            ),
            (
                "STT_LIVE_ENDPOINT",
                &mut self.stt_live_endpoint,
                &settings.stt_live_endpoint,
            ),
        ] {
            if Self::config_runtime_env_var(name).is_err() {
                *target = value.clone();
            }
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

        // API keys (vendor and custom-provider accounts) → Keychain
        if super::keychain::is_known_account(key) {
            UserSettings::with_credential_edit(key, |_| super::keychain::save_key(key, value))?;
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
                | "WHISPER_CONTEXT_WINDOW_SEC"
                | "LIGHT_PLUS_SENTENCE_PAUSE_SEC"
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
                | "RESTORE_CLIPBOARD"
                | SILERO_FUSION_ENV => {
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
            // API keys (vendor and custom-provider accounts) → Keychain
            if super::keychain::is_known_account(key) {
                UserSettings::with_credential_edit(key, |_| super::keychain::save_key(key, value))?;
                if let Some(settings) = settings.as_mut() {
                    settings.cancel_pending_credential_imports(key);
                }
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
                    "STT_FILE_ENDPOINT" | "STT_LIVE_ENDPOINT" => {
                        let lane = if *key == "STT_FILE_ENDPOINT" {
                            crate::stt::SttLane::File
                        } else {
                            crate::stt::SttLane::Live
                        };
                        let endpoint = if value.trim().is_empty() {
                            None
                        } else {
                            Some(crate::stt::validate_stt_endpoint(lane, value)?)
                        };
                        match lane {
                            crate::stt::SttLane::File => settings_ref.stt_file_endpoint = endpoint,
                            crate::stt::SttLane::Live => settings_ref.stt_live_endpoint = endpoint,
                        }
                    }
                    "STT_ENDPOINT" => {
                        (
                            settings_ref.stt_file_endpoint,
                            settings_ref.stt_live_endpoint,
                        ) = super::stt_migration::split_retired_stt_endpoint(value);
                    }
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
                        let normalized = normalize_stt_engine(value).ok_or_else(|| {
                            anyhow::anyhow!(
                                "invalid STT engine {value:?}; expected auto, apple, whisper, or candle"
                            )
                        })?;
                        settings_ref.stt_engine = Some(normalized.clone());
                        Self::reconcile_stt_runtime_key(key, &normalized);
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
                    "WHISPER_CONTEXT_WINDOW_SEC" => {
                        if let Ok(v) = value.parse::<f32>() {
                            settings_ref.whisper_context_window_sec =
                                Some(super::normalize_whisper_context_window_sec(v));
                        }
                    }
                    "LIGHT_PLUS_SENTENCE_PAUSE_SEC" => {
                        if let Ok(v) = value.parse::<f32>() {
                            settings_ref.light_plus_sentence_pause_sec =
                                Some(super::normalize_light_plus_sentence_pause_sec(v));
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
                    | "RESTORE_CLIPBOARD"
                    | SILERO_FUSION_ENV => {
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
                            SILERO_FUSION_ENV => settings_ref.seal_lane_armed = Some(bv),
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

    /// Handle the LLM lane keys, where blank means *clear* rather than "store
    /// an empty string". Removing the field lets the resolver fall back to the
    /// default vendor / vendor model; storing `""` would pin an unusable lane.
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
            "LLM_FORMATTING_PROVIDER" => settings.llm_formatting_provider = normalized,
            "LLM_FORMATTING_MODEL" => settings.llm_formatting_model = normalized,
            "LLM_ASSISTIVE_PROVIDER" => settings.llm_assistive_provider = normalized,
            "LLM_ASSISTIVE_MODEL" => settings.llm_assistive_model = normalized,
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
        let normalized_engine = (key == "CODESCRIBE_STT_ENGINE")
            .then(|| normalize_stt_engine(value))
            .flatten();
        if key == "CODESCRIBE_STT_ENGINE" && normalized_engine.is_none() {
            warn!("Refused retired or unknown STT engine selector: {value}");
            return;
        }
        let value = normalized_engine.as_deref().unwrap_or_else(|| value.trim());
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
        // Preserve every byte until the retired registry targets have live readers.
        // Old code dropped arbitrary legacy LLM values without any backup.
        super::repair::record(super::repair::inspect_env(&Self::env_path()));
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

    #[test]
    #[serial]
    fn no_keychain_entrypoints_never_attempt_credentials_including_stt_migration() {
        let _data = TestEnvGuard::unset("CODESCRIBE_DATA_DIR");
        let _env = TestEnvGuard::unset("CODESCRIBE_ENV_PATH");
        let _pack = TestEnvGuard::unset("CODESCRIBE_VOICE_LAB_SRC");
        let _retired = TestEnvGuard::unset("STT_API_KEY");
        let _bundle = super::super::keychain::test_support::install_bundle(&[]);
        for raw in [
            r#"{"schema_version":3}"#,
            r#"{"schema_version":3,"speech":{"engine":{"cloud_transcription_endpoint":"https://example.test/v1/audio/transcriptions"}}}"#,
        ] {
            for entry in 0..3 {
                let dir = TempDir::new().unwrap();
                set_env_for_test("CODESCRIBE_DATA_DIR", dir.path());
                fs::write(dir.path().join("settings.json"), raw).unwrap();
                let probe = super::super::keychain::CredentialAcquisitionProbe::forbid();
                match entry {
                    0 => {
                        let _ = Config::load_without_keychain();
                    }
                    1 => {
                        Config::load_runtime_snapshot_without_keychain().unwrap();
                    }
                    _ => {
                        Config::load_startup_runtime_snapshot(false);
                    }
                }
                assert!(probe.attempts().is_empty());
            }
        }
    }

    #[test]
    #[serial]
    fn no_keychain_first_import_survives_settings_save_until_authorized_commit() {
        let _data = TestEnvGuard::unset("CODESCRIBE_DATA_DIR");
        let _env = TestEnvGuard::unset("CODESCRIBE_ENV_PATH");
        let _pack = TestEnvGuard::unset("CODESCRIBE_VOICE_LAB_SRC");
        let _key = TestEnvGuard::unset("STT_FILE_API_KEY");
        let _retired = TestEnvGuard::unset("STT_API_KEY");
        let _auto_paste = TestEnvGuard::unset("AUTO_PASTE_ENABLED");
        let _roots = TestEnvGuard::unset("AGENT_WORKSPACE_ROOTS");
        let _bundle = super::super::keychain::test_support::install_bundle(&[]);
        let dir = TempDir::new().unwrap();
        set_env_for_test("CODESCRIBE_DATA_DIR", dir.path());
        let env = "STT_FILE_API_KEY=synthetic-import-key\nAUTO_PASTE_ENABLED=false\nAGENT_WORKSPACE_ROOTS=/tmp/synthetic-workspace\n";
        fs::write(dir.path().join(".env"), env).unwrap();
        {
            let probe = super::super::keychain::CredentialAcquisitionProbe::forbid();
            let snapshot = Config::load_runtime_snapshot_without_keychain().unwrap();
            assert_eq!(snapshot.user_settings().auto_paste_enabled, Some(false));
            assert_eq!(
                snapshot
                    .user_settings()
                    .agent_workspace_roots
                    .as_ref()
                    .unwrap(),
                &["/tmp/synthetic-workspace"]
            );
            assert!(UserSettings::settings_path().exists());
            let mut edited = UserSettings::load();
            assert_eq!(edited.auto_paste_enabled, Some(false));
            assert_eq!(edited.pending_env_key_imports.len(), 1);
            edited.show_dock_icon = Some(true);
            edited.save().unwrap();
            let persisted = fs::read_to_string(UserSettings::settings_path()).unwrap();
            assert!(!persisted.contains("synthetic-import-key"));
            assert_eq!(fs::read_to_string(dir.path().join(".env")).unwrap(), env);
            assert!(probe.attempts().is_empty());
        }
        // Unit-test secret writes use synthetic env, never the OS credential store.
        let snapshot = Config::load_runtime_snapshot().unwrap();
        assert_eq!(snapshot.user_settings().auto_paste_enabled, Some(false));
        assert_eq!(snapshot.user_settings().show_dock_icon, Some(true));
        assert!(snapshot.user_settings().pending_env_key_imports.is_empty());
        assert!(UserSettings::settings_path().exists());
        assert_eq!(
            std::env::var("STT_FILE_API_KEY").unwrap(),
            "synthetic-import-key"
        );
    }

    #[test]
    #[serial]
    fn public_config_reads_newly_acquired_stt_cache_and_preserves_explicit_env() {
        let _data = TestEnvGuard::unset("CODESCRIBE_DATA_DIR");
        let _env = TestEnvGuard::unset("CODESCRIBE_ENV_PATH");
        let _pack = TestEnvGuard::unset("CODESCRIBE_VOICE_LAB_SRC");
        let _file = TestEnvGuard::unset("STT_FILE_API_KEY");
        let _live = TestEnvGuard::unset("STT_LIVE_API_KEY");
        let _retired = TestEnvGuard::unset("STT_API_KEY");
        let dir = TempDir::new().unwrap();
        set_env_for_test("CODESCRIBE_DATA_DIR", dir.path());
        fs::write(dir.path().join("settings.json"), r#"{"schema_version":3}"#).unwrap();
        let _empty = super::super::keychain::test_support::install_bundle(&[]);
        let first = Config::load_without_keychain();
        assert!(first.stt_file_api_key.is_none());
        let _acquired = super::super::keychain::test_support::install_bundle(&[
            ("STT_FILE_API_KEY", "synthetic-file"),
            ("STT_LIVE_API_KEY", "synthetic-live"),
        ]);
        assert!(std::env::var_os("STT_FILE_API_KEY").is_none());
        let loaded = Config::load();
        assert_eq!(loaded.stt_file_api_key.as_deref(), Some("synthetic-file"));
        assert_eq!(loaded.stt_live_api_key.as_deref(), Some("synthetic-live"));
        assert!(std::env::var_os("STT_FILE_API_KEY").is_none());
        set_env_for_test("STT_FILE_API_KEY", "synthetic-explicit");
        let probe = super::super::keychain::CredentialAcquisitionProbe::forbid();
        let loaded = Config::load_without_keychain();
        assert_eq!(
            loaded.stt_file_api_key.as_deref(),
            Some("synthetic-explicit")
        );
        assert_eq!(loaded.stt_live_api_key.as_deref(), Some("synthetic-live"));
        assert!(probe.attempts().is_empty());
        let _retired_cache = super::super::keychain::test_support::install_bundle(&[(
            "STT_API_KEY",
            "synthetic-retired-cache",
        )]);
        let loaded = Config::load_without_keychain();
        assert_eq!(
            loaded.stt_file_api_key.as_deref(),
            Some("synthetic-explicit")
        );
        assert_eq!(
            loaded.stt_live_api_key.as_deref(),
            Some("synthetic-retired-cache")
        );
        assert!(probe.attempts().is_empty());
    }

    #[test]
    #[serial]
    fn bulk_setting_then_key_edit_cannot_reissue_buffered_import_intent() {
        let _data = TestEnvGuard::unset("CODESCRIBE_DATA_DIR");
        let _env = TestEnvGuard::unset("CODESCRIBE_ENV_PATH");
        let _key = TestEnvGuard::unset("STT_FILE_API_KEY");
        let dir = TempDir::new().unwrap();
        set_env_for_test("CODESCRIBE_DATA_DIR", dir.path());
        let source = dir.path().join(".env");
        fs::write(&source, "STT_FILE_API_KEY=synthetic-old-import\n").unwrap();
        let values = Config::parse_env_file(&source).unwrap();
        super::super::migrate::migrate_if_needed(Some(&values), false);
        assert_eq!(UserSettings::load().pending_env_key_imports.len(), 1);
        Config::default()
            .save_to_env_many(&[
                ("SHOW_DOCK_ICON", "true"),
                ("STT_FILE_API_KEY", "synthetic-user-edit"),
            ])
            .unwrap();
        let settings = UserSettings::load();
        assert!(settings.pending_env_key_imports.is_empty());
        assert_eq!(settings.show_dock_icon, Some(true));
        super::super::migrate::migrate_if_needed(None, true);
        assert_eq!(
            std::env::var("STT_FILE_API_KEY").as_deref(),
            Ok("synthetic-user-edit")
        );
    }

    /// Replay launch defects through the public loader using isolated on-disk tiers.
    #[test]
    #[serial]
    fn config_repair_launch_fixture_table() {
        let _data = TestEnvGuard::unset("CODESCRIBE_DATA_DIR");
        let _env = TestEnvGuard::unset("CODESCRIBE_ENV_PATH");
        let _pack = TestEnvGuard::unset("CODESCRIBE_VOICE_LAB_SRC");
        let _format = TestEnvGuard::unset("FORMATTING_LEVEL");
        let _armed = TestEnvGuard::unset(SILERO_FUSION_ENV);
        let cases = [
            (
                "zoom",
                r#"{"schema_version":3,"ui":{"chat_zoom":9}}"#,
                "",
                "FieldReset",
            ),
            (
                "truncated",
                r#"{"schema_version":3,"ui":"#,
                "",
                "FileRecreated",
            ),
            (
                "formatting",
                r#"{"schema_version":3,"speech":{"formatting":{"level":"bogus"}}}"#,
                "",
                "FieldReset",
            ),
            (
                "deprecated",
                r#"{"schema_version":3}"#,
                "WHISPER_SERVER_URL=https://example.test/asr\n",
                "PrecedenceNote",
            ),
            (
                "unknown",
                r#"{"schema_version":3}"#,
                "MY_PRIVATE_TOKEN=do-not-emit-this\n",
                "UnknownEnvKey",
            ),
            (
                "pack",
                r#"{"schema_version":3,"speech":{"engine":{"asr_mode":"cloud","cloud_transcription_endpoint":""}}}"#,
                "",
                "SeededFromPack",
            ),
            (
                "precedence",
                r#"{"schema_version":3,"speech":{"formatting":{"level":"max"}}}"#,
                "",
                "PrecedenceNote",
            ),
        ];
        for (name, settings, env, action) in cases {
            let dir = tempfile::tempdir().unwrap();
            set_env_for_test("CODESCRIBE_DATA_DIR", dir.path());
            remove_env_for_test("CODESCRIBE_VOICE_LAB_SRC");
            remove_env_for_test("FORMATTING_LEVEL");
            if name == "pack" {
                let pack = dir.path().join("examples/operator");
                fs::create_dir_all(pack.join("keys")).unwrap();
                fs::write(pack.join("settings.json"), r#"{"speech":{"engine":{"cloud_transcription_endpoint":"https://example.test/asr"}}}"#).unwrap();
                set_env_for_test("CODESCRIBE_VOICE_LAB_SRC", dir.path());
            }
            if name == "precedence" {
                set_env_for_test("FORMATTING_LEVEL", "off");
            }
            fs::write(dir.path().join("settings.json"), settings).unwrap();
            fs::write(dir.path().join(".env"), env).unwrap();
            let snapshot = Config::load_runtime_snapshot_without_keychain().unwrap();
            let receipt = snapshot.repair_receipt();
            assert!(receipt.unrepairable.is_empty(), "{name}: {receipt:?}");
            assert_eq!(receipt.actions.len(), 1, "{name}: {receipt:?}");
            let json = serde_json::to_string(receipt).unwrap();
            assert!(json.contains(action), "{name}: {json}");
            assert!(!json.contains("do-not-emit-this"));
            if name == "precedence" {
                assert_eq!(snapshot.formatting_policy(), FormattingPolicy::Off);
                assert_eq!(
                    snapshot.user_settings().formatting_level.as_deref(),
                    Some("max")
                );
                assert_eq!(
                    fs::read_to_string(dir.path().join("settings.json")).unwrap(),
                    settings
                );
            }
            assert_eq!(fs::read_to_string(dir.path().join(".env")).unwrap(), env);
        }
        let dir = tempfile::tempdir().unwrap();
        set_env_for_test("CODESCRIBE_DATA_DIR", dir.path());
        fs::write(dir.path().join("settings.json"), r#"{"schema_version":99}"#).unwrap();
        let refused = Config::load_startup_runtime_snapshot(false);
        assert_eq!(refused.repair_receipt().unrepairable.len(), 1);
        assert!(!refused.seal_lane_armed());

        let dir = tempfile::tempdir().unwrap();
        set_env_for_test("CODESCRIBE_DATA_DIR", dir.path());
        set_env_for_test("FORMATTING_LEVEL", "invalid-policy");
        let refused = Config::load_startup_runtime_snapshot(false);
        assert_eq!(refused.formatting_policy(), FormattingPolicy::Off);
        assert!(!refused.repair_receipt().unrepairable.is_empty());
        assert!(!refused.seal_lane_armed());
    }

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
        remove_env_for_test(SILERO_FUSION_ENV);
        tmp
    }

    /// Clear every process-env input that could hand a lane a credential,
    /// a provider, or a model: the vendor accounts, the retired aliases that
    /// still feed OpenAI, and the lane selectors.
    fn clear_llm_lane_env() -> Vec<TestEnvGuard> {
        vec![
            TestEnvGuard::unset("LLM_ASSISTIVE_PROVIDER"),
            TestEnvGuard::unset("LLM_ASSISTIVE_MODEL"),
            TestEnvGuard::unset("LLM_FORMATTING_PROVIDER"),
            TestEnvGuard::unset("LLM_FORMATTING_MODEL"),
            TestEnvGuard::unset("LLM_OPENAI_API_KEY"),
            TestEnvGuard::unset("LLM_XAI_API_KEY"),
            TestEnvGuard::unset("LLM_ANTHROPIC_API_KEY"),
            TestEnvGuard::unset("LLM_LIBRAXIS_API_KEY"),
            TestEnvGuard::unset("LLM_API_KEY"),
            TestEnvGuard::unset("LLM_FORMATTING_API_KEY"),
            TestEnvGuard::unset("LLM_ASSISTIVE_API_KEY"),
            TestEnvGuard::unset("LLM_ENDPOINT"),
            TestEnvGuard::unset("LLM_ASSISTIVE_ENDPOINT"),
            TestEnvGuard::unset("LLM_FORMATTING_ENDPOINT"),
            TestEnvGuard::unset(account_auth::OPENAI_ACCOUNT_TOKENS_ACCOUNT),
        ]
    }

    /// Seal lanes from the current settings.json + env without Keychain I/O.
    fn seal_lanes() -> RuntimeSettingsSnapshot {
        Config::load_runtime_snapshot_without_keychain().expect("seal runtime settings")
    }

    /// One Custom row at `endpoint` on the Responses wire, saved to settings.json
    /// with the assistive lane pointing at it.
    fn save_custom_assistive_row(endpoint: &str, model: Option<&str>) -> String {
        let mut settings = UserSettings::load();
        let row = crate::llm::provider::CustomProvider::new(
            "My Box",
            WireFamily::OpenAiResponses,
            endpoint,
        )
        .expect("valid custom row");
        let id = row.id.clone();
        settings.add_custom_provider(row).expect("add custom row");
        settings.llm_assistive_provider = Some(format!("custom:{id}"));
        settings.llm_assistive_model = model.map(str::to_string);
        settings.save().expect("save settings");
        id
    }

    /// Effect witness for the 2026-09-07 refusal on dragon: sign-in tokens
    /// live only in the Keychain bundle — never in process env — and the sealed
    /// assistive lane must resolve to account auth and be available with no
    /// API key anywhere. The Settings label already read the bundle; the lane
    /// read env and sealed `account_auth=false` for every signed-in operator.
    #[test]
    #[serial]
    fn assistive_lane_seals_account_auth_from_bundle_tokens_without_env() {
        let _tmp = setup_isolated_data_dir();
        let _env = clear_llm_lane_env();
        let tokens = account_auth::AccountTokens::new(
            ProviderKind::OpenAiResponses,
            "access".to_string(),
            Some("refresh".to_string()),
            None,
            None,
            Some(3600),
        );
        let _bundle = super::super::keychain::test_support::install_bundle(&[(
            account_auth::OPENAI_ACCOUNT_TOKENS_ACCOUNT,
            &serde_json::to_string(&tokens).expect("serialize tokens"),
        )]);

        let snapshot = seal_lanes();
        let lane = snapshot.llm_lanes().assistive();
        assert_eq!(lane.vendor(), Some(ProviderKind::OpenAiResponses));
        assert_eq!(lane.endpoint(), "https://api.openai.com/v1/responses");
        assert!(
            lane.credential().api_key().is_none(),
            "no API key may take part in this witness"
        );
        assert!(
            lane.credential().account_auth(),
            "bundle-only sign-in must seal as account auth"
        );
        assert!(lane.available(), "signed-in lane must be available");
        assert!(lane.request_available());
        assert_eq!(lane.unavailable_reason(), None);
    }

    /// Negative control for the witness above: same env, a bundle without a
    /// token record, no API key — the lane must refuse and say why.
    #[test]
    #[serial]
    fn assistive_lane_without_key_or_bundle_tokens_refuses() {
        let _tmp = setup_isolated_data_dir();
        let _env = clear_llm_lane_env();
        let _bundle = super::super::keychain::test_support::install_bundle(&[]);

        let snapshot = seal_lanes();
        let lane = snapshot.llm_lanes().assistive();
        assert!(!lane.credential().account_auth());
        assert!(!lane.available());
        assert!(
            lane.unavailable_reason()
                .is_some_and(|reason| reason.contains("signed-in provider account")),
            "refusal must name the missing account, got {:?}",
            lane.unavailable_reason()
        );
    }

    /// A corrupt token record is "not signed in" at seal time, not a lane that
    /// fails at its first request.
    #[test]
    #[serial]
    fn assistive_lane_treats_corrupt_bundle_tokens_as_not_signed_in() {
        let _tmp = setup_isolated_data_dir();
        let _env = clear_llm_lane_env();
        let _bundle = super::super::keychain::test_support::install_bundle(&[(
            account_auth::OPENAI_ACCOUNT_TOKENS_ACCOUNT,
            "not-a-token-record",
        )]);

        let snapshot = seal_lanes();
        let lane = snapshot.llm_lanes().assistive();
        assert!(!lane.credential().account_auth());
        assert!(!lane.available());
    }

    /// Zero-state seal: both lanes on the OpenAI vendor with the vendor's own
    /// models, keyed on `LLM_OPENAI_API_KEY`, and nothing LLM-related planted
    /// in process env (the old bootstrap seeded six variables).
    #[test]
    #[serial]
    fn zero_state_lanes_seal_openai_vendor_defaults_without_env_seeding() {
        let _tmp = setup_isolated_data_dir();
        let _env = clear_llm_lane_env();
        let _bundle = super::super::keychain::test_support::install_bundle(&[]);
        let _config = Config::load();
        let snapshot = seal_lanes();
        let formatting = snapshot.llm_lanes().formatting();
        let assistive = snapshot.llm_lanes().assistive();
        assert_eq!(formatting.provider(), &ProviderRef::default());
        assert_eq!(formatting.endpoint(), "https://api.openai.com/v1/responses");
        assert_eq!(
            formatting.model(),
            crate::llm::provider::DEFAULT_FORMATTING_MODEL
        );
        assert_eq!(
            assistive.model(),
            crate::llm::provider::DEFAULT_ASSISTIVE_MODEL
        );
        assert_eq!(formatting.credential().key_account(), "LLM_OPENAI_API_KEY");
        assert_eq!(assistive.credential().key_account(), "LLM_OPENAI_API_KEY");
        assert_eq!(formatting.provider_display_name(), "OpenAI (Responses)");
        assert!(!formatting.available());
        for planted in LEGACY_LLM_ENDPOINT_ENV
            .iter()
            .chain(["LLM_FORMATTING_PROVIDER", "LLM_ASSISTIVE_PROVIDER"].iter())
        {
            assert!(
                std::env::var_os(planted).is_none(),
                "loader must not seed {planted} into process env"
            );
        }
    }

    /// Effect witness (§F.4): a vendor endpoint is pinned in code. The retired
    /// endpoint env names change nothing (one warning), and the settings file
    /// has no field that could move a vendor lane off its host.
    #[test]
    #[serial]
    fn vendor_endpoint_cannot_be_overridden() {
        let _tmp = setup_isolated_data_dir();
        let _env = clear_llm_lane_env();
        let _bundle = super::super::keychain::test_support::install_bundle(&[]);
        let _legacy_endpoint = TestEnvGuard::unset("LLM_ASSISTIVE_ENDPOINT");
        set_env_for_test(
            "LLM_ASSISTIVE_ENDPOINT",
            "https://proxy.example/v1/responses",
        );
        set_env_for_test("LLM_ENDPOINT", "https://proxy.example/v1/responses");
        set_env_for_test("LLM_ASSISTIVE_PROVIDER", "xai-responses");
        let snapshot = seal_lanes();
        let lane = snapshot.llm_lanes().assistive();
        assert_eq!(lane.vendor(), Some(ProviderKind::XaiResponses));
        assert_eq!(lane.endpoint(), "https://api.x.ai/v1/responses");
        assert_eq!(lane.credential().key_account(), "LLM_XAI_API_KEY");
    }

    /// Every retired endpoint/model env name is ignored — the lane seals on the
    /// vendor's pinned endpoint and seed model — and each one is warned about
    /// once per process (the warn ledger holds the name afterwards).
    #[test]
    #[serial]
    fn legacy_endpoint_env_is_ignored_with_warning() {
        let _tmp = setup_isolated_data_dir();
        let _env = clear_llm_lane_env();
        let _bundle = super::super::keychain::test_support::install_bundle(&[]);
        let _legacy_model = TestEnvGuard::unset("LLM_MODEL");
        set_env_for_test("LLM_FORMATTING_ENDPOINT", "https://proxy.example/v1");
        set_env_for_test("LLM_ENDPOINT", "https://proxy.example/v1");
        set_env_for_test("LLM_MODEL", "stale-main-model");
        let snapshot = seal_lanes();
        let lane = snapshot.llm_lanes().formatting();
        assert_eq!(lane.endpoint(), ProviderKind::OpenAiResponses.endpoint());
        assert_eq!(lane.model(), crate::llm::provider::DEFAULT_FORMATTING_MODEL);
        let warned = LEGACY_LLM_ENV_WARNED
            .get_or_init(|| Mutex::new(HashSet::new()))
            .lock()
            .expect("warn ledger");
        for planted in ["LLM_FORMATTING_ENDPOINT", "LLM_ENDPOINT", "LLM_MODEL"] {
            assert!(warned.contains(planted), "{planted} was not warned about");
        }
    }

    /// Effect witness: the formatting lane on xAI keeps a Grok model chosen in
    /// settings — the old `owns_model` prefix filter that threw it away is gone.
    #[test]
    #[serial]
    fn formatting_lane_on_xai_keeps_grok_model() {
        let _tmp = setup_isolated_data_dir();
        let _env = clear_llm_lane_env();
        let _bundle = super::super::keychain::test_support::install_bundle(&[(
            "LLM_XAI_API_KEY",
            "xai-secret",
        )]);
        let mut settings = UserSettings::load();
        settings.llm_formatting_provider = Some("xai-responses".to_string());
        settings.llm_formatting_model = Some("grok-4.5-fast".to_string());
        settings.save().expect("save settings");
        let snapshot = seal_lanes();
        let lane = snapshot.llm_lanes().formatting();
        assert_eq!(lane.vendor(), Some(ProviderKind::XaiResponses));
        assert_eq!(lane.model(), "grok-4.5-fast");
        assert_eq!(lane.credential().api_key(), Some("xai-secret"));
        assert!(lane.available());
        // A vendor model from settings is never filtered, whatever its prefix.
        settings.llm_formatting_model = Some("claude-sonnet-5".to_string());
        settings.save().expect("save settings");
        assert_eq!(
            seal_lanes().llm_lanes().formatting().model(),
            "claude-sonnet-5"
        );
    }

    /// Effect witness: a Custom provider without a key is available (key not
    /// required), reads its key only from the bundle, and takes its model
    /// from settings — with no model it seals unavailable and says why.
    #[test]
    #[serial]
    fn custom_provider_without_key_is_available() {
        let _tmp = setup_isolated_data_dir();
        let _env = clear_llm_lane_env();
        let _bundle = super::super::keychain::test_support::install_bundle(&[]);
        let id = save_custom_assistive_row("http://my.local:8080/v1", Some("qwen"));
        assert_eq!(id, "my-box");
        let snapshot = seal_lanes();
        let lane = snapshot.llm_lanes().assistive();
        assert_eq!(lane.provider(), &ProviderRef::Custom("my-box".to_string()));
        assert_eq!(lane.vendor(), None);
        assert_eq!(lane.provider_display_name(), "My Box");
        assert_eq!(lane.endpoint(), "http://my.local:8080/v1/responses");
        assert_eq!(lane.model(), "qwen");
        assert_eq!(lane.credential().key_account(), "LLM_CUSTOM_MY_BOX_API_KEY");
        assert_eq!(lane.credential().api_key(), None);
        assert!(lane.available());
        assert!(lane.request_available());
        assert!(lane.supports_vision("qwen"));

        let _key_bundle = super::super::keychain::test_support::install_bundle(&[(
            "LLM_CUSTOM_MY_BOX_API_KEY",
            "box-secret",
        )]);
        assert_eq!(
            seal_lanes().llm_lanes().assistive().credential().api_key(),
            Some("box-secret")
        );

        let mut settings = UserSettings::load();
        settings.llm_assistive_model = None;
        settings.save().expect("save settings");
        let snapshot = seal_lanes();
        let lane = snapshot.llm_lanes().assistive();
        assert!(!lane.available());
        assert_eq!(
            lane.unavailable_reason(),
            Some("no model selected for provider My Box")
        );
    }

    /// A lane pointing at a Custom id that no longer exists falls back to the
    /// default vendor and names the missing row.
    #[test]
    #[serial]
    fn missing_custom_row_falls_back_to_default_vendor_with_reason() {
        let _tmp = setup_isolated_data_dir();
        let _env = clear_llm_lane_env();
        let _bundle = super::super::keychain::test_support::install_bundle(&[]);
        let mut settings = UserSettings::load();
        settings.llm_assistive_provider = Some("custom:gone".to_string());
        settings.save().expect("save settings");
        let snapshot = seal_lanes();
        let lane = snapshot.llm_lanes().assistive();
        assert_eq!(lane.provider(), &ProviderRef::default());
        assert!(!lane.available());
        assert_eq!(
            lane.unavailable_reason(),
            Some("custom provider `gone` no longer exists")
        );
    }

    /// REMOVE AFTER 2026-10-15: the retired `LLM_API_KEY` env name still feeds
    /// the OpenAI account (only) with one warning, and never a vendor with its
    /// own account.
    #[test]
    #[serial]
    fn legacy_openai_key_alias_warns() {
        let _tmp = setup_isolated_data_dir();
        let _env = clear_llm_lane_env();
        let _bundle = super::super::keychain::test_support::install_bundle(&[]);
        set_env_for_test("LLM_API_KEY", "legacy-secret");
        let snapshot = seal_lanes();
        let lane = snapshot.llm_lanes().formatting();
        assert_eq!(lane.credential().key_account(), "LLM_OPENAI_API_KEY");
        assert_eq!(lane.credential().api_key(), Some("legacy-secret"));
        assert!(lane.available());
        assert!(
            LEGACY_LLM_ENV_WARNED
                .get_or_init(|| Mutex::new(HashSet::new()))
                .lock()
                .expect("warn ledger")
                .contains("LLM_API_KEY")
        );

        set_env_for_test("LLM_ASSISTIVE_PROVIDER", "anthropic-messages");
        let snapshot = seal_lanes();
        let lane = snapshot.llm_lanes().assistive();
        assert_eq!(lane.credential().key_account(), "LLM_ANTHROPIC_API_KEY");
        assert_eq!(lane.credential().api_key(), None);
        assert!(!lane.available());
    }

    /// A custom-provider key saved through the config write path lands in
    /// the Keychain corridor, not in settings.json or process env.
    #[test]
    #[serial]
    fn custom_key_write_routes_to_keychain_not_settings() {
        let _tmp = setup_isolated_data_dir();
        let _bundle = super::super::keychain::test_support::install_bundle(&[]);
        Config::default()
            .save_to_env("LLM_CUSTOM_MY_BOX_API_KEY", "box-secret")
            .expect("save custom key");
        assert!(std::env::var_os("LLM_CUSTOM_MY_BOX_API_KEY").is_none());
        assert!(!UserSettings::settings_path().exists());
        assert!(!Config::env_path().exists());
        assert!(super::super::keychain::key_present(
            "LLM_CUSTOM_MY_BOX_API_KEY"
        ));
    }

    #[test]
    #[serial]
    fn seal_lane_missing_settings_defaults_armed_from_product_settings() {
        let _tmp = setup_isolated_data_dir();
        let snapshot =
            Config::load_runtime_snapshot_without_keychain().expect("seal runtime settings");

        assert!(snapshot.seal_lane_armed());
        assert!(snapshot.seal_lane_setting_armed());
        assert_eq!(snapshot.seal_lane_source().as_str(), "settings");
        assert!(!UserSettings::settings_path().exists());
    }

    #[test]
    #[serial]
    fn seal_lane_settings_only_supports_both_product_values() {
        let _tmp = setup_isolated_data_dir();
        let mut settings = UserSettings {
            seal_lane_armed: Some(false),
            ..Default::default()
        };
        settings.save().expect("save disarmed setting");
        let disarmed =
            Config::load_runtime_snapshot_without_keychain().expect("seal disarmed snapshot");
        assert!(!disarmed.seal_lane_armed());
        assert!(!disarmed.seal_lane_setting_armed());
        assert_eq!(disarmed.seal_lane_source().as_str(), "settings");

        settings.seal_lane_armed = Some(true);
        settings.save().expect("save armed setting");
        let armed = Config::load_runtime_snapshot_without_keychain().expect("seal armed snapshot");
        assert!(armed.seal_lane_armed());
        assert!(armed.seal_lane_setting_armed());
        assert_eq!(armed.seal_lane_source().as_str(), "settings");
        assert_ne!(disarmed.digest(), armed.digest());
    }

    #[test]
    #[serial]
    fn seal_lane_process_env_override_wins_in_both_directions() {
        let _tmp = setup_isolated_data_dir();
        let _override = TestEnvGuard::unset(SILERO_FUSION_ENV);
        let mut settings = UserSettings {
            seal_lane_armed: Some(true),
            ..Default::default()
        };
        settings.save().expect("save armed setting");

        set_env_for_test(SILERO_FUSION_ENV, "0");
        let forced_off =
            Config::load_runtime_snapshot_without_keychain().expect("seal forced-off snapshot");
        assert!(!forced_off.seal_lane_armed());
        assert!(forced_off.seal_lane_setting_armed());
        assert_eq!(forced_off.seal_lane_source().as_str(), "env_override");

        settings.seal_lane_armed = Some(false);
        settings.save().expect("save disarmed setting");
        set_env_for_test(SILERO_FUSION_ENV, "1");
        let forced_on =
            Config::load_runtime_snapshot_without_keychain().expect("seal forced-on snapshot");
        assert!(forced_on.seal_lane_armed());
        assert!(!forced_on.seal_lane_setting_armed());
        assert_eq!(forced_on.seal_lane_source().as_str(), "env_override");
        assert_ne!(forced_off.digest(), forced_on.digest());
    }

    #[test]
    #[serial]
    fn seal_lane_env_file_override_survives_canonical_settings_write() {
        let _tmp = setup_isolated_data_dir();
        let _override = TestEnvGuard::unset(SILERO_FUSION_ENV);
        fs::write(Config::env_path(), format!("{SILERO_FUSION_ENV}=0\n"))
            .expect("write power-user env override");

        Config::default()
            .save_to_env(SILERO_FUSION_ENV, "1")
            .expect("persist product setting through canonical writer");
        let persisted = UserSettings::load();
        assert_eq!(persisted.seal_lane_armed, Some(true));
        assert_eq!(
            fs::read_to_string(Config::env_path()).expect("read untouched env override"),
            format!("{SILERO_FUSION_ENV}=0\n")
        );

        let snapshot =
            Config::load_runtime_snapshot_without_keychain().expect("seal overridden snapshot");
        assert!(!snapshot.seal_lane_armed());
        assert!(snapshot.seal_lane_setting_armed());
        assert_eq!(snapshot.seal_lane_source().as_str(), "env_override");
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

    /// Every LLM lane write key as `(key, sample value, JSON pointer)`.
    fn llm_write_key_cases() -> &'static [(&'static str, &'static str, &'static str)] {
        &[
            (
                "LLM_FORMATTING_PROVIDER",
                "xai-responses",
                "/speech/formatting/llm_provider",
            ),
            (
                "LLM_FORMATTING_MODEL",
                "gpt-formatting-test",
                "/speech/formatting/llm_model",
            ),
            (
                "LLM_ASSISTIVE_PROVIDER",
                "anthropic-messages",
                "/speech/assistive/provider",
            ),
            (
                "LLM_ASSISTIVE_MODEL",
                "gpt-assistive-test",
                "/speech/assistive/llm_model",
            ),
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

    /// The loaded value of one LLM lane key.
    fn lane_setting<'a>(settings: &'a UserSettings, key: &str) -> Option<&'a str> {
        match key {
            "LLM_FORMATTING_PROVIDER" => settings.llm_formatting_provider.as_deref(),
            "LLM_FORMATTING_MODEL" => settings.llm_formatting_model.as_deref(),
            "LLM_ASSISTIVE_PROVIDER" => settings.llm_assistive_provider.as_deref(),
            "LLM_ASSISTIVE_MODEL" => settings.llm_assistive_model.as_deref(),
            _ => None,
        }
    }

    /// Promoted save must land in settings.json and must not set process env.
    #[test]
    #[serial]
    fn save_to_env_persists_promoted_setting_without_process_env_mutation() {
        let _tmp = setup_isolated_data_dir();
        let _model = TestEnvGuard::unset("LLM_FORMATTING_MODEL");

        Config::default()
            .save_to_env("LLM_FORMATTING_MODEL", "runtime-model")
            .expect("save setting");

        assert!(std::env::var("LLM_FORMATTING_MODEL").is_err());
        assert_eq!(
            UserSettings::load().llm_formatting_model.as_deref(),
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
        let _env = clear_llm_lane_env();
        let config = Config::default();

        config
            .save_to_env("LLM_ASSISTIVE_MODEL", "gpt-custom-pick")
            .expect("set assistive model override");

        let set_json: serde_json::Value = serde_json::from_str(
            &fs::read_to_string(UserSettings::settings_path()).expect("read settings after set"),
        )
        .expect("parse settings after set");
        assert_eq!(
            set_json
                .pointer("/speech/assistive/llm_model")
                .and_then(serde_json::Value::as_str),
            Some("gpt-custom-pick")
        );

        config
            .save_to_env("LLM_ASSISTIVE_MODEL", "")
            .expect("reset assistive model override");

        let reset_json: serde_json::Value = serde_json::from_str(
            &fs::read_to_string(UserSettings::settings_path()).expect("read settings after reset"),
        )
        .expect("parse settings after reset");
        assert!(
            reset_json.pointer("/speech/assistive/llm_model").is_none(),
            "reset must remove the override path, got {reset_json}"
        );
        assert_eq!(UserSettings::load().llm_assistive_model, None);
        let runtime_settings = Config::load_runtime_snapshot().expect("runtime settings seal");
        assert_eq!(
            runtime_settings.llm_lanes().assistive().model(),
            crate::llm::provider::DEFAULT_ASSISTIVE_MODEL
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
            &ProviderRef::default()
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
        let _env = clear_llm_lane_env();
        let config = Config::default();

        let set_entries: Vec<(&str, &str)> = llm_write_key_cases()
            .iter()
            .map(|(key, value, _)| (*key, *value))
            .collect();
        config
            .save_to_env_many(&set_entries)
            .expect("set optional LLM overrides");
        let settings = UserSettings::load();
        for (key, value, _) in llm_write_key_cases() {
            assert_eq!(
                lane_setting(&settings, key),
                Some(*value),
                "{key} not stored"
            );
        }

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
            assert!(
                reset_json.pointer(pointer).is_none(),
                "batch reset must remove {key} at {pointer}, got {reset_json}"
            );
            assert_eq!(lane_setting(&settings, key), None, "{key} must be unset");
        }
        let runtime_settings = Config::load_runtime_snapshot().expect("runtime settings seal");
        assert_eq!(
            runtime_settings.llm_lanes().assistive().endpoint(),
            ProviderKind::OpenAiResponses.endpoint()
        );
    }

    /// Batch promoted settings persist without mutating process env or creating .env.
    #[test]
    #[serial]
    fn save_to_env_many_persists_batch_without_process_env_mutation() {
        let _tmp = setup_isolated_data_dir();
        let _model = TestEnvGuard::unset("LLM_ASSISTIVE_MODEL");
        let _workspace_roots = TestEnvGuard::unset("AGENT_WORKSPACE_ROOTS");

        Config::default()
            .save_to_env_many(&[
                ("LLM_ASSISTIVE_MODEL", "batch-model"),
                ("AGENT_WORKSPACE_ROOTS", "/tmp/a:/tmp/b"),
            ])
            .expect("save settings batch");

        assert!(std::env::var("LLM_ASSISTIVE_MODEL").is_err());
        assert!(std::env::var("AGENT_WORKSPACE_ROOTS").is_err());
        assert_eq!(
            UserSettings::load().llm_assistive_model.as_deref(),
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

    /// `whisper_context_window_sec` round-trips through settings.json.
    /// A process env value wins over the file, and both land on the 0.5 step.
    #[test]
    #[serial]
    fn whisper_context_window_round_trips_and_env_wins() {
        let _tmp = setup_isolated_data_dir();
        let previous = std::env::var("WHISPER_CONTEXT_WINDOW_SEC").ok();
        remove_env_for_test("WHISPER_CONTEXT_WINDOW_SEC");

        Config::default()
            .save_to_env("WHISPER_CONTEXT_WINDOW_SEC", "6.2")
            .expect("persist whisper context window");
        let stored = super::super::settings::UserSettings::load();
        assert_eq!(stored.whisper_context_window_sec, Some(6.0));
        let from_file = Config::load();
        assert!(
            (from_file.whisper_context_window_sec - 6.0).abs() < f32::EPSILON,
            "settings.json value {}, expected 6.0",
            from_file.whisper_context_window_sec
        );

        unsafe { std::env::set_var("WHISPER_CONTEXT_WINDOW_SEC", "2.5") };
        let from_env = Config::load();
        assert!(
            (from_env.whisper_context_window_sec - 2.5).abs() < f32::EPSILON,
            "env override {}, expected 2.5",
            from_env.whisper_context_window_sec
        );

        restore_env_for_test("WHISPER_CONTEXT_WINDOW_SEC", previous);
    }

    #[test]
    #[serial]
    fn light_plus_sentence_pause_round_trips_and_env_wins() {
        let _tmp = setup_isolated_data_dir();
        let previous = std::env::var("LIGHT_PLUS_SENTENCE_PAUSE_SEC").ok();
        remove_env_for_test("LIGHT_PLUS_SENTENCE_PAUSE_SEC");

        Config::default()
            .save_to_env("LIGHT_PLUS_SENTENCE_PAUSE_SEC", "1.2")
            .expect("persist Light+ sentence pause");
        let stored = super::super::settings::UserSettings::load();
        assert_eq!(stored.light_plus_sentence_pause_sec, Some(1.2));
        assert!((Config::load().light_plus_sentence_pause_sec - 1.2).abs() < f32::EPSILON);

        unsafe { std::env::set_var("LIGHT_PLUS_SENTENCE_PAUSE_SEC", "0.5") };
        assert!((Config::load().light_plus_sentence_pause_sec - 0.5).abs() < f32::EPSILON);
        restore_env_for_test("LIGHT_PLUS_SENTENCE_PAUSE_SEC", previous);
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

    /// Non-promoted env-managed keys (e.g. STT_FILE_API_KEY) still load from optional .env.
    #[test]
    #[serial]
    fn test_load_still_honors_env_managed_values_from_optional_env_file() {
        let _tmp = setup_isolated_data_dir();

        let env_path = Config::env_path();
        fs::create_dir_all(env_path.parent().expect("env dir")).expect("create env dir");
        fs::write(&env_path, "STT_FILE_API_KEY=test-from-env-file\n").expect("write .env");

        let config = Config::load();
        assert_eq!(
            config.stt_file_api_key.as_deref(),
            Some("test-from-env-file")
        );
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

/// One local producer policy; no inference or environment reads in the session.
fn resolve_local_tail_patch(
    phase: Option<&str>,
) -> crate::asr_session::recorder::LocalTailPatchDisposition {
    use crate::asr_session::recorder::LocalTailPatchDisposition as D;
    match phase.map(str::trim).map(str::to_ascii_lowercase).as_deref() {
        None | Some("") => D::ArmedDefault,
        Some("phase1" | "1") => D::ArmedPhase(1),
        Some("off" | "0" | "false" | "no") => D::DegradedExplicitOff,
        Some(_) => D::DegradedInvalidOverride,
    }
}

#[cfg(test)]
mod local_tail_decision_tests {
    use super::*;
    #[test]
    fn recording_start_local_tail_policy_is_explicit() {
        use crate::asr_session::recorder::LocalTailPatchDisposition as D;
        assert_eq!(resolve_local_tail_patch(Some("phase1")), D::ArmedPhase(1));
        assert_eq!(resolve_local_tail_patch(None), D::ArmedDefault);
        assert_eq!(
            resolve_local_tail_patch(Some("off")),
            D::DegradedExplicitOff
        );
        assert_eq!(
            resolve_local_tail_patch(Some("phase2")),
            D::DegradedInvalidOverride
        );
    }
}

#[cfg(test)]
mod captured_startup_tests {
    use super::super::repair::{ConfigUnrepairable, RepairAction};
    use super::*;

    fn inputs() -> CapturedRuntimeInputs {
        let mut input =
            CapturedRuntimeInputs::defaults_at(PathBuf::from("/fixture/one"), 1_700_000_000_000);
        input.user_settings.formatting_level = Some("correction".into());
        input.settings_bytes = Some(br#"{"formatting_level":"correction"}"#.to_vec());
        input
            .overrides
            .insert("FORMATTING_LEVEL".into(), Ok("smart".into()));
        input
            .overrides
            .insert(SILERO_FUSION_ENV.into(), Ok("true".into()));
        input
            .overrides
            .insert(AI_MAX_RETRIES_ENV.into(), Ok("7".into()));
        input.env_overlay_keys = vec!["BEEP_ON_START".into(), "BEEP_ON_START".into()];
        input.prompts.smart.content = "fixture system prompt".into();
        input.prompts.smart.tuning = Some("  fixture tuning  ".into());
        input.credentials.insert(
            ProviderKind::OpenAiResponses.api_key_account().into(),
            CapturedLaneCredential {
                api_key: Some("fixture-credential-a".into()),
                signed_in: true,
            },
        );
        input
    }

    #[test]
    fn captured_adapter_and_explicit_path_seal_identical_generation_without_acquisition() {
        let probe = StartupAcquisitionProbe::forbid();
        let input = inputs();
        let captures = std::cell::Cell::new(0);
        // Replay the facts at the production capture handoff. This is synthetic
        // source evidence, not a live-host capture or semantic-validation claim.
        let replay = Config::resolve_runtime_snapshot_with_capture(|| {
            captures.set(captures.get() + 1);
            input.clone()
        });
        let explicit = Config::runtime_snapshot_from_captured(input);
        assert_eq!(captures.get(), 1);
        assert_eq!(replay.digest(), explicit.digest());
        assert_eq!(replay.provenance(), explicit.provenance());
        assert_eq!(replay.provenance().loaded_at_unix_ms, 1_700_000_000_000);
        assert_eq!(replay.user_settings(), explicit.user_settings());
        assert_eq!(
            replay.llm_lanes().digest_material(),
            explicit.llm_lanes().digest_material()
        );
        assert_eq!(
            replay.ai_execution().digest_material(),
            explicit.ai_execution().digest_material()
        );
        assert_eq!(
            replay.energy_calibration_status(),
            explicit.energy_calibration_status()
        );
        assert_eq!(replay.repair_receipt(), explicit.repair_receipt());
        assert_eq!(explicit.formatting_policy(), FormattingPolicy::Smart);
        assert_eq!(explicit.ai_execution().formatter().max_retries(), 7);
        assert_eq!(
            explicit
                .ai_execution()
                .formatter()
                .formatting_prompt()
                .unwrap()
                .composed_content(),
            "fixture system prompt\n\nfixture tuning"
        );
        assert!(
            explicit
                .repair_receipt()
                .actions
                .contains(&RepairAction::PrecedenceNote {
                    key: "FORMATTING_LEVEL".into()
                })
        );
        assert!(probe.attempts().is_empty());
    }

    #[test]
    fn captured_prompts_and_repairs_do_not_contaminate_next_generation() {
        let probe = StartupAcquisitionProbe::forbid();
        let original = inputs();
        let first = Config::runtime_snapshot_from_captured(original.clone());
        let mut changed = original.clone();
        changed.prompts.smart.content = "another fixture".into();
        let prompt_changed = Config::runtime_snapshot_from_captured(changed);
        assert_ne!(first.digest(), prompt_changed.digest());
        let mut refused = original.clone();
        refused
            .repair_receipt
            .unrepairable
            .push(ConfigUnrepairable {
                path: PathBuf::from("/fixture/refused/settings.json"),
                reason: "fixture refusal".into(),
            });
        let refused = Config::runtime_snapshot_from_captured(refused);
        assert!(!refused.seal_lane_armed());
        assert_ne!(first.digest(), refused.digest());
        let again = Config::runtime_snapshot_from_captured(original);
        assert_eq!(first.digest(), again.digest());
        assert!(again.repair_receipt().unrepairable.is_empty());
        assert!(probe.attempts().is_empty());
    }

    #[test]
    fn captured_secret_bytes_are_redacted_from_generation_digest() {
        let probe = StartupAcquisitionProbe::forbid();
        let mut first = inputs();
        first.values.stt_file_api_key = Some("fixture-stt-a".into());
        let mut second = first.clone();
        second.values.stt_file_api_key = Some("fixture-stt-b".into());
        second
            .credentials
            .get_mut(ProviderKind::OpenAiResponses.api_key_account())
            .unwrap()
            .api_key = Some("fixture-credential-b".into());
        let first = Config::runtime_snapshot_from_captured(first);
        let second = Config::runtime_snapshot_from_captured(second);
        assert_eq!(first.digest(), second.digest());
        let diagnostic = format!("{:?} {:?}", first.llm_lanes(), first.ai_execution());
        assert!(!diagnostic.contains("fixture-credential-a"));
        assert!(!diagnostic.contains("fixture system prompt"));
        assert!(probe.attempts().is_empty());
    }

    #[test]
    fn explicit_invalid_overrides_keep_fail_closed_semantics() {
        let probe = StartupAcquisitionProbe::forbid();
        let mut invalid = inputs();
        invalid
            .overrides
            .insert("FORMATTING_LEVEL".into(), Ok("not-a-policy".into()));
        invalid.overrides.insert(
            crate::stt::tail_provider::STT_TAIL_PROVIDER_ENV.into(),
            Ok("not-a-provider".into()),
        );
        let snapshot = Config::runtime_snapshot_from_captured(invalid.clone());
        invalid.repair_receipt = snapshot.repair_receipt().clone();
        let repeated = Config::runtime_snapshot_from_captured(invalid);
        assert_eq!(snapshot.digest(), repeated.digest());
        assert_eq!(snapshot.repair_receipt(), repeated.repair_receipt());
        assert_eq!(snapshot.formatting_policy(), FormattingPolicy::Off);
        assert!(
            snapshot
                .ai_execution()
                .formatter()
                .formatting_prompt()
                .is_none()
        );
        assert!(!snapshot.seal_lane_armed());
        assert!(snapshot.tail_provider().is_none());
        assert_eq!(
            snapshot.repair_receipt().unrepairable[0].path,
            PathBuf::from("/fixture/one/settings.json")
        );
        assert!(matches!(
            snapshot.energy_calibration_status(),
            super::super::energy_calibration::EnergyCalibrationStatus::Missing { .. }
        ));
        assert!(probe.attempts().is_empty());
    }

    #[test]
    fn normal_startup_reaches_acquisition_adapter_before_any_host_access() {
        for populate in [false, true] {
            let probe = StartupAcquisitionProbe::forbid();
            let result =
                std::panic::catch_unwind(|| Config::load_startup_runtime_snapshot(populate));
            assert!(result.is_err());
            assert_eq!(probe.attempts(), ["settings capture"]);
        }
    }

    #[test]
    fn acquisition_tripwires_cover_lower_level_sources() {
        let sources: [(&str, fn()); 8] = [
            ("config files/env/keychain", || {
                let _ = Config::load_without_keychain();
            }),
            ("runtime env/cache", || {
                let _ = Config::config_runtime_env_var("FORMATTING_LEVEL");
            }),
            ("credential cache", || {
                let _ = Config::runtime_lane_api_key("LLM_OPENAI_API_KEY");
            }),
            ("account cache", || {
                let _ = Config::signed_in_provider_account("fixture");
            }),
            ("user settings file", || {
                let _ = UserSettings::load();
            }),
            ("settings path/repair registry", || {
                let _ = UserSettings::settings_path();
            }),
            ("prompt file", || {
                let _ = super::super::prompts::prompt_snapshot(
                    super::super::prompts::PromptKind::Assistive,
                );
            }),
            ("calibration file", || {
                let _ = SealedEnergyCalibration::load(Path::new("/fixture/calibration.json"));
            }),
        ];
        for (expected, acquire) in sources {
            let probe = StartupAcquisitionProbe::forbid();
            assert!(std::panic::catch_unwind(acquire).is_err());
            assert_eq!(probe.attempts(), [expected]);
        }
    }
}
