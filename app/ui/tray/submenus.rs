//! Submenu building functions for the tray menu
//!
//! Each function builds a specific submenu and returns its IDs.

use anyhow::Result;
use muda::{CheckMenuItem, MenuId, MenuItem, PredefinedMenuItem, Submenu};

use crate::config::{Config, HoldMods, ToggleTrigger};
use crate::tray::state::HOTKEYS_MENU_ITEMS;
use crate::tray::types::HotkeysMenuItems;

// Type aliases
pub struct HotkeysMenuIds {
    pub copy_cheatsheet: MenuId,
    pub toggle_assistive: MenuId,
    pub toggle_dictation: MenuId,
    pub reset: MenuId,
}

pub(crate) fn hotkeys_summary_lines(config: &Config) -> (String, String) {
    let hold_line = if config.hold_exclusive {
        format!(
            "Hold {} — RAW (Shift/Cmd disabled)",
            config.hold_mods.label()
        )
    } else {
        match config.hold_mods {
            HoldMods::Fn => "Hold Fn — RAW • Fn+Shift — Chat • Fn+Cmd — Selection".to_string(),
            HoldMods::Ctrl => "Hold Ctrl — RAW".to_string(),
            HoldMods::CtrlAlt => {
                "Hold Ctrl — RAW • Ctrl+Option — Format • Ctrl+Shift — Chat • Ctrl+Cmd — Selection"
                    .to_string()
            }
            HoldMods::CtrlShift => "Hold Ctrl+Shift — RAW".to_string(),
            HoldMods::CtrlCmd => "Hold Ctrl+Cmd — RAW".to_string(),
        }
    };

    let toggle_line = format!(
        "Toggle: {}",
        match config.toggle_trigger {
            ToggleTrigger::None => "OFF",
            ToggleTrigger::DoubleCtrl => "Double Ctrl (RAW)",
            ToggleTrigger::DoubleLeftOption => "Left Option (format)",
            ToggleTrigger::DoubleRightOption => "Right Option (assistive)",
            ToggleTrigger::DoubleOption => "Option keys (left=format, right=assistive)",
        }
    );

    (hold_line, toggle_line)
}

/// Build the Hold Hotkeys submenu
pub fn build_hold_hotkeys_submenu() -> Result<(Submenu, HotkeysMenuIds)> {
    let hold_menu = Submenu::new("Hotkeys", true);

    // Read from Config (source of truth for initial state)
    let config = Config::load();
    let current_trigger = config.toggle_trigger;
    let assistive_toggle_enabled = matches!(
        current_trigger,
        ToggleTrigger::DoubleOption | ToggleTrigger::DoubleRightOption
    );
    let dictation_toggle_enabled = matches!(current_trigger, ToggleTrigger::DoubleCtrl);

    let (hold_line, toggle_line) = hotkeys_summary_lines(&config);
    let hold_summary = MenuItem::new(hold_line, false, None);
    let toggle_summary = MenuItem::new(toggle_line, false, None);
    hold_menu.append(&hold_summary)?;
    hold_menu.append(&toggle_summary)?;

    let copy_cheatsheet_item = MenuItem::new("Copy hotkeys cheatsheet", true, None);
    let copy_cheatsheet_id = copy_cheatsheet_item.id().clone();
    hold_menu.append(&copy_cheatsheet_item)?;

    let reset_item = MenuItem::new("Apply recommended preset (Fn + Option toggle)", true, None);
    let reset_id = reset_item.id().clone();
    hold_menu.append(&reset_item)?;
    hold_menu.append(&PredefinedMenuItem::separator())?;

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
            toggle_assistive,
            toggle_dictation,
            toggle_summary,
        });
    });

    Ok((
        hold_menu,
        HotkeysMenuIds {
            copy_cheatsheet: copy_cheatsheet_id,
            toggle_assistive: toggle_assistive_id,
            toggle_dictation: toggle_dictation_id,
            reset: reset_id,
        },
    ))
}
