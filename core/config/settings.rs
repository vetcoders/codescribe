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
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub show_dock_icon: Option<bool>,

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
    "FORMATTING_LEVEL",
    // Sound
    "BEEP_ON_START",
    "SOUND_VOLUME",
    "SOUND_NAME",
    // App visibility
    "SHOW_DOCK_ICON",
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

        if let Ok(existing) = fs::read_to_string(&path)
            && existing == json
        {
            debug!("Settings unchanged; skipping save to {}", path.display());
            return Ok(());
        }

        fs::write(&path, json)?;
        info!("Saved settings to {}", path.display());
        Ok(())
    }

    fn save_if_changed(&self, before: &Self, setter: &str, key: &str) {
        if self == before {
            debug!("{setter}({key}) ignored; value unchanged");
            return;
        }
        if let Err(e) = self.save() {
            warn!("Failed to save after {setter}({key}): {e}");
        }
    }

    /// Normalize zoom value into persisted representation.
    ///
    /// - Clamps to [0.75, 2.0]
    /// - Rounds to 2 decimals (prevents float jitter rewrite spam)
    /// - Stores `None` for effective default zoom (1.0)
    pub fn normalized_chat_zoom(zoom: f64) -> Option<f64> {
        let clamped = zoom.clamp(0.75, 2.0);
        let rounded = (clamped * 100.0).round() / 100.0;
        if (rounded - 1.0).abs() < 0.01 {
            None
        } else {
            Some(rounded)
        }
    }

    /// Set persisted chat zoom, saving only on effective value change.
    ///
    /// Returns `true` when a real setting change was applied.
    pub fn set_chat_zoom(&mut self, zoom: f64) -> bool {
        let normalized = Self::normalized_chat_zoom(zoom);
        if self.chat_zoom == normalized {
            debug!("set_chat_zoom ignored; value unchanged");
            return false;
        }

        self.chat_zoom = normalized;
        if let Err(e) = self.save() {
            warn!("Failed to save after set_chat_zoom: {e}");
        }
        true
    }

    /// Sets a string-valued setting by its .env key name and saves.
    pub fn set_string(&mut self, key: &str, value: &str) {
        let before = self.clone();
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
        self.save_if_changed(&before, "set_string", key);
    }

    /// Sets a boolean-valued setting by its .env key name and saves.
    pub fn set_bool(&mut self, key: &str, value: bool) {
        let before = self.clone();
        match key {
            "AI_FORMATTING_ENABLED" => self.ai_formatting_enabled = Some(value),
            "BEEP_ON_START" => self.beep_on_start = Some(value),
            "SHOW_DOCK_ICON" => self.show_dock_icon = Some(value),
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
        self.save_if_changed(&before, "set_bool", key);
    }

    /// Sets a u64-valued setting by its .env key name and saves.
    pub fn set_u64(&mut self, key: &str, value: u64) {
        let before = self.clone();
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
        self.save_if_changed(&before, "set_u64", key);
    }

    /// Sets an f32-valued setting by its .env key name and saves.
    pub fn set_f32(&mut self, key: &str, value: f32) {
        let before = self.clone();
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
        self.save_if_changed(&before, "set_f32", key);
    }
}

#[cfg(test)]
mod tests {
    use super::UserSettings;
    use serial_test::serial;
    use std::fs;
    use tempfile::TempDir;

    fn setup_isolated_data_dir() -> TempDir {
        let tmp = TempDir::new().expect("tempdir");
        // SAFETY: tests are serial and intentionally override process env.
        unsafe {
            std::env::set_var("CODESCRIBE_DATA_DIR", tmp.path());
        }
        tmp
    }

    #[test]
    fn test_normalized_chat_zoom_rules() {
        assert_eq!(UserSettings::normalized_chat_zoom(1.0), None);
        assert_eq!(UserSettings::normalized_chat_zoom(1.004), None);
        assert_eq!(UserSettings::normalized_chat_zoom(1.125), Some(1.13));
        assert_eq!(UserSettings::normalized_chat_zoom(0.1), Some(0.75));
        assert_eq!(UserSettings::normalized_chat_zoom(4.0), Some(2.0));
    }

    #[test]
    #[serial]
    fn test_set_chat_zoom_writes_only_on_effective_change() {
        let _tmp = setup_isolated_data_dir();
        let mut settings = UserSettings::default();
        let path = UserSettings::settings_path();

        // Default zoom is encoded as None, so this should be a no-op (no file write).
        assert!(!settings.set_chat_zoom(1.0));
        assert!(
            !path.exists(),
            "no-op zoom update should not create settings file"
        );

        assert!(settings.set_chat_zoom(1.125));
        let first_contents = fs::read_to_string(&path).expect("read settings after first write");

        // 1.129 rounds to the same persisted value (1.13), so no write.
        assert!(!settings.set_chat_zoom(1.129));
        let second_contents = fs::read_to_string(&path).expect("read settings after no-op write");
        assert_eq!(first_contents, second_contents);
    }

    #[test]
    #[serial]
    fn test_show_dock_icon_bool_persists_and_roundtrips() {
        let _tmp = setup_isolated_data_dir();
        let mut settings = UserSettings::default();
        settings.set_bool("SHOW_DOCK_ICON", false);

        assert_eq!(settings.show_dock_icon, Some(false));

        let loaded = UserSettings::load();
        assert_eq!(loaded.show_dock_icon, Some(false));
    }
}
