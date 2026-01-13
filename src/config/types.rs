//! Type definitions for CodeScribe configuration.
//!
//! Contains all enums and the main Config struct.

use serde::{Deserialize, Serialize};
use std::str::FromStr;

use super::defaults::*;

/// Modifier key combinations for hold-to-talk
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum HoldMods {
    #[default]
    Ctrl,
    CtrlAlt,
    CtrlShift,
    CtrlCmd,
}

#[allow(dead_code)]
impl HoldMods {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Ctrl => "ctrl",
            Self::CtrlAlt => "ctrl_alt",
            Self::CtrlShift => "ctrl_shift",
            Self::CtrlCmd => "ctrl_cmd",
        }
    }

    /// Human-readable label for menu display
    pub fn label(&self) -> &'static str {
        match self {
            Self::Ctrl => "Ctrl only (Raw)",
            Self::CtrlAlt => "Ctrl+Option",
            Self::CtrlShift => "Ctrl+Shift (AI)",
            Self::CtrlCmd => "Ctrl+Command",
        }
    }
}

impl FromStr for HoldMods {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "ctrl" => Ok(Self::Ctrl),
            "ctrl_alt" | "ctrl+alt" => Ok(Self::CtrlAlt),
            "ctrl_shift" | "ctrl+shift" => Ok(Self::CtrlShift),
            "ctrl_cmd" | "ctrl+cmd" => Ok(Self::CtrlCmd),
            _ => Err(format!("Unknown HoldMods: {}", s)),
        }
    }
}

/// Toggle trigger options
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ToggleTrigger {
    #[default]
    DoubleOption,
    DoubleRightOption,
    None,
}

#[allow(dead_code)]
impl ToggleTrigger {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::DoubleOption => "double_option",
            Self::DoubleRightOption => "double_ralt",
            Self::None => "none",
        }
    }

    /// Human-readable label for menu display
    pub fn label(&self) -> &'static str {
        match self {
            Self::DoubleOption => "double option",
            Self::DoubleRightOption => "double right option",
            Self::None => "disabled",
        }
    }
}

impl FromStr for ToggleTrigger {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "double_option" => Ok(Self::DoubleOption),
            "double_ralt" | "double_right_option" => Ok(Self::DoubleRightOption),
            "none" | "disabled" => Ok(Self::None),
            _ => Err(format!("Unknown ToggleTrigger: {}", s)),
        }
    }
}

/// Language options for Whisper transcription
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Language {
    #[default]
    Auto,
    Polish,
    English,
}

impl Language {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Polish => "pl",
            Self::English => "en",
        }
    }
}

impl FromStr for Language {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "auto" => Ok(Self::Auto),
            "pl" | "polish" => Ok(Self::Polish),
            "en" | "english" => Ok(Self::English),
            _ => Err(format!("Unknown Language: {}", s)),
        }
    }
}

/// AI provider options
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum AiProvider {
    #[default]
    Harmony,
    Ollama,
}

impl FromStr for AiProvider {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "harmony" => Ok(Self::Harmony),
            "ollama" => Ok(Self::Ollama),
            _ => Err(format!("Unknown AiProvider: {}", s)),
        }
    }
}

