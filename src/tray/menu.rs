//! Main menu building logic for the tray menu
//!
//! Menu structure:
//! - Status line (dynamic)
//! - Copy Last to Clipboard
//! - Hold Hotkeys submenu (root level)
//! - History submenu (with Format Last, Format Last 5)
//! - Settings submenu (flat: AI Formatting + config items)
//! - Help/About
//! - Quit

use std::cell::RefCell;

use anyhow::Result;
use muda::accelerator::{Accelerator, Code, Modifiers};
use muda::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem, Submenu};

use crate::config::Config;
use crate::tray::submenus::{build_history_submenu, build_hold_hotkeys_submenu};
use crate::tray::types::MenuIds;

// Thread-local storage for menu items that need dynamic updates
thread_local! {
    pub static STATUS_MENU_ITEM: RefCell<Option<MenuItem>> = const { RefCell::new(None) };
    pub static AI_FORMATTING_ITEM: RefCell<Option<CheckMenuItem>> = const { RefCell::new(None) };
    pub static QUALITY_MENU_ITEM: RefCell<Option<MenuItem>> = const { RefCell::new(None) };
}

/// Build the tray menu
///
/// Menu structure:
/// ```text
/// Status: Idle
/// Open GUI...              ← Opens Tauri window
/// ─────────────
/// Copy Last to Clipboard
/// ─────────────
/// Hold Hotkeys ▸
/// History ▸
/// ─────────────
/// Settings ▸
///     ├── [✓] AI Formatting
///     ├── Edit Config File
///     ├── Edit AI Prompt
///     ├── Open Prompts Folder
///     └── Reset AI Context
/// ─────────────
/// Help
/// About
/// ─────────────
/// Quit
/// ```
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

    // 4. Separator
    menu.append(&PredefinedMenuItem::separator())?;

    // 5. Hold Hotkeys submenu (root level)
    let (hold_hotkeys_menu, hold_ids) = build_hold_hotkeys_submenu()?;
    menu.append(&hold_hotkeys_menu)?;

    // 6. History submenu (with Format Last actions)
    let (history_menu, history_ids) = build_history_submenu()?;
    menu.append(&history_menu)?;

    // 6b. Quality menu item (shows pending mismatches from daemon)
    let pending = crate::quality_loop::get_pending_mismatches();
    let quality_label = if pending > 0 {
        format!("Quality: {} pending", pending)
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

    // 8. Settings Submenu (flat structure)
    let settings_menu = Submenu::new("Settings", true);

    // 8a. AI Formatting toggle
    let ai_enabled = Config::load().ai_formatting_enabled;
    let ai_formatting_item = CheckMenuItem::new("AI Formatting", true, ai_enabled, None);
    let ai_formatting_id = ai_formatting_item.id().clone();
    settings_menu.append(&ai_formatting_item)?;

    // Store for dynamic updates
    AI_FORMATTING_ITEM.with(|cell| {
        *cell.borrow_mut() = Some(ai_formatting_item);
    });

    settings_menu.append(&PredefinedMenuItem::separator())?;

    // 8b. Edit Config File
    let edit_config_item = MenuItem::new("Edit Config File", true, None);
    let edit_config_id = edit_config_item.id().clone();
    settings_menu.append(&edit_config_item)?;

    // 8c. Edit AI Prompt
    let edit_prompt_item = MenuItem::new("Edit AI Prompt", true, None);
    let edit_prompt_id = edit_prompt_item.id().clone();
    settings_menu.append(&edit_prompt_item)?;

    // 8d. Open Prompts Folder
    let open_prompt_folder_item = MenuItem::new("Open Prompts Folder", true, None);
    let open_prompt_folder_id = open_prompt_folder_item.id().clone();
    settings_menu.append(&open_prompt_folder_item)?;

    // 8e. Reset AI Context
    let reset_context_item = MenuItem::new("Reset AI Context", true, None);
    let reset_context_id = reset_context_item.id().clone();
    settings_menu.append(&reset_context_item)?;

    menu.append(&settings_menu)?;

    // 9. Separator
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

    // Destructure history_ids
    let (
        format_last_id,
        format_last_five_id,
        history_save_id,
        keep_audio_id,
        history_copy_latest_id,
        history_open_folder_id,
    ) = history_ids;

    Ok((
        menu,
        MenuIds {
            ai_formatting: ai_formatting_id,
            copy_last: copy_last_id,
            show_overlay: show_overlay_id,
            format_last: format_last_id,
            format_last_five: format_last_five_id,
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
            // History submenu
            history_save: history_save_id,
            keep_audio: keep_audio_id,
            history_copy_latest: history_copy_latest_id,
            history_open_folder: history_open_folder_id,
            // Settings
            settings_edit_config: edit_config_id,
            settings_edit_prompt: edit_prompt_id,
            settings_open_prompt_folder: open_prompt_folder_id,
            settings_reset_context: reset_context_id,
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
pub fn toggle_ai_formatting() -> bool {
    // Read current state from Config (source of truth - is_checked() unreliable on macOS)
    let current_state = Config::load().ai_formatting_enabled;
    let new_state = !current_state;

    // Update checkbox visual
    AI_FORMATTING_ITEM.with(|cell| {
        if let Some(ref item) = *cell.borrow() {
            item.set_checked(new_state);
        }
    });

    // Persist to config
    let config = Config::load();
    let _ = config.save_to_env("AI_FORMATTING_ENABLED", if new_state { "1" } else { "0" });

    new_state
}

/// Update the quality label in the menu
/// Call this periodically to reflect daemon state changes
pub fn update_quality_label() {
    let pending = crate::quality_loop::get_pending_mismatches();
    let label = if pending > 0 {
        format!("Quality: {} pending", pending)
    } else {
        "Quality: OK".to_string()
    };

    QUALITY_MENU_ITEM.with(|cell| {
        if let Some(ref item) = *cell.borrow() {
            item.set_text(&label);
        }
    });
}
