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
            Self::Ctrl => "Ctrl",
            Self::CtrlAlt => "Ctrl+Option",
            Self::CtrlShift => "Ctrl+Shift",
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
            Self::DoubleOption => "left+right option",
            Self::DoubleRightOption => "right option only",
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
/// NOTE: No "Auto" - Whisper requires explicit language for reliable transcription
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Language {
    #[default]
    Polish,
    English,
}

impl Language {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Polish => "pl",
            Self::English => "en",
        }
    }
}

impl FromStr for Language {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "pl" | "polish" => Ok(Self::Polish),
            "en" | "english" => Ok(Self::English),
            // Legacy "auto" maps to Polish (default)
            "auto" | "" => Ok(Self::Polish),
            _ => Err(format!("Unknown Language: {}", s)),
        }
    }
}

/// Strategy for sending transcripts to AI
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TranscriptSendMode {
    #[default]
    EndOfUtterance, // Wait for silence, then send (classic)
    Streaming, // Send chunks as they arrive (incremental)
}

impl TranscriptSendMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::EndOfUtterance => "end_of_utterance",
            Self::Streaming => "streaming",
        }
    }
}

impl FromStr for TranscriptSendMode {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "end_of_utterance" | "end" | "delayed" => Ok(Self::EndOfUtterance),
            "streaming" | "stream" | "incremental" => Ok(Self::Streaming),
            _ => Err(format!("Unknown TranscriptSendMode: {}", s)),
        }
    }
}

/// Overlay position strategy
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum OverlayPositionMode {
    #[default]
    SnappedTopRight,
    Custom,
}

impl OverlayPositionMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::SnappedTopRight => "snapped_top_right",
            Self::Custom => "custom",
        }
    }
}

impl FromStr for OverlayPositionMode {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "snapped_top_right" | "snap" | "top_right" => Ok(Self::SnappedTopRight),
            "custom" | "manual" => Ok(Self::Custom),
            _ => Err(format!("Unknown OverlayPositionMode: {}", s)),
        }
    }
}

/// CodeScribe configuration structure.
///
/// This struct contains all configuration options for the app.
/// Values are loaded from .env file (single source of truth).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    // ===== Hotkeys =====
    /// Modifier keys for hold-to-talk
    #[serde(default)]
    pub hold_mods: HoldMods,

    /// Whether to ignore extra modifiers when hold key is pressed
    #[serde(default)]
    pub hold_exclusive: bool,

    /// Toggle trigger method:
    /// - DoubleOption: left=normal toggle, right=assistive toggle
    /// - DoubleRightOption: right=assistive only
    /// - None: disabled
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

    /// Strategy for sending transcript (end-of-utterance vs streaming)
    #[serde(default)]
    pub transcript_send_mode: TranscriptSendMode,

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

    /// Overlay position mode
    #[serde(default)]
    pub overlay_position_mode: OverlayPositionMode,

    /// Custom X coordinate for overlay (if mode is Custom)
    #[serde(default)]
    pub overlay_custom_x: Option<f64>,

    /// Custom Y coordinate for overlay (if mode is Custom)
    #[serde(default)]
    pub overlay_custom_y: Option<f64>,

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

    // ===== Debugging =====
    /// Whether to dump raw audio files to logs/audio directory
    #[serde(default = "default_dump_audio_logs")]
    pub dump_audio_logs: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            hold_mods: HoldMods::default(),
            hold_exclusive: false, // Allow Shift/Cmd mode modifiers by default
            toggle_trigger: ToggleTrigger::default(),
            hold_start_delay_ms: default_hold_start_delay_ms(),
            whisper_language: Language::default(),
            ai_formatting_enabled: false,
            transcript_send_mode: TranscriptSendMode::default(),
            ai_max_tokens: default_ai_max_tokens(),
            ai_assistive_max_tokens: default_ai_assistive_max_tokens(),
            show_tray_glyph: default_show_tray_glyph(),
            hold_indicator: default_hold_indicator(),
            hold_badge_size: default_hold_badge_size(),
            hold_badge_offset_x: default_hold_badge_offset_x(),
            hold_badge_offset_y: default_hold_badge_offset_y(),
            overlay_position_mode: OverlayPositionMode::default(),
            overlay_custom_x: None,
            overlay_custom_y: None,
            beep_on_start: default_beep_on_start(),
            sound_name: default_sound_name(),
            sound_volume: default_sound_volume(),
            audio_input_device: None,
            history_enabled: default_history_enabled(),
            use_local_stt: false,
            local_model: default_local_model(),
            stt_endpoint: None,
            llm_endpoint: None,
            llm_api_key: None,
            stt_api_key: None,
            restore_clipboard: default_restore_clipboard(),
            restore_clipboard_delay_ms: default_restore_clipboard_delay_ms(),
            start_at_login: false,
            dump_audio_logs: default_dump_audio_logs(),
        }
    }
}

impl Config {
    /// Sanitize configuration values to ensure they're valid.
    pub fn sanitize(&mut self) {
        // Token limits: 0 = no limit (API decides). Don't override.
        // Tokens are cheap, lost notes are not.

        // Clamp sound volume
        self.sound_volume = self.sound_volume.clamp(0.0, 1.0);

        // Validate badge size
        if self.hold_badge_size < 8 || self.hold_badge_size > 64 {
            self.hold_badge_size = 12;
        }
    }
}
