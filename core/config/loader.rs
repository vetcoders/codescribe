//! Configuration loading and saving functionality.
//!
//! Handles loading from .env file and environment variables.
//! Single source of truth: ~/.codescribe/.env

use directories::BaseDirs;
use std::collections::HashMap;
use std::fs;
use std::io::Write;
use std::path::PathBuf;
use tracing::{debug, info, warn};

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
        // Ensure we have a user .env (copy from template if present)
        Self::ensure_env_file();

        // One-time migration from legacy keys inside .env
        Self::migrate_env_legacy_keys();

        // Load .env file if it exists
        let env_path = Self::env_path();
        if env_path.exists() {
            let _ = dotenvy::from_path(&env_path);
        }

        let mut config = Self::default();

        // Override with environment variables
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
        // SILENCE_DB deprecated - Silero VAD uses probability threshold (CODESCRIBE_VAD_THRESHOLD)
        // Kept for backward compatibility but not used
        if let Ok(val) = std::env::var("SILENCE_DB")
            && let Ok(db) = val.parse()
        {
            self.silence_db = db;
        }
        // Prefer new VAD naming, fallback to legacy SILENCE_HANG_SEC
        if let Ok(val) = std::env::var("CODESCRIBE_VAD_MAX_SILENCE_SEC")
            && let Ok(sec) = val.parse::<f32>()
        {
            self.silence_hang_sec = sec.clamp(0.1, 10.0);
        } else if let Ok(val) = std::env::var("SILENCE_HANG_SEC")
            && let Ok(sec) = val.parse::<f32>()
        {
            self.silence_hang_sec = sec.clamp(0.1, 10.0);
        }

        // History (always on to avoid data loss)
        if let Ok(val) = std::env::var("HISTORY_ENABLED") {
            self.history_enabled = val.parse().unwrap_or(true);
        }
        self.history_enabled = true;

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

        // Debugging (always on to keep paired .wav with transcripts)
        if let Ok(val) = std::env::var("DUMP_AUDIO_LOGS") {
            self.dump_audio_logs = matches!(val.as_str(), "1" | "true" | "yes" | "on");
        }
        self.dump_audio_logs = true;
    }

    /// Save a single configuration value to .env file.
    ///
    /// This updates the .env file by:
    /// 1. Reading existing content
    /// 2. Updating/adding the specified key
    /// 3. Writing back to file
    ///
    /// # Arguments
    /// * `key` - Environment variable name (e.g., "BEEP_ON_START")
    /// * `value` - Value to save
    pub fn save_to_env(&self, key: &str, value: &str) -> anyhow::Result<()> {
        let env_path = Self::env_path();

        // Ensure config directory exists
        if let Some(parent) = env_path.parent() {
            fs::create_dir_all(parent)?;
        }

        // Read existing .env content
        let mut env_vars = if env_path.exists() {
            Self::parse_env_file(&env_path)?
        } else {
            HashMap::new()
        };

        // Update the specific key
        env_vars.insert(key.to_string(), value.to_string());

        // Write back to file (persists for next app launch)
        Self::write_env_file(&env_path, &env_vars)?;

        // Also update runtime env var (dotenvy doesn't override existing vars)
        // SAFETY: Called from main thread during menu interaction, single-threaded context
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

    /// Write HashMap to .env file.
    pub fn write_env_file(path: &PathBuf, vars: &HashMap<String, String>) -> anyhow::Result<()> {
        // Path comes from Config::env_path() which is hardcoded to ~/.codescribe/.env
        // nosemgrep: tainted-path
        let mut file = fs::File::create(path)?;

        writeln!(file, "# CodeScribe Configuration")?;
        writeln!(file, "# Generated automatically - edit with care")?;
        writeln!(file)?;

        // Sort keys for consistent output
        let mut keys: Vec<_> = vars.keys().collect();
        keys.sort();

        for key in keys {
            if let Some(value) = vars.get(key) {
                writeln!(file, "{}={}", key, value)?;
            }
        }

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

    /// Ensure the user .env exists by copying from the template if available.
    /// Falls back to an empty generated file with header comments.
    fn ensure_env_file() {
        let env_path = Self::env_path();

        if env_path.exists() {
            return;
        }

        // Build candidate template locations (highest to lowest priority)
        let mut candidates: Vec<PathBuf> = Vec::new();

        // 1) Explicit override
        if let Ok(custom) = std::env::var("CODESCRIBE_ENV_TEMPLATE") {
            candidates.push(PathBuf::from(shellexpand::tilde(&custom).into_owned()));
        }

        // 2) Bundle Resources/.env.example (when running from .app)
        if let Ok(exe) = std::env::current_exe()
            && let Some(dir) = exe.parent()
        {
            candidates.push(dir.join("../Resources/.env.example"));
        }

        // 3) Repo root .env.example (dev mode; CARGO_MANIFEST_DIR points to codescribe-rs)
        let repo_root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..");
        candidates.push(repo_root.join(".env.example"));

        // 4) Current working directory .env.example as a last resort
        if let Ok(cwd) = std::env::current_dir() {
            candidates.push(cwd.join(".env.example"));
        }

        // Find the first existing template
        if let Some(template) = candidates.into_iter().find(|p| p.exists()) {
            if let Some(parent) = env_path.parent()
                && let Err(e) = fs::create_dir_all(parent)
            {
                warn!("Failed to create config dir for .env: {}", e);
                return;
            }

            match fs::copy(&template, &env_path) {
                Ok(_) => info!("Created user .env from template at {}", template.display()),
                Err(e) => warn!(
                    "Failed to copy .env template from {}: {}",
                    template.display(),
                    e
                ),
            }
            return;
        }

        // No template found: generate minimal file with headers
        let vars = HashMap::new();
        if let Err(e) = Self::write_env_file(&env_path, &vars) {
            warn!("Failed to generate default .env: {}", e);
        } else {
            debug!("Generated empty .env with default header (no template found)");
        }
    }

    /// Get the configuration directory path (`$HOME/.codescribe`).
    ///
    /// Can be overridden with `CODESCRIBE_DATA_DIR` or `CODESCRIBE_APP_DIR`
    /// environment variables.
    pub fn config_dir() -> PathBuf {
        // Check for environment variable overrides
        if let Ok(custom) = std::env::var("CODESCRIBE_DATA_DIR") {
            return PathBuf::from(shellexpand::tilde(&custom).into_owned());
        }

        if let Ok(custom) = std::env::var("CODESCRIBE_APP_DIR") {
            return PathBuf::from(shellexpand::tilde(&custom).into_owned());
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
