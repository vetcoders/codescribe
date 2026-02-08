//! Menu action handlers for tray menu events
//! Handles menu item clicks and dispatches appropriate events.

use muda::MenuId;
use std::process::Command;
use tracing::{debug, info};

use crate::config::{Config, HoldMods, ToggleTrigger};
use crate::os::clipboard;
use crate::os::permissions;
use crate::tray::state::{NOTES_MENU_ITEMS, send_menu_event};
use crate::tray::types::{MenuIds, TrayMenuEvent};

#[cfg(target_os = "macos")]
fn notify(title: &str, message: &str) {
    crate::os::notifications::notify(title, message);
}

/// Handle menu item click and send appropriate event
/// Note: Settings handlers removed - settings now in Chat Overlay Settings tab
pub fn handle_menu_event(event_id: &MenuId, menu_ids: &MenuIds) {
    // Top-level items
    if event_id == &menu_ids.copy_last {
        handle_copy_last();
    } else if event_id == &menu_ids.show_overlay {
        crate::show_voice_chat_overlay();
    } else if event_id == &menu_ids.run_onboarding {
        crate::show_bootstrap_overlay();
    } else if event_id == &menu_ids.open_history {
        handle_open_history_folder();
    } else if event_id == &menu_ids.copy_diagnostics {
        handle_copy_diagnostics();
    } else if event_id == &menu_ids.open_accessibility_settings {
        handle_open_accessibility_settings();
    } else if event_id == &menu_ids.open_input_monitoring_settings {
        handle_open_input_monitoring_settings();
    } else if event_id == &menu_ids.reset_input_monitoring_permission {
        handle_reset_input_monitoring_permission();
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
    // Notes
    else if event_id == &menu_ids.notes_toggle_quick_notes {
        handle_toggle_quick_notes();
    } else if event_id == &menu_ids.notes_toggle_save_only {
        handle_toggle_quick_notes_save_only();
    } else if event_id == &menu_ids.notes_open_folder {
        handle_open_notes_folder();
    } else if event_id == &menu_ids.notes_open_today {
        handle_open_today_note();
    }
    // Hotkeys submenu
    else if event_id == &menu_ids.hotkeys_copy_cheatsheet {
        handle_copy_hotkeys_cheatsheet();
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

fn base_hold_cheatsheet_label(hold_mods: HoldMods) -> &'static str {
    match hold_mods {
        HoldMods::Fn => "Fn",
        HoldMods::Ctrl => "Ctrl",
        HoldMods::CtrlAlt => "Ctrl+Option",
        HoldMods::CtrlShift => "Ctrl+Shift",
        HoldMods::CtrlCmd => "Ctrl+Command",
    }
}

fn hands_off_cheatsheet_label(trigger: ToggleTrigger) -> &'static str {
    match trigger {
        ToggleTrigger::DoubleCtrl => "Double Ctrl (RAW)",
        ToggleTrigger::DoubleLeftOption => "Left Option (normal)",
        ToggleTrigger::DoubleRightOption => "Right Option (assistive)",
        ToggleTrigger::DoubleOption => "Option keys (left=format, right=assistive)",
        ToggleTrigger::None => "OFF",
    }
}

fn handle_copy_hotkeys_cheatsheet() {
    let cfg = Config::load();

    let base = base_hold_cheatsheet_label(cfg.hold_mods);
    let hands_off = hands_off_cheatsheet_label(cfg.toggle_trigger);

    let (base_raw, format_ai, talk_ai, sel_ai) = match cfg.hold_mods {
        HoldMods::CtrlAlt => {
            let raw = "Ctrl".to_string();
            let format_ai = Some("Ctrl+Option".to_string());
            let talk_ai = if cfg.hold_exclusive {
                "Ctrl (Shift/Cmd modes disabled)".to_string()
            } else {
                "Ctrl+Shift".to_string()
            };
            let sel_ai = if cfg.hold_exclusive {
                "Ctrl (Shift/Cmd modes disabled)".to_string()
            } else {
                "Ctrl+Command".to_string()
            };
            (raw, format_ai, talk_ai, sel_ai)
        }
        _ => {
            let talk_ai = if cfg.hold_exclusive {
                format!("{base} (Shift/Cmd modes disabled)")
            } else {
                format!("{base}+Shift")
            };
            let sel_ai = if cfg.hold_exclusive {
                format!("{base} (Shift/Cmd modes disabled)")
            } else {
                format!("{base}+Command")
            };
            (base.to_string(), None, talk_ai, sel_ai)
        }
    };

    let mut text = format!(
        "CodeScribe hotkeys\n\
\n\
- Hands-off (RAW): {hands_off}\n\
- Hold-to-talk (RAW): {base_raw} (hold)\n"
    );

    if let Some(format_ai) = format_ai {
        text.push_str(&format!("- Format with AI: {format_ai} (hold)\n"));
    }

    text.push_str(&format!(
        "- Talk to AI: {talk_ai} (hold)\n\
- Selected text → AI: {sel_ai} (hold)\n"
    ));

    if let Err(e) = clipboard::set_clipboard(&text) {
        info!("Failed to copy hotkeys cheatsheet: {}", e);
        #[cfg(target_os = "macos")]
        notify("CodeScribe", &format!("Copy failed: {e}"));
        return;
    }

    #[cfg(target_os = "macos")]
    notify("CodeScribe", "Copied hotkeys cheatsheet");
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

#[cfg(target_os = "macos")]
fn open_privacy_settings(deeplink: &str) {
    let url = format!(
        "x-apple.systempreferences:com.apple.preference.security?{}",
        deeplink
    );
    let _ = Command::new("open").arg(url).spawn();
}

#[cfg(target_os = "macos")]
fn handle_open_accessibility_settings() {
    send_menu_event(TrayMenuEvent::OpenAccessibilitySettings);
    open_privacy_settings("Privacy_Accessibility");
}

#[cfg(target_os = "macos")]
fn handle_open_input_monitoring_settings() {
    send_menu_event(TrayMenuEvent::OpenInputMonitoringSettings);
    open_privacy_settings("Privacy_ListenEvent");
}

#[cfg(target_os = "macos")]
fn handle_reset_input_monitoring_permission() {
    send_menu_event(TrayMenuEvent::ResetInputMonitoringPermission);

    // Reset TCC for ListenEvent (Input Monitoring) for our bundle id.
    // User still needs to re-grant in System Settings after restart.
    let _ = Command::new("tccutil")
        .args(["reset", "ListenEvent", "com.codescribe.app"])
        .spawn();

    open_privacy_settings("Privacy_ListenEvent");

    let _ = Command::new("osascript")
        .arg("-e")
        .arg(
            r#"display notification "Input Monitoring reset. Re-open CodeScribe, then enable it in Input Monitoring settings." with title "CodeScribe""#,
        )
        .spawn();
}

#[cfg(not(target_os = "macos"))]
fn handle_open_accessibility_settings() {
    send_menu_event(TrayMenuEvent::OpenAccessibilitySettings);
}

#[cfg(not(target_os = "macos"))]
fn handle_open_input_monitoring_settings() {
    send_menu_event(TrayMenuEvent::OpenInputMonitoringSettings);
}

#[cfg(not(target_os = "macos"))]
fn handle_reset_input_monitoring_permission() {
    send_menu_event(TrayMenuEvent::ResetInputMonitoringPermission);
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
// Notes Handlers
// ============================================================================

fn handle_toggle_quick_notes() {
    let config = Config::load();
    let new_state = !config.quick_notes_enabled;

    info!(
        "Toggling Quick Notes: {}",
        if new_state { "ON" } else { "OFF" }
    );

    let _ = config.save_to_env("QUICK_NOTES_ENABLED", if new_state { "1" } else { "0" });
    send_menu_event(TrayMenuEvent::SetQuickNotesEnabled(new_state));

    NOTES_MENU_ITEMS.with(|items_cell| {
        if let Some(ref items) = *items_cell.borrow() {
            items.quick_notes_toggle.set_checked(new_state);
            // If disabled, also uncheck "save-only" in the UI (config remains on disk).
            if !new_state {
                items.quick_notes_save_only.set_checked(false);
            }
        }
    });

    #[cfg(target_os = "macos")]
    {
        let _ = Command::new("osascript")
            .arg("-e")
            .arg(format!(
                r#"display notification "Quick Notes: {}" with title "CodeScribe""#,
                if new_state { "ON" } else { "OFF" }
            ))
            .spawn();
    }
}

fn handle_toggle_quick_notes_save_only() {
    let config = Config::load();
    let enabled = config.quick_notes_enabled;
    let new_state = !config.quick_notes_save_only;

    if !enabled && new_state {
        // UX: turning "save-only" ON implies Quick Notes ON.
        let _ = config.save_to_env("QUICK_NOTES_ENABLED", "1");
        send_menu_event(TrayMenuEvent::SetQuickNotesEnabled(true));
    }

    let _ = config.save_to_env("QUICK_NOTES_SAVE_ONLY", if new_state { "1" } else { "0" });
    send_menu_event(TrayMenuEvent::SetQuickNotesSaveOnly(new_state));

    NOTES_MENU_ITEMS.with(|items_cell| {
        if let Some(ref items) = *items_cell.borrow() {
            if !enabled && new_state {
                items.quick_notes_toggle.set_checked(true);
            }
            items.quick_notes_save_only.set_checked(new_state);
        }
    });
}

fn handle_open_notes_folder() {
    crate::state::notes::open_notes_folder();
}

fn handle_open_today_note() {
    crate::state::notes::open_today_note();
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
            "CodeScribe v{}\n\nSpeech-to-text for macOS\n\nCreated by M&K (c)2026 VetCoders",
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
