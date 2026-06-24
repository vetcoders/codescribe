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
//! - `handlers` - Menu action handlers
//!
//! ## Menu Structure
//!
//! ```text
//! Status: Starting... / Idle
//! Show Agent
//! Open history...
//! Copy last transcript
//! Notes ▸
//! Diagnostics ▸
//! Continue Onboarding... (when onboarding is incomplete)
//! Settings
//! Help
//! About
//! Quit
//! ```

mod handlers;
mod icons;
mod menu;
mod state;
mod types;

use std::sync::OnceLock;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use crate::os::hotkeys;
use anyhow::Result;
use crossbeam_channel::TryRecvError;
use tao::event::Event;
use tao::event_loop::{ControlFlow, EventLoopBuilder};
use tracing::{debug, info};
use tray_icon::{TrayIconBuilder, menu::MenuEvent};

// Re-export public API
pub use menu::update_quality_label;
pub use menu::update_silero_vad_label;
pub use state::send_menu_event;
pub use state::{menu_event_receiver, update_tray_status};
pub use types::{MenuIds, TrayMenuEvent, TrayStatus};

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
pub fn run() -> Result<()> {
    run_with_hotkeys(None)
}

pub fn run_with_startup<F>(
    hotkey_manager: Option<hotkeys::HotkeyManager>,
    on_started: F,
) -> Result<()>
where
    F: FnOnce() + 'static,
{
    run_inner(hotkey_manager, on_started)
}

