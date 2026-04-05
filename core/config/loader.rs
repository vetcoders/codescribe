//! Configuration loading and saving functionality.
//!
//! Handles loading from .env file and environment variables.
//! Single source of truth: ~/.codescribe/.env

use directories::BaseDirs;
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use tracing::{info, warn};

use super::types::{
    Config, HoldMods, Language, OverlayPositionMode, ToggleTrigger, TranscriptSendMode,
};

impl Config {
    /// Load configuration from disk or environment.
    ///
    /// Priority order:
    /// 1. Environment variables
    /// 2. .env file in config directory (~/.codescribe/.env)
    /// 3. Default values
    ///
    /// If the .env file doesn't exist or is malformed, returns default configuration
    /// without raising an error.
    pub fn load() -> Self {
        let env_path = Self::env_path();
        let pre_env_use_local_stt = std::env::var("USE_LOCAL_STT").ok();
        let mut file_env_vars: Option<HashMap<String, String>> = None;
        let mut env_use_local_stt: Option<bool> = None;

        // Load .env file if it exists (power-user overrides only)
        // In production, .env doesn't exist — regular users use settings.json
        if env_path.exists() {
            // Migrate legacy keys inside existing .env (power users only)
            Self::migrate_env_legacy_keys();

            if let Ok(vars) = Self::parse_env_file(&env_path) {
                env_use_local_stt = vars
                    .get("USE_LOCAL_STT")
                    .and_then(|raw| parse_use_local_stt(raw, ".env"));
                file_env_vars = Some(vars);
            }
            let _ = dotenvy::from_path(&env_path);
        }

        // One-time migration from .env-only to tiered config
        super::migrate::migrate_if_needed(file_env_vars.as_ref());

        // Load API keys from Keychain (only if not already set by .env)
        super::keychain::populate_env_from_keychain();

        // Load user settings from JSON
        let user_settings = super::settings::UserSettings::load();

        let mut config = Self::default();

        // Apply user settings first (lowest priority after defaults)
        config.apply_user_settings(&user_settings);

        // Override with environment variables (.env + runtime; highest priority)
        config.load_from_env();
        if let Some(v) = env_use_local_stt {
            config.use_local_stt = v;
        } else if let Some(v) = user_settings.use_local_stt {
            config.use_local_stt = v;
        } else {
            if pre_env_use_local_stt.is_some() {
                warn!(
                    "Ignoring USE_LOCAL_STT from runtime environment; only ~/.codescribe/.env can disable local STT"
                );
            }
            config.use_local_stt = true;
        }
        config.sanitize();
        config
    }

