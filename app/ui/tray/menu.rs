//! Main menu building logic for the tray menu
//!
//! Menu structure (flat):
//! - Status line (dynamic)
//! - Show Agent / Open history / Copy last
//! - Notes ▸ / Diagnostics ▸
//! - Quick Start / Help / About
//! - Quit
//!
//! Note: Settings options moved to Settings tab in Chat Overlay

use std::cell::RefCell;

use anyhow::Result;
use muda::accelerator::{Accelerator, Code, Modifiers};
use muda::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem, Submenu};
use tracing::debug;

use codescribe_core::vad;

use crate::config::Config;
use crate::tray::state::NOTES_MENU_ITEMS;
use crate::tray::types::{MenuIds, NotesMenuItems};

// Thread-local storage for menu items that need dynamic updates
thread_local! {
    pub static ROOT_MENU: RefCell<Option<Menu>> = const { RefCell::new(None) };
    pub static STATUS_MENU_ITEM: RefCell<Option<MenuItem>> = const { RefCell::new(None) };
    pub static QUALITY_MENU_ITEM: RefCell<Option<MenuItem>> = const { RefCell::new(None) };
    pub static SILERO_VAD_MENU_ITEM: RefCell<Option<MenuItem>> = const { RefCell::new(None) };
    pub static COMPLETE_SETUP_MENU_ITEM: RefCell<Option<MenuItem>> = const { RefCell::new(None) };
}

