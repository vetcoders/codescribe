//! Type definitions for the tray module
//!
//! Contains all enums and structs used by the tray system.

use anyhow::Result;
use muda::{CheckMenuItem, MenuId, MenuItem};
use tracing::debug;
use tray_icon::Icon;

use crate::tray::icons::{create_fallback_icon, load_custom_icon};

// Re-export config enums for menu use (single source of truth)
pub use crate::config::{HoldMods, Language, ToggleTrigger};

/// Status of the CodeScribe system, reflected in tray icon
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayStatus {
    /// Idle, waiting for activation
    Idle,
    /// Actively listening/recording
    Listening,
    /// Processing/transcribing
    Thinking,
    /// Successfully completed
    Success,
    /// Error state - backend not available
    Error,
}

impl TrayStatus {
    /// Get the human-readable tooltip for this status
    pub fn tooltip(&self) -> String {
        match self {
            TrayStatus::Idle => "CodeScribe - Ready".to_string(),
            TrayStatus::Listening => "CodeScribe - Recording...".to_string(),
            TrayStatus::Thinking => "CodeScribe - Processing...".to_string(),
            TrayStatus::Success => "CodeScribe - Done!".to_string(),
            TrayStatus::Error => "CodeScribe - Backend unavailable!".to_string(),
        }
    }

    /// Create an icon from this status using the custom CodeScribe logo
    /// Falls back to simple circle if custom icon fails
    pub fn to_icon(self) -> Result<Icon> {
        load_custom_icon(self).or_else(|e| {
            debug!("Custom icon failed, using fallback: {}", e);
            create_fallback_icon(self)
        })
    }
}

/// Menu events that can be sent to the main controller.
#[derive(Debug, Clone)]
pub enum TrayMenuEvent {
    // Top-level actions
    ToggleHotkeys,
    StartAtLogin(bool),
    /// User clicked Quit - show confirmation dialog
    Quit,
    /// Close tray AND stop backend server
    QuitCloseAll,
    /// Close tray but leave backend server running
    QuitLeaveServer,

    // Language submenu
    SetLanguage(Language),

    // Models submenu (Whisper model selection)
    SetWhisperModel(WhisperModel),
    OpenModelsFolder,

    // Formatting submenu
    SetFormattingProvider(FormattingProvider),
    ToggleAiFormatting,

    // Hold Hotkeys submenu
    SetHoldMods(HoldMods),
    ToggleHoldExclusive,
    SetToggleTrigger(ToggleTrigger),

    // History submenu
    ToggleHistory,
    CopyLatestToClipboard,
    OpenHistoryFolder,
    SelectHistoryEntry(usize),

    // Appearance submenu
    ToggleStatusGlyph,
    RefreshTrayIcon,

    // Feedback submenu
    ToggleStartSound,
    SetSoundType(SoundType),
    SetVolume(VolumeLevel),

    // Permissions submenu
    CheckPermissions,
    OpenAccessibilitySettings,
    OpenMicrophoneSettings,

    // Tools submenu
    OpenVoiceLab,
    OpenTeacher,
    OpenNativeLab,
    NewConversation,
}

/// Formatting provider selection (maps to config::AiProvider)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FormattingProvider {
    Harmony,
    Ollama,
}

/// Sound type for audio feedback
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SoundType {
    Tink,
    Pop,
}

/// Volume level presets
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VolumeLevel {
    Mute,   // 0%
    Low,    // 25%
    Medium, // 50%
    High,   // 75%
    Full,   // 100%
}

impl VolumeLevel {
    /// Convert to f32 value (0.0 - 1.0)
    pub fn as_f32(self) -> f32 {
        match self {
            VolumeLevel::Mute => 0.0,
            VolumeLevel::Low => 0.25,
            VolumeLevel::Medium => 0.5,
            VolumeLevel::High => 0.75,
            VolumeLevel::Full => 1.0,
        }
    }

    /// Get display label
    pub fn label(self) -> &'static str {
        match self {
            VolumeLevel::Mute => "🔇 Mute (0%)",
            VolumeLevel::Low => "🔈 Low (25%)",
            VolumeLevel::Medium => "🔉 Medium (50%)",
            VolumeLevel::High => "🔊 High (75%)",
            VolumeLevel::Full => "🔊 Full (100%)",
        }
    }

    /// Get VolumeLevel from f32 value (rounds to nearest)
    pub fn from_f32(value: f32) -> Self {
        if value <= 0.125 {
            VolumeLevel::Mute
        } else if value <= 0.375 {
            VolumeLevel::Low
        } else if value <= 0.625 {
            VolumeLevel::Medium
        } else if value <= 0.875 {
            VolumeLevel::High
        } else {
            VolumeLevel::Full
        }
    }
}

