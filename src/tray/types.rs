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

    // Hold Hotkeys submenu
    SetHoldMods(HoldMods),
    ToggleHoldExclusive,
    SetToggleTrigger(ToggleTrigger),

    // History submenu
    ToggleHistory,
    CopyLatestToClipboard,
    OpenHistoryFolder,
    SelectHistoryEntry(usize),
}

// ============================================================================
// Menu Item Storage Structs
// ============================================================================

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

// ============================================================================
// Menu IDs Structure
// ============================================================================

/// Menu item IDs for tracking all clickable items
pub struct MenuIds {
    // Top-level
    pub ai_formatting: MenuId,
    pub copy_last: MenuId,
    pub format_last: MenuId,
    pub format_last_five: MenuId,
    pub help: MenuId,
    pub about: MenuId,
    pub quit: MenuId,

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
    pub keep_audio: MenuId,
    pub history_copy_latest: MenuId,
    pub history_open_folder: MenuId,

    // Settings submenu
    pub settings_edit_config: MenuId,
    pub settings_edit_prompt: MenuId,
    pub settings_open_prompt_folder: MenuId,
    pub settings_reset_context: MenuId,

    // Quality submenu
    pub quality_open_report: MenuId,
}
