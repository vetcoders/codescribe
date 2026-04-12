//! Type definitions for CodeScribe configuration.
//!
//! Contains all enums and the main Config struct.

use serde::{Deserialize, Serialize};
use std::str::FromStr;

use super::defaults::*;

/// First-class work modes used by the runtime and settings UI.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum WorkMode {
    Dictation,
    Formatting,
    Assistive,
}

impl WorkMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Dictation => "dictation",
            Self::Formatting => "formatting",
            Self::Assistive => "assistive",
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            Self::Dictation => "Dictation",
            Self::Formatting => "Formatting",
            Self::Assistive => "Assistive",
        }
    }

    pub fn description(&self) -> &'static str {
        match self {
            Self::Dictation => "Fast transcript / auto-paste mode.",
            Self::Formatting => "AI formatting pass for dictation text.",
            Self::Assistive => "AI assistive conversation mode.",
        }
    }

    pub fn is_assistive(&self) -> bool {
        matches!(self, Self::Assistive)
    }

    pub fn defaults_to_auto_paste(&self) -> bool {
        !self.is_assistive()
    }

    pub fn forces_ai(&self) -> bool {
        matches!(self, Self::Formatting | Self::Assistive)
    }
}

impl FromStr for WorkMode {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "dictation" | "raw" => Ok(Self::Dictation),
            "formatting" | "format" => Ok(Self::Formatting),
            "assistive" | "chat" => Ok(Self::Assistive),
            _ => Err(format!("Unknown WorkMode: {}", s)),
        }
    }
}

/// Normalized binding gesture persisted per work mode.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum ShortcutBinding {
    Disabled,
    HoldFn,
    HoldCtrl,
    HoldCtrlAlt,
    HoldCtrlShift,
    HoldCtrlCmd,
    DoubleCtrl,
    DoubleLeftOption,
    DoubleRightOption,
}

impl ShortcutBinding {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Disabled => "Disabled",
            Self::HoldFn => "Hold Fn/Globe",
            Self::HoldCtrl => "Hold Ctrl",
            Self::HoldCtrlAlt => "Hold Ctrl+Option",
            Self::HoldCtrlShift => "Hold Ctrl+Shift",
            Self::HoldCtrlCmd => "Hold Ctrl+Command",
            Self::DoubleCtrl => "Double-tap Ctrl",
            Self::DoubleLeftOption => "Double-tap Left Option",
            Self::DoubleRightOption => "Double-tap Right Option",
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Disabled => "disabled",
            Self::HoldFn => "hold_fn",
            Self::HoldCtrl => "hold_ctrl",
            Self::HoldCtrlAlt => "hold_ctrl_alt",
            Self::HoldCtrlShift => "hold_ctrl_shift",
            Self::HoldCtrlCmd => "hold_ctrl_cmd",
            Self::DoubleCtrl => "double_ctrl",
            Self::DoubleLeftOption => "double_left_option",
            Self::DoubleRightOption => "double_right_option",
        }
    }
}