/// Whisper model variants available for local STT
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WhisperModel {
    Small,
    Medium,
    LargeV3,
    LargeV3Turbo,
    LargeV3Q8,
}

impl WhisperModel {
    /// Human-readable label for the menu
    pub fn label(&self) -> &'static str {
        match self {
            WhisperModel::Small => "Small",
            WhisperModel::Medium => "Medium",
            WhisperModel::LargeV3 => "Large v3",
            WhisperModel::LargeV3Turbo => "Large v3 Turbo",
            WhisperModel::LargeV3Q8 => "Large v3 Q8",
        }
    }

    /// Directory name / model identifier
    pub fn model_id(&self) -> &'static str {
        match self {
            WhisperModel::Small => "whisper-small",
            WhisperModel::Medium => "whisper-medium",
            WhisperModel::LargeV3 => "whisper-large-v3",
            WhisperModel::LargeV3Turbo => "whisper-large-v3-turbo",
            WhisperModel::LargeV3Q8 => "whisper-large-v3-mlx-q8",
        }
    }
}

// ============================================================================
// Menu Item Storage Structs
// ============================================================================

/// Model menu items for dynamic updates
pub struct ModelMenuItems {
    pub small: CheckMenuItem,
    pub medium: CheckMenuItem,
    pub large_v3: CheckMenuItem,
    pub large_v3_turbo: CheckMenuItem,
    pub large_v3_q8: CheckMenuItem,
    pub label: MenuItem,
}

/// Hold Hotkeys menu items for radio-button behavior
pub struct HoldMenuItems {
    pub ctrl: CheckMenuItem,
    pub ctrl_opt: CheckMenuItem,
    pub ctrl_shift: CheckMenuItem,
    pub ctrl_cmd: CheckMenuItem,
    pub label: MenuItem,
}

/// Toggle Trigger menu items for radio-button behavior
pub struct ToggleMenuItems {
    pub double_opt: CheckMenuItem,
    pub double_ralt: CheckMenuItem,
    pub disabled: CheckMenuItem,
    pub label: MenuItem,
}

/// History menu label for dynamic updates
pub struct HistoryMenuItems {
    pub latest_label: MenuItem,
}

// ============================================================================
// Menu IDs Structure
// ============================================================================

/// Menu item IDs for tracking all clickable items
pub struct MenuIds {
    // Top-level
    pub enable_hotkeys: MenuId,
    pub start_at_login: MenuId,
    pub quit: MenuId,

    // Language submenu
    pub lang_auto: MenuId,
    pub lang_polish: MenuId,
    pub lang_english: MenuId,

    // Models submenu (Whisper model selection)
    pub model_small: MenuId,
    pub model_medium: MenuId,
    pub model_large_v3: MenuId,
    pub model_large_v3_turbo: MenuId,
    pub model_large_v3_q8: MenuId,
    pub model_open_folder: MenuId,

    // Formatting submenu
    pub fmt_toggle: MenuId,
    pub fmt_harmony: MenuId,
    pub fmt_ollama: MenuId,

    // Hold Hotkeys submenu
    pub hold_ctrl: MenuId,
    pub hold_ctrl_opt: MenuId,
    pub hold_ctrl_shift: MenuId,
    pub hold_ctrl_cmd: MenuId,
    pub hold_exclusive: MenuId,
    pub toggle_double_opt: MenuId,
    pub toggle_double_ralt: MenuId,
    pub toggle_disabled: MenuId,

    // History submenu
    pub history_save: MenuId,
    pub history_copy_latest: MenuId,
    pub history_open_folder: MenuId,

    // Appearance submenu
    pub appearance_glyph: MenuId,
    pub appearance_refresh: MenuId,

    // Feedback submenu
    pub feedback_start_sound: MenuId,
    pub feedback_sound_tink: MenuId,
    pub feedback_sound_pop: MenuId,
    pub volume_mute: MenuId,
    pub volume_low: MenuId,
    pub volume_medium: MenuId,
    pub volume_high: MenuId,
    pub volume_full: MenuId,

    // Permissions submenu
    pub perm_check: MenuId,
    pub perm_accessibility: MenuId,
    pub perm_microphone: MenuId,

    // Tools submenu
    pub tools_voice_lab: MenuId,
    pub tools_teacher: MenuId,
    pub tools_native_lab: MenuId,
    pub tools_new_conversation: MenuId,
}
