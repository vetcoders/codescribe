//! Menu action handlers for tray menu events
//!
//! Handles menu item clicks and dispatches appropriate events.

use muda::MenuId;
use std::process::Command;
use tracing::{debug, info};

use crate::config::{Config, HoldMods, ToggleTrigger};
use crate::os::clipboard;
use crate::os::hotkeys;
use crate::os::notifications;
use crate::os::permissions;
use crate::tray::state::{HOTKEYS_MENU_ITEMS, NOTES_MENU_ITEMS, send_menu_event};
use crate::tray::types::{MenuIds, TrayMenuEvent, VadPreset};

#[cfg(target_os = "macos")]
fn notify(title: &str, message: &str) {
    notifications::notify(title, message);
}

fn hotkeys_state_summary() -> String {
    let hold = std::env::var("HOLD_MODS").unwrap_or_else(|_| "<unset>".to_string());
    let toggle = std::env::var("TOGGLE_TRIGGER").unwrap_or_else(|_| "<unset>".to_string());
    format!("HOLD_MODS={hold}, TOGGLE_TRIGGER={toggle}")
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
    else if event_id == &menu_ids.hotkeys_toggle_assistive {
        handle_toggle_assistive_toggle();
    } else if event_id == &menu_ids.hotkeys_toggle_dictation {
        handle_toggle_dictation_toggle();
    } else if event_id == &menu_ids.hotkeys_reset {
        handle_reset_hotkeys();
    } else if event_id == &menu_ids.hotkeys_copy_cheatsheet {
        handle_copy_hotkeys_cheatsheet();
    } else if event_id == &menu_ids.hotkeys_hold_ctrl {
        handle_set_hold_mods(HoldMods::Ctrl);
    } else if event_id == &menu_ids.hotkeys_hold_ctrl_alt {
        handle_set_hold_mods(HoldMods::CtrlAlt);
    } else if event_id == &menu_ids.hotkeys_hold_ctrl_shift {
        handle_set_hold_mods(HoldMods::CtrlShift);
    } else if event_id == &menu_ids.hotkeys_hold_ctrl_cmd {
        handle_set_hold_mods(HoldMods::CtrlCmd);
    }
    // Quality - Open Report
    else if event_id == &menu_ids.quality_open_report {
        handle_open_quality_report();
    } else if event_id == &menu_ids.silero_vad_install {
        handle_install_silero_vad();
    } else if event_id == &menu_ids.vad_preset_sensitive {
        send_menu_event(TrayMenuEvent::SetVadPreset(VadPreset::Sensitive));
    } else if event_id == &menu_ids.vad_preset_balanced {
        send_menu_event(TrayMenuEvent::SetVadPreset(VadPreset::Balanced));
    } else if event_id == &menu_ids.vad_preset_conservative {
        send_menu_event(TrayMenuEvent::SetVadPreset(VadPreset::Conservative));
    } else {
        debug!("Unknown menu event id: {:?}", event_id);
    }
}

