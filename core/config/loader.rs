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
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use tracing::{info, warn};

use super::defaults::{
    default_assistive_model, default_formatting_model, default_llm_endpoint, default_llm_model,
};
use super::types::{Config, Language, OverlayPositionMode, TranscriptSendMode};

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

        // Optional .env remains available for env-managed / power-user keys, but
        // promoted settings are intentionally excluded so stale ~/.codescribe/.env
        // cannot shadow user choices persisted in settings.json.
        if let Some(vars) = file_env_vars.as_ref() {
            Self::inject_file_env_for_runtime(vars);
        }

        // Load API keys from Keychain (only if not already set by .env)
        super::keychain::populate_env_from_keychain();

        // Load user settings from JSON
        let user_settings = super::settings::UserSettings::load();

        let mut config = Self::default();

        // Apply user settings first (lowest priority after defaults)
        config.apply_user_settings(&user_settings);

        // Override with environment variables (explicit runtime env + injected env-managed .env).
        config.load_from_env();
        config.apply_default_llm_runtime_env();
        config.sanitize();
        config
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
        std::env::var(key)
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
        Self::config_init_set_env_if_missing("LLM_ASSISTIVE_ENDPOINT", &endpoint);
        Self::config_init_set_env_if_missing("LLM_ASSISTIVE_MODEL", default_assistive_model());
    }

    /// Load configuration values from environment variables.
    pub fn load_from_env(&mut self) {
        // Hotkeys
        if let Ok(val) = std::env::var("HOLD_EXCLUSIVE") {
            self.hold_exclusive = matches!(val.as_str(), "1" | "true" | "yes" | "on");
        }
        if let Ok(val) = std::env::var("HOLD_START_DELAY_MS")
            && let Ok(ms) = val.parse()
        {
            self.hold_start_delay_ms = ms;
        }
        if let Ok(val) = std::env::var("DOUBLE_TAP_INTERVAL_MS")
            && let Ok(ms) = val.parse()
        {
            self.double_tap_interval_ms = ms;
        }
        if let Ok(val) = std::env::var("TOGGLE_SILENCE_SEC")
            && let Ok(sec) = val.parse()
        {
            self.toggle_silence_sec = sec;
        }

        // Language
        if let Ok(val) = std::env::var("WHISPER_LANGUAGE")
            && let Ok(lang) = val.parse::<Language>()
        {
            self.whisper_language = lang;
        }

        // AI Formatting
        if let Ok(val) = std::env::var("AI_FORMATTING_ENABLED") {
            self.ai_formatting_enabled =
                matches!(val.as_str(), "1" | "true" | "yes" | "on" | "enabled");
        }
        if let Ok(val) = std::env::var("TRANSCRIPT_SEND_MODE")
            && let Ok(mode) = val.parse::<TranscriptSendMode>()
        {
            self.transcript_send_mode = mode;
        }
        if let Ok(val) = std::env::var("CODESCRIBE_TRANSCRIPT_TAGGING") {
            self.transcript_tagging_enabled =
                matches!(val.as_str(), "1" | "true" | "yes" | "on" | "enabled");
        }
        if let Ok(val) = std::env::var("CODESCRIBE_TRANSCRIPT_TAG_TEMPLATE") {
            self.transcript_tag_template = val;
        }
        if let Ok(val) = std::env::var("AI_MAX_TOKENS")
            && let Ok(tokens) = val.parse()
        {
            self.ai_max_tokens = tokens;
        }
        if let Ok(val) = std::env::var("AI_ASSISTIVE_MAX_TOKENS")
            && let Ok(tokens) = val.parse()
        {
            self.ai_assistive_max_tokens = tokens;
        }

        // UI
        if let Ok(val) = std::env::var("SHOW_TRAY_GLYPH") {
            self.show_tray_glyph = val.parse().unwrap_or(true);
        }
        if let Ok(val) = std::env::var("SHOW_DOCK_ICON") {
            self.show_dock_icon = matches!(val.as_str(), "1" | "true" | "yes" | "on");
        }
        if let Ok(val) = std::env::var("TRANSCRIPTION_OVERLAY_ENABLED") {
            self.transcription_overlay_enabled =
                matches!(val.as_str(), "1" | "true" | "yes" | "on");
        }
        if let Ok(val) = std::env::var("HOLD_INDICATOR") {
            self.hold_indicator = val.parse().unwrap_or(true);
        }
        if let Ok(val) = std::env::var("HOLD_BADGE_SIZE")
            && let Ok(size) = val.parse()
        {
            self.hold_badge_size = size;
        }
        if let Ok(val) = std::env::var("HOLD_BADGE_OFFSET_X")
            && let Ok(offset) = val.parse()
        {
            self.hold_badge_offset_x = offset;
        }
        if let Ok(val) = std::env::var("HOLD_BADGE_OFFSET_Y")
            && let Ok(offset) = val.parse()
        {
            self.hold_badge_offset_y = offset;
        }

        if let Ok(val) = std::env::var("OVERLAY_POSITION_MODE")
            && let Ok(mode) = val.parse::<OverlayPositionMode>()
        {
            self.overlay_position_mode = mode;
        }
        if let Ok(val) = std::env::var("OVERLAY_CUSTOM_X")
            && let Ok(x) = val.parse()
        {
            self.overlay_custom_x = Some(x);
        }
        if let Ok(val) = std::env::var("OVERLAY_CUSTOM_Y")
            && let Ok(y) = val.parse()
        {
            self.overlay_custom_y = Some(y);
        }

        // Sound
        if let Ok(val) = std::env::var("BEEP_ON_START") {
            self.beep_on_start = matches!(val.as_str(), "1" | "true" | "yes" | "on");
        }
        if let Ok(val) = std::env::var("AGENT_ENTER_SENDS") {
            self.agent_enter_sends = matches!(val.as_str(), "1" | "true" | "yes" | "on");
        }
        if let Ok(val) = std::env::var("SOUND_NAME") {
            self.sound_name = val;
        }
        if let Ok(val) = std::env::var("SOUND_VOLUME")
            && let Ok(volume) = val.parse()
        {
            self.sound_volume = volume;
        }

        // Audio
        if let Ok(val) = std::env::var("AUDIO_INPUT_DEVICE") {
            self.audio_input_device = (!val.trim().is_empty()).then_some(val);
        }
        // VAD config lives in `core/vad/config.rs` with hardcoded defaults and
        // opt-in power-user env overrides (`CODESCRIBE_UTTERANCE_GAP_SEC`,
        // `CODESCRIBE_TAIL_SILENCE_SEC`, `CODESCRIBE_TAIL_DROP_ENABLED`).
        // No legacy SILENCE_* variables - single source of truth.

        // History (default: on to avoid data loss)
        if let Ok(val) = std::env::var("HISTORY_ENABLED") {
            self.history_enabled = val.parse().unwrap_or(true);
        }

        // Quick Notes (default: off)
        if let Ok(val) = std::env::var("QUICK_NOTES_ENABLED") {
            self.quick_notes_enabled = matches!(val.as_str(), "1" | "true" | "yes" | "on");
        }
        if let Ok(val) = std::env::var("QUICK_NOTES_SAVE_ONLY") {
            self.quick_notes_save_only = matches!(val.as_str(), "1" | "true" | "yes" | "on");
        }

        // Backends - LLM
        // LLM_API_KEY for cloud providers
        if let Ok(val) = std::env::var("LLM_API_KEY") {
            self.llm_api_key = Some(val);
        }
        if let Ok(val) = std::env::var("LLM_ENDPOINT") {
            self.llm_endpoint = Some(val);
        }

        // Backends - STT
        if let Ok(val) = std::env::var("STT_ENDPOINT") {
            self.stt_endpoint = Some(val);
        }
        // STT_API_KEY for cloud STT
        if let Ok(val) = std::env::var("STT_API_KEY") {
            self.stt_api_key = Some(val);
        }

        // Local STT (Pure Rust Whisper)
        if let Ok(val) = std::env::var("USE_LOCAL_STT") {
            self.use_local_stt = matches!(val.as_str(), "1" | "true" | "yes" | "on");
        }
        if let Ok(val) = std::env::var("LOCAL_MODEL") {
            self.local_model = val;
        }

        // Clipboard
        if let Ok(val) = std::env::var("RESTORE_CLIPBOARD") {
            self.restore_clipboard = val.parse().unwrap_or(true);
        }
        if let Ok(val) = std::env::var("RESTORE_CLIPBOARD_DELAY_MS")
            && let Ok(delay) = val.parse()
        {
            self.restore_clipboard_delay_ms = delay;
        }

        // System
        if let Ok(val) = std::env::var("START_AT_LOGIN") {
            self.start_at_login = matches!(val.as_str(), "1" | "true" | "yes" | "on");
        }

        // Debugging (default: on to keep paired .wav with transcripts)
        if let Ok(val) = std::env::var("DUMP_AUDIO_LOGS") {
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
        // SAFETY: config init happens before background workers consume configuration,
        // so process-env mutation is confined to a single writer during bootstrap.
        unsafe { std::env::set_var(key, value.as_ref()) };
    }

    fn ui_thread_set_env(key: &str, value: &str) {
        // SAFETY: settings writes originate from the main UI thread; runtime readers
        // consume refreshed Config snapshots rather than racing direct env access.
        unsafe { std::env::set_var(key, value) };
    }

    /// Apply user settings from JSON (lower priority than .env).
    /// Only applies values that are Some AND not already overridden by env vars.
    fn apply_user_settings(&mut self, settings: &super::settings::UserSettings) {
        // Helper: only apply if the env var is NOT set
        macro_rules! apply_parsed_if_no_env {
            ($env_key:expr, $field:expr, $val:expr) => {
                if std::env::var($env_key).is_err() {
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
        if std::env::var("HOLD_START_DELAY_MS").is_err()
            && let Some(v) = settings.hold_start_delay_ms
        {
            self.hold_start_delay_ms = v;
        }
        if std::env::var("DOUBLE_TAP_INTERVAL_MS").is_err()
            && let Some(v) = settings.double_tap_interval_ms
        {
            self.double_tap_interval_ms = v;
        }
        if std::env::var("TOGGLE_SILENCE_SEC").is_err()
            && let Some(v) = settings.toggle_silence_sec
        {
            self.toggle_silence_sec = v;
        }
        if std::env::var("HOLD_EXCLUSIVE").is_err()
            && let Some(v) = settings.hold_exclusive
        {
            self.hold_exclusive = v;
        }
        // AI
        if std::env::var("AI_FORMATTING_ENABLED").is_err()
            && let Some(v) = settings.ai_formatting_enabled
        {
            self.ai_formatting_enabled = v;
        }
        if std::env::var("CODESCRIBE_TRANSCRIPT_TAGGING").is_err()
            && let Some(v) = settings.transcript_tagging_enabled
        {
            self.transcript_tagging_enabled = v;
        }
        if std::env::var("CODESCRIBE_TRANSCRIPT_TAG_TEMPLATE").is_err()
            && let Some(ref v) = settings.transcript_tag_template
        {
            self.transcript_tag_template = v.clone();
        }
        if std::env::var("FORMATTING_LEVEL").is_err()
            && let Some(ref v) = settings.formatting_level
        {
            // FORMATTING_LEVEL is read from env at runtime (not a Config field).
            Self::safe_set_env("FORMATTING_LEVEL", v);
        }
        // Sound
        if std::env::var("BEEP_ON_START").is_err()
            && let Some(v) = settings.beep_on_start
        {
            self.beep_on_start = v;
        }
        if std::env::var("SHOW_DOCK_ICON").is_err()
            && let Some(v) = settings.show_dock_icon
        {
            self.show_dock_icon = v;
        }
        if std::env::var("TRANSCRIPTION_OVERLAY_ENABLED").is_err()
            && let Some(v) = settings.transcription_overlay_enabled
        {
            self.transcription_overlay_enabled = v;
            Self::safe_set_env("TRANSCRIPTION_OVERLAY_ENABLED", if v { "1" } else { "0" });
        }
        if std::env::var("SOUND_VOLUME").is_err()
            && let Some(v) = settings.sound_volume
        {
            self.sound_volume = v;
        }
        // LLM endpoints (from JSON, lower priority than .env)
        if std::env::var("LLM_ENDPOINT").is_err()
            && let Some(ref v) = settings.llm_endpoint
        {
            self.llm_endpoint = Some(v.clone());
        }
        if std::env::var("LLM_MODEL").is_err()
            && let Some(ref v) = settings.llm_model
        {
            // LLM_MODEL is not in Config struct but read from env at runtime
            // Set env var so downstream code picks it up
            Self::safe_set_env("LLM_MODEL", v);
        }
        // Assistive LLM (not in Config struct, read from env at runtime)
        if std::env::var("LLM_ASSISTIVE_ENDPOINT").is_err()
            && let Some(ref v) = settings.llm_assistive_endpoint
        {
            Self::safe_set_env("LLM_ASSISTIVE_ENDPOINT", v);
        }
        if std::env::var("LLM_ASSISTIVE_MODEL").is_err()
            && let Some(ref v) = settings.llm_assistive_model
        {
            Self::safe_set_env("LLM_ASSISTIVE_MODEL", v);
        }
        // ── Promoted fields (previously .env only) ──

        // LLM formatting (not in Config struct, read from env at runtime)
        if std::env::var("LLM_FORMATTING_ENDPOINT").is_err()
            && let Some(ref v) = settings.llm_formatting_endpoint
        {
            Self::safe_set_env("LLM_FORMATTING_ENDPOINT", v);
        }
        if std::env::var("LLM_FORMATTING_MODEL").is_err()
            && let Some(ref v) = settings.llm_formatting_model
        {
            Self::safe_set_env("LLM_FORMATTING_MODEL", v);
        }

        // Local STT
        if std::env::var("USE_LOCAL_STT").is_err()
            && let Some(v) = settings.use_local_stt
        {
            self.use_local_stt = v;
            Self::config_init_set_env("USE_LOCAL_STT", if v { "1" } else { "0" });
        }
        if std::env::var("LOCAL_MODEL").is_err()
            && let Some(ref v) = settings.local_model
        {
            self.local_model = v.clone();
        }

        // STT endpoint
        if std::env::var("STT_ENDPOINT").is_err()
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
        if std::env::var("AUDIO_INPUT_DEVICE").is_err()
            && let Some(ref v) = settings.audio_input_device
        {
            self.audio_input_device = Some(v.clone());
        }

        // Sound name
        if std::env::var("SOUND_NAME").is_err()
            && let Some(ref v) = settings.sound_name
        {
            self.sound_name = v.clone();
        }

        // History
        if std::env::var("HISTORY_ENABLED").is_err()
            && let Some(v) = settings.history_enabled
        {
            self.history_enabled = v;
        }

        // Quick Notes
        if std::env::var("QUICK_NOTES_ENABLED").is_err()
            && let Some(v) = settings.quick_notes_enabled
        {
            self.quick_notes_enabled = v;
        }
        if std::env::var("QUICK_NOTES_SAVE_ONLY").is_err()
            && let Some(v) = settings.quick_notes_save_only
        {
            self.quick_notes_save_only = v;
        }

        // System
        if std::env::var("START_AT_LOGIN").is_err()
            && let Some(v) = settings.start_at_login
        {
            self.start_at_login = v;
        }
        if std::env::var("QUBE_DAEMON_AUTOSTART").is_err()
            && let Some(v) = settings.qube_daemon_autostart
        {
            Self::config_init_set_env("QUBE_DAEMON_AUTOSTART", if v { "1" } else { "0" });
        }
        if std::env::var("AGENT_ENTER_SENDS").is_err()
            && let Some(v) = settings.agent_enter_sends
        {
            self.agent_enter_sends = v;
        }

        // ── Voice Lab survivors (runtime env vars, not Config struct fields) ──
        if std::env::var("CODESCRIBE_BUFFER_DELAY_MS").is_err()
            && let Some(v) = settings.buffer_delay_ms
        {
            Self::config_init_set_env("CODESCRIBE_BUFFER_DELAY_MS", v.to_string());
        }
        if std::env::var("CODESCRIBE_TYPING_CPS").is_err()
            && let Some(v) = settings.typing_cps
        {
            Self::config_init_set_env("CODESCRIBE_TYPING_CPS", v.to_string());
        }
        if std::env::var("CODESCRIBE_EMIT_WORDS_MAX").is_err()
            && let Some(v) = settings.emit_words_max
        {
            Self::config_init_set_env("CODESCRIBE_EMIT_WORDS_MAX", v.to_string());
        }
        if std::env::var("CODESCRIBE_BUFFERED_INTERIM_SEC").is_err()
            && let Some(v) = settings.buffered_interim_sec
        {
            Self::config_init_set_env("CODESCRIBE_BUFFERED_INTERIM_SEC", format!("{v:.1}"));
        }
        if std::env::var("WHISPER_MODEL").is_err()
            && let Some(ref v) = settings.whisper_model
        {
            Self::safe_set_env("WHISPER_MODEL", v);
        }
        if std::env::var("BACKEND_MAX_UPLOAD_MB").is_err()
            && let Some(v) = settings.backend_max_upload_mb
        {
            Self::config_init_set_env("BACKEND_MAX_UPLOAD_MB", v.to_string());
        }
    }

    /// Save a configuration value, routing to the appropriate tier:
    /// - API keys → Keychain
    /// - Regular-user fields → settings.json
    /// - Everything else → .env
    pub fn save_to_env(&self, key: &str, value: &str) -> anyhow::Result<()> {
        // API keys → Keychain
        if super::keychain::KEYCHAIN_ACCOUNTS.contains(&key) {
            super::keychain::save_key(key, value)?;
            // Also update runtime env var.
            Self::ui_thread_set_env(key, value);
            return Ok(());
        }

        // Regular-user fields → settings.json
        let is_regular = super::settings::is_promoted_key(key);

        if is_regular {
            let mut settings = super::settings::UserSettings::load();
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
                | "TRANSCRIPT_TAGGING_ENABLED"
                | "BEEP_ON_START"
                | "SHOW_DOCK_ICON"
                | "TRANSCRIPTION_OVERLAY_ENABLED"
                | "HOLD_EXCLUSIVE"
                | "USE_LOCAL_STT"
                | "HISTORY_ENABLED"
                | "QUICK_NOTES_ENABLED"
                | "QUICK_NOTES_SAVE_ONLY"
                | "START_AT_LOGIN"
                | "QUBE_DAEMON_AUTOSTART"
                | "AGENT_ENTER_SENDS" => {
                    let bool_val = matches!(value, "1" | "true" | "yes" | "on");
                    settings.set_bool(key, bool_val);
                }
                _ => {
                    settings.set_string(key, value);
                }
            }
            // Also update runtime env var.
            Self::ui_thread_set_env(key, value);
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
        Self::ui_thread_set_env(key, value);
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
            // API keys → Keychain
            if super::keychain::KEYCHAIN_ACCOUNTS.contains(key) {
                super::keychain::save_key(key, value)?;
                Self::ui_thread_set_env(key, value);
                continue;
            }

            // Regular-user fields → settings.json
            let is_regular = super::settings::is_promoted_key(key);

            if is_regular {
                let settings_ref = settings.get_or_insert_with(super::settings::UserSettings::load);
                match *key {
                    // ── Strings ──
                    "WHISPER_LANGUAGE" => {
                        settings_ref.whisper_language = Some((*value).to_string())
                    }
                    "LLM_ENDPOINT" => settings_ref.llm_endpoint = Some((*value).to_string()),
                    "LLM_MODEL" => settings_ref.llm_model = Some((*value).to_string()),
                    "LLM_ASSISTIVE_ENDPOINT" => {
                        settings_ref.llm_assistive_endpoint = Some((*value).to_string())
                    }
                    "LLM_ASSISTIVE_MODEL" => {
                        settings_ref.llm_assistive_model = Some((*value).to_string())
                    }
                    "FORMATTING_LEVEL" => {
                        settings_ref.formatting_level = Some((*value).to_string())
                    }
                    "LLM_FORMATTING_ENDPOINT" => {
                        settings_ref.llm_formatting_endpoint = Some((*value).to_string())
                    }
                    "LLM_FORMATTING_MODEL" => {
                        settings_ref.llm_formatting_model = Some((*value).to_string())
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
                    | "TRANSCRIPT_TAGGING_ENABLED"
                    | "BEEP_ON_START"
                    | "SHOW_DOCK_ICON"
                    | "TRANSCRIPTION_OVERLAY_ENABLED"
                    | "HOLD_EXCLUSIVE"
                    | "USE_LOCAL_STT"
                    | "HISTORY_ENABLED"
                    | "QUICK_NOTES_ENABLED"
                    | "QUICK_NOTES_SAVE_ONLY"
                    | "START_AT_LOGIN"
                    | "QUBE_DAEMON_AUTOSTART"
                    | "AGENT_ENTER_SENDS" => {
                        let bv = matches!(*value, "1" | "true" | "yes" | "on");
                        match *key {
                            "AI_FORMATTING_ENABLED" => {
                                settings_ref.ai_formatting_enabled = Some(bv)
                            }
                            "BEEP_ON_START" => settings_ref.beep_on_start = Some(bv),
                            "SHOW_DOCK_ICON" => settings_ref.show_dock_icon = Some(bv),
                            "TRANSCRIPTION_OVERLAY_ENABLED" => {
                                settings_ref.transcription_overlay_enabled = Some(bv)
                            }
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
                            _ => {}
                        }
                    }
                    _ => {}
                }
                Self::ui_thread_set_env(key, value);
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
            Self::ui_thread_set_env(key, value);
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

    /// Parse .env file into HashMap.
    pub fn parse_env_file(path: &PathBuf) -> anyhow::Result<HashMap<String, String>> {
        // `path` is always internally derived from `Config::env_path()`
        // (config_dir()/.env, or the `CODESCRIBE_ENV_PATH` override used by tests
        // and power users) — never raw request or end-user input. No external
        // path-traversal source reaches this read.
        let contents = fs::read_to_string(path)?;
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
        remove_env_for_test("USE_LOCAL_STT");
        tmp
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
