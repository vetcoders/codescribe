//! Submenu building functions for the tray menu
//!
//! Each function builds a specific submenu and returns its IDs.

use anyhow::Result;
use muda::{CheckMenuItem, MenuId, MenuItem, PredefinedMenuItem, Submenu};

use crate::tray::state::HOTKEYS_MENU_ITEMS;
use crate::tray::types::HotkeysMenuItems;

// Type aliases
pub struct HotkeysMenuIds {
    pub copy_cheatsheet: MenuId,
    pub hold_ctrl: MenuId,
    pub hold_ctrl_alt: MenuId,
    pub hold_ctrl_shift: MenuId,
    pub hold_ctrl_cmd: MenuId,
    pub toggle_assistive: MenuId,
    pub toggle_dictation: MenuId,
    pub reset: MenuId,
}

/// Build the Hold Hotkeys submenu
pub fn build_hold_hotkeys_submenu() -> Result<(Submenu, HotkeysMenuIds)> {
    let hold_menu = Submenu::new("Hotkeys", true);

    // Read from Config (source of truth for initial state)
    let config = crate::config::Config::load();
    let current_trigger = config.toggle_trigger;
    let assistive_toggle_enabled = matches!(
        current_trigger,
        crate::config::ToggleTrigger::DoubleOption
            | crate::config::ToggleTrigger::DoubleRightOption
    );
    let dictation_toggle_enabled =
        matches!(current_trigger, crate::config::ToggleTrigger::DoubleCtrl);

    let hold_label = config.hold_mods.label();
    let hold_summary = MenuItem::new(
        format!("Hold {hold_label}: RAW | {hold_label}+Shift: Chat | {hold_label}+Cmd: Selection",),
        false,
        None,
    );
    hold_menu.append(&hold_summary)?;

    let copy_cheatsheet_item = MenuItem::new("Copy hotkeys cheatsheet", true, None);
    let copy_cheatsheet_id = copy_cheatsheet_item.id().clone();
    hold_menu.append(&copy_cheatsheet_item)?;

    let reset_item = MenuItem::new(
        "Apply recommended preset (Ctrl+Option + Double Ctrl)",
        true,
        None,
    );
    let reset_id = reset_item.id().clone();
    hold_menu.append(&reset_item)?;
    hold_menu.append(&PredefinedMenuItem::separator())?;

    let hold_key_label = MenuItem::new("Hold key:", false, None);
    hold_menu.append(&hold_key_label)?;

    let hold_ctrl = CheckMenuItem::new(
        "Use Ctrl (not recommended with Double Ctrl toggle)",
        true,
        config.hold_mods == crate::config::HoldMods::Ctrl,
        None,
    );
    let hold_ctrl_id = hold_ctrl.id().clone();
    hold_menu.append(&hold_ctrl)?;

    let hold_ctrl_alt = CheckMenuItem::new(
        "Use Ctrl+Option (recommended)",
        true,
        config.hold_mods == crate::config::HoldMods::CtrlAlt,
        None,
    );
    let hold_ctrl_alt_id = hold_ctrl_alt.id().clone();
    hold_menu.append(&hold_ctrl_alt)?;

    let hold_ctrl_shift = CheckMenuItem::new(
        "Use Ctrl+Shift",
        true,
        config.hold_mods == crate::config::HoldMods::CtrlShift,
        None,
    );
    let hold_ctrl_shift_id = hold_ctrl_shift.id().clone();
    hold_menu.append(&hold_ctrl_shift)?;

    let hold_ctrl_cmd = CheckMenuItem::new(
        "Use Ctrl+Command",
        true,
        config.hold_mods == crate::config::HoldMods::CtrlCmd,
        None,
    );
    let hold_ctrl_cmd_id = hold_ctrl_cmd.id().clone();
    hold_menu.append(&hold_ctrl_cmd)?;

    hold_menu.append(&PredefinedMenuItem::separator())?;

    let toggle_label = MenuItem::new(
        format!(
            "Hands-off toggle: {}",
            match current_trigger {
                crate::config::ToggleTrigger::None => "OFF",
                crate::config::ToggleTrigger::DoubleCtrl => "Double Ctrl (RAW)",
                crate::config::ToggleTrigger::DoubleLeftOption => "Left Option (normal)",
                crate::config::ToggleTrigger::DoubleRightOption => "Right Option (assistive)",
                crate::config::ToggleTrigger::DoubleOption =>
                    "Option keys (left=format, right=assistive)",
            }
        ),
        false,
        None,
    );
    hold_menu.append(&toggle_label)?;

    let toggle_dictation = CheckMenuItem::new(
        "Enable double Ctrl toggle (RAW hands-off)",
        true,
        dictation_toggle_enabled,
        None,
    );
    let toggle_dictation_id = toggle_dictation.id().clone();
    hold_menu.append(&toggle_dictation)?;

    let toggle_assistive = CheckMenuItem::new(
        "Enable right Option toggle (assistive)",
        true,
        assistive_toggle_enabled,
        None,
    );
    let toggle_assistive_id = toggle_assistive.id().clone();
    hold_menu.append(&toggle_assistive)?;

    HOTKEYS_MENU_ITEMS.with(|items_cell| {
        *items_cell.borrow_mut() = Some(HotkeysMenuItems {
            hold_summary,
            hold_ctrl,
            hold_ctrl_alt,
            hold_ctrl_shift,
            hold_ctrl_cmd,
            toggle_assistive,
            toggle_dictation,
            toggle_label,
        });
    });

    Ok((
        hold_menu,
        HotkeysMenuIds {
            copy_cheatsheet: copy_cheatsheet_id,
            hold_ctrl: hold_ctrl_id,
            hold_ctrl_alt: hold_ctrl_alt_id,
            hold_ctrl_shift: hold_ctrl_shift_id,
            hold_ctrl_cmd: hold_ctrl_cmd_id,
            toggle_assistive: toggle_assistive_id,
            toggle_dictation: toggle_dictation_id,
            reset: reset_id,
        },
    ))
}
