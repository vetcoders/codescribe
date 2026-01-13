//! Thread-local state and cross-thread update channels
//!
//! Manages tray status updates from async tasks to the main thread.

use std::cell::RefCell;
use std::sync::OnceLock;

use crossbeam_channel::{Receiver, Sender, unbounded};
use tracing::debug;

use crate::tray::menu::update_status_label;
use crate::tray::types::{
    HistoryMenuItems, HoldMenuItems, ModelMenuItems, ToggleMenuItems, TrayMenuEvent, TrayStatus,
};

// ============================================================================
// Thread-local Menu Item Storage
// ============================================================================

thread_local! {
    pub static MODEL_MENU_ITEMS: RefCell<Option<ModelMenuItems>> = const { RefCell::new(None) };
    pub static HOLD_MENU_ITEMS: RefCell<Option<HoldMenuItems>> = const { RefCell::new(None) };
    pub static TOGGLE_MENU_ITEMS: RefCell<Option<ToggleMenuItems>> = const { RefCell::new(None) };
    pub static HISTORY_MENU_ITEMS: RefCell<Option<HistoryMenuItems>> = const { RefCell::new(None) };
}

// ============================================================================
// Global Channels
// ============================================================================

/// Channel for status updates (crossbeam for sync safety)
pub static STATUS_CHANNEL: OnceLock<Sender<TrayStatus>> = OnceLock::new();

/// Channel for menu events
pub static MENU_EVENT_CHANNEL: OnceLock<Sender<TrayMenuEvent>> = OnceLock::new();

// ============================================================================
// Public API Functions
// ============================================================================

/// Update the tray icon to reflect current status
pub fn update_tray_status(status: TrayStatus) -> anyhow::Result<()> {
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

/// Get a receiver for menu events (call once from main controller)
pub fn menu_event_receiver() -> anyhow::Result<Receiver<TrayMenuEvent>> {
    let (tx, rx) = unbounded();
    MENU_EVENT_CHANNEL
        .set(tx)
        .map_err(|_| anyhow::anyhow!("Menu event channel already initialized"))?;
    Ok(rx)
}

/// Send a menu event to the main controller
pub fn send_menu_event(event: TrayMenuEvent) {
    if let Some(sender) = MENU_EVENT_CHANNEL.get() {
        if let Err(e) = sender.send(event) {
            debug!("Failed to send menu event: {}", e);
        }
    }
}

// ============================================================================
// Main Thread Apply Functions
// ============================================================================

/// Apply status update to the menu label (must be called on main thread)
pub fn apply_status_update(status: TrayStatus) {
    update_status_label(status.menu_label());
}

// ============================================================================
// Channel Initialization (for run loop)
// ============================================================================

/// Initialize status channel, returning receiver for the event loop
pub fn init_channels() -> anyhow::Result<Receiver<TrayStatus>> {
    // Create channel for status updates
    let (status_tx, status_rx): (Sender<TrayStatus>, Receiver<TrayStatus>) = unbounded();
    STATUS_CHANNEL
        .set(status_tx)
        .map_err(|_| anyhow::anyhow!("Status channel already initialized"))?;

    Ok(status_rx)
}