fn base_hold_cheatsheet_label(hold_mods: HoldMods) -> &'static str {
    match hold_mods {
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

    let text = format!(
        "CodeScribe hotkeys\n\
\n\
- Hands-off (RAW): {hands_off}\n\
- Hold-to-talk (RAW): {base} (hold)\n\
- Talk to AI: {talk_ai} (hold)\n\
- Selected text → AI: {sel_ai} (hold)\n"
    );

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

fn update_hold_menu(items: &crate::tray::types::HotkeysMenuItems, mods: HoldMods) {
    let label = mods.label();
    items.hold_summary.set_text(format!(
        "Hold {label}: RAW | {label}+Shift: Chat | {label}+Cmd: Selection"
    ));
    items.hold_ctrl.set_checked(mods == HoldMods::Ctrl);
    items.hold_ctrl_alt.set_checked(mods == HoldMods::CtrlAlt);
    items
        .hold_ctrl_shift
        .set_checked(mods == HoldMods::CtrlShift);
    items.hold_ctrl_cmd.set_checked(mods == HoldMods::CtrlCmd);
}

fn update_toggle_menu(items: &crate::tray::types::HotkeysMenuItems, trigger: ToggleTrigger) {
    items
        .toggle_dictation
        .set_checked(trigger == ToggleTrigger::DoubleCtrl);
    items.toggle_assistive.set_checked(matches!(
        trigger,
        ToggleTrigger::DoubleOption | ToggleTrigger::DoubleRightOption
    ));
    items.toggle_label.set_text(format!(
        "Hands-off toggle: {}",
        match trigger {
            ToggleTrigger::None => "OFF",
            ToggleTrigger::DoubleCtrl => "Double Ctrl (RAW)",
            ToggleTrigger::DoubleLeftOption => "Left Option (normal)",
            ToggleTrigger::DoubleRightOption => "Right Option (assistive)",
            ToggleTrigger::DoubleOption => "Option keys (left=format, right=assistive)",
        }
    ));
}

/// Set base hold modifiers (radio behavior) and keep config consistent.
fn handle_set_hold_mods(new_mods: HoldMods) {
    let config = Config::load();

    if let Err(e) = config.save_to_env("HOLD_MODS", new_mods.as_str()) {
        #[cfg(target_os = "macos")]
        notify("CodeScribe", &format!("Failed to save HOLD_MODS: {e}"));
        return;
    }
    hotkeys::set_hold_mods(new_mods);
    send_menu_event(TrayMenuEvent::SetHoldMods(new_mods));

    // If user switches to Ctrl-only hold while DoubleCtrl toggle is enabled,
    // we must disable the toggle (those two conflict by design).
    let current_trigger = config.toggle_trigger;
    if new_mods == HoldMods::Ctrl && current_trigger == ToggleTrigger::DoubleCtrl {
        if let Err(e) = config.save_to_env("TOGGLE_TRIGGER", ToggleTrigger::None.as_str()) {
            #[cfg(target_os = "macos")]
            notify("CodeScribe", &format!("Failed to save TOGGLE_TRIGGER: {e}"));
            return;
        }
        hotkeys::set_toggle_trigger(ToggleTrigger::None);
        send_menu_event(TrayMenuEvent::SetToggleTrigger(ToggleTrigger::None));
    }

    HOTKEYS_MENU_ITEMS.with(|items_cell| {
        if let Some(ref items) = *items_cell.borrow() {
            update_hold_menu(items, new_mods);
            if new_mods == HoldMods::Ctrl && current_trigger == ToggleTrigger::DoubleCtrl {
                update_toggle_menu(items, ToggleTrigger::None);
            }
        }
    });

    #[cfg(target_os = "macos")]
    notify(
        "CodeScribe",
        &format!("Hotkeys updated ({})", hotkeys_state_summary()),
    );
}

// ============================================================================
// Hotkeys Handlers
// ============================================================================

/// Toggle the assistive "right Option" trigger on/off.
fn handle_toggle_assistive_toggle() {
    let config = Config::load();
    let currently_enabled = matches!(
        config.toggle_trigger,
        ToggleTrigger::DoubleOption | ToggleTrigger::DoubleRightOption
    );
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
            items.toggle_dictation.set_checked(false);
            items.toggle_label.set_text(format!(
                "Hands-off toggle: {}",
                match new_trigger {
                    ToggleTrigger::None => "OFF",
                    ToggleTrigger::DoubleCtrl => "Double Ctrl (RAW)",
                    ToggleTrigger::DoubleLeftOption => "Left Option (normal)",
                    ToggleTrigger::DoubleRightOption => "Right Option (assistive)",
                    ToggleTrigger::DoubleOption => "Option keys (left=format, right=assistive)",
                }
            ));
        }
    });
}

