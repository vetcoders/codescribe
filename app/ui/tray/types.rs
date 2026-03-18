//! Type definitions for the tray module
//!
//! Contains all enums and structs used by the tray system.

use anyhow::Result;
use muda::{CheckMenuItem, MenuId};
use tracing::debug;
use tray_icon::Icon;

use crate::tray::icons::{create_fallback_icon, load_custom_icon};

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
    /// Open help/documentation in browser
    OpenHelp,
    /// Show about dialog
    ShowAbout,
    /// User clicked Quit - clean shutdown
    Quit,

    // History (open folder)
    OpenHistoryFolder,

    // Diagnostics
    CopyDiagnostics,
    InstallSileroVad,

    // Notes
    SetQuickNotesEnabled(bool),
    SetQuickNotesSaveOnly(bool),
}

// ============================================================================
// Menu Item Storage Structs
// ============================================================================

/// Notes menu items that need runtime updates.
pub struct NotesMenuItems {
    pub quick_notes_toggle: CheckMenuItem,
    pub quick_notes_save_only: CheckMenuItem,
}

// ============================================================================
// Menu IDs Structure
// ============================================================================

/// Menu item IDs for tracking all clickable items
/// Note: Settings opens the persistent Settings window; onboarding is separate.
pub struct MenuIds {
    // Top-level
    pub copy_last: MenuId,
    pub show_overlay: MenuId,
    pub open_settings: MenuId,
    pub continue_onboarding: Option<MenuId>,
    pub open_history: MenuId,
    pub copy_diagnostics: MenuId,
    pub help: MenuId,
    pub about: MenuId,
    pub quit: MenuId,

    // Quality
    pub quality_open_report: MenuId,

    // Models
    pub silero_vad_install: MenuId,

    // Notes
    pub notes_toggle_quick_notes: MenuId,
    pub notes_toggle_save_only: MenuId,
    pub notes_open_folder: MenuId,
    pub notes_open_today: MenuId,
}
