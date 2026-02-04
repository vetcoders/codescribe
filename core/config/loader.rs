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
        // One-time migration from .env-only to tiered config
        super::migrate::migrate_if_needed();

        // Load .env file if it exists (power-user overrides only)
        // In production, .env doesn't exist — regular users use settings.json
        let env_path = Self::env_path();
        if env_path.exists() {
            // Migrate legacy keys inside existing .env (power users only)
            Self::migrate_env_legacy_keys();
            let _ = dotenvy::from_path(&env_path);
        }

        // Load API keys from Keychain (only if not already set by .env)
        super::keychain::populate_env_from_keychain();

        // Load user settings from JSON
        let mut user_settings = super::settings::UserSettings::load();
        Self::migrate_hotkey_settings(&mut user_settings);

        let mut config = Self::default();

        // Apply user settings first (lowest priority after defaults)
        config.apply_user_settings(&user_settings);

        // Override with environment variables (.env + runtime; highest priority)
        config.load_from_env();
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
        if let Ok(val) = std::env::var("HOLD_EXCLUSIVE") {
            self.hold_exclusive = matches!(val.as_str(), "1" | "true" | "yes" | "on");
        }
        if let Ok(val) = std::env::var("TOGGLE_TRIGGER")
            && let Ok(trigger) = val.parse::<ToggleTrigger>()
        {
            self.toggle_trigger = trigger;
        }
        if let Ok(val) = std::env::var("HOLD_START_DELAY_MS")
            && let Ok(ms) = val.parse()
        {
            self.hold_start_delay_ms = ms;
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
        // VAD config is managed by core/vad/config.rs (CODESCRIBE_VAD_* env vars)
        // No legacy SILENCE_* variables - single source of truth

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
        apply_parsed_if_no_env!("HOLD_MODS", self.hold_mods, settings.hold_mods);
        apply_parsed_if_no_env!(
            "TOGGLE_TRIGGER",
            self.toggle_trigger,
            settings.toggle_trigger
        );
        if std::env::var("HOLD_START_DELAY_MS").is_err()
            && let Some(v) = settings.hold_start_delay_ms
        {
            self.hold_start_delay_ms = v;
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
        // Sound
        if std::env::var("BEEP_ON_START").is_err()
            && let Some(v) = settings.beep_on_start
        {
            self.beep_on_start = v;
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
            unsafe { std::env::set_var("LLM_MODEL", v) };
        }
        // Double-tap toggles (read from env at runtime, not in Config struct)
        if std::env::var("HOTKEY_DOUBLE_TAP_LEFT").is_err()
            && let Some(v) = settings.double_tap_left
        {
            unsafe { std::env::set_var("HOTKEY_DOUBLE_TAP_LEFT", if v { "1" } else { "0" }) };
        }
        if std::env::var("HOTKEY_DOUBLE_TAP_RIGHT").is_err()
            && let Some(v) = settings.double_tap_right
        {
            unsafe { std::env::set_var("HOTKEY_DOUBLE_TAP_RIGHT", if v { "1" } else { "0" }) };
        }
        // Buffered stream (read from env at runtime)
        if std::env::var("CODESCRIBE_BUFFERED_STREAM").is_err()
            && let Some(v) = settings.buffered_stream
        {
            unsafe { std::env::set_var("CODESCRIBE_BUFFERED_STREAM", if v { "1" } else { "0" }) };
        }
    }

    /// Upgrade legacy hotkey settings stored in settings.json to the new toggle_trigger field.
    fn migrate_hotkey_settings(settings: &mut super::settings::UserSettings) {
        if settings.toggle_trigger.is_some() || std::env::var("TOGGLE_TRIGGER").is_ok() {
            return;
        }

        let left = settings.double_tap_left;
        let right = settings.double_tap_right;

        if left.is_none() && right.is_none() {
            return;
        }

        let derived = match (left.unwrap_or(false), right.unwrap_or(false)) {
            (true, true) => "double_option",
            (true, false) => "double_lalt",
            (false, true) => "double_ralt",
            (false, false) => "none",
        };

        settings.toggle_trigger = Some(derived.to_string());
        if let Err(e) = settings.save() {
            warn!("Failed to migrate hotkey settings to TOGGLE_TRIGGER: {e}");
        } else {
            info!("Migrated legacy HOTKEY_DOUBLE_TAP_* to TOGGLE_TRIGGER={derived}");
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
            // Also update runtime env var
            unsafe { std::env::set_var(key, value) };
            return Ok(());
        }

        // Regular-user fields → settings.json
        let is_regular = matches!(
            key,
            "WHISPER_LANGUAGE"
                | "HOLD_MODS"
                | "HOLD_START_DELAY_MS"
                | "HOLD_EXCLUSIVE"
                | "AI_FORMATTING_ENABLED"
                | "CODESCRIBE_BUFFERED_STREAM"
                | "BEEP_ON_START"
                | "SOUND_VOLUME"
                | "VAD_PRESET"
                | "LLM_ENDPOINT"
                | "LLM_MODEL"
                | "LLM_ASSISTIVE_ENDPOINT"
                | "LLM_ASSISTIVE_MODEL"
                | "TOGGLE_TRIGGER"
                | "HOTKEY_DOUBLE_TAP_LEFT"
                | "HOTKEY_DOUBLE_TAP_RIGHT"
        );

        if is_regular {
            let mut settings = super::settings::UserSettings::load();
            // Route to appropriate setter based on value type
            match key {
                "HOLD_START_DELAY_MS" => {
                    if let Ok(v) = value.parse::<u64>() {
                        settings.set_u64(key, v);
                    }
                }
                "SOUND_VOLUME" => {
                    if let Ok(v) = value.parse::<f32>() {
                        settings.set_f32(key, v);
                    }
                }
                "AI_FORMATTING_ENABLED"
                | "CODESCRIBE_BUFFERED_STREAM"
                | "BEEP_ON_START"
                | "HOLD_EXCLUSIVE"
                | "HOTKEY_DOUBLE_TAP_LEFT"
                | "HOTKEY_DOUBLE_TAP_RIGHT" => {
                    let bool_val = matches!(value, "1" | "true" | "yes" | "on");
                    settings.set_bool(key, bool_val);
                }
                _ => {
                    settings.set_string(key, value);
                }
            }
            // Also update runtime env var
            unsafe { std::env::set_var(key, value) };
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
        unsafe { std::env::set_var(key, value) };
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
    /// Can be overridden with `CODESCRIBE_DATA_DIR` or `CODESCRIBE_APP_DIR`
    /// environment variables.
    pub fn config_dir() -> PathBuf {
        // Helper to canonicalize if path exists (resolves macOS /var → /private/var)
        let maybe_canonicalize = |p: PathBuf| -> PathBuf { p.canonicalize().unwrap_or(p) };

        // Check for environment variable overrides
        if let Ok(custom) = std::env::var("CODESCRIBE_DATA_DIR") {
            return maybe_canonicalize(PathBuf::from(shellexpand::tilde(&custom).into_owned()));
        }

        if let Ok(custom) = std::env::var("CODESCRIBE_APP_DIR") {
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
