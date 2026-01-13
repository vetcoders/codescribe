//! Submenu building functions for the tray menu
//!
//! Each function builds a specific submenu and returns its IDs.
//! Some functions are prepared for future use but not yet integrated into the main menu.

#![allow(dead_code)]

use anyhow::Result;
use muda::{
    CheckMenuItem, IconMenuItem, MenuId, MenuItem, NativeIcon, PredefinedMenuItem, Submenu,
};

use crate::tray::state::{
    HISTORY_MENU_ITEMS, HOLD_MENU_ITEMS, MODEL_MENU_ITEMS, TOGGLE_MENU_ITEMS,
};
use crate::tray::types::{
    HistoryMenuItems, HoldMenuItems, HoldMods, ModelMenuItems, ToggleMenuItems, VolumeLevel,
};

// Type aliases
pub type ModelMenuIds = (MenuId, MenuId, MenuId, MenuId, MenuId, MenuId);
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
pub type FeedbackMenuIds = (
    MenuId,
    MenuId,
    MenuId,
    MenuId,
    MenuId,
    MenuId,
    MenuId,
    MenuId,
);

/// Build the Language submenu
pub fn build_language_submenu() -> Result<(Submenu, MenuId, MenuId, MenuId)> {
    let lang_menu = Submenu::new("Language", true);
    let lang_auto = CheckMenuItem::new("Auto", true, true, None);
    let lang_auto_id = lang_auto.id().clone();
    let lang_polish = CheckMenuItem::new("Polish (PL)", true, false, None);
    let lang_polish_id = lang_polish.id().clone();
    let lang_english = CheckMenuItem::new("English (EN)", true, false, None);
    let lang_english_id = lang_english.id().clone();

    lang_menu.append(&lang_auto)?;
    lang_menu.append(&lang_polish)?;
    lang_menu.append(&lang_english)?;

    Ok((lang_menu, lang_auto_id, lang_polish_id, lang_english_id))
}

/// Build the Models submenu (Whisper model selection)
pub fn build_models_submenu() -> Result<(Submenu, ModelMenuIds)> {
    let models_menu = Submenu::new("Models", true);

    let current_whisper =
        std::env::var("WHISPER_VARIANT").unwrap_or_else(|_| "large-v3-q8".to_string());
    let current_label = match current_whisper.as_str() {
        "small" => "Small",
        "medium" => "Medium",
        "large-v3" => "Large v3",
        "large-v3-turbo" => "Large v3 Turbo",
        "large-v3-q8" | "large-v3-mlx-q8" => "Large v3 Q8",
        _ => &current_whisper,
    };
    let whisper_label = MenuItem::new(format!("Whisper: {}", current_label), false, None);
    models_menu.append(&whisper_label)?;
    models_menu.append(&PredefinedMenuItem::separator())?;

    let model_small =
        CheckMenuItem::new("Use Whisper: Small", true, current_whisper == "small", None);
    let model_small_id = model_small.id().clone();
    let model_medium = CheckMenuItem::new(
        "Use Whisper: Medium",
        true,
        current_whisper == "medium",
        None,
    );
    let model_medium_id = model_medium.id().clone();
    let model_large_v3 = CheckMenuItem::new(
        "Use Whisper: Large v3",
        true,
        current_whisper == "large-v3",
        None,
    );
    let model_large_v3_id = model_large_v3.id().clone();
    let model_large_v3_turbo = CheckMenuItem::new(
        "Use Whisper: Large v3 Turbo",
        true,
        current_whisper == "large-v3-turbo",
        None,
    );
    let model_large_v3_turbo_id = model_large_v3_turbo.id().clone();
    let model_large_v3_q8 = CheckMenuItem::new(
        "Use Whisper: Large v3 Q8",
        true,
        current_whisper == "large-v3-q8" || current_whisper == "large-v3-mlx-q8",
        None,
    );
    let model_large_v3_q8_id = model_large_v3_q8.id().clone();

    models_menu.append(&model_small)?;
    models_menu.append(&model_medium)?;
    models_menu.append(&model_large_v3)?;
    models_menu.append(&model_large_v3_turbo)?;
    models_menu.append(&model_large_v3_q8)?;
    models_menu.append(&PredefinedMenuItem::separator())?;

    let model_open_folder = MenuItem::new("Open Models Folder", true, None);
    let model_open_folder_id = model_open_folder.id().clone();
    models_menu.append(&model_open_folder)?;

    MODEL_MENU_ITEMS.with(|items_cell| {
        *items_cell.borrow_mut() = Some(ModelMenuItems {
            small: model_small,
            medium: model_medium,
            large_v3: model_large_v3,
            large_v3_turbo: model_large_v3_turbo,
            large_v3_q8: model_large_v3_q8,
            label: whisper_label,
        });
    });

    Ok((
        models_menu,
        (
            model_small_id,
            model_medium_id,
            model_large_v3_id,
            model_large_v3_turbo_id,
            model_large_v3_q8_id,
            model_open_folder_id,
        ),
    ))
}

