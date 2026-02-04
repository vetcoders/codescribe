//! Main menu building logic for the tray menu
//!
//! Menu structure (flat):
//! - Status line (dynamic)
//! - Show Chat Overlay / Open history / Copy last
//! - Hotkeys ▸
//! - Prompts ▸ / Notes ▸ / Diagnostics ▸
//! - Quick Start / Help / About
//! - Quit
//!
//! Note: Settings options moved to Settings tab in Chat Overlay

use std::cell::RefCell;

use anyhow::Result;
use muda::accelerator::{Accelerator, Code, Modifiers};
use muda::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem, Submenu};

use codescribe_core::vad;

use crate::config::Config;
use crate::tray::state::NOTES_MENU_ITEMS;
use crate::tray::submenus::build_hold_hotkeys_submenu;
use crate::tray::types::{MenuIds, NotesMenuItems, VadPreset};

// Thread-local storage for menu items that need dynamic updates
thread_local! {
    pub static STATUS_MENU_ITEM: RefCell<Option<MenuItem>> = const { RefCell::new(None) };
    pub static QUALITY_MENU_ITEM: RefCell<Option<MenuItem>> = const { RefCell::new(None) };
    pub static SILERO_VAD_MENU_ITEM: RefCell<Option<MenuItem>> = const { RefCell::new(None) };
    pub static VAD_PRESET_MENU_ITEMS: RefCell<Option<VadPresetMenuItems>> = const { RefCell::new(None) };
}

struct VadPresetMenuItems {
    sensitive: CheckMenuItem,
    balanced: CheckMenuItem,
    conservative: CheckMenuItem,
}

const PRESET_SENSITIVE_THRESHOLD: f32 = 0.35;
const PRESET_BALANCED_THRESHOLD: f32 = 0.5;
const PRESET_CONSERVATIVE_THRESHOLD: f32 = 0.7;

const PRESET_SENSITIVE_SILENCE_SEC: f32 = 1.8;
const PRESET_BALANCED_SILENCE_SEC: f32 = 1.2;
const PRESET_CONSERVATIVE_SILENCE_SEC: f32 = 0.8;

