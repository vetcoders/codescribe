//! Type definitions for the tray module
//!
//! Contains all enums and structs used by the tray system.

use anyhow::Result;
use muda::MenuId;
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
/// Minimal set for simplified tray menu.
#[derive(Debug, Clone)]
pub enum TrayMenuEvent {
    /// Open settings file in editor
    OpenSettings,
    /// Open help/documentation in browser
    OpenHelp,
    /// Show about dialog
    ShowAbout,
    /// User clicked Quit - clean shutdown
    Quit,
}

// ============================================================================
// Menu IDs Structure
// ============================================================================

/// Menu item IDs for tracking clickable items (minimal set)
pub struct MenuIds {
    pub settings: MenuId,
    pub help: MenuId,
    pub about: MenuId,
    pub quit: MenuId,
}
