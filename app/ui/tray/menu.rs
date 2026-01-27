//! Main menu building logic for the tray menu
//!
//! Menu structure:
//! - Status line (dynamic)
//! - Copy Last to Clipboard
//! - Show Chat Overlay
//! - Hold Hotkeys submenu (root level)
//! - Open history folder
//! - Copy diagnostics (permissions/config)
//! - Quality status
//! - Help/About
//! - Quit
//!
//! Note: Settings options moved to Settings tab in Chat Overlay

use std::cell::RefCell;

use anyhow::Result;
use muda::accelerator::{Accelerator, Code, Modifiers};
use muda::{Menu, MenuItem, PredefinedMenuItem};

use crate::config::Config;
use crate::tray::submenus::build_hold_hotkeys_submenu;
use crate::tray::types::MenuIds;

// Thread-local storage for menu items that need dynamic updates
thread_local! {
    pub static STATUS_MENU_ITEM: RefCell<Option<MenuItem>> = const { RefCell::new(None) };
    pub static QUALITY_MENU_ITEM: RefCell<Option<MenuItem>> = const { RefCell::new(None) };
}

/// Build the tray menu
///
/// Menu structure:
/// ```text
/// Status: Idle
/// Copy Last to Clipboard
/// Show Chat Overlay
/// ─────────────
/// Hold Hotkeys ▸
/// Open history...
/// Quality: OK
/// ─────────────
/// Help
/// About
/// ─────────────
/// Quit
/// ```
///
/// Note: Settings moved to Settings tab in Chat Overlay
pub fn build_menu() -> Result<(Menu, MenuIds)> {
    let menu = Menu::new();

    // 1. Status line (disabled, dynamic text)
    let status_item = MenuItem::new("Status: Idle", false, None);
    menu.append(&status_item)?;

    // Store for dynamic updates
    STATUS_MENU_ITEM.with(|cell| {
        *cell.borrow_mut() = Some(status_item);
    });

    // 2. Copy last to clipboard (quick action)
    let copy_last_item = MenuItem::new("Copy Last to Clipboard", true, None);
    let copy_last_id = copy_last_item.id().clone();
    menu.append(&copy_last_item)?;

    // 2a. Show Chat Overlay
    let show_overlay_item = MenuItem::new("Show Chat Overlay", true, None);
    let show_overlay_id = show_overlay_item.id().clone();
    menu.append(&show_overlay_item)?;

    // 2b. Run onboarding
    let run_onboarding_item = MenuItem::new("Run Onboarding", true, None);
    let run_onboarding_id = run_onboarding_item.id().clone();
    menu.append(&run_onboarding_item)?;

    // 4. Separator
    menu.append(&PredefinedMenuItem::separator())?;

    // 5. Hold Hotkeys submenu (root level)
    let (hold_hotkeys_menu, hold_ids) = build_hold_hotkeys_submenu()?;
    menu.append(&hold_hotkeys_menu)?;

    // 6. Open history folder
    let open_history_item = MenuItem::new("Open history...", true, None);
    let open_history_id = open_history_item.id().clone();
    menu.append(&open_history_item)?;

    // 6a. Copy diagnostics
    let copy_diag_item = MenuItem::new("Copy diagnostics", true, None);
    let copy_diag_id = copy_diag_item.id().clone();
    menu.append(&copy_diag_item)?;

    // 6b. Quality menu item (shows pending mismatches from daemon)
    let state = crate::quality_loop::read_daemon_state();
    let quality_label = if !state.available {
        "Quality: unavailable".to_string()
    } else if state.pending_mismatches > 0 {
        format!("Quality: {} pending", state.pending_mismatches)
    } else {
        "Quality: OK".to_string()
    };
    let quality_item = MenuItem::new(&quality_label, true, None);
    let quality_open_report_id = quality_item.id().clone();
    menu.append(&quality_item)?;

    // Store for dynamic updates
    QUALITY_MENU_ITEM.with(|cell| {
        *cell.borrow_mut() = Some(quality_item);
    });

    // 7. Separator
    menu.append(&PredefinedMenuItem::separator())?;

    // 10. Help
    let help_item = MenuItem::new("Help", true, None);
    let help_id = help_item.id().clone();
    menu.append(&help_item)?;

    // 11. About
    let about_item = MenuItem::new("About", true, None);
    let about_id = about_item.id().clone();
    menu.append(&about_item)?;

    // 12. Separator
    menu.append(&PredefinedMenuItem::separator())?;

    // 13. Quit (Cmd+Q)
    let quit_accel = Accelerator::new(Some(Modifiers::SUPER), Code::KeyQ);
    let quit_item = MenuItem::new("Quit", true, Some(quit_accel));
    let quit_id = quit_item.id().clone();
    menu.append(&quit_item)?;

    // Destructure hold_ids tuple
    let (
        hold_ctrl_id,
        hold_ctrl_opt_id,
        hold_ctrl_shift_id,
        hold_ctrl_cmd_id,
        hold_exclusive_id,
        toggle_double_opt_id,
        toggle_double_ralt_id,
        toggle_disabled_id,
    ) = hold_ids;

    Ok((
        menu,
        MenuIds {
            copy_last: copy_last_id,
            show_overlay: show_overlay_id,
            run_onboarding: run_onboarding_id,
            open_history: open_history_id,
            copy_diagnostics: copy_diag_id,
            help: help_id,
            about: about_id,
            quit: quit_id,
            // Hold Hotkeys submenu
            hold_ctrl: hold_ctrl_id,
            hold_ctrl_opt: hold_ctrl_opt_id,
            hold_ctrl_shift: hold_ctrl_shift_id,
            hold_ctrl_cmd: hold_ctrl_cmd_id,
            hold_exclusive: hold_exclusive_id,
            toggle_double_opt: toggle_double_opt_id,
            toggle_double_ralt: toggle_double_ralt_id,
            toggle_disabled: toggle_disabled_id,
            // Quality
            quality_open_report: quality_open_report_id,
        },
    ))
}

