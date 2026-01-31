//! Menu action handlers for tray menu events
//!
//! Handles menu item clicks and dispatches appropriate events.

use muda::MenuId;
use std::process::Command;
use tracing::{debug, info};

use crate::config::{Config, HoldMods, ToggleTrigger};
use crate::os::clipboard;
use crate::os::permissions;
use crate::tray::state::{HOTKEYS_MENU_ITEMS, send_menu_event};
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
    } else if event_id == &menu_ids.copy_diagnostics {
        handle_copy_diagnostics();
    } else if event_id == &menu_ids.open_assistive_prompt {
        handle_open_assistive_prompt();
    } else if event_id == &menu_ids.open_formatting_prompt {
        handle_open_formatting_prompt();
    } else if event_id == &menu_ids.open_prompts_folder {
        handle_open_prompts_folder();
    } else if event_id == &menu_ids.help {
        handle_open_help();
    } else if event_id == &menu_ids.about {
        handle_show_about();
    } else if event_id == &menu_ids.quit {
        send_menu_event(TrayMenuEvent::Quit);
    }
    // Hotkeys submenu
    else if event_id == &menu_ids.hotkeys_toggle_assistive {
        handle_toggle_assistive_toggle();
    } else if event_id == &menu_ids.hotkeys_reset {
        handle_reset_hotkeys();
    }
    // Quality - Open Report
    else if event_id == &menu_ids.quality_open_report {
        handle_open_quality_report();
    } else if event_id == &menu_ids.silero_vad_install {
        handle_install_silero_vad();
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
            if let Err(e) = clipboard::set_clipboard(&text) {
                info!("Failed to copy to clipboard: {}", e);
            } else {
                info!("Copied last transcript to clipboard ({} chars)", text.len());
            }
        }
    } else {
        info!("No transcript history available");
    }
}

fn handle_copy_diagnostics() {
    send_menu_event(TrayMenuEvent::CopyDiagnostics);

    let report = permissions::diagnostics_report();
    if let Err(e) = clipboard::set_clipboard(&report) {
        info!("Failed to copy diagnostics to clipboard: {}", e);
        return;
    }

    #[cfg(target_os = "macos")]
    {
        let _ = Command::new("osascript")
            .arg("-e")
            .arg(
                r#"display notification "Copied diagnostics to clipboard" with title "CodeScribe""#,
            )
            .spawn();
    }
}

fn handle_open_assistive_prompt() {
    send_menu_event(TrayMenuEvent::OpenAssistivePrompt);
    crate::config::open_prompt_file("assistive.txt");
}

fn handle_open_formatting_prompt() {
    send_menu_event(TrayMenuEvent::OpenFormattingPrompt);
    crate::config::open_prompt_file("formatting.txt");
}

fn handle_open_prompts_folder() {
    send_menu_event(TrayMenuEvent::OpenPromptsFolder);
    crate::config::open_prompts_folder();
}

fn handle_install_silero_vad() {
    send_menu_event(TrayMenuEvent::InstallSileroVad);
}

// ============================================================================
// Hotkeys Handlers
// ============================================================================

/// Toggle the assistive "right Option" trigger on/off.
fn handle_toggle_assistive_toggle() {
    let config = Config::load();
    let currently_enabled = config.toggle_trigger != ToggleTrigger::None;
    let new_trigger = if currently_enabled {
        ToggleTrigger::None
    } else {
        ToggleTrigger::DoubleRightOption
    };

    info!(
        "Toggling assistive right-Option trigger: {} -> {:?}",
        if currently_enabled { "ON" } else { "OFF" },
        new_trigger
    );

    // Persist to config + notify daemon to re-sync hotkeys deterministically.
    let _ = config.save_to_env("TOGGLE_TRIGGER", new_trigger.as_str());
    send_menu_event(TrayMenuEvent::SetToggleTrigger(new_trigger));

    // Update menu visuals.
    HOTKEYS_MENU_ITEMS.with(|items_cell| {
        if let Some(ref items) = *items_cell.borrow() {
            items
                .toggle_assistive
                .set_checked(new_trigger != ToggleTrigger::None);
            items.toggle_label.set_text(format!(
                "Right Option toggle (assistive): {}",
                if new_trigger != ToggleTrigger::None {
                    "ON"
                } else {
                    "OFF"
                }
            ));
        }
    });
}

/// Reset hotkeys to a safe, recommended default.
fn handle_reset_hotkeys() {
    info!("Resetting hotkeys to recommended defaults");

    let config = Config::load();

    // Ensure Shift/Cmd mode layer is enabled.
    let _ = config.save_to_env("HOLD_EXCLUSIVE", "0");
    // Recommended: Hold Ctrl for RAW; Shift/Cmd as modes.
    let _ = config.save_to_env("HOLD_MODS", HoldMods::Ctrl.as_str());
    // Recommended: enable assistive toggle (right Option) by default.
    let _ = config.save_to_env("TOGGLE_TRIGGER", ToggleTrigger::DoubleRightOption.as_str());

    send_menu_event(TrayMenuEvent::SetHoldMods(HoldMods::Ctrl));
    send_menu_event(TrayMenuEvent::SetToggleTrigger(
        ToggleTrigger::DoubleRightOption,
    ));
    send_menu_event(TrayMenuEvent::ResetShortcuts);

    HOTKEYS_MENU_ITEMS.with(|items_cell| {
        if let Some(ref items) = *items_cell.borrow() {
            items.toggle_assistive.set_checked(true);
            items
                .toggle_label
                .set_text("Right Option toggle (assistive): ON");
        }
    });
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