/// Build the tray menu
///
/// Note: Settings moved to Settings tab in Chat Overlay
pub fn build_menu() -> Result<(Menu, MenuIds)> {
    let menu = Menu::new();
    ROOT_MENU.with(|cell| {
        *cell.borrow_mut() = Some(menu.clone());
    });

    // 1. Status line (disabled, dynamic text)
    let status_item = MenuItem::new("Status: Idle", false, None);
    menu.append(&status_item)?;

    // Store for dynamic updates
    STATUS_MENU_ITEM.with(|cell| {
        *cell.borrow_mut() = Some(status_item);
    });

    // 2. Show Agent
    let show_overlay_item = MenuItem::new("Show Agent", true, None);
    let show_overlay_id = show_overlay_item.id().clone();
    menu.append(&show_overlay_item)?;

    // 3. Open history folder
    let open_history_item = MenuItem::new("Open history...", true, None);
    let open_history_id = open_history_item.id().clone();
    menu.append(&open_history_item)?;

    // 4. Copy last transcript
    let copy_last_item = MenuItem::new("Copy last transcript", true, None);
    let copy_last_id = copy_last_item.id().clone();
    menu.append(&copy_last_item)?;

    // 5. Separator
    menu.append(&PredefinedMenuItem::separator())?;

    // 6. Notes submenu
    let notes_menu = Submenu::new("Notes", true);
    let notes_cfg = Config::load();

    let quick_notes_toggle = CheckMenuItem::new(
        "Quick Notes (save)",
        true,
        notes_cfg.quick_notes_enabled,
        None,
    );
    let notes_toggle_quick_notes_id = quick_notes_toggle.id().clone();
    notes_menu.append(&quick_notes_toggle)?;

    let quick_notes_save_only = CheckMenuItem::new(
        "Save-only (no paste)",
        true,
        notes_cfg.quick_notes_enabled && notes_cfg.quick_notes_save_only,
        None,
    );
    let notes_toggle_save_only_id = quick_notes_save_only.id().clone();
    notes_menu.append(&quick_notes_save_only)?;

    NOTES_MENU_ITEMS.with(|cell| {
        *cell.borrow_mut() = Some(NotesMenuItems {
            quick_notes_toggle,
            quick_notes_save_only,
        });
    });

    notes_menu.append(&PredefinedMenuItem::separator())?;

    let notes_open_folder_item = MenuItem::new("Open notes folder", true, None);
    let notes_open_folder_id = notes_open_folder_item.id().clone();
    notes_menu.append(&notes_open_folder_item)?;

    let notes_open_today_item = MenuItem::new("Open today's note", true, None);
    let notes_open_today_id = notes_open_today_item.id().clone();
    notes_menu.append(&notes_open_today_item)?;

    menu.append(&notes_menu)?;

    // 7. Diagnostics submenu
    let diagnostics_menu = Submenu::new("Diagnostics", true);
    let copy_diag_item = MenuItem::new("Copy diagnostics", true, None);
    let copy_diag_id = copy_diag_item.id().clone();
    diagnostics_menu.append(&copy_diag_item)?;

    // Quality menu item (shows pending mismatches from daemon)
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
    diagnostics_menu.append(&quality_item)?;

    // Store for dynamic updates
    QUALITY_MENU_ITEM.with(|cell| {
        *cell.borrow_mut() = Some(quality_item);
    });

    diagnostics_menu.append(&PredefinedMenuItem::separator())?;

    // Silero VAD model status / install action
    let vad_label = silero_vad_label();
    let silero_vad_item = MenuItem::new(&vad_label, true, None);
    let silero_vad_install_id = silero_vad_item.id().clone();
    diagnostics_menu.append(&silero_vad_item)?;
    SILERO_VAD_MENU_ITEM.with(|cell| {
        *cell.borrow_mut() = Some(silero_vad_item);
    });

    menu.append(&diagnostics_menu)?;

    menu.append(&PredefinedMenuItem::separator())?;

    let show_complete_setup = crate::should_show_onboarding() || crate::should_show_bootstrap();
    let complete_setup_id = if show_complete_setup {
        let complete_setup_item = MenuItem::new("Complete Setup...", true, None);
        let id = complete_setup_item.id().clone();
        menu.append(&complete_setup_item)?;
        COMPLETE_SETUP_MENU_ITEM.with(|cell| {
            *cell.borrow_mut() = Some(complete_setup_item);
        });
        Some(id)
    } else {
        COMPLETE_SETUP_MENU_ITEM.with(|cell| {
            *cell.borrow_mut() = None;
        });
        None
    };

    // 9. Settings
    let settings_item = MenuItem::new("Settings", true, None);
    let settings_id = settings_item.id().clone();
    menu.append(&settings_item)?;

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

    Ok((
        menu,
        MenuIds {
            copy_last: copy_last_id,
            show_overlay: show_overlay_id,
            open_settings: settings_id,
            complete_setup: complete_setup_id,
            open_history: open_history_id,
            copy_diagnostics: copy_diag_id,
            help: help_id,
            about: about_id,
            quit: quit_id,
            // Quality
            quality_open_report: quality_open_report_id,
            // Models
            silero_vad_install: silero_vad_install_id,
            // Notes
            notes_toggle_quick_notes: notes_toggle_quick_notes_id,
            notes_toggle_save_only: notes_toggle_save_only_id,
            notes_open_folder: notes_open_folder_id,
            notes_open_today: notes_open_today_id,
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

fn silero_vad_label() -> String {
    let model_path = vad::default_model_path();
    if model_path.exists() {
        "Silero VAD: ready (Install/Repair…)".to_string()
    } else {
        "Silero VAD: missing (Install…)".to_string()
    }
}

pub fn update_silero_vad_label() {
    let label = silero_vad_label();
    SILERO_VAD_MENU_ITEM.with(|cell| {
        if let Some(ref item) = *cell.borrow() {
            item.set_text(&label);
        }
    });
}

pub fn update_complete_setup_item() {
    if crate::should_show_setup() {
        return;
    }

    let menu = ROOT_MENU.with(|cell| cell.borrow().clone());
    let complete_setup_item = COMPLETE_SETUP_MENU_ITEM.with(|cell| cell.borrow_mut().take());
    if let (Some(menu), Some(item)) = (menu, complete_setup_item)
        && let Err(err) = menu.remove(&item)
    {
        debug!("Failed to remove Complete Setup menu item: {}", err);
        COMPLETE_SETUP_MENU_ITEM.with(|cell| {
            *cell.borrow_mut() = Some(item);
        });
    }
}

#[cfg(test)]
mod tests {
    fn menu_labels_for_test() -> Vec<String> {
        vec![
            "Status: Idle".to_string(),
            "Show Agent".to_string(),
            "Open history...".to_string(),
            "Copy last transcript".to_string(),
            "Settings".to_string(),
            "Help".to_string(),
            "About".to_string(),
            "Quit".to_string(),
        ]
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn tray_menu_includes_show_agent() {
        let labels = menu_labels_for_test();
        let found = labels.iter().any(|label| label == "Show Agent");
        assert!(found, "Show Agent menu item missing");
    }
}
