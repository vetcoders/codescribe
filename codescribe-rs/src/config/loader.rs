//! Configuration loading and saving functionality.
//!
//! Handles loading from .env files, settings.json (legacy), and environment variables.

use directories::BaseDirs;
use std::collections::HashMap;
use std::fs;
use std::io::Write;
use std::path::PathBuf;

use super::types::{AiProvider, Config, HoldMods, Language, ToggleTrigger};

impl Config {
    /// Load configuration from disk or environment.
    ///
    /// Priority order:
    /// 1. Environment variables
    /// 2. .env file in config directory
    /// 3. settings.json (legacy)
    /// 4. Default values
    ///
    /// If the files don't exist or are malformed, returns default configuration
    /// without raising an error.
    pub fn load() -> Self {
        // Load .env file if it exists
        let env_path = Self::env_path();
        if env_path.exists() {
            let _ = dotenvy::from_path(&env_path);
        }

        let mut config = Self::default();

        // Try loading from settings.json (legacy)
        let json_path = Self::settings_path();
        if json_path.exists() {
            if let Ok(contents) = fs::read_to_string(&json_path) {
                if let Ok(json_config) = serde_json::from_str::<Config>(&contents) {
                    config = json_config;
                }
            }
        }

        // Override with environment variables
        config.load_from_env();
        config.sanitize();
        config
    }

    /// Load configuration values from environment variables.
    pub fn load_from_env(&mut self) {
        // Hotkeys
        if let Ok(val) = std::env::var("HOLD_MODS") {
            if let Ok(mods) = val.parse::<HoldMods>() {
                self.hold_mods = mods;
            }
        }
        if let Ok(val) = std::env::var("HOLD_EXCLUSIVE") {
            self.hold_exclusive = val.parse().unwrap_or(false);
        }
        if let Ok(val) = std::env::var("TOGGLE_TRIGGER") {
            if let Ok(trigger) = val.parse::<ToggleTrigger>() {
                self.toggle_trigger = trigger;
            }
        }
        if let Ok(val) = std::env::var("HOLD_START_DELAY_MS") {
            if let Ok(ms) = val.parse() {
                self.hold_start_delay_ms = ms;
            }
        }

        // Language
        if let Ok(val) = std::env::var("WHISPER_LANGUAGE") {
            if let Ok(lang) = val.parse::<Language>() {
                self.whisper_language = lang;
            }
        }

        // AI Formatting
        if let Ok(val) = std::env::var("AI_FORMATTING_ENABLED") {
            self.ai_formatting_enabled = val.parse().unwrap_or(false);
        }
        if let Ok(val) = std::env::var("AI_PROVIDER") {
            if let Ok(provider) = val.parse::<AiProvider>() {
                self.ai_provider = provider;
            }
        }
        if let Ok(val) = std::env::var("AI_MAX_TOKENS") {
            if let Ok(tokens) = val.parse() {
                self.ai_max_tokens = tokens;
            }
        }
        if let Ok(val) = std::env::var("AI_ASSISTIVE_MAX_TOKENS") {
            if let Ok(tokens) = val.parse() {
                self.ai_assistive_max_tokens = tokens;
            }
        }

        // UI
        if let Ok(val) = std::env::var("SHOW_TRAY_GLYPH") {
            self.show_tray_glyph = val.parse().unwrap_or(true);
        }
        if let Ok(val) = std::env::var("HOLD_INDICATOR") {
            self.hold_indicator = val.parse().unwrap_or(true);
        }
        if let Ok(val) = std::env::var("HOLD_BADGE_SIZE") {
            if let Ok(size) = val.parse() {
                self.hold_badge_size = size;
            }
        }
        if let Ok(val) = std::env::var("HOLD_BADGE_OFFSET_X") {
            if let Ok(offset) = val.parse() {
                self.hold_badge_offset_x = offset;
            }
        }
        if let Ok(val) = std::env::var("HOLD_BADGE_OFFSET_Y") {
            if let Ok(offset) = val.parse() {
                self.hold_badge_offset_y = offset;
            }
        }

        // Sound
        if let Ok(val) = std::env::var("BEEP_ON_START") {
            self.beep_on_start = val.parse().unwrap_or(true);
        }
        if let Ok(val) = std::env::var("SOUND_NAME") {
            self.sound_name = val;
        }
        if let Ok(val) = std::env::var("SOUND_VOLUME") {
            if let Ok(volume) = val.parse() {
                self.sound_volume = volume;
            }
        }

        // History
        if let Ok(val) = std::env::var("HISTORY_ENABLED") {
            self.history_enabled = val.parse().unwrap_or(true);
        }

        // Backends
        if let Ok(val) = std::env::var("WHISPER_SERVER_URL") {
            self.whisper_server_url = val;
        }
        if let Ok(val) = std::env::var("LLM_SERVER_URL") {
            self.llm_server_url = val;
        }
        if let Ok(val) = std::env::var("OLLAMA_HOST") {
            self.ollama_host = val;
        }
        if let Ok(val) = std::env::var("OLLAMA_MODEL") {
            self.ollama_model = val;
        }

        // Clipboard
        if let Ok(val) = std::env::var("RESTORE_CLIPBOARD") {
            self.restore_clipboard = val.parse().unwrap_or(true);
        }
        if let Ok(val) = std::env::var("RESTORE_CLIPBOARD_DELAY_MS") {
            if let Ok(delay) = val.parse() {
                self.restore_clipboard_delay_ms = delay;
            }
        }

        // System
        if let Ok(val) = std::env::var("START_AT_LOGIN") {
            self.start_at_login = val.parse().unwrap_or(false);
        }
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

        // Write back to file
        Self::write_env_file(&env_path, &env_vars)?;

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

        // Default to $HOME/.codescribe (lowercase!)
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

    /// Get the full path to the settings.json file (legacy).
    pub fn settings_path() -> PathBuf {
        // Check for custom settings path
        if let Ok(custom) = std::env::var("CODESCRIBE_SETTINGS_PATH") {
            return PathBuf::from(shellexpand::tilde(&custom).into_owned());
        }

        Self::config_dir().join("settings.json")
    }
}
