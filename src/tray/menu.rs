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
use muda::{Menu, MenuItem, PredefinedMenuItem};

use crate::tray::types::MenuIds;

// Thread-local storage for the status menu item (needs to be updated dynamically)
thread_local! {
    pub static STATUS_MENU_ITEM: RefCell<Option<MenuItem>> = const { RefCell::new(None) };
}

/// Build the minimal tray menu
///
/// Menu structure:
/// ```text
/// [●] Status: Idle          ← DYNAMIC (updated via update_status_label)
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

    // 3. Settings
    let settings_item = MenuItem::new("Settings...", true, None);
    let settings_id = settings_item.id().clone();
    menu.append(&settings_item)?;

    // 4. Help
    let help_item = MenuItem::new("Help", true, None);
    let help_id = help_item.id().clone();
    menu.append(&help_item)?;

    // 5. About
    let about_item = MenuItem::new("About", true, None);
    let about_id = about_item.id().clone();
    menu.append(&about_item)?;

    // 6. Separator
    menu.append(&PredefinedMenuItem::separator())?;

    // 7. Quit
    let quit_item = MenuItem::new("Quit", true, None);
    let quit_id = quit_item.id().clone();
    menu.append(&quit_item)?;

    Ok((
        menu,
        MenuIds {
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