/// Build the Formatting submenu
pub fn build_formatting_submenu() -> Result<(Submenu, MenuId, MenuId, MenuId)> {
    let fmt_menu = Submenu::new("Formatting", true);

    let ai_enabled = std::env::var("FORMAT_ENABLED")
        .map(|v| v == "1" || v.to_lowercase() == "true")
        .unwrap_or(false);
    let fmt_toggle = CheckMenuItem::new("Enable AI Formatting", true, ai_enabled, None);
    let fmt_toggle_id = fmt_toggle.id().clone();
    fmt_menu.append(&fmt_toggle)?;
    fmt_menu.append(&PredefinedMenuItem::separator())?;

    let fmt_provider_label = MenuItem::new("Provider", false, None);
    fmt_menu.append(&fmt_provider_label)?;

    let current_provider = std::env::var("AI_PROVIDER").unwrap_or_else(|_| "harmony".to_string());
    let fmt_harmony = CheckMenuItem::new(
        "Harmony (LibraxisAI)",
        true,
        current_provider == "harmony",
        None,
    );
    let fmt_harmony_id = fmt_harmony.id().clone();
    let fmt_ollama = CheckMenuItem::new("Ollama (Local)", true, current_provider == "ollama", None);
    let fmt_ollama_id = fmt_ollama.id().clone();

    fmt_menu.append(&fmt_harmony)?;
    fmt_menu.append(&fmt_ollama)?;
    fmt_menu.append(&PredefinedMenuItem::separator())?;

    let assistive_label = MenuItem::new("Assistive: Ctrl+Shift → AI chat mode", false, None);
    fmt_menu.append(&assistive_label)?;

    Ok((fmt_menu, fmt_toggle_id, fmt_harmony_id, fmt_ollama_id))
}

