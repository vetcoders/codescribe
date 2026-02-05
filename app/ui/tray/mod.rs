//! System tray icon and menu for CodeScribe
//!
//! Provides visual status feedback and menu controls via macOS menu bar icon.
//! Uses tao event loop for proper macOS integration.
//!
//! ## Module Structure
//!
//! - `types` - Type definitions (TrayStatus, TrayMenuEvent, MenuIds)
//! - `icons` - Icon rendering and status glyph management
//! - `state` - Cross-thread channels for status updates
//! - `menu` - Menu building logic
//! - `submenus` - Submenu building functions
//! - `handlers` - Menu action handlers
//!
//! ## Menu Structure
//!
//! ```text
//! Status: Done!
//! ─────────────
//! [✓] AI Formatting
//!     Copy Last to Clipboard
//! ─────────────
//! Settings ▸
//!     ├── Hold Hotkeys ▸
//!     │   ├── Ctrl only
//!     │   ├── Ctrl+Option
//!     │   ├── Ctrl+Shift
//!     │   └── Ctrl+Command
//!     ├── Recent Transcripts ▸
//!     │   ├── [5 entries]
//!     │   └── Open Folder
//!     └── Edit Config File
//! ─────────────
//! Help
//! About
//! ─────────────
//! Quit
//! ```

mod handlers;
mod icons;
mod menu;
mod state;
mod submenus;
mod types;

use std::sync::OnceLock;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use crate::os::hotkeys;
use anyhow::Result;
use codescribe_core::vad;
use crossbeam_channel::TryRecvError;
use tao::event::Event;
use tao::event_loop::{ControlFlow, EventLoopBuilder};
use tracing::{debug, info};
use tray_icon::{TrayIconBuilder, menu::MenuEvent};

// Re-export public API
pub use menu::update_silero_vad_label;
pub use menu::update_vad_preset_checks;
pub use menu::{toggle_ai_formatting, update_quality_label};
pub use state::send_menu_event;
pub use state::{menu_event_receiver, update_tray_status};
pub use types::{MenuIds, TrayMenuEvent, TrayStatus, VadPreset};

// ============================================================================
// Shutdown Management
// ============================================================================

/// Global shutdown flag for graceful exit
static SHUTDOWN_REQUESTED: OnceLock<std::sync::atomic::AtomicBool> = OnceLock::new();

/// Request graceful shutdown of the tray application.
///
/// This can be called from any thread to signal that the app should exit.
/// The event loop will check this flag and perform cleanup before exiting.
pub fn request_shutdown() {
    if let Some(flag) = SHUTDOWN_REQUESTED.get() {
        flag.store(true, Ordering::SeqCst);
        info!("Shutdown requested");
    }
}

/// Check if shutdown has been requested
pub fn is_shutdown_requested() -> bool {
    SHUTDOWN_REQUESTED
        .get()
        .map(|f| f.load(Ordering::SeqCst))
        .unwrap_or(false)
}

// ============================================================================
// Run Functions
// ============================================================================

/// Run the tray application (blocking)
///
/// Uses tao event loop for proper macOS integration.
/// Optionally accepts a HotkeyManager to process hotkey events in the same loop.
pub fn run() -> Result<()> {
    run_with_hotkeys(None)
}

