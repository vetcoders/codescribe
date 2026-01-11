//! Menu action handlers for tray menu events
//!
//! Handles menu item clicks and dispatches appropriate events.

use muda::MenuId;
use tracing::{debug, info};

use crate::tray::state::{
    apply_hold_mods_selection, apply_model_selection, apply_toggle_trigger_selection,
    send_menu_event,
};
use crate::tray::types::{
    FormattingProvider, HoldMods, Language, MenuIds, SoundType, ToggleTrigger, TrayMenuEvent,
    VolumeLevel, WhisperModel,
};

/// Handle menu item click and send appropriate event
pub fn handle_menu_event(event_id: &MenuId, menu_ids: &MenuIds) {
    // Top-level actions
    if event_id == &menu_ids.enable_hotkeys {
        send_menu_event(TrayMenuEvent::ToggleHotkeys);
    } else if event_id == &menu_ids.start_at_login {
        let current = crate::launchd::is_login_item_enabled();
        send_menu_event(TrayMenuEvent::StartAtLogin(!current));
    } else if event_id == &menu_ids.quit {
        send_menu_event(TrayMenuEvent::Quit);
    }
    // Language submenu
    else if event_id == &menu_ids.lang_auto {
        send_menu_event(TrayMenuEvent::SetLanguage(Language::Auto));
    } else if event_id == &menu_ids.lang_polish {
        send_menu_event(TrayMenuEvent::SetLanguage(Language::Polish));
    } else if event_id == &menu_ids.lang_english {
        send_menu_event(TrayMenuEvent::SetLanguage(Language::English));
    }
    // Models submenu
    else if event_id == &menu_ids.model_small {
        apply_model_selection("small");
        send_menu_event(TrayMenuEvent::SetWhisperModel(WhisperModel::Small));
    } else if event_id == &menu_ids.model_medium {
        apply_model_selection("medium");
        send_menu_event(TrayMenuEvent::SetWhisperModel(WhisperModel::Medium));
    } else if event_id == &menu_ids.model_large_v3 {
        apply_model_selection("large-v3");
        send_menu_event(TrayMenuEvent::SetWhisperModel(WhisperModel::LargeV3));
    } else if event_id == &menu_ids.model_large_v3_turbo {
        apply_model_selection("large-v3-turbo");
        send_menu_event(TrayMenuEvent::SetWhisperModel(WhisperModel::LargeV3Turbo));
    } else if event_id == &menu_ids.model_large_v3_q8 {
        apply_model_selection("large-v3-mlx-q8");
        send_menu_event(TrayMenuEvent::SetWhisperModel(WhisperModel::LargeV3Q8));
    } else if event_id == &menu_ids.model_open_folder {
        handle_open_models_folder();
    }
    // Formatting submenu
    else if event_id == &menu_ids.fmt_toggle {
        send_menu_event(TrayMenuEvent::ToggleAiFormatting);
    } else if event_id == &menu_ids.fmt_harmony {
        send_menu_event(TrayMenuEvent::SetFormattingProvider(
            FormattingProvider::Harmony,
        ));
    } else if event_id == &menu_ids.fmt_ollama {
        send_menu_event(TrayMenuEvent::SetFormattingProvider(
            FormattingProvider::Ollama,
        ));
    }
    // Hold Hotkeys submenu
    else if event_id == &menu_ids.hold_ctrl {
        apply_hold_mods_selection(HoldMods::Ctrl);
        send_menu_event(TrayMenuEvent::SetHoldMods(HoldMods::Ctrl));
    } else if event_id == &menu_ids.hold_ctrl_opt {
        apply_hold_mods_selection(HoldMods::CtrlAlt);
        send_menu_event(TrayMenuEvent::SetHoldMods(HoldMods::CtrlAlt));
    } else if event_id == &menu_ids.hold_ctrl_shift {
        apply_hold_mods_selection(HoldMods::CtrlShift);
        send_menu_event(TrayMenuEvent::SetHoldMods(HoldMods::CtrlShift));
    } else if event_id == &menu_ids.hold_ctrl_cmd {
        apply_hold_mods_selection(HoldMods::CtrlCmd);
        send_menu_event(TrayMenuEvent::SetHoldMods(HoldMods::CtrlCmd));
    } else if event_id == &menu_ids.hold_exclusive {
        send_menu_event(TrayMenuEvent::ToggleHoldExclusive);
    } else if event_id == &menu_ids.toggle_double_opt {
        apply_toggle_trigger_selection(ToggleTrigger::DoubleOption);
        send_menu_event(TrayMenuEvent::SetToggleTrigger(ToggleTrigger::DoubleOption));
    } else if event_id == &menu_ids.toggle_double_ralt {
        apply_toggle_trigger_selection(ToggleTrigger::DoubleRightOption);
        send_menu_event(TrayMenuEvent::SetToggleTrigger(
            ToggleTrigger::DoubleRightOption,
        ));
    } else if event_id == &menu_ids.toggle_disabled {
        apply_toggle_trigger_selection(ToggleTrigger::None);
        send_menu_event(TrayMenuEvent::SetToggleTrigger(ToggleTrigger::None));
    }
    // History submenu
    else if event_id == &menu_ids.history_save {
        send_menu_event(TrayMenuEvent::ToggleHistory);
    } else if event_id == &menu_ids.history_copy_latest {
        send_menu_event(TrayMenuEvent::CopyLatestToClipboard);
    } else if event_id == &menu_ids.history_open_folder {
        send_menu_event(TrayMenuEvent::OpenHistoryFolder);
    }
    // Appearance submenu
    else if event_id == &menu_ids.appearance_glyph {
        send_menu_event(TrayMenuEvent::ToggleStatusGlyph);
    } else if event_id == &menu_ids.appearance_refresh {
        send_menu_event(TrayMenuEvent::RefreshTrayIcon);
    }
    // Feedback submenu
    else if event_id == &menu_ids.feedback_start_sound {
        send_menu_event(TrayMenuEvent::ToggleStartSound);
    } else if event_id == &menu_ids.feedback_sound_tink {
        send_menu_event(TrayMenuEvent::SetSoundType(SoundType::Tink));
    } else if event_id == &menu_ids.feedback_sound_pop {
        send_menu_event(TrayMenuEvent::SetSoundType(SoundType::Pop));
    }
    // Volume submenu
    else if event_id == &menu_ids.volume_mute {
        send_menu_event(TrayMenuEvent::SetVolume(VolumeLevel::Mute));
    } else if event_id == &menu_ids.volume_low {
        send_menu_event(TrayMenuEvent::SetVolume(VolumeLevel::Low));
    } else if event_id == &menu_ids.volume_medium {
        send_menu_event(TrayMenuEvent::SetVolume(VolumeLevel::Medium));
    } else if event_id == &menu_ids.volume_high {
        send_menu_event(TrayMenuEvent::SetVolume(VolumeLevel::High));
    } else if event_id == &menu_ids.volume_full {
        send_menu_event(TrayMenuEvent::SetVolume(VolumeLevel::Full));
    }
    // Permissions submenu
    else if event_id == &menu_ids.perm_check {
        send_menu_event(TrayMenuEvent::CheckPermissions);
        crate::permissions::check_all_permissions();
    } else if event_id == &menu_ids.perm_accessibility {
        handle_open_accessibility_settings();
    } else if event_id == &menu_ids.perm_microphone {
        handle_open_microphone_settings();
    }
    // Tools submenu
    else if event_id == &menu_ids.tools_voice_lab {
        handle_open_voice_lab();
    } else if event_id == &menu_ids.tools_teacher {
        handle_open_teacher();
    } else if event_id == &menu_ids.tools_native_lab {
        handle_open_native_lab();
    } else if event_id == &menu_ids.tools_new_conversation {
        codescribe::conversation::reset_conversation();
        send_menu_event(TrayMenuEvent::NewConversation);
        info!("New conversation started - context reset");
    } else {
        debug!("Unknown menu event id: {:?}", event_id);
    }
}

