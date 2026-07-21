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
use std::collections::{HashMap, HashSet};
use std::env::VarError;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};
use tracing::{info, warn};

use super::defaults::{
    default_assistive_model, default_assistive_provider, default_formatting_model,
    default_formatting_provider, default_llm_endpoint, default_llm_model,
};
use super::settings::{
    DEFAULT_AGENT_WORKSPACE_ROOT, FormattingPolicy, normalize_agent_workspace_roots,
    parse_agent_workspace_roots,
};
use super::types::{
    Config, DeferredInsertShortcut, Language, OverlayPositionMode, TranscriptSendMode,
};

static CONFIG_ENV_BOOTSTRAPPED: AtomicBool = AtomicBool::new(false);
static CONFIG_ENV_BOOTSTRAP_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
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

    fn load_with_keychain_population(populate_keychain: bool) -> Self {
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

    fn can_seed_process_env() -> bool {
        cfg!(test) || !CONFIG_ENV_BOOTSTRAPPED.load(Ordering::SeqCst)
    }

    fn mark_process_env_bootstrapped(seed_process_env: bool) {
        if seed_process_env && !cfg!(test) {
            CONFIG_ENV_BOOTSTRAPPED.store(true, Ordering::SeqCst);
        }
    }

    fn seeded_env_keys() -> &'static Mutex<HashSet<String>> {
        CONFIG_SEEDED_ENV_KEYS.get_or_init(|| Mutex::new(HashSet::new()))
    }

    fn remember_seeded_env_key(key: &str) {
        if cfg!(test) {
            return;
        }
        if let Ok(mut keys) = Self::seeded_env_keys().lock() {
            keys.insert(key.to_string());
        }
    }

    fn was_seeded_env_key(key: &str) -> bool {
        if cfg!(test) {
            return false;
        }
        Self::seeded_env_keys()
            .lock()
            .map(|keys| keys.contains(key))
            .unwrap_or(false)
    }

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
    pub fn formatting_policy() -> anyhow::Result<FormattingPolicy> {
        let runtime = Self::config_runtime_env_var("FORMATTING_LEVEL").ok();
        let settings = super::settings::UserSettings::load();
        FormattingPolicy::resolve(runtime.as_deref(), settings.formatting_level.as_deref())
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

    fn env_missing_or_empty(key: &str) -> bool {
        Self::config_runtime_env_var(key)
            .ok()
            .is_none_or(|value| value.trim().is_empty())
    }

    fn config_init_set_env_if_missing(key: &str, value: impl AsRef<str>) {
        if Self::env_missing_or_empty(key) {
            Self::config_init_set_env(key, value.as_ref());
        }
    }

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
        // `CODESCRIBE_TAIL_SILENCE_SEC`, `CODESCRIBE_TAIL_DROP_ENABLED`).
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

        // ── STT engine / layered transcription (F1) ──
        // Explicit process env wins; settings.json seeds the env-only knobs that
        // core/stt reads per-call (selected_engine / layered_phase).
        if Self::config_runtime_env_var("CODESCRIBE_STT_ENGINE").is_err()
            && let Some(ref v) = settings.stt_engine
        {
            Self::safe_set_env("CODESCRIBE_STT_ENGINE", v);
        }
        if Self::config_runtime_env_var("CODESCRIBE_LAYERED_TRANSCRIPTION").is_err()
            && let Some(ref v) = settings.layered_transcription
        {
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
                | "BACKEND_MAX_UPLOAD_MB" => {
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
                | "CODESCRIBE_STT_INITIAL_PROMPT_ENABLED" => {
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
        Self::write_env_file(&env_path, &env_vars)?;
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
                    | "CODESCRIBE_STT_INITIAL_PROMPT_ENABLED" => {
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
            Self::write_env_file(&path, &vars)?;
        }

        Ok(())
    }

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

fn canonical_existing_file(path: &Path) -> anyhow::Result<PathBuf> {
    let path = path.canonicalize()?;
    if !path.is_file() {
        anyhow::bail!("Config env path is not a file: {}", path.display());
    }
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::UserSettings;
    use serial_test::serial;
    use std::fs;
    use tempfile::TempDir;

    fn set_env_for_test<V: AsRef<std::ffi::OsStr>>(key: &str, value: V) {
        // SAFETY: these tests are marked `serial` and do not start background workers,
        // so process-env mutation stays confined to the active test case.
        unsafe { std::env::set_var(key, value) };
    }

    fn remove_env_for_test(key: &str) {
        // SAFETY: same invariant as `set_env_for_test` above.
        unsafe { std::env::remove_var(key) };
    }

    fn restore_env_for_test(key: &str, previous: Option<String>) {
        if let Some(value) = previous {
            set_env_for_test(key, value);
        } else {
            remove_env_for_test(key);
        }
    }

    struct TestEnvGuard {
        key: &'static str,
        previous: Option<String>,
    }

    impl TestEnvGuard {
        fn unset(key: &'static str) -> Self {
            let previous = std::env::var(key).ok();
            remove_env_for_test(key);
            Self { key, previous }
        }
    }

    impl Drop for TestEnvGuard {
        fn drop(&mut self) {
            restore_env_for_test(self.key, self.previous.take());
        }
    }

    fn setup_isolated_data_dir() -> TempDir {
        let tmp = TempDir::new().expect("tempdir");
        set_env_for_test("CODESCRIBE_DATA_DIR", tmp.path());
        remove_env_for_test("CODESCRIBE_ENV_PATH");
        remove_env_for_test("USE_LOCAL_STT");
        remove_env_for_test("CODESCRIBE_STT_INITIAL_PROMPT_ENABLED");
        tmp
    }

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

    #[test]
    #[serial]
    fn hold_indicator_ui_writes_existing_env_keys_without_settings_json_drift() {
        let _tmp = setup_isolated_data_dir();
        let _indicator = TestEnvGuard::unset("HOLD_INDICATOR");
        let _size = TestEnvGuard::unset("HOLD_BADGE_SIZE");
        let settings = UserSettings {
            show_dock_icon: Some(true),
            ..UserSettings::default()
        };
        settings.save().expect("seed settings json");
        let settings_before = fs::read(UserSettings::settings_path()).expect("read settings json");
        let config = Config::default();

        config
            .save_to_env("HOLD_BADGE_SIZE", "8")
            .expect("save stored badge size");
        config
            .save_to_env("HOLD_INDICATOR", "0")
            .expect("disable indicator");
        let disabled = Config::parse_env_file(&Config::env_path()).expect("read env");
        assert_eq!(
            disabled.get("HOLD_INDICATOR").map(String::as_str),
            Some("0")
        );
        assert_eq!(
            disabled.get("HOLD_BADGE_SIZE").map(String::as_str),
            Some("8"),
            "Off must preserve the stored badge size"
        );
        let disabled_config = Config::load_without_keychain();
        assert!(!disabled_config.hold_indicator);
        assert_eq!(disabled_config.hold_badge_size, 8);

        for size in [4, 8, 12] {
            let size = size.to_string();
            config
                .save_to_env_many(&[("HOLD_INDICATOR", "1"), ("HOLD_BADGE_SIZE", &size)])
                .expect("save enabled badge size");
            let persisted = Config::parse_env_file(&Config::env_path()).expect("read env");
            assert_eq!(
                persisted.get("HOLD_INDICATOR").map(String::as_str),
                Some("1")
            );
            assert_eq!(
                persisted.get("HOLD_BADGE_SIZE").map(String::as_str),
                Some(size.as_str())
            );
            // Test builds intentionally allow repeated process-env bootstrap,
            // unlike production's one-shot tracked bootstrap. Remove the prior
            // injected snapshot so this reload exercises the newly persisted
            // values instead of the test-only stale process copy.
            // SAFETY: this test is serial and the guards above restore both keys.
            unsafe {
                std::env::remove_var("HOLD_INDICATOR");
                std::env::remove_var("HOLD_BADGE_SIZE");
            }
            let live = Config::load_without_keychain();
            assert!(live.hold_indicator);
            assert_eq!(live.hold_badge_size.to_string(), size);
        }

        let settings_after = fs::read(UserSettings::settings_path()).expect("read settings json");
        assert_eq!(
            settings_before, settings_after,
            "settings.json must not gain badge keys"
        );
    }

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
        assert_eq!(
            crate::llm::lane_truth::endpoint(
                crate::llm::provider::LlmMode::Assistive,
                &Config::default(),
            ),
            crate::config::DEFAULT_OPENAI_RESPONSES_ENDPOINT
        );
    }

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
        assert_eq!(
            crate::llm::lane_truth::provider(crate::llm::provider::LlmMode::Assistive),
            crate::llm::provider::ProviderKind::OpenAiResponses
        );
    }

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
        assert_eq!(
            crate::llm::lane_truth::endpoint(
                crate::llm::provider::LlmMode::Assistive,
                &Config::default(),
            ),
            crate::config::DEFAULT_OPENAI_RESPONSES_ENDPOINT
        );
    }

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
