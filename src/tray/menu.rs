//! Main menu building logic for the tray menu
//!
//! Constructs the tray menu with nested Settings submenu:
//! - Status line (dynamic)
//! - AI Formatting toggle
//! - Copy Last to Clipboard
//! - Settings submenu (Hold Hotkeys, Recent Transcripts, Edit Config)
//! - Help/About
//! - Quit

use std::cell::RefCell;

use anyhow::Result;
use muda::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem, Submenu};

use crate::config::Config;
use crate::tray::submenus::{build_history_submenu, build_hold_hotkeys_submenu};
use crate::tray::types::MenuIds;

// Thread-local storage for menu items that need dynamic updates
thread_local! {
    pub static STATUS_MENU_ITEM: RefCell<Option<MenuItem>> = const { RefCell::new(None) };
    pub static AI_FORMATTING_ITEM: RefCell<Option<CheckMenuItem>> = const { RefCell::new(None) };
}

/// Build the tray menu with nested Settings
///
/// Menu structure:
/// ```text
/// Status: Done!
/// ─────────────
/// [✓] AI Formatting
///     Copy Last to Clipboard
/// ─────────────
/// Settings ▸
///     ├── Hold Hotkeys ▸
///     │   ├── Ctrl only
///     │   ├── Ctrl+Option
///     │   ├── Ctrl+Shift
///     │   └── Ctrl+Command
///     ├── Recent Transcripts ▸
///     │   ├── [5 entries]
///     │   └── Open Folder
///     └── Edit Config File
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

    // 2. Separator
    menu.append(&PredefinedMenuItem::separator())?;

    // 3. AI Formatting toggle (reads initial state from config)
    let ai_enabled = Config::load().ai_formatting_enabled;
    let ai_formatting_item = CheckMenuItem::new("AI Formatting", true, ai_enabled, None);
    let ai_formatting_id = ai_formatting_item.id().clone();
    menu.append(&ai_formatting_item)?;

    // Store for dynamic updates
    AI_FORMATTING_ITEM.with(|cell| {
        *cell.borrow_mut() = Some(ai_formatting_item);
    });

    // 4. Copy last to clipboard
    let copy_last_item = MenuItem::new("Copy Last to Clipboard", true, None);
    let copy_last_id = copy_last_item.id().clone();
    menu.append(&copy_last_item)?;

    // 4b. Format Last Transcript
    let format_last_item = MenuItem::new("Format Last Transcript", true, None);
    let format_last_id = format_last_item.id().clone();
    menu.append(&format_last_item)?;

    // 4c. Format Last 5 Transcripts
    let format_last_five_item = MenuItem::new("Format Last 5 Transcripts", true, None);
    let format_last_five_id = format_last_five_item.id().clone();
    menu.append(&format_last_five_item)?;

    // 5. Separator
    menu.append(&PredefinedMenuItem::separator())?;

    // 6. Settings Submenu (nested)
    let settings_menu = Submenu::new("Settings", true);

    // 6a. Hold Hotkeys submenu
    let (hold_hotkeys_menu, hold_ids) = build_hold_hotkeys_submenu()?;
    settings_menu.append(&hold_hotkeys_menu)?;

    // 6b. Recent Transcripts submenu (History)
    let (history_menu, history_save_id, history_copy_latest_id, history_open_folder_id) =
        build_history_submenu()?;
    settings_menu.append(&history_menu)?;

    // 6c. Separator before Edit Config
    settings_menu.append(&PredefinedMenuItem::separator())?;

    // 6d. Edit Config File
    let edit_config_item = MenuItem::new("Edit Config File", true, None);
    let edit_config_id = edit_config_item.id().clone();
    settings_menu.append(&edit_config_item)?;

    // 6e. Edit AI Prompt
    let edit_prompt_item = MenuItem::new("Edit AI Prompt", true, None);
    let edit_prompt_id = edit_prompt_item.id().clone();
    settings_menu.append(&edit_prompt_item)?;

    // 6f. Open Prompts Folder
    let open_prompt_folder_item = MenuItem::new("Open Prompts Folder", true, None);
    let open_prompt_folder_id = open_prompt_folder_item.id().clone();
    settings_menu.append(&open_prompt_folder_item)?;

    // 6g. Reset AI Context
    let reset_context_item = MenuItem::new("Reset AI Context", true, None);
    let reset_context_id = reset_context_item.id().clone();
    settings_menu.append(&reset_context_item)?;

    menu.append(&settings_menu)?;

    // 7. Help
    let help_item = MenuItem::new("Help", true, None);
    let help_id = help_item.id().clone();
    menu.append(&help_item)?;

    // 8. About
    let about_item = MenuItem::new("About", true, None);
    let about_id = about_item.id().clone();
    menu.append(&about_item)?;

    // 9. Separator
    menu.append(&PredefinedMenuItem::separator())?;

    // 10. Quit
    let quit_item = MenuItem::new("Quit", true, None);
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
            ai_formatting: ai_formatting_id,
            copy_last: copy_last_id,
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
            history_copy_latest: history_copy_latest_id,
            history_open_folder: history_open_folder_id,
            // Settings
            settings_edit_config: edit_config_id,
            settings_edit_prompt: edit_prompt_id,
            settings_open_prompt_folder: open_prompt_folder_id,
            settings_reset_context: reset_context_id,
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
    let new_state = AI_FORMATTING_ITEM.with(|cell| {
        if let Some(ref item) = *cell.borrow() {
            let new_state = !item.is_checked();
            item.set_checked(new_state);
            new_state
        } else {
            false
        }
    });

    // Persist to config
    let config = Config::load();
    let _ = config.save_to_env("AI_FORMATTING_ENABLED", if new_state { "1" } else { "0" });

    new_state
}