// ============================================================================
// Handler Helper Functions
// ============================================================================

/// Open the models folder in Finder
fn handle_open_models_folder() {
    send_menu_event(TrayMenuEvent::OpenModelsFolder);
    #[cfg(target_os = "macos")]
    {
        use std::process::Command;
        if let Ok(home) = std::env::var("HOME") {
            let models_path = format!("{}/.CodeScribe/models", home);
            let _ = std::fs::create_dir_all(&models_path);
            let _ = Command::new("open").arg(&models_path).spawn();
        }
    }
}

/// Open Accessibility settings in System Preferences
fn handle_open_accessibility_settings() {
    send_menu_event(TrayMenuEvent::OpenAccessibilitySettings);
    #[cfg(target_os = "macos")]
    {
        use std::process::Command;
        let _ = Command::new("open")
            .arg("x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility")
            .spawn();
    }
}

/// Open Microphone settings in System Preferences
fn handle_open_microphone_settings() {
    send_menu_event(TrayMenuEvent::OpenMicrophoneSettings);
    #[cfg(target_os = "macos")]
    {
        use std::process::Command;
        let _ = Command::new("open")
            .arg("x-apple.systempreferences:com.apple.preference.security?Privacy_Microphone")
            .spawn();
    }
}

/// Open Voice Lab in browser (starts local Rust server if needed)
fn handle_open_voice_lab() {
    send_menu_event(TrayMenuEvent::OpenVoiceLab);
    #[cfg(target_os = "macos")]
    {
        use std::process::Command;
        // Start lab server (no-op if already running)
        crate::lab_server::start_lab_server();
        let lab_url = crate::lab_server::lab_url();
        info!("Opening Voice Lab: {}", lab_url);
        let _ = Command::new("open").arg(&lab_url).spawn();
    }
}

