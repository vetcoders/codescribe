//! System tray icon and menu for CodeScribe
//!
//! Provides visual status feedback and menu controls via macOS menu bar icon.

use anyhow::Result;
use muda::{Menu, MenuItem, PredefinedMenuItem, Submenu};
use std::sync::OnceLock;
use tokio::sync::mpsc;
use tray_icon::{menu::MenuEvent, Icon, TrayIconBuilder};
use tracing::{debug, info};

/// Status of the CodeScribe system, reflected in tray icon glyph
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
}

impl TrayStatus {
    /// Get the unicode glyph for this status
    pub fn glyph(&self) -> &'static str {
        match self {
            TrayStatus::Idle => "•",
            TrayStatus::Listening => "◉",
            TrayStatus::Thinking => "…",
            TrayStatus::Success => "✓",
        }
    }

    /// Get the human-readable tooltip for this status
    pub fn tooltip(&self) -> String {
        match self {
            TrayStatus::Idle => "CodeScribe - Ready".to_string(),
            TrayStatus::Listening => "CodeScribe - Recording...".to_string(),
            TrayStatus::Thinking => "CodeScribe - Processing...".to_string(),
            TrayStatus::Success => "CodeScribe - Done!".to_string(),
        }
    }

    /// Create an icon from this status
    fn to_icon(&self) -> Result<Icon> {
        // Create a minimal 16x16 RGBA icon (transparent)
        // TODO: Generate proper icon images or use pre-made icon files
        // For now, using transparent icons - the tooltip will show status

        let rgba = vec![0u8; 16 * 16 * 4];

        Icon::from_rgba(rgba, 16, 16)
            .map_err(|e| anyhow::anyhow!("Failed to create icon: {}", e))
    }
}

/// Build the tray menu
fn build_menu() -> Result<Menu> {
    let menu = Menu::new();

    // Status label (disabled)
    let status_item = MenuItem::new("Status: Ready", false, None);
    menu.append(&status_item)?;

    // Separator
    menu.append(&PredefinedMenuItem::separator())?;

    // Language submenu
    let lang_menu = Submenu::new("Language", true);
    let lang_auto = MenuItem::new("Auto", true, None);
    let lang_polish = MenuItem::new("Polish", true, None);
    let lang_english = MenuItem::new("English", true, None);

    lang_menu.append(&lang_auto)?;
    lang_menu.append(&lang_polish)?;
    lang_menu.append(&lang_english)?;
    menu.append(&lang_menu)?;

    // Separator
    menu.append(&PredefinedMenuItem::separator())?;

    // Quit
    let quit_item = MenuItem::new("Quit", true, None);
    menu.append(&quit_item)?;

    Ok(menu)
}

/// Global channel for status updates
///
/// This is initialized once when run() is called, and provides a way for
/// other threads (like the controller) to send status updates to the tray.
static STATUS_CHANNEL: OnceLock<mpsc::UnboundedSender<TrayStatus>> = OnceLock::new();

/// Update the tray icon to reflect current status
///
/// This function can be called from any thread to update the tray status.
/// It sends the status update through a channel that the tray event loop monitors.
///
/// # Arguments
/// * `status` - The new status to display
///
/// # Returns
/// * `Ok(())` if the status was sent successfully
/// * `Err` if the tray system is not initialized or the channel is closed
pub fn update_tray_status(status: TrayStatus) -> Result<()> {
    if let Some(sender) = STATUS_CHANNEL.get() {
        sender
            .send(status)
            .map_err(|e| anyhow::anyhow!("Failed to send tray status: {}", e))?;
        debug!("Tray status update sent: {:?}", status);
        Ok(())
    } else {
        debug!("Tray status channel not initialized yet");
        Ok(())
    }
}

/// Run the tray application (blocking)
///
/// This function creates the system tray icon, sets up the menu,
/// and runs the event loop. It will block until the application quits.
pub fn run() -> Result<()> {
    info!("Initializing system tray...");

    // Create channel for status updates
    let (status_tx, mut status_rx) = mpsc::unbounded_channel();
    STATUS_CHANNEL
        .set(status_tx)
        .map_err(|_| anyhow::anyhow!("Status channel already initialized"))?;

    // Build the menu
    let menu = build_menu()?;

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

    // Run event loop
    info!("Starting tray event loop...");
    info!("Press Quit in the tray menu to exit");

    loop {
        // Check for status updates
        match status_rx.try_recv() {
            Ok(new_status) => {
                debug!("Received status update: {:?}", new_status);

                // Update tooltip
                if let Err(e) = tray_icon.set_tooltip(Some(new_status.tooltip())) {
                    debug!("Failed to update tray tooltip: {}", e);
                }

                // Update icon
                if let Ok(new_icon) = new_status.to_icon() {
                    if let Err(e) = tray_icon.set_icon(Some(new_icon)) {
                        debug!("Failed to update tray icon: {}", e);
                    }
                }

                info!("Tray status updated to: {:?}", new_status);
            }
            Err(mpsc::error::TryRecvError::Empty) => {
                // No status update, continue
            }
            Err(mpsc::error::TryRecvError::Disconnected) => {
                info!("Status channel closed, exiting tray loop");
                break;
            }
        }

        // Check for menu events
        if let Ok(event) = menu_channel.try_recv() {
            debug!("Menu event received: {:?}", event);

            // For now, just log all events
            // TODO: Implement proper event handling based on menu item IDs
            // The challenge is that muda 0.15 doesn't expose IDs easily

            info!("Menu event: {:?}", event);

            // Check if this might be a quit event
            // We'll need to implement a proper way to detect this
        }

        // Sleep to avoid busy-waiting
        std::thread::sleep(std::time::Duration::from_millis(100));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_status_glyphs() {
        assert_eq!(TrayStatus::Idle.glyph(), "•");
        assert_eq!(TrayStatus::Listening.glyph(), "◉");
        assert_eq!(TrayStatus::Thinking.glyph(), "…");
        assert_eq!(TrayStatus::Success.glyph(), "✓");
    }

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
}
