//! User-facing settings stored as JSON (GUI-managed).
//!
//! These are the "regular user" tier. Power users override via ~/.codescribe/.env.

use directories::BaseDirs;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use tracing::{debug, info, warn};

/// Regular-user settings (JSON, GUI-managed).
/// All fields are Option — None means "use default or .env override".
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct UserSettings {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub whisper_language: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hold_mods: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hold_exclusive: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub toggle_trigger: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hold_start_delay_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub double_tap_interval_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub toggle_silence_sec: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ai_formatting_enabled: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub buffered_stream: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub beep_on_start: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sound_volume: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vad_preset: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub llm_endpoint: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub llm_model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub llm_assistive_endpoint: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub llm_assistive_model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub double_tap_left: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub double_tap_right: Option<bool>,
}

impl UserSettings {
    /// Returns the settings directory.
    ///
    /// Respects `CODESCRIBE_DATA_DIR` for test isolation; otherwise uses
    /// `~/Library/Application Support/CodeScribe/`.
    pub fn settings_dir() -> PathBuf {
        let dir = if let Ok(test_dir) = std::env::var("CODESCRIBE_DATA_DIR") {
            PathBuf::from(test_dir)
        } else {
            BaseDirs::new()
                .map(|b| b.data_dir().join("CodeScribe"))
                .unwrap_or_else(|| {
                    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
                    PathBuf::from(home).join("Library/Application Support/CodeScribe")
                })
        };

        if !dir.exists()
            && let Err(e) = fs::create_dir_all(&dir)
        {
            warn!("Failed to create settings dir {}: {e}", dir.display());
        }
        dir
    }

    /// Returns the path to `settings.json`.
    pub fn settings_path() -> PathBuf {
        Self::settings_dir().join("settings.json")
    }

    /// Loads settings from disk. Returns `Default` on any error.
    pub fn load() -> Self {
        let path = Self::settings_path();
        match fs::read_to_string(&path) {
            Ok(contents) => match serde_json::from_str(&contents) {
                Ok(s) => {
                    debug!("Loaded settings from {}", path.display());
                    s
                }
                Err(e) => {
                    debug!("Failed to parse {}: {e}, using defaults", path.display());
                    Self::default()
                }
            },
            Err(e) => {
                debug!(
                    "No settings file at {} ({e}), using defaults",
                    path.display()
                );
                Self::default()
            }
        }
    }

    /// Persists current settings to disk as pretty-printed JSON.
    pub fn save(&self) -> anyhow::Result<()> {
        let dir = Self::settings_dir();
        fs::create_dir_all(&dir)?;
        let path = Self::settings_path();
        let json = serde_json::to_string_pretty(self)?;
        fs::write(&path, json)?;
        info!("Saved settings to {}", path.display());
        Ok(())
    }

    /// Sets a string-valued setting by its .env key name and saves.
    pub fn set_string(&mut self, key: &str, value: &str) {
        match key {
            "WHISPER_LANGUAGE" => self.whisper_language = Some(value.to_owned()),
            "HOLD_MODS" => self.hold_mods = Some(value.to_owned()),
            "TOGGLE_TRIGGER" => self.toggle_trigger = Some(value.to_owned()),
            "LLM_ENDPOINT" => self.llm_endpoint = Some(value.to_owned()),
            "LLM_MODEL" => self.llm_model = Some(value.to_owned()),
            "LLM_ASSISTIVE_ENDPOINT" => self.llm_assistive_endpoint = Some(value.to_owned()),
            "LLM_ASSISTIVE_MODEL" => self.llm_assistive_model = Some(value.to_owned()),
            "VAD_PRESET" => self.vad_preset = Some(value.to_owned()),
            other => {
                warn!("Unknown string setting key: {other}");
                return;
            }
        }
        if let Err(e) = self.save() {
            warn!("Failed to save after set_string({key}): {e}");
        }
    }

    /// Sets a boolean-valued setting by its .env key name and saves.
    pub fn set_bool(&mut self, key: &str, value: bool) {
        match key {
            "AI_FORMATTING_ENABLED" => self.ai_formatting_enabled = Some(value),
            "CODESCRIBE_BUFFERED_STREAM" => self.buffered_stream = Some(value),
            "BEEP_ON_START" => self.beep_on_start = Some(value),
            "HOLD_EXCLUSIVE" => self.hold_exclusive = Some(value),
            "HOTKEY_DOUBLE_TAP_LEFT" => self.double_tap_left = Some(value),
            "HOTKEY_DOUBLE_TAP_RIGHT" => self.double_tap_right = Some(value),
            other => {
                warn!("Unknown bool setting key: {other}");
                return;
            }
        }
        if let Err(e) = self.save() {
            warn!("Failed to save after set_bool({key}): {e}");
        }
    }

    /// Sets a u64-valued setting by its .env key name and saves.
    pub fn set_u64(&mut self, key: &str, value: u64) {
        match key {
            "HOLD_START_DELAY_MS" => self.hold_start_delay_ms = Some(value),
            "DOUBLE_TAP_INTERVAL_MS" => self.double_tap_interval_ms = Some(value),
            other => {
                warn!("Unknown u64 setting key: {other}");
                return;
            }
        }
        if let Err(e) = self.save() {
            warn!("Failed to save after set_u64({key}): {e}");
        }
    }

    /// Sets an f32-valued setting by its .env key name and saves.
    pub fn set_f32(&mut self, key: &str, value: f32) {
        match key {
            "SOUND_VOLUME" => self.sound_volume = Some(value),
            "TOGGLE_SILENCE_SEC" => self.toggle_silence_sec = Some(value),
            other => {
                warn!("Unknown f32 setting key: {other}");
                return;
            }
        }
        if let Err(e) = self.save() {
            warn!("Failed to save after set_f32({key}): {e}");
        }
    }
}