/// CodeScribe configuration structure.
///
/// This struct contains all configuration options for the app.
/// Values are loaded from .env file (primary) or settings.json (fallback).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    // ===== Hotkeys =====
    /// Modifier keys for hold-to-talk
    #[serde(default)]
    pub hold_mods: HoldMods,

    /// Whether to ignore extra modifiers when hold key is pressed
    #[serde(default)]
    pub hold_exclusive: bool,

    /// Toggle trigger method (double Option, double RAlt, or none)
    #[serde(default)]
    pub toggle_trigger: ToggleTrigger,

    /// Delay in milliseconds before starting recording after holding key
    #[serde(default = "default_hold_start_delay_ms")]
    pub hold_start_delay_ms: u64,

    // ===== Language =====
    /// Whisper language preference
    #[serde(default)]
    pub whisper_language: Language,

    // ===== AI Formatting =====
    /// Whether AI formatting is enabled for transcriptions
    #[serde(default)]
    pub ai_formatting_enabled: bool,

    /// AI provider for formatting
    #[serde(default)]
    pub ai_provider: AiProvider,

    /// Maximum tokens for regular AI completions
    #[serde(default = "default_ai_max_tokens")]
    pub ai_max_tokens: i32,

    /// Maximum tokens for assistive AI completions
    #[serde(default = "default_ai_assistive_max_tokens")]
    pub ai_assistive_max_tokens: i32,

    // ===== UI =====
    /// Whether to show tray icon glyph
    #[serde(default = "default_show_tray_glyph")]
    pub show_tray_glyph: bool,

    /// Whether to show hold indicator badge
    #[serde(default = "default_hold_indicator")]
    pub hold_indicator: bool,

    /// Size of hold indicator badge in pixels
    #[serde(default = "default_hold_badge_size")]
    pub hold_badge_size: u32,

    /// X offset of hold indicator badge
    #[serde(default = "default_hold_badge_offset_x")]
    pub hold_badge_offset_x: i32,

    /// Y offset of hold indicator badge
    #[serde(default = "default_hold_badge_offset_y")]
    pub hold_badge_offset_y: i32,

    // ===== Sound =====
    /// Whether to play a beep sound when recording starts
    #[serde(default = "default_beep_on_start")]
    pub beep_on_start: bool,

    /// System sound name to play (e.g., "Tink", "Pop")
    #[serde(default = "default_sound_name")]
    pub sound_name: String,

    /// Sound volume (0.0 to 1.0)
    #[serde(default = "default_sound_volume")]
    pub sound_volume: f32,

    // ===== Audio =====
    /// Preferred audio input device name (cpal) (optional)
    pub audio_input_device: Option<String>,

    // ===== History =====
    /// Whether to keep transcription history
    #[serde(default = "default_history_enabled")]
    pub history_enabled: bool,

    // ===== Backends =====
    /// Whether to use local STT instead of cloud
    #[serde(default)]
    pub use_local_stt: bool,

    /// Local model name (tiny, base, small, large-v3)
    #[serde(default = "default_local_model")]
    pub local_model: String,

    /// Full STT endpoint URL (e.g., https://api.libraxis.cloud/stt/v1/transcribe)
    pub stt_endpoint: Option<String>,

    /// Whisper server URL
    #[serde(default = "default_whisper_server_url")]
    pub whisper_server_url: String,

    /// LLM server URL
    #[serde(default = "default_llm_server_url")]
    pub llm_server_url: String,

    /// Ollama host URL
    #[serde(default = "default_ollama_host")]
    pub ollama_host: String,

    /// Ollama model name
    #[serde(default = "default_ollama_model")]
    pub ollama_model: String,

    /// Full LLM endpoint URL (e.g., https://api.libraxis.cloud/v1/responses)
    pub llm_endpoint: Option<String>,

    /// API key for cloud LLM providers
    pub llm_api_key: Option<String>,

    /// API key for cloud STT providers
    pub stt_api_key: Option<String>,

    // ===== Clipboard =====
    /// Whether to restore previous clipboard after paste
    #[serde(default = "default_restore_clipboard")]
    pub restore_clipboard: bool,

    /// Delay in milliseconds before restoring clipboard
    #[serde(default = "default_restore_clipboard_delay_ms")]
    pub restore_clipboard_delay_ms: u64,

    // ===== System =====
    /// Whether to start app at login
    #[serde(default)]
    pub start_at_login: bool,

    // ===== Legacy =====
    /// Backend ports to try connecting to (legacy, for backwards compatibility)
    #[serde(default = "default_backend_ports")]
    pub backend_ports: Vec<u16>,

    /// Silence threshold in decibels (legacy)
    #[serde(default = "default_silence_db")]
    pub silence_db: f32,

    /// Silence hang time in seconds (legacy)
    #[serde(default = "default_silence_hang_sec")]
    pub silence_hang_sec: f32,

    // ===== Debugging =====
    /// Whether to dump raw audio files to logs/audio directory
    #[serde(default = "default_dump_audio_logs")]
    pub dump_audio_logs: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            hold_mods: HoldMods::default(),
            hold_exclusive: true, // Ignore extra modifiers by default (Ctrl+K won't trigger)
            toggle_trigger: ToggleTrigger::default(),
            hold_start_delay_ms: default_hold_start_delay_ms(),
            whisper_language: Language::default(),
            ai_formatting_enabled: false,
            ai_provider: AiProvider::default(),
            ai_max_tokens: default_ai_max_tokens(),
            ai_assistive_max_tokens: default_ai_assistive_max_tokens(),
            show_tray_glyph: default_show_tray_glyph(),
            hold_indicator: default_hold_indicator(),
            hold_badge_size: default_hold_badge_size(),
            hold_badge_offset_x: default_hold_badge_offset_x(),
            hold_badge_offset_y: default_hold_badge_offset_y(),
            beep_on_start: default_beep_on_start(),
            sound_name: default_sound_name(),
            sound_volume: default_sound_volume(),
            audio_input_device: None,
            history_enabled: default_history_enabled(),
            use_local_stt: false,
            local_model: default_local_model(),
            stt_endpoint: None,
            whisper_server_url: default_whisper_server_url(),
            llm_server_url: default_llm_server_url(),
            ollama_host: default_ollama_host(),
            ollama_model: default_ollama_model(),
            llm_endpoint: None,
            llm_api_key: None,
            stt_api_key: None,
            restore_clipboard: default_restore_clipboard(),
            restore_clipboard_delay_ms: default_restore_clipboard_delay_ms(),
            start_at_login: false,
            backend_ports: default_backend_ports(),
            silence_db: default_silence_db(),
            silence_hang_sec: default_silence_hang_sec(),
            dump_audio_logs: default_dump_audio_logs(),
        }
    }
}

impl Config {
    /// Sanitize configuration values to ensure they're valid.
    pub fn sanitize(&mut self) {
        // Ensure token limits are reasonable
        if self.ai_max_tokens <= 0 {
            self.ai_max_tokens = 512;
        }
        if self.ai_assistive_max_tokens <= 0 {
            self.ai_assistive_max_tokens = 2048;
        }

        // Validate audio thresholds (legacy)
        if self.silence_db > 0.0 || self.silence_db < -100.0 {
            self.silence_db = -45.0;
        }
        if self.silence_hang_sec <= 0.0 || self.silence_hang_sec > 10.0 {
            self.silence_hang_sec = 0.8;
        }

        // Ensure at least one backend port is configured (legacy)
        if self.backend_ports.is_empty() {
            self.backend_ports = default_backend_ports();
        }

        // Clamp sound volume
        self.sound_volume = self.sound_volume.clamp(0.0, 1.0);

        // Validate badge size
        if self.hold_badge_size < 8 || self.hold_badge_size > 64 {
            self.hold_badge_size = 12;
        }
    }
}