/// Build the Hold Hotkeys submenu
pub fn build_hold_hotkeys_submenu() -> Result<(Submenu, HoldMenuIds)> {
    let hold_menu = Submenu::new("Hold Hotkeys", true);

    let hold_current_label = MenuItem::new("Current: Ctrl only (Raw)", false, None);
    hold_menu.append(&hold_current_label)?;
    hold_menu.append(&PredefinedMenuItem::separator())?;

    let hold_ctrl = CheckMenuItem::new(
        format!("Hold: {}", HoldMods::Ctrl.label()),
        true,
        true,
        None,
    );
    let hold_ctrl_id = hold_ctrl.id().clone();
    let hold_ctrl_opt = CheckMenuItem::new(
        format!("Hold: {}", HoldMods::CtrlAlt.label()),
        true,
        false,
        None,
    );
    let hold_ctrl_opt_id = hold_ctrl_opt.id().clone();
    let hold_ctrl_shift = CheckMenuItem::new(
        format!("Hold: {}", HoldMods::CtrlShift.label()),
        true,
        false,
        None,
    );
    let hold_ctrl_shift_id = hold_ctrl_shift.id().clone();
    let hold_ctrl_cmd = CheckMenuItem::new(
        format!("Hold: {}", HoldMods::CtrlCmd.label()),
        true,
        false,
        None,
    );
    let hold_ctrl_cmd_id = hold_ctrl_cmd.id().clone();

    hold_menu.append(&hold_ctrl)?;
    hold_menu.append(&hold_ctrl_opt)?;
    hold_menu.append(&hold_ctrl_shift)?;
    hold_menu.append(&hold_ctrl_cmd)?;
    hold_menu.append(&PredefinedMenuItem::separator())?;

    let hold_exclusive = CheckMenuItem::new("Exclusive (ignore extra modifiers)", true, true, None);
    let hold_exclusive_id = hold_exclusive.id().clone();
    hold_menu.append(&hold_exclusive)?;
    hold_menu.append(&PredefinedMenuItem::separator())?;

    let toggle_label = MenuItem::new("Toggle: double option", false, None);
    hold_menu.append(&toggle_label)?;
    let toggle_double_opt = CheckMenuItem::new("Use double Option (⌥⌥)", true, true, None);
    let toggle_double_opt_id = toggle_double_opt.id().clone();
    let toggle_double_ralt = CheckMenuItem::new("Use double Right Option", true, false, None);
    let toggle_double_ralt_id = toggle_double_ralt.id().clone();
    let toggle_disabled = CheckMenuItem::new("Disable toggle", true, false, None);
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

/// Build the Recent Transcripts submenu (History)
pub fn build_history_submenu() -> Result<(Submenu, MenuId, MenuId, MenuId)> {
    let history_menu = Submenu::new("Recent Transcripts", true);

    let recent_entries = crate::history::recent_entries(5);
    let latest_label = if let Some(entry) = recent_entries.first() {
        format!("Latest: {}", entry.label())
    } else {
        "Latest: (none)".to_string()
    };
    let history_latest_label = MenuItem::new(latest_label, false, None);
    history_menu.append(&history_latest_label)?;
    history_menu.append(&PredefinedMenuItem::separator())?;

    let history_save = CheckMenuItem::new("Save transcripts to History", true, true, None);
    let history_save_id = history_save.id().clone();
    history_menu.append(&history_save)?;
    history_menu.append(&PredefinedMenuItem::separator())?;

    if recent_entries.is_empty() {
        let placeholder_entry = MenuItem::new("(no recent entries)", false, None);
        history_menu.append(&placeholder_entry)?;
    } else {
        for entry in recent_entries.iter().take(5) {
            let label = entry.label();
            let display = if label.chars().count() > 40 {
                format!("{}...", label.chars().take(37).collect::<String>())
            } else {
                label.to_string()
            };
            history_menu.append(&MenuItem::new(display, true, None))?;
        }
    }
    history_menu.append(&PredefinedMenuItem::separator())?;

    let history_copy_latest = MenuItem::new("Copy Latest to Clipboard", true, None);
    let history_copy_latest_id = history_copy_latest.id().clone();
    let history_open_folder = MenuItem::new("Open Folder", true, None);
    let history_open_folder_id = history_open_folder.id().clone();

    history_menu.append(&history_copy_latest)?;
    history_menu.append(&history_open_folder)?;

    HISTORY_MENU_ITEMS.with(|items_cell| {
        *items_cell.borrow_mut() = Some(HistoryMenuItems {
            latest_label: history_latest_label,
        });
    });

    Ok((
        history_menu,
        history_save_id,
        history_copy_latest_id,
        history_open_folder_id,
    ))
}

/// Build the Appearance submenu
pub fn build_appearance_submenu() -> Result<(Submenu, MenuId, MenuId)> {
    let appearance_menu = Submenu::new("Appearance", true);

    let appearance_glyph = CheckMenuItem::new("Show status glyph next to icon", true, true, None);
    let appearance_glyph_id = appearance_glyph.id().clone();
    appearance_menu.append(&appearance_glyph)?;
    appearance_menu.append(&PredefinedMenuItem::separator())?;

    let appearance_refresh = MenuItem::new("Refresh Tray Icon", true, None);
    let appearance_refresh_id = appearance_refresh.id().clone();
    appearance_menu.append(&appearance_refresh)?;

    Ok((appearance_menu, appearance_glyph_id, appearance_refresh_id))
}

/// Build the Feedback submenu
pub fn build_feedback_submenu() -> Result<(Submenu, FeedbackMenuIds)> {
    let feedback_menu = Submenu::new("Feedback", true);

    let feedback_start_sound = CheckMenuItem::new("Enable Start Sound", true, true, None);
    let feedback_start_sound_id = feedback_start_sound.id().clone();
    feedback_menu.append(&feedback_start_sound)?;
    feedback_menu.append(&PredefinedMenuItem::separator())?;

    let feedback_sound_tink = CheckMenuItem::new("Sound: Tink", true, true, None);
    let feedback_sound_tink_id = feedback_sound_tink.id().clone();
    let feedback_sound_pop = CheckMenuItem::new("Sound: Pop", true, false, None);
    let feedback_sound_pop_id = feedback_sound_pop.id().clone();
    feedback_menu.append(&feedback_sound_tink)?;
    feedback_menu.append(&feedback_sound_pop)?;

    let volume_menu = Submenu::new("Volume", true);
    let volume_mute = CheckMenuItem::new(VolumeLevel::Mute.label(), true, false, None);
    let volume_mute_id = volume_mute.id().clone();
    let volume_low = CheckMenuItem::new(VolumeLevel::Low.label(), true, false, None);
    let volume_low_id = volume_low.id().clone();
    let volume_medium = CheckMenuItem::new(VolumeLevel::Medium.label(), true, true, None);
    let volume_medium_id = volume_medium.id().clone();
    let volume_high = CheckMenuItem::new(VolumeLevel::High.label(), true, false, None);
    let volume_high_id = volume_high.id().clone();
    let volume_full = CheckMenuItem::new(VolumeLevel::Full.label(), true, false, None);
    let volume_full_id = volume_full.id().clone();
    volume_menu.append(&volume_mute)?;
    volume_menu.append(&volume_low)?;
    volume_menu.append(&volume_medium)?;
    volume_menu.append(&volume_high)?;
    volume_menu.append(&volume_full)?;
    feedback_menu.append(&volume_menu)?;

    Ok((
        feedback_menu,
        (
            feedback_start_sound_id,
            feedback_sound_tink_id,
            feedback_sound_pop_id,
            volume_mute_id,
            volume_low_id,
            volume_medium_id,
            volume_high_id,
            volume_full_id,
        ),
    ))
}

/// Build the Permissions submenu
pub fn build_permissions_submenu() -> Result<(Submenu, MenuId, MenuId, MenuId)> {
    let permissions_menu = Submenu::new("Permissions", true);

    let ax_status = if crate::permissions::check_accessibility()
        == crate::permissions::PermissionStatus::Granted
    {
        "✓"
    } else {
        "✗"
    };
    let mic_status = match crate::permissions::check_microphone() {
        crate::permissions::PermissionStatus::Granted => "✓",
        crate::permissions::PermissionStatus::NotDetermined => "?",
        _ => "✗",
    };
    let perm_status_label = MenuItem::new(
        format!("AX: {} | Mic: {}", ax_status, mic_status),
        false,
        None,
    );
    permissions_menu.append(&perm_status_label)?;
    permissions_menu.append(&PredefinedMenuItem::separator())?;

    let perm_check = MenuItem::new("Check Permissions Now", true, None);
    let perm_check_id = perm_check.id().clone();
    permissions_menu.append(&perm_check)?;
    permissions_menu.append(&PredefinedMenuItem::separator())?;

    let perm_accessibility = MenuItem::new("Open Accessibility Settings", true, None);
    let perm_accessibility_id = perm_accessibility.id().clone();
    permissions_menu.append(&perm_accessibility)?;

    let perm_microphone = MenuItem::new("Open Microphone Settings", true, None);
    let perm_microphone_id = perm_microphone.id().clone();
    permissions_menu.append(&perm_microphone)?;

    Ok((
        permissions_menu,
        perm_check_id,
        perm_accessibility_id,
        perm_microphone_id,
    ))
}

/// Build the Tools submenu
pub fn build_tools_submenu() -> Result<(Submenu, MenuId, MenuId, MenuId, MenuId)> {
    let tools_menu = Submenu::new("Tools", true);

    // Voice Lab - Advanced icon (settings/lab)
    let tools_voice_lab =
        IconMenuItem::with_native_icon("Open Voice Lab", true, Some(NativeIcon::Advanced), None);
    let tools_voice_lab_id = tools_voice_lab.id().clone();
    tools_menu.append(&tools_voice_lab)?;

    // Teacher - Info icon (educational)
    let tools_teacher =
        IconMenuItem::with_native_icon("Calibration Teacher", true, Some(NativeIcon::Info), None);
    let tools_teacher_id = tools_teacher.id().clone();
    tools_menu.append(&tools_teacher)?;

    // Native Lab (Tauri) - Computer icon (native app)
    let tools_native_lab = IconMenuItem::with_native_icon(
        "Open Native Lab (Tauri)",
        true,
        Some(NativeIcon::Computer),
        None,
    );
    let tools_native_lab_id = tools_native_lab.id().clone();
    tools_menu.append(&tools_native_lab)?;

    tools_menu.append(&PredefinedMenuItem::separator())?;

    // New Conversation - Add icon (refresh/new)
    let tools_new_conversation =
        IconMenuItem::with_native_icon("New Conversation", true, Some(NativeIcon::Add), None);
    let tools_new_conversation_id = tools_new_conversation.id().clone();
    tools_menu.append(&tools_new_conversation)?;

    Ok((
        tools_menu,
        tools_voice_lab_id,
        tools_teacher_id,
        tools_native_lab_id,
        tools_new_conversation_id,
    ))
}
