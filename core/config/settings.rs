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
    pub formatting_level: Option<String>,
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub chat_zoom: Option<f64>,

    // ── Promoted from .env (settings.json is now source of truth) ──
    #[serde(skip_serializing_if = "Option::is_none")]
    pub llm_formatting_endpoint: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub llm_formatting_model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub use_local_stt: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub local_model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stt_endpoint: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub transcript_send_mode: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub audio_input_device: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sound_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub history_enabled: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub quick_notes_enabled: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub quick_notes_save_only: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub start_at_login: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent_enter_sends: Option<bool>,

    // ── Voice Lab survivors (user-facing UX knobs) ──
    #[serde(skip_serializing_if = "Option::is_none")]
    pub buffer_delay_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub typing_cps: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub emit_words_max: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub buffered_interim_sec: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub whisper_model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub backend_max_upload_mb: Option<u64>,
}

/// Canonical list of env keys that route to `settings.json` (not `.env`).
///
/// Used by `Config::save_to_env`, `Config::save_to_env_many`, and IPC
/// `persist_config` to decide whether a key is "promoted" (GUI-managed)
/// or power-user (.env-managed).
///
/// **Single source of truth** — add new promoted keys here only.
pub const PROMOTED_SETTINGS_KEYS: &[&str] = &[
    // Hotkeys
    "WHISPER_LANGUAGE",
    "HOLD_MODS",
    "HOLD_START_DELAY_MS",
    "DOUBLE_TAP_INTERVAL_MS",
    "TOGGLE_SILENCE_SEC",
    "HOLD_EXCLUSIVE",
    "TOGGLE_TRIGGER",
    "HOTKEY_DOUBLE_TAP_LEFT",
    "HOTKEY_DOUBLE_TAP_RIGHT",
    // AI / Formatting
    "AI_FORMATTING_ENABLED",
    "CODESCRIBE_BUFFERED_STREAM",
    "FORMATTING_LEVEL",
    // Sound
    "BEEP_ON_START",
    "SOUND_VOLUME",
    "SOUND_NAME",
    // LLM endpoints
    "LLM_ENDPOINT",
    "LLM_MODEL",
    "LLM_ASSISTIVE_ENDPOINT",
    "LLM_ASSISTIVE_MODEL",
    "LLM_FORMATTING_ENDPOINT",
    "LLM_FORMATTING_MODEL",
    // Promoted from .env
    "USE_LOCAL_STT",
    "LOCAL_MODEL",
    "STT_ENDPOINT",
    "TRANSCRIPT_SEND_MODE",
    "AUDIO_INPUT_DEVICE",
    "HISTORY_ENABLED",
    "QUICK_NOTES_ENABLED",
    "QUICK_NOTES_SAVE_ONLY",
    "START_AT_LOGIN",
    "AGENT_ENTER_SENDS",
    // Voice Lab survivors
    "CODESCRIBE_BUFFER_DELAY_MS",
    "CODESCRIBE_TYPING_CPS",
    "CODESCRIBE_EMIT_WORDS_MAX",
    "CODESCRIBE_BUFFERED_INTERIM_SEC",
    "WHISPER_MODEL",
    "BACKEND_MAX_UPLOAD_MB",
];

/// Check if a key is a promoted (settings.json) setting.
pub fn is_promoted_key(key: &str) -> bool {
    PROMOTED_SETTINGS_KEYS.contains(&key)
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
            "FORMATTING_LEVEL" => self.formatting_level = Some(value.to_owned()),
            "LLM_FORMATTING_ENDPOINT" => self.llm_formatting_endpoint = Some(value.to_owned()),
            "LLM_FORMATTING_MODEL" => self.llm_formatting_model = Some(value.to_owned()),
            "LOCAL_MODEL" => self.local_model = Some(value.to_owned()),
            "STT_ENDPOINT" => self.stt_endpoint = Some(value.to_owned()),
            "TRANSCRIPT_SEND_MODE" => self.transcript_send_mode = Some(value.to_owned()),
            "AUDIO_INPUT_DEVICE" => self.audio_input_device = Some(value.to_owned()),
            "SOUND_NAME" => self.sound_name = Some(value.to_owned()),
            "WHISPER_MODEL" => self.whisper_model = Some(value.to_owned()),
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
            "USE_LOCAL_STT" => self.use_local_stt = Some(value),
            "HISTORY_ENABLED" => self.history_enabled = Some(value),
            "QUICK_NOTES_ENABLED" => self.quick_notes_enabled = Some(value),
            "QUICK_NOTES_SAVE_ONLY" => self.quick_notes_save_only = Some(value),
            "START_AT_LOGIN" => self.start_at_login = Some(value),
            "AGENT_ENTER_SENDS" => self.agent_enter_sends = Some(value),
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
            "CODESCRIBE_BUFFER_DELAY_MS" => self.buffer_delay_ms = Some(value),
            "CODESCRIBE_EMIT_WORDS_MAX" => self.emit_words_max = Some(value),
            "BACKEND_MAX_UPLOAD_MB" => self.backend_max_upload_mb = Some(value),
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
            "CODESCRIBE_TYPING_CPS" => self.typing_cps = Some(value),
            "CODESCRIBE_BUFFERED_INTERIM_SEC" => self.buffered_interim_sec = Some(value),
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
