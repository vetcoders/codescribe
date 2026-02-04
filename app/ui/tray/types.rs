//! Type definitions for the tray module
//!
//! Contains all enums and structs used by the tray system.

use anyhow::Result;
use muda::{CheckMenuItem, MenuId, MenuItem};
use tracing::debug;
use tray_icon::Icon;

use crate::tray::icons::{create_fallback_icon, load_custom_icon};

// Re-export config enums for menu use (single source of truth)
pub use crate::config::{HoldMods, ToggleTrigger};

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

    /// Get the status line text for the menu
    pub fn menu_label(&self) -> &'static str {
        match self {
            TrayStatus::Idle => "Status: Idle",
            TrayStatus::Listening => "Status: Recording...",
            TrayStatus::Thinking => "Status: Processing...",
            TrayStatus::Success => "Status: Done!",
            TrayStatus::Error => "Status: Error",
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
/// Some variants are prepared for future use but handlers may not be implemented yet.
#[derive(Debug, Clone)]
pub enum TrayMenuEvent {
    /// Copy last transcript to clipboard
    CopyLast,
    /// Open settings file in editor
    OpenSettings,
    /// Open help/documentation in browser
    OpenHelp,
    /// Show about dialog
    ShowAbout,
    /// User clicked Quit - clean shutdown
    Quit,

    /// Run onboarding (bootstrap) flow
    RunOnboarding,

    // Hold Hotkeys submenu
    SetHoldMods(HoldMods),
    SetToggleTrigger(ToggleTrigger),
    SetHoldExclusive(bool),

    // History (open folder)
    OpenHistoryFolder,

    // Diagnostics
    CopyDiagnostics,
    OpenAccessibilitySettings,
    OpenInputMonitoringSettings,
    ResetInputMonitoringPermission,
    InstallSileroVad,
    SetVadPreset(VadPreset),

    // Prompts
    OpenAssistivePrompt,
    OpenFormattingPrompt,
    OpenPromptsFolder,

    // Notes
    SetQuickNotesEnabled(bool),
    SetQuickNotesSaveOnly(bool),

    // Hotkeys
    ResetShortcuts,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VadPreset {
    Sensitive,
    Balanced,
    Conservative,
}

// ============================================================================
// Menu Item Storage Structs
// ============================================================================

/// Hotkeys menu items that need runtime updates.
pub struct HotkeysMenuItems {
    pub hold_summary: MenuItem,
    pub hold_ctrl: CheckMenuItem,
    pub hold_ctrl_alt: CheckMenuItem,
    pub hold_ctrl_shift: CheckMenuItem,
    pub hold_ctrl_cmd: CheckMenuItem,
    pub toggle_assistive: CheckMenuItem,
    pub toggle_dictation: CheckMenuItem,
    pub toggle_label: MenuItem,
}

/// Notes menu items that need runtime updates.
pub struct NotesMenuItems {
    pub quick_notes_toggle: CheckMenuItem,
    pub quick_notes_save_only: CheckMenuItem,
}

// ============================================================================
// Menu IDs Structure
// ============================================================================

/// Menu item IDs for tracking all clickable items
/// Note: Settings options moved to Settings tab in Chat Overlay
pub struct MenuIds {
    // Top-level
    pub copy_last: MenuId,
    pub show_overlay: MenuId,
    pub run_onboarding: MenuId,
    pub open_history: MenuId,
    pub copy_diagnostics: MenuId,
    pub open_accessibility_settings: MenuId,
    pub open_input_monitoring_settings: MenuId,
    pub reset_input_monitoring_permission: MenuId,
    pub open_assistive_prompt: MenuId,
    pub open_formatting_prompt: MenuId,
    pub open_prompts_folder: MenuId,
    pub help: MenuId,
    pub about: MenuId,
    pub quit: MenuId,

    // Hotkeys submenu
    pub hotkeys_toggle_assistive: MenuId,
    pub hotkeys_toggle_dictation: MenuId,
    pub hotkeys_reset: MenuId,
    pub hotkeys_copy_cheatsheet: MenuId,
    pub hotkeys_hold_ctrl: MenuId,
    pub hotkeys_hold_ctrl_alt: MenuId,
    pub hotkeys_hold_ctrl_shift: MenuId,
    pub hotkeys_hold_ctrl_cmd: MenuId,

    // Quality
    pub quality_open_report: MenuId,

    // Models
    pub silero_vad_install: MenuId,

    // VAD presets
    pub vad_preset_sensitive: MenuId,
    pub vad_preset_balanced: MenuId,
    pub vad_preset_conservative: MenuId,

    // Notes
    pub notes_toggle_quick_notes: MenuId,
    pub notes_toggle_save_only: MenuId,
    pub notes_open_folder: MenuId,
    pub notes_open_today: MenuId,
}
