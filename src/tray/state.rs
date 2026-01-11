//! Thread-local state and cross-thread update channels
//!
//! Manages menu item state that must be accessed from the main thread,
//! with channels for updates from async tasks.

use std::cell::RefCell;
use std::sync::OnceLock;

use crossbeam_channel::{Receiver, Sender, unbounded};
use tracing::{debug, info};

use crate::tray::types::{
    HistoryMenuItems, HoldMenuItems, HoldMods, ModelMenuItems, ToggleMenuItems, ToggleTrigger,
    TrayMenuEvent, TrayStatus,
};

// ============================================================================
// Thread-Local Menu Item Storage
// ============================================================================

// Thread-local storage for menu items (CheckMenuItem contains Rc, not Send/Sync)
// Updates are done via channels from other threads
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

/// Channel for model selection updates from async tasks
pub static MODEL_UPDATE_CHANNEL: OnceLock<Sender<String>> = OnceLock::new();

/// Channel for history label updates from async tasks
pub static HISTORY_UPDATE_CHANNEL: OnceLock<Sender<String>> = OnceLock::new();

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

/// Update the model selection in the menu
///
/// Variant should be one of: "small", "medium", "large-v3", "large-v3-turbo"
/// Thread-safe: can be called from any thread (sends via channel to main thread)
pub fn update_model_selection(variant: &str) {
    if let Some(sender) = MODEL_UPDATE_CHANNEL.get() {
        if let Err(e) = sender.send(variant.to_string()) {
            debug!("Failed to send model update: {}", e);
        }
    } else {
        debug!("Model update channel not initialized");
    }
}

/// Update the history label in the menu (thread-safe)
/// Call this after saving a new transcription
pub fn update_history_label(text: &str) {
    if let Some(sender) = HISTORY_UPDATE_CHANNEL.get() {
        // Truncate for menu display
        let display = if text.chars().count() > 30 {
            let truncated: String = text.chars().take(27).collect();
            format!("Latest: {}...", truncated)
        } else {
            format!("Latest: {}", text)
        };
        if let Err(e) = sender.send(display) {
            debug!("Failed to send history update: {}", e);
        }
    } else {
        debug!("History update channel not initialized");
    }
}

// ============================================================================
// Main Thread Apply Functions
// ============================================================================

/// Actually update the model menu items (must be called on main thread)
pub fn apply_model_selection(variant: &str) {
    MODEL_MENU_ITEMS.with(|items_cell| {
        if let Some(items) = items_cell.borrow().as_ref() {
            // Uncheck all models
            items.small.set_checked(false);
            items.medium.set_checked(false);
            items.large_v3.set_checked(false);
            items.large_v3_turbo.set_checked(false);
            items.large_v3_q8.set_checked(false);

            // Check the selected model
            match variant {
                "small" => items.small.set_checked(true),
                "medium" => items.medium.set_checked(true),
                "large-v3" => items.large_v3.set_checked(true),
                "large-v3-turbo" => items.large_v3_turbo.set_checked(true),
                "large-v3-q8" | "large-v3-mlx-q8" => items.large_v3_q8.set_checked(true),
                _ => debug!("Unknown model variant: {}", variant),
            }

            // Update the label text
            let label_text = match variant {
                "small" => "Whisper: Small",
                "medium" => "Whisper: Medium",
                "large-v3" => "Whisper: Large v3",
                "large-v3-turbo" => "Whisper: Large v3 Turbo",
                "large-v3-q8" | "large-v3-mlx-q8" => "Whisper: Large v3 Q8",
                _ => variant,
            };
            items.label.set_text(label_text);

            info!("Model selection updated to: {}", variant);
        }
    });
}

/// Actually update the history label (must be called on main thread)
pub fn apply_history_label_update(label_text: &str) {
    HISTORY_MENU_ITEMS.with(|items_cell| {
        if let Some(items) = items_cell.borrow().as_ref() {
            items.latest_label.set_text(label_text);
            info!("History label updated: {}", label_text);
        }
    });
}

/// Apply hold mods selection (radio-button behavior)
/// Must be called on main thread
pub fn apply_hold_mods_selection(mods: HoldMods) {
    HOLD_MENU_ITEMS.with(|items_cell| {
        if let Some(items) = items_cell.borrow().as_ref() {
            // Uncheck all
            items.ctrl.set_checked(false);
            items.ctrl_opt.set_checked(false);
            items.ctrl_shift.set_checked(false);
            items.ctrl_cmd.set_checked(false);

            // Check selected
            match mods {
                HoldMods::Ctrl => items.ctrl.set_checked(true),
                HoldMods::CtrlAlt => items.ctrl_opt.set_checked(true),
                HoldMods::CtrlShift => items.ctrl_shift.set_checked(true),
                HoldMods::CtrlCmd => items.ctrl_cmd.set_checked(true),
            }

            // Update the label text
            items.label.set_text(format!("Current: {}", mods.label()));

            info!("Hold mods selection updated to: {:?}", mods);
        }
    });
}

/// Apply toggle trigger selection (radio-button behavior)
/// Must be called on main thread
pub fn apply_toggle_trigger_selection(trigger: ToggleTrigger) {
    TOGGLE_MENU_ITEMS.with(|items_cell| {
        if let Some(items) = items_cell.borrow().as_ref() {
            // Uncheck all
            items.double_opt.set_checked(false);
            items.double_ralt.set_checked(false);
            items.disabled.set_checked(false);

            // Check selected
            match trigger {
                ToggleTrigger::DoubleOption => items.double_opt.set_checked(true),
                ToggleTrigger::DoubleRightOption => items.double_ralt.set_checked(true),
                ToggleTrigger::None => items.disabled.set_checked(true),
            }

            // Update the label text
            items.label.set_text(format!("Toggle: {}", trigger.label()));

            info!("Toggle trigger selection updated to: {:?}", trigger);
        }
    });
}

// ============================================================================
// Channel Initialization (for run loop)
// ============================================================================

/// Initialize all update channels, returning receivers for the event loop
pub fn init_channels() -> anyhow::Result<(Receiver<TrayStatus>, Receiver<String>, Receiver<String>)>
{
    // Create channel for status updates
    let (status_tx, status_rx): (Sender<TrayStatus>, Receiver<TrayStatus>) = unbounded();
    STATUS_CHANNEL
        .set(status_tx)
        .map_err(|_| anyhow::anyhow!("Status channel already initialized"))?;

    // Create channel for model selection updates
    let (model_tx, model_rx): (Sender<String>, Receiver<String>) = unbounded();
    MODEL_UPDATE_CHANNEL
        .set(model_tx)
        .map_err(|_| anyhow::anyhow!("Model update channel already initialized"))?;

    // Create channel for history label updates
    let (history_tx, history_rx): (Sender<String>, Receiver<String>) = unbounded();
    HISTORY_UPDATE_CHANNEL
        .set(history_tx)
        .map_err(|_| anyhow::anyhow!("History update channel already initialized"))?;

    Ok((status_rx, model_rx, history_rx))
}