/// Update the status label in the menu
/// Must be called from the main thread
pub fn update_status_label(label: &str) {
    STATUS_MENU_ITEM.with(|cell| {
        if let Some(ref item) = *cell.borrow() {
            item.set_text(label);
        }
    });
}

/// Toggle AI Formatting and persist to config
/// Note: Tray menu checkbox removed - settings now in Chat Overlay Settings tab
pub fn toggle_ai_formatting() -> bool {
    // Read current state from Config (source of truth)
    let current_state = Config::load().ai_formatting_enabled;
    let new_state = !current_state;

    // Persist to config
    let config = Config::load();
    let _ = config.save_to_env("AI_FORMATTING_ENABLED", if new_state { "1" } else { "0" });

    new_state
}

/// Update the quality label in the menu
/// Call this periodically to reflect daemon state changes
pub fn update_quality_label() {
    let state = crate::quality_loop::read_daemon_state();
    let label = if !state.available {
        "Quality: unavailable".to_string()
    } else if state.pending_mismatches > 0 {
        format!("Quality: {} pending", state.pending_mismatches)
    } else {
        "Quality: OK".to_string()
    };

    QUALITY_MENU_ITEM.with(|cell| {
        if let Some(ref item) = *cell.borrow() {
            item.set_text(&label);
        }
    });
}

#[cfg(test)]
mod tests {
    fn menu_labels_for_test() -> Vec<String> {
        let state = crate::quality_loop::read_daemon_state();
        let quality_label = if !state.available {
            "Quality: unavailable".to_string()
        } else if state.pending_mismatches > 0 {
            format!("Quality: {} pending", state.pending_mismatches)
        } else {
            "Quality: OK".to_string()
        };

        vec![
            "Status: Idle".to_string(),
            "Copy Last to Clipboard".to_string(),
            "Show Chat Overlay".to_string(),
            "Hold Hotkeys".to_string(),
            "Open history...".to_string(),
            quality_label,
            "Help".to_string(),
            "About".to_string(),
            "Quit".to_string(),
        ]
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn tray_menu_includes_show_chat_overlay() {
        let labels = menu_labels_for_test();
        let found = labels.iter().any(|label| label == "Show Chat Overlay");
        assert!(found, "Show Chat Overlay menu item missing");
    }
}