fn shutdown_hotkeys(hotkey_manager: &mut Option<hotkeys::HotkeyManager>) {
    if let Some(hk_manager) = hotkey_manager.as_mut() {
        hk_manager.shutdown();
    }
    hotkeys::shutdown_global_hotkey_manager();
    *hotkey_manager = None;
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
/// - Hotkey runtime is explicitly shut down (event tap disabled, run loop stopped, thread joined)
/// - Tray icon is removed
/// - All channels are closed
pub fn run_with_hotkeys(hotkey_manager: Option<hotkeys::HotkeyManager>) -> Result<()> {
    run_inner(hotkey_manager, || {})
}

fn run_inner<F>(hotkey_manager: Option<hotkeys::HotkeyManager>, on_started: F) -> Result<()>
where
    F: FnOnce() + 'static,
{
    info!("Initializing system tray...");

    // Inject layoutRegionGuides stub into NSVisualEffectView early,
    // before AppKit creates any internal instances (Tahoe beta workaround).
    super::shared::helpers::ensure_layout_region_guides_exists();

    // Initialize shutdown flag
    SHUTDOWN_REQUESTED.get_or_init(|| std::sync::atomic::AtomicBool::new(false));

    // Initialize status channel
    let status_rx = state::init_channels()?;

    // Build event loop (must be on main thread for macOS)
    let event_loop = EventLoopBuilder::new().build();

    // Build the menu and get IDs
    let initial_status = TrayStatus::Starting;
    let (menu, menu_ids) = menu::build_menu(initial_status)?;

    // Create initial icon
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

    if hotkey_manager.is_some() || hotkeys::is_global_hotkey_manager_active() {
        info!("Global hotkeys enabled");
    }

    info!("Starting tray event loop...");
    info!("Press Quit in the tray menu to exit");

    // Poll interval for checking channels
    let poll_interval = Duration::from_millis(100);
    let startup_blink_interval = Duration::from_millis(500);
    let mut current_status = initial_status;
    let mut startup_glyph_visible = true;
    let mut last_startup_blink = Instant::now();
    let mut last_menu_refresh = Instant::now();
    let mut hotkey_manager = hotkey_manager;

    on_started();

    // Run the event loop
    event_loop.run(move |event, _, control_flow| {
        // Use WaitUntil to avoid busy-waiting while still checking channels.
        // Startup blinking needs a shorter wake-up while the app is not ready.
        let wake_interval = if current_status == TrayStatus::Starting {
            poll_interval.min(startup_blink_interval)
        } else {
            poll_interval
        };
        *control_flow = ControlFlow::WaitUntil(Instant::now() + wake_interval);

        // Handle dock icon click (macOS Reopen event)
        if let Event::Reopen { .. } = event {
            debug!("Dock icon clicked → opening Settings window");
            crate::ui::settings::show_settings_window();
            return;
        }

        // Check for programmatic shutdown request
        if is_shutdown_requested() {
            info!("Shutdown flag detected, performing cleanup...");
            let _ = crate::qube_lifecycle::stop_managed();
            shutdown_hotkeys(&mut hotkey_manager);
            *control_flow = ControlFlow::Exit;
            return;
        }

        // Periodic menu label refresh (must run on main thread)
        if last_menu_refresh.elapsed() >= Duration::from_secs(2) {
            menu::update_quality_label();
            menu::update_silero_vad_label();
            menu::update_onboarding_item();
            last_menu_refresh = Instant::now();
        }

        if current_status == TrayStatus::Starting
            && last_startup_blink.elapsed() >= startup_blink_interval
        {
            startup_glyph_visible = !startup_glyph_visible;
            last_startup_blink = Instant::now();
            if let Ok(new_icon) = current_status.to_icon_with_glyph(startup_glyph_visible)
                && let Err(e) = tray_icon.set_icon(Some(new_icon))
            {
                debug!("Failed to blink startup tray icon: {}", e);
            }
        }

        // Check for status updates (non-blocking)
        match status_rx.try_recv() {
            Ok(new_status) => {
                debug!("Received status update: {:?}", new_status);
                current_status = new_status;
                startup_glyph_visible = true;
                last_startup_blink = Instant::now();

                // Update menu label
                state::apply_status_update(new_status);

                // Update tooltip
                if let Err(e) = tray_icon.set_tooltip(Some(new_status.tooltip())) {
                    debug!("Failed to update tray tooltip: {}", e);
                }

                // Update icon
                if let Ok(new_icon) = new_status.to_icon_with_glyph(true)
                    && let Err(e) = tray_icon.set_icon(Some(new_icon))
                {
                    debug!("Failed to update tray icon: {}", e);
                }

                info!("Tray status updated to: {:?}", new_status);
            }
            Err(TryRecvError::Empty) => {}
            Err(TryRecvError::Disconnected) => {
                info!("Status channel closed, exiting");
                let _ = crate::qube_lifecycle::stop_managed();
                shutdown_hotkeys(&mut hotkey_manager);
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
                let _ = crate::qube_lifecycle::stop_managed();
                shutdown_hotkeys(&mut hotkey_manager);
                *control_flow = ControlFlow::Exit;
            }
        }
    });

    // Note: This code is unreachable because event_loop.run() never returns on macOS.
    // Hotkeys are shut down in-loop before requesting exit.
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
        let startup_icon = TrayStatus::Starting.to_icon_with_glyph(false);
        assert!(startup_icon.is_ok());
    }

    #[test]
    fn test_status_tooltips() {
        assert_eq!(TrayStatus::Starting.tooltip(), "CodeScribe - Starting...");
        assert_eq!(TrayStatus::Idle.tooltip(), "CodeScribe - Ready");
        assert_eq!(TrayStatus::Listening.tooltip(), "CodeScribe - Recording...");
        assert_eq!(TrayStatus::Thinking.tooltip(), "CodeScribe - Processing...");
        assert_eq!(TrayStatus::Success.tooltip(), "CodeScribe - Done!");
        assert_eq!(
            TrayStatus::Thermal.tooltip(),
            "CodeScribe - Thermal throttling"
        );
        assert_eq!(
            TrayStatus::HotkeyConflict.tooltip(),
            "CodeScribe - Hotkey conflict"
        );
    }

    #[test]
    fn test_status_menu_labels() {
        assert_eq!(TrayStatus::Starting.menu_label(), "Status: Starting...");
        assert_eq!(TrayStatus::Idle.menu_label(), "Status: Idle");
        assert_eq!(TrayStatus::Listening.menu_label(), "Status: Recording...");
        assert_eq!(TrayStatus::Thinking.menu_label(), "Status: Processing...");
        assert_eq!(TrayStatus::Success.menu_label(), "Status: Done!");
        assert_eq!(
            TrayStatus::Thermal.menu_label(),
            "Status: Thermal throttling"
        );
        assert_eq!(
            TrayStatus::HotkeyConflict.menu_label(),
            "Status: Hotkey conflict"
        );
    }
}
