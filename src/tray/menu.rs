//! Main menu building logic for the tray menu
//!
//! Constructs a minimal tray menu with only essential items:
//! - Status line (dynamic)
//! - Settings (opens config in editor)
//! - Help (opens docs)
//! - About (shows version)
//! - Quit

use std::cell::RefCell;

use anyhow::Result;
use muda::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem};

use crate::config::Config;
use crate::tray::types::MenuIds;

// Thread-local storage for menu items that need dynamic updates
thread_local! {
    pub static STATUS_MENU_ITEM: RefCell<Option<MenuItem>> = const { RefCell::new(None) };
    pub static AI_FORMATTING_ITEM: RefCell<Option<CheckMenuItem>> = const { RefCell::new(None) };
}

/// Build the minimal tray menu
///
/// Menu structure:
/// ```text
/// [●] Status: Idle          ← DYNAMIC (updated via update_status_label)
/// ─────────────────
/// [✓] AI Formatting         ← CheckMenuItem (Shift+Ctrl triggers formatted mode)
/// ─────────────────
/// ⚙ Settings...             → Opens ~/.codescribe/.env in editor
/// ? Help                    → Opens docs/README in browser
/// ⓘ About                   → Shows version dialog
/// ─────────────────
/// ⏻ Quit
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

    // 5. Separator
    menu.append(&PredefinedMenuItem::separator())?;

    // 6. Settings
    let settings_item = MenuItem::new("Settings...", true, None);
    let settings_id = settings_item.id().clone();
    menu.append(&settings_item)?;

    // 6. Help
    let help_item = MenuItem::new("Help", true, None);
    let help_id = help_item.id().clone();
    menu.append(&help_item)?;

    // 7. About
    let about_item = MenuItem::new("About", true, None);
    let about_id = about_item.id().clone();
    menu.append(&about_item)?;

    // 8. Separator
    menu.append(&PredefinedMenuItem::separator())?;

    // 9. Quit
    let quit_item = MenuItem::new("Quit", true, None);
    let quit_id = quit_item.id().clone();
    menu.append(&quit_item)?;

    Ok((
        menu,
        MenuIds {
            ai_formatting: ai_formatting_id,
            copy_last: copy_last_id,
            settings: settings_id,
            help: help_id,
            about: about_id,
            quit: quit_id,
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