/// Run the tray application with optional hotkey manager
///
/// The hotkey manager must be created on main thread before calling this.
///
/// ## Shutdown Behavior
///
/// The event loop will exit when:
/// - User clicks Quit in the tray menu
/// - `request_shutdown()` is called from any thread
/// - Status channel is disconnected
///
/// On exit, cleanup is performed:
/// - Hotkey manager is dropped (unregisters hotkeys)
/// - Tray icon is removed
/// - All channels are closed
pub fn run_with_hotkeys(hotkey_manager: Option<hotkeys::HotkeyManager>) -> Result<()> {
    info!("Initializing system tray...");

    // Initialize shutdown flag
    SHUTDOWN_REQUESTED.get_or_init(|| std::sync::atomic::AtomicBool::new(false));

    // Initialize status channel
    let status_rx = state::init_channels()?;

    // Build event loop (must be on main thread for macOS)
    let event_loop = EventLoopBuilder::new().build();

    // Build the menu and get IDs
    let (menu, menu_ids) = menu::build_menu()?;

    // Create initial icon
    let initial_status = TrayStatus::Idle;
    let icon = initial_status.to_icon()?;

    // Build the tray icon
    let tray_icon = TrayIconBuilder::new()
        .with_menu(Box::new(menu))
        .with_tooltip(initial_status.tooltip())
        .with_icon(icon)
        .build()?;

    info!("System tray initialized");

    // Get menu event receiver
    let menu_channel = MenuEvent::receiver();

    if hotkey_manager.is_some() {
        info!("Global hotkeys enabled");
    }

    info!("Starting tray event loop...");
    info!("Press Quit in the tray menu to exit");

    // Poll interval for checking channels
    let poll_interval = Duration::from_millis(100);
    let mut last_menu_refresh = Instant::now();

    // Run the event loop
    event_loop.run(move |event, _, control_flow| {
        // Use WaitUntil to avoid busy-waiting while still checking channels
        *control_flow = ControlFlow::WaitUntil(Instant::now() + poll_interval);

        // Handle dock icon click (macOS Reopen event)
        // Note: GUI was removed, dock click now just logs the event
        if let Event::Reopen { .. } = event {
            debug!("Dock icon clicked (no GUI available)");
            return;
        }

        // Check for programmatic shutdown request
        if is_shutdown_requested() {
            info!("Shutdown flag detected, performing cleanup...");
            vad::shutdown();
            *control_flow = ControlFlow::Exit;
            return;
        }

        // Process hotkey events (integrated with main event loop for macOS)
        if let Some(ref hk_manager) = hotkey_manager {
            hk_manager.process_events();
        }

        // Periodic menu label refresh (must run on main thread)
        if last_menu_refresh.elapsed() >= Duration::from_secs(2) {
            menu::update_quality_label();
            menu::update_silero_vad_label();
            menu::update_vad_preset_checks();
            last_menu_refresh = Instant::now();
        }

        // Check for status updates (non-blocking)
        match status_rx.try_recv() {
            Ok(new_status) => {
                debug!("Received status update: {:?}", new_status);

                // Update menu label
                state::apply_status_update(new_status);

                // Update tooltip
                if let Err(e) = tray_icon.set_tooltip(Some(new_status.tooltip())) {
                    debug!("Failed to update tray tooltip: {}", e);
                }

                // Update icon
                if let Ok(new_icon) = new_status.to_icon()
                    && let Err(e) = tray_icon.set_icon(Some(new_icon))
                {
                    debug!("Failed to update tray icon: {}", e);
                }

                info!("Tray status updated to: {:?}", new_status);
            }
            Err(TryRecvError::Empty) => {}
            Err(TryRecvError::Disconnected) => {
                info!("Status channel closed, exiting");
                *control_flow = ControlFlow::Exit;
            }
        }

        // Check for menu events (non-blocking)
        if let Ok(event) = menu_channel.try_recv() {
            debug!("Menu event received: id={:?}", event.id);
            // Handle menu item clicks
            handlers::handle_menu_event(&event.id, &menu_ids);

            // Handle Quit specially to exit event loop
            if event.id == menu_ids.quit {
                info!("Quit requested via menu, exiting...");
                vad::shutdown();
                *control_flow = ControlFlow::Exit;
            }
        }
    });

    // Note: This code is unreachable because event_loop.run() never returns
    // on macOS. Cleanup happens when the closures are dropped.
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_icon_creation() {
        let icon = TrayStatus::Idle.to_icon();
        assert!(icon.is_ok());
    }

    #[test]
    fn test_status_tooltips() {
        assert_eq!(TrayStatus::Idle.tooltip(), "CodeScribe - Ready");
        assert_eq!(TrayStatus::Listening.tooltip(), "CodeScribe - Recording...");
        assert_eq!(TrayStatus::Thinking.tooltip(), "CodeScribe - Processing...");
        assert_eq!(TrayStatus::Success.tooltip(), "CodeScribe - Done!");
    }

    #[test]
    fn test_status_menu_labels() {
        assert_eq!(TrayStatus::Idle.menu_label(), "Status: Idle");
        assert_eq!(TrayStatus::Listening.menu_label(), "Status: Recording...");
        assert_eq!(TrayStatus::Thinking.menu_label(), "Status: Processing...");
        assert_eq!(TrayStatus::Success.menu_label(), "Status: Done!");
    }
}