/// Open Teacher/Calibration in browser
fn handle_open_teacher() {
    send_menu_event(TrayMenuEvent::OpenTeacher);
    #[cfg(target_os = "macos")]
    {
        use std::process::Command;
        // Start lab server (no-op if already running)
        crate::lab_server::start_lab_server();
        let teacher_url = format!("{}#calibrate", crate::lab_server::lab_url());
        info!("Opening Calibration Teacher: {}", teacher_url);
        let _ = Command::new("open").arg(&teacher_url).spawn();
    }
}

/// Open Native Lab (Tauri app)
fn handle_open_native_lab() {
    send_menu_event(TrayMenuEvent::OpenNativeLab);
    #[cfg(target_os = "macos")]
    {
        use std::process::Command;

        // Try to find codescribe-app binary in common locations
        let binary_name = "codescribe-app";
        let possible_paths = [
            // Installed app in /Applications
            "/Applications/CodeScribe.app/Contents/MacOS/codescribe-app",
            // Development build (release)
            concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/tauri-app/target/release/codescribe-app"
            ),
            // Development build (debug)
            concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/tauri-app/target/debug/codescribe-app"
            ),
        ];

        // First check known paths
        for path in &possible_paths {
            if std::path::Path::new(path).exists() {
                info!("Launching Native Lab from: {}", path);
                match Command::new(path).spawn() {
                    Ok(_) => return,
                    Err(e) => {
                        debug!("Failed to launch from {}: {}", path, e);
                    }
                }
            }
        }

        // Fall back to PATH lookup
        match Command::new(binary_name).spawn() {
            Ok(_) => {
                info!("Launched Native Lab via PATH: {}", binary_name);
            }
            Err(e) => {
                info!(
                    "Native Lab binary '{}' not found. Build it with: cd tauri-app && cargo tauri build. Error: {}",
                    binary_name, e
                );
            }
        }
    }
}
