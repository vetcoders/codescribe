//! Main menu building logic for the tray menu
//!
//! Constructs the complete tray menu by composing submenus.

use anyhow::Result;
use muda::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem};

use crate::tray::submenus::{
    build_appearance_submenu, build_feedback_submenu, build_formatting_submenu,
    build_history_submenu, build_hold_hotkeys_submenu, build_language_submenu,
    build_models_submenu, build_permissions_submenu, build_tools_submenu,
};
use crate::tray::types::MenuIds;

/// Build the complete tray menu with all submenus
pub fn build_menu() -> Result<(Menu, MenuIds)> {
    let menu = Menu::new();

    // 1. Status: Ready (disabled label)
    let status_item = MenuItem::new("Status: Ready", false, None);
    menu.append(&status_item)?;

    // 2. Enable Hotkeys (checkbox toggle)
    let enable_hotkeys = CheckMenuItem::new("Enable Hotkeys", true, true, None);
    let enable_hotkeys_id = enable_hotkeys.id().clone();
    menu.append(&enable_hotkeys)?;

    // 3. Separator
    menu.append(&PredefinedMenuItem::separator())?;

    // 4. Language submenu
    let (lang_menu, lang_auto_id, lang_polish_id, lang_english_id) = build_language_submenu()?;
    menu.append(&lang_menu)?;

    // 5. Models submenu
    let (models_menu, model_ids) = build_models_submenu()?;
    menu.append(&models_menu)?;

    // 6. Formatting submenu
    let (fmt_menu, fmt_toggle_id, fmt_harmony_id, fmt_ollama_id) = build_formatting_submenu()?;
    menu.append(&fmt_menu)?;

    // 7. Hold Hotkeys submenu
    let (hold_menu, hold_ids) = build_hold_hotkeys_submenu()?;
    menu.append(&hold_menu)?;

    // 8. History submenu
    let (history_menu, history_save_id, history_copy_latest_id, history_open_folder_id) =
        build_history_submenu()?;
    menu.append(&history_menu)?;

    // 9. Appearance submenu
    let (appearance_menu, appearance_glyph_id, appearance_refresh_id) =
        build_appearance_submenu()?;
    menu.append(&appearance_menu)?;

    // 10. Feedback submenu
    let (feedback_menu, feedback_ids) = build_feedback_submenu()?;
    menu.append(&feedback_menu)?;

    // 11. Tools submenu
    let (tools_menu, tools_voice_lab_id, tools_teacher_id, tools_native_lab_id, tools_new_conversation_id) =
        build_tools_submenu()?;
    menu.append(&tools_menu)?;

    // 12. Permissions submenu
    let (permissions_menu, perm_check_id, perm_accessibility_id, perm_microphone_id) =
        build_permissions_submenu()?;
    menu.append(&permissions_menu)?;

    // 13. Separator
    menu.append(&PredefinedMenuItem::separator())?;

    // 14. Start at Login
    let is_enabled = crate::launchd::is_login_item_enabled();
    let start_at_login = CheckMenuItem::new("Start at Login", true, is_enabled, None);
    let start_at_login_id = start_at_login.id().clone();
    menu.append(&start_at_login)?;

    // 15. Quit
    let quit_item = MenuItem::new("Quit", true, None);
    let quit_id = quit_item.id().clone();
    menu.append(&quit_item)?;

    Ok((
        menu,
        MenuIds {
            enable_hotkeys: enable_hotkeys_id,
            start_at_login: start_at_login_id,
            quit: quit_id,
            lang_auto: lang_auto_id,
            lang_polish: lang_polish_id,
            lang_english: lang_english_id,
            model_small: model_ids.0,
            model_medium: model_ids.1,
            model_large_v3: model_ids.2,
            model_large_v3_turbo: model_ids.3,
            model_large_v3_q8: model_ids.4,
            model_open_folder: model_ids.5,
            fmt_toggle: fmt_toggle_id,
            fmt_harmony: fmt_harmony_id,
            fmt_ollama: fmt_ollama_id,
            hold_ctrl: hold_ids.0,
            hold_ctrl_opt: hold_ids.1,
            hold_ctrl_shift: hold_ids.2,
            hold_ctrl_cmd: hold_ids.3,
            hold_exclusive: hold_ids.4,
            toggle_double_opt: hold_ids.5,
            toggle_double_ralt: hold_ids.6,
            toggle_disabled: hold_ids.7,
            history_save: history_save_id,
            history_copy_latest: history_copy_latest_id,
            history_open_folder: history_open_folder_id,
            appearance_glyph: appearance_glyph_id,
            appearance_refresh: appearance_refresh_id,
            feedback_start_sound: feedback_ids.0,
            feedback_sound_tink: feedback_ids.1,
            feedback_sound_pop: feedback_ids.2,
            volume_mute: feedback_ids.3,
            volume_low: feedback_ids.4,
            volume_medium: feedback_ids.5,
            volume_high: feedback_ids.6,
            volume_full: feedback_ids.7,
            perm_check: perm_check_id,
            perm_accessibility: perm_accessibility_id,
            perm_microphone: perm_microphone_id,
            tools_voice_lab: tools_voice_lab_id,
            tools_teacher: tools_teacher_id,
            tools_native_lab: tools_native_lab_id,
            tools_new_conversation: tools_new_conversation_id,
        },
    ))
}