    /// Load configuration values from environment variables.
    pub fn load_from_env(&mut self) {
        // Hotkeys
        if let Ok(val) = std::env::var("HOLD_MODS")
            && let Ok(mods) = val.parse::<HoldMods>()
        {
            self.hold_mods = mods;
        }
        if let Ok(val) = std::env::var("TOGGLE_TRIGGER")
            && let Ok(trigger) = val.parse::<ToggleTrigger>()
        {
            self.toggle_trigger = trigger;
        }
        if let Ok(val) = std::env::var("HOLD_EXCLUSIVE")
            && let Some(enabled) = parse_env_bool(&val, "HOLD_EXCLUSIVE")
        {
            self.hold_exclusive = enabled;
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
        if let Ok(val) = std::env::var("AI_FORMATTING_ENABLED")
            && let Some(enabled) = parse_env_bool(&val, "AI_FORMATTING_ENABLED")
        {
            self.ai_formatting_enabled = enabled;
        }
        if let Ok(val) = std::env::var("TRANSCRIPT_SEND_MODE")
            && let Ok(mode) = val.parse::<TranscriptSendMode>()
        {
            self.transcript_send_mode = mode;
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
        if let Ok(val) = std::env::var("SHOW_TRAY_GLYPH")
            && let Some(enabled) = parse_env_bool(&val, "SHOW_TRAY_GLYPH")
        {
            self.show_tray_glyph = enabled;
        }
        if let Ok(val) = std::env::var("SHOW_DOCK_ICON")
            && let Some(enabled) = parse_env_bool(&val, "SHOW_DOCK_ICON")
        {
            self.show_dock_icon = enabled;
        }
        if let Ok(val) = std::env::var("TRANSCRIPTION_OVERLAY_ENABLED")
            && let Some(enabled) = parse_env_bool(&val, "TRANSCRIPTION_OVERLAY_ENABLED")
        {
            self.transcription_overlay_enabled = enabled;
        }
        if let Ok(val) = std::env::var("HOLD_INDICATOR")
            && let Some(enabled) = parse_env_bool(&val, "HOLD_INDICATOR")
        {
            self.hold_indicator = enabled;
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
        if let Ok(val) = std::env::var("BEEP_ON_START")
            && let Some(enabled) = parse_env_bool(&val, "BEEP_ON_START")
        {
            self.beep_on_start = enabled;
        }
        if let Ok(val) = std::env::var("AGENT_ENTER_SENDS")
            && let Some(enabled) = parse_env_bool(&val, "AGENT_ENTER_SENDS")
        {
            self.agent_enter_sends = enabled;
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
        // VAD config is managed by core/vad/config.rs (hardcoded Silero defaults)
        // No legacy SILENCE_* variables - single source of truth

        // History (default: on to avoid data loss)
        if let Ok(val) = std::env::var("HISTORY_ENABLED")
            && let Some(enabled) = parse_env_bool(&val, "HISTORY_ENABLED")
        {
            self.history_enabled = enabled;
        }

        // Quick Notes (default: off)
        if let Ok(val) = std::env::var("QUICK_NOTES_ENABLED")
            && let Some(enabled) = parse_env_bool(&val, "QUICK_NOTES_ENABLED")
        {
            self.quick_notes_enabled = enabled;
        }
        if let Ok(val) = std::env::var("QUICK_NOTES_SAVE_ONLY")
            && let Some(enabled) = parse_env_bool(&val, "QUICK_NOTES_SAVE_ONLY")
        {
            self.quick_notes_save_only = enabled;
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
        if let Ok(val) = std::env::var("USE_LOCAL_STT")
            && let Some(enabled) = parse_env_bool(&val, "USE_LOCAL_STT")
        {
            self.use_local_stt = enabled;
        }
        if let Ok(val) = std::env::var("LOCAL_MODEL") {
            self.local_model = val;
        }

        // Clipboard
        if let Ok(val) = std::env::var("RESTORE_CLIPBOARD")
            && let Some(enabled) = parse_env_bool(&val, "RESTORE_CLIPBOARD")
        {
            self.restore_clipboard = enabled;
        }
        if let Ok(val) = std::env::var("RESTORE_CLIPBOARD_DELAY_MS")
            && let Ok(delay) = val.parse()
        {
            self.restore_clipboard_delay_ms = delay;
        }

        // System
        if let Ok(val) = std::env::var("START_AT_LOGIN")
            && let Some(enabled) = parse_env_bool(&val, "START_AT_LOGIN")
        {
            self.start_at_login = enabled;
        }

        // Debugging (default: on to keep paired .wav with transcripts)
        if let Ok(val) = std::env::var("DUMP_AUDIO_LOGS")
            && let Some(enabled) = parse_env_bool(&val, "DUMP_AUDIO_LOGS")
        {
            self.dump_audio_logs = enabled;
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
        // SAFETY: single-threaded config init, no other threads reading env yet.
        unsafe { std::env::set_var(key, value) };
    }

    /// Apply user settings from JSON (lower priority than .env).
    /// Only applies values that are Some AND not already overridden by env vars.
    fn apply_user_settings(&mut self, settings: &super::settings::UserSettings) {
        // Helper: only apply if the env var is absent or invalid for the target type.
        macro_rules! apply_parsed_if_no_valid_env {
            ($env_key:expr, $ty:ty, $field:expr, $val:expr) => {
                if !env_var_parses::<$ty>($env_key) {
                    if let Some(ref v) = $val {
                        if let Ok(parsed) = v.parse::<$ty>() {
                            $field = parsed;
                        }
                    }
                }
            };
        }

        // Language
        apply_parsed_if_no_valid_env!(
            "WHISPER_LANGUAGE",
            Language,
            self.whisper_language,
            settings.whisper_language
        );
        // Hotkeys
        if !env_var_parses::<HoldMods>("HOLD_MODS") {
            self.hold_mods = settings.legacy_hold_mods();
        }
        if !env_var_parses::<ToggleTrigger>("TOGGLE_TRIGGER") {
            self.toggle_trigger = settings.legacy_toggle_trigger();
        }
        if !env_var_parses::<u64>("HOLD_START_DELAY_MS")
            && let Some(v) = settings.hold_start_delay_ms
        {
            self.hold_start_delay_ms = v;
        }
        if !env_var_parses::<u64>("DOUBLE_TAP_INTERVAL_MS")
            && let Some(v) = settings.double_tap_interval_ms
        {
            self.double_tap_interval_ms = v;
        }
        if !env_var_parses::<f32>("TOGGLE_SILENCE_SEC")
            && let Some(v) = settings.toggle_silence_sec
        {
            self.toggle_silence_sec = v;
        }
        if !env_var_has_valid_bool("HOLD_EXCLUSIVE")
            && let Some(v) = settings.hold_exclusive
        {
            self.hold_exclusive = v;
        }
        // AI
        if !env_var_has_valid_bool("AI_FORMATTING_ENABLED")
            && let Some(v) = settings.ai_formatting_enabled
        {
            self.ai_formatting_enabled = v;
        }
        if std::env::var("FORMATTING_LEVEL").is_err()
            && let Some(ref v) = settings.formatting_level
        {
            // FORMATTING_LEVEL is read from env at runtime (not a Config field).
            Self::safe_set_env("FORMATTING_LEVEL", v);
        }
        // Sound
        if !env_var_has_valid_bool("BEEP_ON_START")
            && let Some(v) = settings.beep_on_start
        {
            self.beep_on_start = v;
        }
        if !env_var_has_valid_bool("SHOW_DOCK_ICON")
            && let Some(v) = settings.show_dock_icon
        {
            self.show_dock_icon = v;
        }
        if !env_var_has_valid_bool("TRANSCRIPTION_OVERLAY_ENABLED")
            && let Some(v) = settings.transcription_overlay_enabled
        {
            self.transcription_overlay_enabled = v;
            unsafe {
                std::env::set_var("TRANSCRIPTION_OVERLAY_ENABLED", if v { "1" } else { "0" })
            };
        }
        if !env_var_parses::<f32>("SOUND_VOLUME")
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
        if !env_var_has_valid_bool("USE_LOCAL_STT")
            && let Some(v) = settings.use_local_stt
        {
            self.use_local_stt = v;
            unsafe { std::env::set_var("USE_LOCAL_STT", if v { "1" } else { "0" }) };
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
        apply_parsed_if_no_valid_env!(
            "TRANSCRIPT_SEND_MODE",
            TranscriptSendMode,
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
        if !env_var_has_valid_bool("HISTORY_ENABLED")
            && let Some(v) = settings.history_enabled
        {
            self.history_enabled = v;
        }

        // Quick Notes
        if !env_var_has_valid_bool("QUICK_NOTES_ENABLED")
            && let Some(v) = settings.quick_notes_enabled
        {
            self.quick_notes_enabled = v;
        }
        if !env_var_has_valid_bool("QUICK_NOTES_SAVE_ONLY")
            && let Some(v) = settings.quick_notes_save_only
        {
            self.quick_notes_save_only = v;
        }

        // System
        if !env_var_has_valid_bool("START_AT_LOGIN")
            && let Some(v) = settings.start_at_login
        {
            self.start_at_login = v;
        }
        if !env_var_has_valid_bool("CODESCRIBE_AUTOSTART_QUALITY_DAEMON")
            && let Some(v) = settings.quality_daemon_autostart
        {
            unsafe {
                std::env::set_var(
                    "CODESCRIBE_AUTOSTART_QUALITY_DAEMON",
                    if v { "1" } else { "0" },
                )
            };
        }
        if !env_var_has_valid_bool("AGENT_ENTER_SENDS")
            && let Some(v) = settings.agent_enter_sends
        {
            self.agent_enter_sends = v;
        }

        // ── Voice Lab survivors (runtime env vars, not Config struct fields) ──
        if !env_var_parses::<u64>("CODESCRIBE_BUFFER_DELAY_MS")
            && let Some(v) = settings.buffer_delay_ms
        {
            unsafe { std::env::set_var("CODESCRIBE_BUFFER_DELAY_MS", v.to_string()) };
        }
        if !env_var_parses::<f32>("CODESCRIBE_TYPING_CPS")
            && let Some(v) = settings.typing_cps
        {
            unsafe { std::env::set_var("CODESCRIBE_TYPING_CPS", v.to_string()) };
        }
        if !env_var_parses::<u64>("CODESCRIBE_EMIT_WORDS_MAX")
            && let Some(v) = settings.emit_words_max
        {
            unsafe { std::env::set_var("CODESCRIBE_EMIT_WORDS_MAX", v.to_string()) };
        }
        if !env_var_parses::<f32>("CODESCRIBE_BUFFERED_INTERIM_SEC")
            && let Some(v) = settings.buffered_interim_sec
        {
            unsafe { std::env::set_var("CODESCRIBE_BUFFERED_INTERIM_SEC", format!("{v:.1}")) };
        }
        if std::env::var("WHISPER_MODEL").is_err()
            && let Some(ref v) = settings.whisper_model
        {
            Self::safe_set_env("WHISPER_MODEL", v);
        }
        if !env_var_parses::<u64>("BACKEND_MAX_UPLOAD_MB")
            && let Some(v) = settings.backend_max_upload_mb
        {
            unsafe { std::env::set_var("BACKEND_MAX_UPLOAD_MB", v.to_string()) };
        }
    }

    /// Save a configuration value, routing to the appropriate tier:
    /// - API keys → Keychain
    /// - Regular-user fields → settings.json
    /// - Everything else → .env
    pub fn save_to_env(&self, key: &str, value: &str) -> anyhow::Result<()> {
        self.save_to_env_many(&[(key, value)])
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
        let mut legacy_hold_override: Option<HoldMods> = None;
        let mut legacy_toggle_override: Option<ToggleTrigger> = None;
        let mut pending_env_updates: Vec<(String, String)> = Vec::new();

        for (key, value) in entries {
            // API keys → Keychain
            if super::keychain::KEYCHAIN_ACCOUNTS.contains(key) {
                super::keychain::save_key(key, value)?;
                pending_env_updates.push(((*key).to_string(), (*value).to_string()));
                continue;
            }

            if *key == "HOLD_MODS" || *key == "TOGGLE_TRIGGER" {
                if *key == "HOLD_MODS" {
                    legacy_hold_override =
                        Some(value.parse::<HoldMods>().map_err(|e| anyhow::anyhow!(e))?);
                } else {
                    legacy_toggle_override = Some(
                        value
                            .parse::<ToggleTrigger>()
                            .map_err(|e| anyhow::anyhow!(e))?,
                    );
                }

                let path = env_path.get_or_insert_with(Self::env_path).clone();
                let vars_ref = env_vars.get_or_insert_with(|| {
                    if path.exists() {
                        Self::parse_env_file(&path).unwrap_or_default()
                    } else {
                        HashMap::new()
                    }
                });
                vars_ref.insert((*key).to_string(), (*value).to_string());
                pending_env_updates.push(((*key).to_string(), (*value).to_string()));
                continue;
            }

            // Regular-user fields → settings.json
            let is_regular = super::settings::is_promoted_key(key);

            if is_regular {
                let settings_ref = settings.get_or_insert_with(super::settings::UserSettings::load);
                let normalized_value = match *key {
                    // ── Strings ──
                    "WHISPER_LANGUAGE" => {
                        let normalized = normalize_language_value(key, value)?;
                        settings_ref.whisper_language = Some(normalized.clone());
                        normalized
                    }
                    "LLM_ENDPOINT" => {
                        let normalized = value.trim().to_string();
                        settings_ref.llm_endpoint = Some(normalized.clone());
                        normalized
                    }
                    "LLM_MODEL" => {
                        let normalized = value.trim().to_string();
                        settings_ref.llm_model = Some(normalized.clone());
                        normalized
                    }
                    "LLM_ASSISTIVE_ENDPOINT" => {
                        let normalized = value.trim().to_string();
                        settings_ref.llm_assistive_endpoint = Some(normalized.clone());
                        normalized
                    }
                    "LLM_ASSISTIVE_MODEL" => {
                        let normalized = value.trim().to_string();
                        settings_ref.llm_assistive_model = Some(normalized.clone());
                        normalized
                    }
                    "FORMATTING_LEVEL" => {
                        let normalized = normalize_formatting_level_value(key, value)?;
                        settings_ref.formatting_level = Some(normalized.clone());
                        normalized
                    }
                    "LLM_FORMATTING_ENDPOINT" => {
                        let normalized = value.trim().to_string();
                        settings_ref.llm_formatting_endpoint = Some(normalized.clone());
                        normalized
                    }
                    "LLM_FORMATTING_MODEL" => {
                        let normalized = value.trim().to_string();
                        settings_ref.llm_formatting_model = Some(normalized.clone());
                        normalized
                    }
                    "LOCAL_MODEL" => {
                        let normalized = value.trim().to_string();
                        settings_ref.local_model = Some(normalized.clone());
                        normalized
                    }
                    "STT_ENDPOINT" => {
                        let normalized = value.trim().to_string();
                        settings_ref.stt_endpoint = Some(normalized.clone());
                        normalized
                    }
                    "TRANSCRIPT_SEND_MODE" => {
                        let normalized = normalize_transcript_send_mode_value(key, value)?;
                        settings_ref.transcript_send_mode = Some(normalized.clone());
                        normalized
                    }
                    "AUDIO_INPUT_DEVICE" => {
                        let normalized = value.trim().to_string();
                        settings_ref.audio_input_device = Some(normalized.clone());
                        normalized
                    }
                    "SOUND_NAME" => {
                        let normalized = value.trim().to_string();
                        settings_ref.sound_name = Some(normalized.clone());
                        normalized
                    }
                    "WHISPER_MODEL" => {
                        let normalized = value.trim().to_string();
                        settings_ref.whisper_model = Some(normalized.clone());
                        normalized
                    }
                    // ── u64 ──
                    "HOLD_START_DELAY_MS" => {
                        let v = parse_promoted_u64(key, value)?;
                        settings_ref.hold_start_delay_ms = Some(v);
                        v.to_string()
                    }
                    "DOUBLE_TAP_INTERVAL_MS" => {
                        let v = parse_promoted_u64(key, value)?;
                        settings_ref.double_tap_interval_ms = Some(v);
                        v.to_string()
                    }
                    "CODESCRIBE_BUFFER_DELAY_MS" => {
                        let v = parse_promoted_u64(key, value)?;
                        settings_ref.buffer_delay_ms = Some(v);
                        v.to_string()
                    }
                    "CODESCRIBE_EMIT_WORDS_MAX" => {
                        let v = parse_promoted_u64(key, value)?;
                        settings_ref.emit_words_max = Some(v);
                        v.to_string()
                    }
                    "BACKEND_MAX_UPLOAD_MB" => {
                        let v = parse_promoted_u64(key, value)?;
                        settings_ref.backend_max_upload_mb = Some(v);
                        v.to_string()
                    }
                    // ── f32 ──
                    "TOGGLE_SILENCE_SEC" => {
                        let v = parse_promoted_f32(key, value)?;
                        settings_ref.toggle_silence_sec = Some(v);
                        v.to_string()
                    }
                    "CODESCRIBE_TYPING_CPS" => {
                        let v = parse_promoted_f32(key, value)?;
                        settings_ref.typing_cps = Some(v);
                        v.to_string()
                    }
                    "CODESCRIBE_BUFFERED_INTERIM_SEC" => {
                        let v = parse_promoted_f32(key, value)?;
                        settings_ref.buffered_interim_sec = Some(v);
                        v.to_string()
                    }
                    "SOUND_VOLUME" => {
                        let v = parse_promoted_f32(key, value)?;
                        settings_ref.sound_volume = Some(v);
                        v.to_string()
                    }
                    // ── Bools ──
                    "AI_FORMATTING_ENABLED"
                    | "BEEP_ON_START"
                    | "SHOW_DOCK_ICON"
                    | "TRANSCRIPTION_OVERLAY_ENABLED"
                    | "HOLD_EXCLUSIVE"
                    | "USE_LOCAL_STT"
                    | "HISTORY_ENABLED"
                    | "QUICK_NOTES_ENABLED"
                    | "QUICK_NOTES_SAVE_ONLY"
                    | "START_AT_LOGIN"
                    | "CODESCRIBE_AUTOSTART_QUALITY_DAEMON"
                    | "AGENT_ENTER_SENDS" => {
                        let bv = parse_promoted_bool(key, value)?;
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
                            "CODESCRIBE_AUTOSTART_QUALITY_DAEMON" => {
                                settings_ref.quality_daemon_autostart = Some(bv)
                            }
                            "AGENT_ENTER_SENDS" => settings_ref.agent_enter_sends = Some(bv),
                            _ => {}
                        }
                        if bv { "1".to_string() } else { "0".to_string() }
                    }
                    _ => value.trim().to_string(),
                };
                pending_env_updates.push(((*key).to_string(), normalized_value));
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
            pending_env_updates.push(((*key).to_string(), (*value).to_string()));
        }

        if legacy_hold_override.is_some() || legacy_toggle_override.is_some() {
            let settings_ref = settings.get_or_insert_with(super::settings::UserSettings::load);
            let hold = legacy_hold_override.unwrap_or_else(|| settings_ref.legacy_hold_mods());
            let toggle =
                legacy_toggle_override.unwrap_or_else(|| settings_ref.legacy_toggle_trigger());
            settings_ref.apply_legacy_hotkeys(hold, toggle);
        }

        if let Some(settings) = settings {
            settings.save()?;
        }
        if let (Some(path), Some(vars)) = (env_path, env_vars) {
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent)?;
            }
            Self::write_env_file(&path, &vars)?;
        }

        for (key, value) in pending_env_updates {
            unsafe { std::env::set_var(&key, &value) };
        }

        Ok(())
    }

    /// Parse .env file into HashMap.
    pub fn parse_env_file(path: &PathBuf) -> anyhow::Result<HashMap<String, String>> {
        // Path comes from Config::env_path() which is hardcoded to ~/.codescribe/.env
        // nosemgrep: tainted-path
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
            output_lines.push("# Added by CodeScribe".to_string());

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

fn parse_bool_value(raw: &str) -> Option<bool> {
    let normalized = raw.trim().to_lowercase();
    match normalized.as_str() {
        "1" | "true" | "yes" | "on" | "enabled" => Some(true),
        "0" | "false" | "no" | "off" | "disabled" => Some(false),
        _ => None,
    }
}

fn parse_env_bool(raw: &str, key: &str) -> Option<bool> {
    match parse_bool_value(raw) {
        Some(value) => Some(value),
        None => {
            warn!("Ignoring invalid {key} value in environment: {raw}");
            None
        }
    }
}

fn env_var_has_valid_bool(key: &str) -> bool {
    std::env::var(key)
        .ok()
        .and_then(|raw| parse_bool_value(&raw))
        .is_some()
}

fn env_var_parses<T>(key: &str) -> bool
where
    T: std::str::FromStr,
{
    std::env::var(key)
        .ok()
        .is_some_and(|raw| raw.parse::<T>().is_ok())
}

fn parse_use_local_stt(raw: &str, source: &str) -> Option<bool> {
    match parse_bool_value(raw) {
        Some(value) => Some(value),
        None => {
            warn!("Ignoring invalid USE_LOCAL_STT value in {source}: {raw}");
            None
        }
    }
}

fn parse_promoted_bool(key: &str, value: &str) -> anyhow::Result<bool> {
    let normalized = value.trim().to_lowercase();
    match normalized.as_str() {
        "1" | "true" | "yes" | "on" | "enabled" => Ok(true),
        "0" | "false" | "no" | "off" | "disabled" => Ok(false),
        _ => Err(anyhow::anyhow!(
            "Invalid value for {key}: expected boolean-compatible value, got {:?}",
            value
        )),
    }
}

fn parse_promoted_u64(key: &str, value: &str) -> anyhow::Result<u64> {
    value.trim().parse::<u64>().map_err(|error| {
        anyhow::anyhow!(
            "Invalid value for {key}: expected unsigned integer, got {:?}: {error}",
            value
        )
    })
}

fn parse_promoted_f32(key: &str, value: &str) -> anyhow::Result<f32> {
    value.trim().parse::<f32>().map_err(|error| {
        anyhow::anyhow!(
            "Invalid value for {key}: expected decimal number, got {:?}: {error}",
            value
        )
    })
}

fn normalize_language_value(key: &str, value: &str) -> anyhow::Result<String> {
    value
        .trim()
        .parse::<Language>()
        .map(|language| language.as_str().to_string())
        .map_err(|error| anyhow::anyhow!("Invalid value for {key}: {error}"))
}

fn normalize_transcript_send_mode_value(key: &str, value: &str) -> anyhow::Result<String> {
    value
        .trim()
        .parse::<TranscriptSendMode>()
        .map(|mode| mode.as_str().to_string())
        .map_err(|error| anyhow::anyhow!("Invalid value for {key}: {error}"))
}

fn normalize_formatting_level_value(key: &str, value: &str) -> anyhow::Result<String> {
    let normalized = value.trim().to_lowercase();
    match normalized.as_str() {
        "raw" | "medium" | "creative" => Ok(normalized),
        _ => Err(anyhow::anyhow!(
            "Invalid value for {key}: expected one of raw|medium|creative, got {:?}",
            value
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::UserSettings;
    use serial_test::serial;
    use std::fs;
    use tempfile::TempDir;

    fn setup_isolated_data_dir() -> TempDir {
        let tmp = TempDir::new().expect("tempdir");
        unsafe {
            std::env::set_var("CODESCRIBE_DATA_DIR", tmp.path());
            for key in [
                "USE_LOCAL_STT",
                "QUICK_NOTES_ENABLED",
                "QUICK_NOTES_SAVE_ONLY",
                "WHISPER_LANGUAGE",
                "SHOW_TRAY_GLYPH",
                "AI_FORMATTING_ENABLED",
                "HOLD_START_DELAY_MS",
            ] {
                std::env::remove_var(key);
            }
        }
        tmp
    }

    #[test]
    fn test_hotkey_timing_params_applied_from_settings() {
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
    fn test_load_ignores_invalid_bool_env_and_keeps_settings_value() {
        let _tmp = setup_isolated_data_dir();
        unsafe {
            std::env::set_var("QUICK_NOTES_ENABLED", "definitely");
        }

        let mut settings = UserSettings::load();
        settings.quick_notes_enabled = Some(true);
        settings.save().expect("save settings");

        let config = Config::load();
        assert!(
            config.quick_notes_enabled,
            "invalid bool env should not suppress a valid settings.json value"
        );
    }

    #[test]
    #[serial]
    fn test_load_ignores_invalid_enum_env_and_keeps_settings_value() {
        let _tmp = setup_isolated_data_dir();
        unsafe {
            std::env::set_var("WHISPER_LANGUAGE", "de");
        }

        let mut settings = UserSettings::load();
        settings.whisper_language = Some("en".to_string());
        settings.save().expect("save settings");

        let config = Config::load();
        assert_eq!(
            config.whisper_language,
            Language::English,
            "invalid enum env should not suppress a valid settings.json value"
        );
    }

    #[test]
    #[serial]
    fn test_load_ignores_invalid_unbacked_bool_env_and_keeps_default() {
        let _tmp = setup_isolated_data_dir();
        unsafe {
            std::env::set_var("SHOW_TRAY_GLYPH", "mystery");
        }

        let config = Config::load();
        assert!(
            config.show_tray_glyph,
            "invalid bool env should leave default-true unbacked flags untouched"
        );
    }

    #[test]
    #[serial]
    fn test_save_to_env_rejects_invalid_promoted_bool_values() {
        let _tmp = setup_isolated_data_dir();
        unsafe {
            std::env::remove_var("AI_FORMATTING_ENABLED");
        }

        let config = Config::load();
        let error = config
            .save_to_env("AI_FORMATTING_ENABLED", "sometimes")
            .expect_err("invalid bool should fail fast");

        assert!(error.to_string().contains("AI_FORMATTING_ENABLED"));
        assert!(
            std::env::var("AI_FORMATTING_ENABLED").is_err(),
            "invalid bool must not leak into runtime env"
        );

        let settings = UserSettings::load();
        assert_eq!(settings.ai_formatting_enabled, None);
    }

    #[test]
    #[serial]
    fn test_save_to_env_rejects_invalid_promoted_numeric_values() {
        let _tmp = setup_isolated_data_dir();
        unsafe {
            std::env::remove_var("HOLD_START_DELAY_MS");
        }

        let config = Config::load();
        let error = config
            .save_to_env("HOLD_START_DELAY_MS", "fast")
            .expect_err("invalid numeric should fail fast");

        assert!(error.to_string().contains("HOLD_START_DELAY_MS"));
        assert!(
            std::env::var("HOLD_START_DELAY_MS").is_err(),
            "invalid numeric must not leak into runtime env"
        );

        let settings = UserSettings::load();
        assert_eq!(settings.hold_start_delay_ms, None);
    }

    #[test]
    #[serial]
    fn test_save_to_env_rejects_invalid_promoted_enum_values() {
        let _tmp = setup_isolated_data_dir();
        unsafe {
            std::env::remove_var("WHISPER_LANGUAGE");
        }

        let config = Config::load();
        let error = config
            .save_to_env("WHISPER_LANGUAGE", "de")
            .expect_err("invalid enum should fail fast");

        assert!(error.to_string().contains("WHISPER_LANGUAGE"));
        assert!(
            std::env::var("WHISPER_LANGUAGE").is_err(),
            "invalid enum must not leak into runtime env"
        );

        let settings = UserSettings::load();
        assert_eq!(settings.whisper_language, None);
    }

    #[test]
    #[serial]
    fn test_save_to_env_many_is_atomic_when_validation_fails() {
        let _tmp = setup_isolated_data_dir();
        unsafe {
            std::env::remove_var("QUICK_NOTES_ENABLED");
            std::env::remove_var("HOLD_START_DELAY_MS");
        }

        let config = Config::load();
        config
            .save_to_env_many(&[
                ("QUICK_NOTES_ENABLED", "1"),
                ("HOLD_START_DELAY_MS", "still not a number"),
            ])
            .expect_err("batch save should abort when any promoted value is invalid");

        assert!(
            std::env::var("QUICK_NOTES_ENABLED").is_err(),
            "batch failure should not leak earlier valid values into runtime env"
        );
        assert!(
            std::env::var("HOLD_START_DELAY_MS").is_err(),
            "batch failure should keep the invalid value out of runtime env"
        );

        let settings = UserSettings::load();
        assert_eq!(settings.quick_notes_enabled, None);
        assert_eq!(settings.hold_start_delay_ms, None);
    }
}
