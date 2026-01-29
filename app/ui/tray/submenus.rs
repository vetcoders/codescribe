//! Submenu building functions for the tray menu
//!
//! Each function builds a specific submenu and returns its IDs.

use anyhow::Result;
use muda::{CheckMenuItem, MenuId, MenuItem, PredefinedMenuItem, Submenu};

use crate::tray::state::{HOLD_MENU_ITEMS, TOGGLE_MENU_ITEMS};
use crate::tray::types::{HoldMenuItems, HoldMods, ToggleMenuItems};

// Type aliases
pub type HoldMenuIds = (MenuId, MenuId, MenuId, MenuId, MenuId, MenuId);

/// Build the Hold Hotkeys submenu
pub fn build_hold_hotkeys_submenu() -> Result<(Submenu, HoldMenuIds)> {
    let hold_menu = Submenu::new("Hotkeys", true);

    // Read from Config (source of truth for initial state)
    let config = crate::config::Config::load();
    let current_mods = config.hold_mods;
    let current_trigger = config.toggle_trigger;

    let (base_for_summary, show_deprecated_hold_note) = match current_mods {
        HoldMods::Ctrl => ("Ctrl", false),
        HoldMods::CtrlAlt => ("Ctrl+Option", false),
        _ => ("Ctrl", true),
    };

    let hold_summary = MenuItem::new(
        format!(
            "Hold: {base} → RAW, {base}+Shift → Chat, {base}+Cmd → Selection{}",
            if config.hold_exclusive {
                " (Shift/Cmd modes OFF)"
            } else {
                ""
            },
            base = base_for_summary
        ),
        false,
        None,
    );
    hold_menu.append(&hold_summary)?;

    let reset_item = MenuItem::new("Reset hotkeys (recommended)", true, None);
    let reset_id = reset_item.id().clone();
    hold_menu.append(&reset_item)?;
    hold_menu.append(&PredefinedMenuItem::separator())?;

    let hold_current_label =
        MenuItem::new(format!("Current: {}", current_mods.label()), false, None);
    hold_menu.append(&hold_current_label)?;
    if show_deprecated_hold_note {
        let note = MenuItem::new(
            "Note: custom hold mods are deprecated; use Reset shortcuts",
            false,
            None,
        );
        hold_menu.append(&note)?;
    }

    let hold_ctrl = CheckMenuItem::new(
        "Hold: Ctrl (recommended)",
        true,
        current_mods == HoldMods::Ctrl,
        None,
    );
    let hold_ctrl_id = hold_ctrl.id().clone();
    let hold_ctrl_opt = CheckMenuItem::new(
        "Hold: Ctrl+Option (legacy)",
        true,
        current_mods == HoldMods::CtrlAlt,
        None,
    );
    let hold_ctrl_opt_id = hold_ctrl_opt.id().clone();

    hold_menu.append(&hold_ctrl)?;
    hold_menu.append(&hold_ctrl_opt)?;
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
            toggle_double_opt_id,
            toggle_double_ralt_id,
            toggle_disabled_id,
            reset_id,
        ),
    ))
}