/// Toggle the RAW hands-off "double Ctrl" trigger on/off.
fn handle_toggle_dictation_toggle() {
    let config = Config::load();
    let currently_enabled = matches!(config.toggle_trigger, ToggleTrigger::DoubleCtrl);
    let new_trigger = if currently_enabled {
        ToggleTrigger::None
    } else {
        ToggleTrigger::DoubleCtrl
    };

    info!(
        "Toggling RAW double-Ctrl trigger: {} -> {:?}",
        if currently_enabled { "ON" } else { "OFF" },
        new_trigger
    );

    if let Err(e) = config.save_to_env("TOGGLE_TRIGGER", new_trigger.as_str()) {
        #[cfg(target_os = "macos")]
        notify("CodeScribe", &format!("Failed to save TOGGLE_TRIGGER: {e}"));
        return;
    }
    hotkeys::set_toggle_trigger(new_trigger);
    send_menu_event(TrayMenuEvent::SetToggleTrigger(new_trigger));

    // If enabling DoubleCtrl and hold is Ctrl-only, switch hold to Ctrl+Option automatically
    // (otherwise hold-to-talk is disabled to avoid Ctrl+shortcut conflicts).
    let mut hold_mods_for_ui = config.hold_mods;
    if new_trigger == ToggleTrigger::DoubleCtrl && config.hold_mods == HoldMods::Ctrl {
        hold_mods_for_ui = HoldMods::CtrlAlt;
        if let Err(e) = config.save_to_env("HOLD_MODS", hold_mods_for_ui.as_str()) {
            #[cfg(target_os = "macos")]
            notify("CodeScribe", &format!("Failed to save HOLD_MODS: {e}"));
            return;
        }
        let _ = config.save_to_env("HOLD_EXCLUSIVE", "0");
        hotkeys::set_hold_mods(hold_mods_for_ui);
        hotkeys::set_exclusive_mode(false);
        send_menu_event(TrayMenuEvent::SetHoldMods(hold_mods_for_ui));
    }

    HOTKEYS_MENU_ITEMS.with(|items_cell| {
        if let Some(ref items) = *items_cell.borrow() {
            update_toggle_menu(items, new_trigger);
            if new_trigger == ToggleTrigger::DoubleCtrl {
                items.toggle_assistive.set_checked(false);
            }
            update_hold_menu(items, hold_mods_for_ui);
        }
    });

    #[cfg(target_os = "macos")]
    notify(
        "CodeScribe",
        &format!("Hotkeys updated ({})", hotkeys_state_summary()),
    );
}

/// Reset hotkeys to a safe, recommended default.
fn handle_reset_hotkeys() {
    info!("Resetting hotkeys to recommended defaults");

    let config = Config::load();

    // Ensure Shift/Cmd mode layer is enabled.
    if let Err(e) = config.save_to_env("HOLD_EXCLUSIVE", "0") {
        #[cfg(target_os = "macos")]
        notify("CodeScribe", &format!("Failed to save HOLD_EXCLUSIVE: {e}"));
        return;
    }
    // Recommended: Hold Ctrl+Option for RAW (doesn't break Ctrl+shortcuts).
    if let Err(e) = config.save_to_env("HOLD_MODS", HoldMods::CtrlAlt.as_str()) {
        #[cfg(target_os = "macos")]
        notify("CodeScribe", &format!("Failed to save HOLD_MODS: {e}"));
        return;
    }
    // Recommended: enable hands-off RAW toggle on double Ctrl.
    if let Err(e) = config.save_to_env("TOGGLE_TRIGGER", ToggleTrigger::DoubleCtrl.as_str()) {
        #[cfg(target_os = "macos")]
        notify("CodeScribe", &format!("Failed to save TOGGLE_TRIGGER: {e}"));
        return;
    }

    hotkeys::set_exclusive_mode(false);
    hotkeys::set_hold_mods(HoldMods::CtrlAlt);
    hotkeys::set_toggle_trigger(ToggleTrigger::DoubleCtrl);

    send_menu_event(TrayMenuEvent::SetHoldMods(HoldMods::CtrlAlt));
    send_menu_event(TrayMenuEvent::SetToggleTrigger(ToggleTrigger::DoubleCtrl));
    send_menu_event(TrayMenuEvent::ResetShortcuts);

    HOTKEYS_MENU_ITEMS.with(|items_cell| {
        if let Some(ref items) = *items_cell.borrow() {
            update_hold_menu(items, HoldMods::CtrlAlt);
            update_toggle_menu(items, ToggleTrigger::DoubleCtrl);
            items.toggle_assistive.set_checked(false);
        }
    });

    #[cfg(target_os = "macos")]
    notify(
        "CodeScribe",
        "Applied recommended hotkeys: Hold Ctrl+Option, Double Ctrl hands-off",
    );
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