impl FromStr for ShortcutBinding {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "disabled" => Ok(Self::Disabled),
            "hold_fn" => Ok(Self::HoldFn),
            "hold_ctrl" => Ok(Self::HoldCtrl),
            "hold_ctrl_alt" => Ok(Self::HoldCtrlAlt),
            "hold_ctrl_shift" => Ok(Self::HoldCtrlShift),
            "hold_ctrl_cmd" => Ok(Self::HoldCtrlCmd),
            "double_ctrl" => Ok(Self::DoubleCtrl),
            "double_left_option" => Ok(Self::DoubleLeftOption),
            "double_right_option" => Ok(Self::DoubleRightOption),
            _ => Err(format!("Unknown ShortcutBinding: {}", s)),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ModeBinding {
    pub mode: WorkMode,
    pub binding: ShortcutBinding,
}

pub fn default_mode_bindings() -> Vec<ModeBinding> {
    vec![
        ModeBinding {
            mode: WorkMode::Dictation,
            binding: ShortcutBinding::HoldFn,
        },
        ModeBinding {
            mode: WorkMode::Formatting,
            binding: ShortcutBinding::DoubleLeftOption,
        },
        ModeBinding {
            mode: WorkMode::Assistive,
            binding: ShortcutBinding::DoubleRightOption,
        },
    ]
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
    /// Whether to ignore extra modifiers when hold key is pressed
    #[serde(default)]
    pub hold_exclusive: bool,

    /// Delay in milliseconds before starting recording after holding key
    #[serde(default = "default_hold_start_delay_ms")]
    pub hold_start_delay_ms: u64,

    /// Double-tap interval for toggle detection (milliseconds)
    #[serde(default = "default_double_tap_interval_ms")]
    pub double_tap_interval_ms: u64,

    /// Silence duration (seconds) before sending a toggle utterance
    #[serde(default = "default_toggle_silence_sec")]
    pub toggle_silence_sec: f32,

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

    /// Whether app should appear in Dock
    #[serde(default = "default_show_dock_icon")]
    pub show_dock_icon: bool,

    /// Whether non-assistive dictation should render through the floating overlay.
    ///
    /// When disabled, the runtime switches to a buffered no-overlay profile
    /// intended for longer recordings and lower local Whisper pressure.
    #[serde(default = "default_transcription_overlay_enabled")]
    pub transcription_overlay_enabled: bool,

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

    // ===== Quick Notes =====
    /// When enabled, dictation saves into a daily note file (and does not auto-paste).
    #[serde(default)]
    pub quick_notes_enabled: bool,

    /// When Quick Notes is enabled: if true, do not auto-paste (save-only).
    /// If false, we both save the note and paste as usual.
    #[serde(default)]
    pub quick_notes_save_only: bool,

    // ===== Backends =====
    /// Whether the local pipeline is the authority for the committed transcript.
    ///
    /// Live preview always stays local and provisional.
    ///
    /// When false, cloud STT becomes the committed verdict after capture if
    /// endpoint credentials are configured. If that verdict is unavailable, the
    /// app must surface any degraded fallback explicitly instead of silently
    /// promoting preview text.
    #[serde(default)]
    pub use_local_stt: bool,

    /// Local model name (tiny, base, small, large-v3)
    #[serde(default = "default_local_model")]
    pub local_model: String,

    /// Cloud STT endpoint used when cloud is selected as the committed verdict path.
    pub stt_endpoint: Option<String>,

    /// Full LLM endpoint URL (e.g., https://api.libraxis.cloud/v1/responses)
    pub llm_endpoint: Option<String>,

    /// API key for cloud LLM providers
    pub llm_api_key: Option<String>,

    /// API key for cloud STT providers used on the committed verdict path
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

    // ===== Agent =====
    /// When true, Enter sends the message (Shift+Enter for newline).
    /// When false, Enter inserts newline (Cmd+Enter sends).
    #[serde(default = "default_agent_enter_sends")]
    pub agent_enter_sends: bool,
    // ===== Debugging =====
    /// Whether to dump raw audio files to logs/audio directory
    #[serde(default = "default_dump_audio_logs")]
    pub dump_audio_logs: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            hold_exclusive: false, // Allow Shift/Cmd mode modifiers by default
            hold_start_delay_ms: default_hold_start_delay_ms(),
            double_tap_interval_ms: default_double_tap_interval_ms(),
            toggle_silence_sec: default_toggle_silence_sec(),
            whisper_language: Language::default(),
            ai_formatting_enabled: false,
            transcript_send_mode: TranscriptSendMode::default(),
            ai_max_tokens: default_ai_max_tokens(),
            ai_assistive_max_tokens: default_ai_assistive_max_tokens(),
            show_tray_glyph: default_show_tray_glyph(),
            show_dock_icon: default_show_dock_icon(),
            transcription_overlay_enabled: default_transcription_overlay_enabled(),
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
            quick_notes_enabled: false,
            quick_notes_save_only: false,
            use_local_stt: true,
            local_model: default_local_model(),
            stt_endpoint: None,
            llm_endpoint: None,
            llm_api_key: None,
            stt_api_key: None,
            restore_clipboard: default_restore_clipboard(),
            restore_clipboard_delay_ms: default_restore_clipboard_delay_ms(),
            start_at_login: false,
            agent_enter_sends: default_agent_enter_sends(),
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

        // Clamp toggle silence to a reasonable range
        self.toggle_silence_sec = self.toggle_silence_sec.clamp(0.5, 30.0);

        // Clamp double-tap interval to safe bounds
        self.double_tap_interval_ms = self.double_tap_interval_ms.clamp(100, 450);

        // Validate badge size
        if self.hold_badge_size < 8 || self.hold_badge_size > 64 {
            self.hold_badge_size = 12;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::ShortcutBinding;

    #[test]
    fn shortcut_binding_parser_rejects_legacy_aliases() {
        assert!("none".parse::<ShortcutBinding>().is_err());
        assert!("fn".parse::<ShortcutBinding>().is_err());
        assert!("double_lalt".parse::<ShortcutBinding>().is_err());
        assert!("double_ralt".parse::<ShortcutBinding>().is_err());
    }
}
