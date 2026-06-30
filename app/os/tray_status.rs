//! Surviving home for `TrayStatus` and `update_tray_status`.
//!
//! Relocated out of the legacy AppKit `app/ui/tray` module. The new SwiftUI
//! app owns the menu bar (`MenuBarExtra`), so the legacy AppKit status channel
//! no longer exists: `update_tray_status` is a no-op stub kept only so the
//! surviving non-UI consumers (thermal observer, hotkey bridge) keep compiling
//! with the same signature.
//!
//! Only the pure methods of `TrayStatus` are copied here. The icon-rendering
//! methods (`to_icon` / `to_icon_with_glyph`) stay behind in `app/ui/tray`
//! because they depend on `tray_icon::Icon` and `crate::tray::icons::*`, which
//! die with the legacy AppKit tray.

use tracing::trace;

/// Status of the Codescribe system, formerly reflected in the AppKit tray icon.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayStatus {
    /// App process is visible, but runtime initialization is still in progress.
    Starting,
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
    /// System thermal pressure is high enough to throttle STT.
    Thermal,
    /// A hotkey gesture was detected but blocked before dispatch.
    HotkeyConflict,
}

impl TrayStatus {
    /// Get the human-readable tooltip for this status
    pub fn tooltip(&self) -> String {
        match self {
            TrayStatus::Starting => "Codescribe - Starting...".to_string(),
            TrayStatus::Idle => "Codescribe - Ready".to_string(),
            TrayStatus::Listening => "Codescribe - Recording...".to_string(),
            TrayStatus::Thinking => "Codescribe - Processing...".to_string(),
            TrayStatus::Success => "Codescribe - Done!".to_string(),
            TrayStatus::Error => "Codescribe - Backend unavailable!".to_string(),
            TrayStatus::Thermal => "Codescribe - Thermal throttling".to_string(),
            TrayStatus::HotkeyConflict => "Codescribe - Hotkey conflict".to_string(),
        }
    }

    /// Get the status line text for the menu
    pub fn menu_label(&self) -> &'static str {
        match self {
            TrayStatus::Starting => "Status: Starting...",
            TrayStatus::Idle => "Status: Idle",
            TrayStatus::Listening => "Status: Recording...",
            TrayStatus::Thinking => "Status: Processing...",
            TrayStatus::Success => "Status: Done!",
            TrayStatus::Error => "Status: Error",
            TrayStatus::Thermal => "Status: Thermal throttling",
            TrayStatus::HotkeyConflict => "Status: Hotkey conflict",
        }
    }
}

/// Update the tray icon to reflect current status.
///
/// No-op stub: SwiftUI owns the menu bar, so legacy AppKit tray status is not
/// pushed anywhere. Kept callable with the same signature surviving consumers
/// expect.
pub fn update_tray_status(_status: TrayStatus) {
    // SwiftUI owns the menu bar; legacy AppKit tray status is a no-op.
    trace!(status = ?_status, "update_tray_status no-op (SwiftUI owns the menu bar)");
}
