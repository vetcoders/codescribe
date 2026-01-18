//! Submenu building functions for the tray menu
//!
//! Each function builds a specific submenu and returns its IDs.

use anyhow::Result;
use muda::{CheckMenuItem, MenuId, MenuItem, PredefinedMenuItem, Submenu};

use crate::tray::state::{HOLD_MENU_ITEMS, TOGGLE_MENU_ITEMS};
use crate::tray::types::{HoldMenuItems, HoldMods, ToggleMenuItems};

// Type aliases
pub type HoldMenuIds = (
    MenuId,
    MenuId,
    MenuId,
    MenuId,
    MenuId,
    MenuId,
    MenuId,
    MenuId,
);
pub type HistoryMenuIds = (MenuId, MenuId, MenuId, MenuId, MenuId, MenuId);

/// Build the Hold Hotkeys submenu
pub fn build_hold_hotkeys_submenu() -> Result<(Submenu, HoldMenuIds)> {
    let hold_menu = Submenu::new("Hold Hotkeys", true);

    // Read from Config (source of truth for initial state)
    let config = crate::config::Config::load();
    let current_mods = config.hold_mods;
    let current_trigger = config.toggle_trigger;

    let hold_current_label =
        MenuItem::new(format!("Current: {}", current_mods.label()), false, None);
    hold_menu.append(&hold_current_label)?;
    hold_menu.append(&PredefinedMenuItem::separator())?;

    let hold_ctrl = CheckMenuItem::new(
        format!("Hold: {}", HoldMods::Ctrl.label()),
        true,
        current_mods == HoldMods::Ctrl,
        None,
    );
    let hold_ctrl_id = hold_ctrl.id().clone();
    let hold_ctrl_opt = CheckMenuItem::new(
        format!("Hold: {}", HoldMods::CtrlAlt.label()),
        true,
        current_mods == HoldMods::CtrlAlt,
        None,
    );
    let hold_ctrl_opt_id = hold_ctrl_opt.id().clone();
    let hold_ctrl_shift = CheckMenuItem::new(
        format!("Hold: {}", HoldMods::CtrlShift.label()),
        true,
        current_mods == HoldMods::CtrlShift,
        None,
    );
    let hold_ctrl_shift_id = hold_ctrl_shift.id().clone();
    let hold_ctrl_cmd = CheckMenuItem::new(
        format!("Hold: {}", HoldMods::CtrlCmd.label()),
        true,
        current_mods == HoldMods::CtrlCmd,
        None,
    );
    let hold_ctrl_cmd_id = hold_ctrl_cmd.id().clone();

    hold_menu.append(&hold_ctrl)?;
    hold_menu.append(&hold_ctrl_opt)?;
    hold_menu.append(&hold_ctrl_shift)?;
    hold_menu.append(&hold_ctrl_cmd)?;
    hold_menu.append(&PredefinedMenuItem::separator())?;

    let hold_exclusive = CheckMenuItem::new(
        "Exclusive (ignore extra modifiers)",
        true,
        config.hold_exclusive,
        None,
    );
    let hold_exclusive_id = hold_exclusive.id().clone();
    hold_menu.append(&hold_exclusive)?;
    hold_menu.append(&PredefinedMenuItem::separator())?;

    let toggle_label = MenuItem::new(format!("Toggle: {}", current_trigger.label()), false, None);
    hold_menu.append(&toggle_label)?;
    let toggle_double_opt = CheckMenuItem::new(
        "Enable left Option (normal) + right Option (assistive)",
        true,
        current_trigger == crate::config::ToggleTrigger::DoubleOption,
        None,
    );
    let toggle_double_opt_id = toggle_double_opt.id().clone();
    let toggle_double_ralt = CheckMenuItem::new(
        "Enable right Option (assistive only)",
        true,
        current_trigger == crate::config::ToggleTrigger::DoubleRightOption,
        None,
    );
    let toggle_double_ralt_id = toggle_double_ralt.id().clone();
    let toggle_disabled = CheckMenuItem::new(
        "Disable toggles",
        true,
        current_trigger == crate::config::ToggleTrigger::None,
        None,
    );
    let toggle_disabled_id = toggle_disabled.id().clone();

    hold_menu.append(&toggle_double_opt)?;
    hold_menu.append(&toggle_double_ralt)?;
    hold_menu.append(&toggle_disabled)?;

    HOLD_MENU_ITEMS.with(|items_cell| {
        *items_cell.borrow_mut() = Some(HoldMenuItems {
            ctrl: hold_ctrl,
            ctrl_opt: hold_ctrl_opt,
            ctrl_shift: hold_ctrl_shift,
            ctrl_cmd: hold_ctrl_cmd,
            label: hold_current_label,
        });
    });

    TOGGLE_MENU_ITEMS.with(|items_cell| {
        *items_cell.borrow_mut() = Some(ToggleMenuItems {
            double_opt: toggle_double_opt,
            double_ralt: toggle_double_ralt,
            disabled: toggle_disabled,
            label: toggle_label,
        });
    });

    Ok((
        hold_menu,
        (
            hold_ctrl_id,
            hold_ctrl_opt_id,
            hold_ctrl_shift_id,
            hold_ctrl_cmd_id,
            hold_exclusive_id,
            toggle_double_opt_id,
            toggle_double_ralt_id,
            toggle_disabled_id,
        ),
    ))
}

/// Build the History submenu
/// Returns: (Submenu, (format_last, format_last_5, save_history, keep_audio, copy_latest, open_folder))
pub fn build_history_submenu() -> Result<(Submenu, HistoryMenuIds)> {
    let history_menu = Submenu::new("History", true);

    // Format actions at the top
    let format_last = MenuItem::new("Format Last Transcript", true, None);
    let format_last_id = format_last.id().clone();
    history_menu.append(&format_last)?;

    let format_last_5 = MenuItem::new("Format Last 5 Transcripts", true, None);
    let format_last_5_id = format_last_5.id().clone();
    history_menu.append(&format_last_5)?;

    history_menu.append(&PredefinedMenuItem::separator())?;

    // Save toggles (paired: history + audio)
    let config = crate::config::Config::load();

    let history_save = CheckMenuItem::new("Save to History", true, config.history_enabled, None);
    let history_save_id = history_save.id().clone();
    history_menu.append(&history_save)?;

    let keep_audio = CheckMenuItem::new("Keep Audio", true, config.dump_audio_logs, None);
    let keep_audio_id = keep_audio.id().clone();
    history_menu.append(&keep_audio)?;

    history_menu.append(&PredefinedMenuItem::separator())?;

    // Copy and Open actions
    let history_copy_latest = MenuItem::new("Copy Latest", true, None);
    let history_copy_latest_id = history_copy_latest.id().clone();
    history_menu.append(&history_copy_latest)?;

    let history_open_folder = MenuItem::new("Open Folder", true, None);
    let history_open_folder_id = history_open_folder.id().clone();
    history_menu.append(&history_open_folder)?;

    Ok((
        history_menu,
        (
            format_last_id,
            format_last_5_id,
            history_save_id,
            keep_audio_id,
            history_copy_latest_id,
            history_open_folder_id,
        ),
    ))
}
