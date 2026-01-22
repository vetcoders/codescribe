//! Menu action handlers for tray menu events
//!
//! Handles menu item clicks and dispatches appropriate events.

use muda::MenuId;
use std::process::Command;
use tracing::{debug, info};

use crate::config::{Config, HoldMods, ToggleTrigger};
use crate::tray::state::{HOLD_MENU_ITEMS, TOGGLE_MENU_ITEMS, send_menu_event};
use crate::tray::types::{MenuIds, TrayMenuEvent};

/// Handle menu item click and send appropriate event
/// Note: Settings handlers removed - settings now in Chat Overlay Settings tab
pub fn handle_menu_event(event_id: &MenuId, menu_ids: &MenuIds) {
    // Top-level items
    if event_id == &menu_ids.copy_last {
        handle_copy_last();
    } else if event_id == &menu_ids.show_overlay {
        crate::show_voice_chat_overlay();
    } else if event_id == &menu_ids.open_history {
        handle_open_history_folder();
    } else if event_id == &menu_ids.help {
        handle_open_help();
    } else if event_id == &menu_ids.about {
        handle_show_about();
    } else if event_id == &menu_ids.quit {
        send_menu_event(TrayMenuEvent::Quit);
    }
    // Hold Hotkeys submenu
    else if event_id == &menu_ids.hold_ctrl {
        handle_set_hold_mods(HoldMods::Ctrl);
    } else if event_id == &menu_ids.hold_ctrl_opt {
        handle_set_hold_mods(HoldMods::CtrlAlt);
    } else if event_id == &menu_ids.hold_ctrl_shift {
        handle_set_hold_mods(HoldMods::CtrlShift);
    } else if event_id == &menu_ids.hold_ctrl_cmd {
        handle_set_hold_mods(HoldMods::CtrlCmd);
    } else if event_id == &menu_ids.hold_exclusive {
        handle_toggle_hold_exclusive();
    }
    // Toggle trigger submenu
    else if event_id == &menu_ids.toggle_double_opt {
        handle_set_toggle_trigger(ToggleTrigger::DoubleOption);
    } else if event_id == &menu_ids.toggle_double_ralt {
        handle_set_toggle_trigger(ToggleTrigger::DoubleRightOption);
    } else if event_id == &menu_ids.toggle_disabled {
        handle_set_toggle_trigger(ToggleTrigger::None);
    }
    // Quality - Open Report
    else if event_id == &menu_ids.quality_open_report {
        handle_open_quality_report();
    } else {
        debug!("Unknown menu event id: {:?}", event_id);
    }
}

/// Copy last transcript to clipboard
fn handle_copy_last() {
    send_menu_event(TrayMenuEvent::CopyLast);

    // Get last transcript from history
    if let Some(last_entry) = crate::state::history::latest_entry() {
        if let Ok(text) = std::fs::read_to_string(&last_entry.path) {
            if let Err(e) = crate::clipboard::set_clipboard(&text) {
                info!("Failed to copy to clipboard: {}", e);
            } else {
                info!("Copied last transcript to clipboard ({} chars)", text.len());
            }
        }
    } else {
        info!("No transcript history available");
    }
}

// ============================================================================
// Hold Hotkeys Handlers
// ============================================================================

/// Set hold modifier keys and update menu checkmarks
fn handle_set_hold_mods(mods: HoldMods) {
    info!("Setting hold mods to: {:?}", mods);
    send_menu_event(TrayMenuEvent::SetHoldMods(mods));

    // Update menu checkmarks (radio behavior)
    HOLD_MENU_ITEMS.with(|items_cell| {
        if let Some(ref items) = *items_cell.borrow() {
            items.ctrl.set_checked(mods == HoldMods::Ctrl);
            items.ctrl_opt.set_checked(mods == HoldMods::CtrlAlt);
            items.ctrl_shift.set_checked(mods == HoldMods::CtrlShift);
            items.ctrl_cmd.set_checked(mods == HoldMods::CtrlCmd);
            items.label.set_text(format!("Current: {}", mods.label()));
        }
    });

    // Persist to config
    let config = Config::load();
    let _ = config.save_to_env("HOLD_MODS", mods.as_str());
}

/// Toggle hold exclusive mode
fn handle_toggle_hold_exclusive() {
    send_menu_event(TrayMenuEvent::ToggleHoldExclusive);

    let config = Config::load();
    let new_state = !config.hold_exclusive;
    let _ = config.save_to_env("HOLD_EXCLUSIVE", if new_state { "1" } else { "0" });
    info!(
        "Hold exclusive toggled: {}",
        if new_state { "ON" } else { "OFF" }
    );
}

/// Set toggle trigger and update menu checkmarks
fn handle_set_toggle_trigger(trigger: ToggleTrigger) {
    info!("Setting toggle trigger to: {:?}", trigger);
    send_menu_event(TrayMenuEvent::SetToggleTrigger(trigger));

    // Update menu checkmarks (radio behavior)
    TOGGLE_MENU_ITEMS.with(|items_cell| {
        if let Some(ref items) = *items_cell.borrow() {
            items
                .double_opt
                .set_checked(trigger == ToggleTrigger::DoubleOption);
            items
                .double_ralt
                .set_checked(trigger == ToggleTrigger::DoubleRightOption);
            items.disabled.set_checked(trigger == ToggleTrigger::None);
            items.label.set_text(format!("Toggle: {}", trigger.label()));
        }
    });

    // Persist to config
    let config = Config::load();
    let _ = config.save_to_env("TOGGLE_TRIGGER", trigger.as_str());
}

/// Open history folder in Finder
fn handle_open_history_folder() {
    send_menu_event(TrayMenuEvent::OpenHistoryFolder);
    crate::state::history::open_history_folder();
    info!("Opening history folder");
}

/// Open help documentation in browser
fn handle_open_help() {
    send_menu_event(TrayMenuEvent::OpenHelp);

    #[cfg(target_os = "macos")]
    {
        // Try local docs first, fall back to GitHub
        let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
        let local_docs = format!("{}/.codescribe/docs/README.md", home);

        let url = if std::path::Path::new(&local_docs).exists() {
            local_docs
        } else {
            "https://github.com/VetCoders/CodeScribe#readme".to_string()
        };

        info!("Opening help: {}", url);
        let _ = Command::new("open").arg(&url).spawn();
    }
}

/// Show about dialog with version
fn handle_show_about() {
    send_menu_event(TrayMenuEvent::ShowAbout);

    #[cfg(target_os = "macos")]
    {
        let version = env!("CARGO_PKG_VERSION");
        let message = format!(
            "CodeScribe v{}\\n\\nSpeech-to-text for macOS\\n\\nCreated by M&K (c)2026 VetCoders",
            version
        );

        // Use osascript for native dialog
        let script = format!(
            r#"display dialog "{}" buttons {{"OK"}} default button "OK" with title "About CodeScribe" with icon note"#,
            message
        );

        info!("Showing about dialog");
        let _ = Command::new("osascript").arg("-e").arg(&script).spawn();
    }
}

// ============================================================================
// Quality Handlers
// ============================================================================

/// Open the latest quality report in browser
fn handle_open_quality_report() {
    info!("Opening quality report...");

    if crate::quality_loop::open_latest_report() {
        info!("Opened quality report");
    } else {
        // No report available - show notification
        info!("No quality report available");
        let _ = Command::new("osascript")
            .arg("-e")
            .arg(r#"display notification "No quality report available. Run: codescribe-loop --daemon" with title "CodeScribe Quality""#)
            .spawn();
    }
}