/// Build the tray menu
///
/// Note: Settings moved to Settings tab in Chat Overlay
pub fn build_menu() -> Result<(Menu, MenuIds)> {
    let menu = Menu::new();

    // 1. Status line (disabled, dynamic text)
    let status_item = MenuItem::new("Status: Idle", false, None);
    menu.append(&status_item)?;

    // Store for dynamic updates
    STATUS_MENU_ITEM.with(|cell| {
        *cell.borrow_mut() = Some(status_item);
    });

    // 2. Show Chat Overlay
    let show_overlay_item = MenuItem::new("Show Chat Overlay", true, None);
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

    // 6. Hotkeys (promoted to root level)
    let (hold_hotkeys_menu, hold_ids) = build_hold_hotkeys_submenu()?;
    menu.append(&hold_hotkeys_menu)?;

    menu.append(&PredefinedMenuItem::separator())?;

    // 7. Prompts submenu
    let prompts_menu = Submenu::new("Edit prompts…", true);
    let open_assistive_prompt_item = MenuItem::new("Assistive…", true, None);
    let open_assistive_prompt_id = open_assistive_prompt_item.id().clone();
    prompts_menu.append(&open_assistive_prompt_item)?;

    let open_formatting_prompt_item = MenuItem::new("Formatting…", true, None);
    let open_formatting_prompt_id = open_formatting_prompt_item.id().clone();
    prompts_menu.append(&open_formatting_prompt_item)?;

    let open_prompts_folder_item = MenuItem::new("Open prompts folder", true, None);
    let open_prompts_folder_id = open_prompts_folder_item.id().clone();
    prompts_menu.append(&open_prompts_folder_item)?;

    menu.append(&prompts_menu)?;

    // 6b. Notes submenu
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

    // 6c. Diagnostics submenu
    let diagnostics_menu = Submenu::new("Diagnostics", true);
    let copy_diag_item = MenuItem::new("Copy diagnostics", true, None);
    let copy_diag_id = copy_diag_item.id().clone();
    diagnostics_menu.append(&copy_diag_item)?;

    let open_accessibility_item = MenuItem::new("Open Accessibility settings…", true, None);
    let open_accessibility_id = open_accessibility_item.id().clone();
    diagnostics_menu.append(&open_accessibility_item)?;

    let open_input_monitoring_item = MenuItem::new("Open Input Monitoring settings…", true, None);
    let open_input_monitoring_id = open_input_monitoring_item.id().clone();
    diagnostics_menu.append(&open_input_monitoring_item)?;

    let reset_input_monitoring_item =
        MenuItem::new("Reset Input Monitoring permission (restart)…", true, None);
    let reset_input_monitoring_id = reset_input_monitoring_item.id().clone();
    diagnostics_menu.append(&reset_input_monitoring_item)?;

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

    // VAD presets submenu (radio-ish checkboxes)
    let vad_preset_menu = Submenu::new("VAD preset", true);
    let active = current_vad_preset();

    let sensitive = CheckMenuItem::new(
        "Sensitive (less chopping, quiet speech)",
        true,
        active == Some(VadPreset::Sensitive),
        None,
    );
    let vad_preset_sensitive_id = sensitive.id().clone();
    vad_preset_menu.append(&sensitive)?;

    let balanced = CheckMenuItem::new(
        "Balanced (default)",
        true,
        active == Some(VadPreset::Balanced),
        None,
    );
    let vad_preset_balanced_id = balanced.id().clone();
    vad_preset_menu.append(&balanced)?;

    let conservative = CheckMenuItem::new(
        "Conservative (noisy room, may cut more)",
        true,
        active == Some(VadPreset::Conservative),
        None,
    );
    let vad_preset_conservative_id = conservative.id().clone();
    vad_preset_menu.append(&conservative)?;

    VAD_PRESET_MENU_ITEMS.with(|cell| {
        *cell.borrow_mut() = Some(VadPresetMenuItems {
            sensitive,
            balanced,
            conservative,
        });
    });

    diagnostics_menu.append(&vad_preset_menu)?;

    menu.append(&diagnostics_menu)?;

    menu.append(&PredefinedMenuItem::separator())?;

    // 8. Onboarding
    let onboarding_item = MenuItem::new("Settings", true, None);
    let onboarding_id = onboarding_item.id().clone();
    menu.append(&onboarding_item)?;

    // 9. Help
    let help_item = MenuItem::new("Help", true, None);
    let help_id = help_item.id().clone();
    menu.append(&help_item)?;

    // 9. About
    let about_item = MenuItem::new("About", true, None);
    let about_id = about_item.id().clone();
    menu.append(&about_item)?;

    // 10. Separator
    menu.append(&PredefinedMenuItem::separator())?;

    // 11. Quit (Cmd+Q)
    let quit_accel = Accelerator::new(Some(Modifiers::SUPER), Code::KeyQ);
    let quit_item = MenuItem::new("Quit", true, Some(quit_accel));
    let quit_id = quit_item.id().clone();
    menu.append(&quit_item)?;

    let hotkeys_toggle_assistive_id = hold_ids.toggle_assistive;
    let hotkeys_toggle_dictation_id = hold_ids.toggle_dictation;
    let hotkeys_reset_id = hold_ids.reset;
    let hotkeys_copy_cheatsheet_id = hold_ids.copy_cheatsheet;
    let hotkeys_hold_ctrl_id = hold_ids.hold_ctrl;
    let hotkeys_hold_ctrl_alt_id = hold_ids.hold_ctrl_alt;
    let hotkeys_hold_ctrl_shift_id = hold_ids.hold_ctrl_shift;
    let hotkeys_hold_ctrl_cmd_id = hold_ids.hold_ctrl_cmd;

    Ok((
        menu,
        MenuIds {
            copy_last: copy_last_id,
            show_overlay: show_overlay_id,
            run_onboarding: onboarding_id,
            open_history: open_history_id,
            copy_diagnostics: copy_diag_id,
            open_accessibility_settings: open_accessibility_id,
            open_input_monitoring_settings: open_input_monitoring_id,
            reset_input_monitoring_permission: reset_input_monitoring_id,
            open_assistive_prompt: open_assistive_prompt_id,
            open_formatting_prompt: open_formatting_prompt_id,
            open_prompts_folder: open_prompts_folder_id,
            help: help_id,
            about: about_id,
            quit: quit_id,
            // Hotkeys submenu
            hotkeys_toggle_assistive: hotkeys_toggle_assistive_id,
            hotkeys_toggle_dictation: hotkeys_toggle_dictation_id,
            hotkeys_reset: hotkeys_reset_id,
            hotkeys_copy_cheatsheet: hotkeys_copy_cheatsheet_id,
            hotkeys_hold_ctrl: hotkeys_hold_ctrl_id,
            hotkeys_hold_ctrl_alt: hotkeys_hold_ctrl_alt_id,
            hotkeys_hold_ctrl_shift: hotkeys_hold_ctrl_shift_id,
            hotkeys_hold_ctrl_cmd: hotkeys_hold_ctrl_cmd_id,
            // Quality
            quality_open_report: quality_open_report_id,
            // Models
            silero_vad_install: silero_vad_install_id,
            // VAD presets
            vad_preset_sensitive: vad_preset_sensitive_id,
            vad_preset_balanced: vad_preset_balanced_id,
            vad_preset_conservative: vad_preset_conservative_id,

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

/// Toggle AI Formatting and persist to config
/// Note: Tray menu checkbox removed - settings now in Chat Overlay Settings tab
pub fn toggle_ai_formatting() -> bool {
    // Read current state from Config (source of truth)
    let current_state = Config::load().ai_formatting_enabled;
    let new_state = !current_state;

    // Persist to config
    let config = Config::load();
    let _ = config.save_to_env("AI_FORMATTING_ENABLED", if new_state { "1" } else { "0" });

    new_state
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

pub fn update_vad_preset_checks() {
    let active = current_vad_preset();
    VAD_PRESET_MENU_ITEMS.with(|cell| {
        let borrowed = cell.borrow();
        let Some(items) = borrowed.as_ref() else {
            return;
        };

        items
            .sensitive
            .set_checked(active == Some(VadPreset::Sensitive));
        items
            .balanced
            .set_checked(active == Some(VadPreset::Balanced));
        items
            .conservative
            .set_checked(active == Some(VadPreset::Conservative));
    });
}

fn current_vad_preset() -> Option<VadPreset> {
    let threshold = std::env::var("CODESCRIBE_VAD_THRESHOLD")
        .ok()
        .and_then(|v| v.parse::<f32>().ok())
        .unwrap_or(PRESET_BALANCED_THRESHOLD);
    let silence = std::env::var("CODESCRIBE_VAD_MAX_SILENCE_SEC")
        .ok()
        .and_then(|v| v.parse::<f32>().ok())
        .unwrap_or(PRESET_BALANCED_SILENCE_SEC);

    const EPS: f32 = 0.05;

    if (threshold - PRESET_SENSITIVE_THRESHOLD).abs() <= EPS
        && (silence - PRESET_SENSITIVE_SILENCE_SEC).abs() <= EPS
    {
        return Some(VadPreset::Sensitive);
    }
    if (threshold - PRESET_BALANCED_THRESHOLD).abs() <= EPS
        && (silence - PRESET_BALANCED_SILENCE_SEC).abs() <= EPS
    {
        return Some(VadPreset::Balanced);
    }
    if (threshold - PRESET_CONSERVATIVE_THRESHOLD).abs() <= EPS
        && (silence - PRESET_CONSERVATIVE_SILENCE_SEC).abs() <= EPS
    {
        return Some(VadPreset::Conservative);
    }

    None
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

#[cfg(test)]
mod tests {
    fn menu_labels_for_test() -> Vec<String> {
        vec![
            "Status: Idle".to_string(),
            "Show Chat Overlay".to_string(),
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
    fn tray_menu_includes_show_chat_overlay() {
        let labels = menu_labels_for_test();
        let found = labels.iter().any(|label| label == "Show Chat Overlay");
        assert!(found, "Show Chat Overlay menu item missing");
    }
}
