//! Menu action handlers for tray menu events
//!
//! Handles menu item clicks and dispatches appropriate events.

use muda::MenuId;
use std::process::Command;
use tracing::{debug, info};

use crate::config::{Config, HoldMods, ToggleTrigger};
use crate::tray::menu::toggle_ai_formatting;
use crate::tray::state::{HOLD_MENU_ITEMS, TOGGLE_MENU_ITEMS, send_menu_event};
use crate::tray::types::{MenuIds, TrayMenuEvent};

/// Handle menu item click and send appropriate event
pub fn handle_menu_event(event_id: &MenuId, menu_ids: &MenuIds) {
    // Top-level items
    if event_id == &menu_ids.ai_formatting {
        handle_toggle_ai_formatting();
    } else if event_id == &menu_ids.copy_last {
        handle_copy_last();
    } else if event_id == &menu_ids.format_last {
        handle_format_last();
    } else if event_id == &menu_ids.format_last_five {
        handle_format_last_five();
    } else if event_id == &menu_ids.settings_edit_config {
        handle_open_settings();
    } else if event_id == &menu_ids.settings_edit_prompt {
        handle_edit_prompt();
    } else if event_id == &menu_ids.settings_open_prompt_folder {
        handle_open_prompts_folder();
    } else if event_id == &menu_ids.settings_reset_context {
        handle_reset_context();
    } else if event_id == &menu_ids.help {
        handle_open_help();
    } else if event_id == &menu_ids.about {
        handle_show_about();
    } else if event_id == &menu_ids.quit {
        send_menu_event(TrayMenuEvent::Quit);
    }
    // Hold Hotkeys submenu
    else if event_id == &menu_ids.hold_ctrl {
        handle_set_hold_mods(HoldMods::Ctrl);
    } else if event_id == &menu_ids.hold_ctrl_opt {
        handle_set_hold_mods(HoldMods::CtrlAlt);
    } else if event_id == &menu_ids.hold_ctrl_shift {
        handle_set_hold_mods(HoldMods::CtrlShift);
    } else if event_id == &menu_ids.hold_ctrl_cmd {
        handle_set_hold_mods(HoldMods::CtrlCmd);
    } else if event_id == &menu_ids.hold_exclusive {
        handle_toggle_hold_exclusive();
    }
    // Toggle trigger submenu
    else if event_id == &menu_ids.toggle_double_opt {
        handle_set_toggle_trigger(ToggleTrigger::DoubleOption);
    } else if event_id == &menu_ids.toggle_double_ralt {
        handle_set_toggle_trigger(ToggleTrigger::DoubleRightOption);
    } else if event_id == &menu_ids.toggle_disabled {
        handle_set_toggle_trigger(ToggleTrigger::None);
    }
    // History submenu
    else if event_id == &menu_ids.history_save {
        handle_toggle_history();
    } else if event_id == &menu_ids.keep_audio {
        handle_toggle_keep_audio();
    } else if event_id == &menu_ids.history_copy_latest {
        handle_copy_latest_to_clipboard();
    } else if event_id == &menu_ids.history_open_folder {
        handle_open_history_folder();
    }
    // Settings - Open GUI
    else if event_id == &menu_ids.settings_open_gui {
        open_gui();
    }
    // Quality - Open Report
    else if event_id == &menu_ids.quality_open_report {
        handle_open_quality_report();
    } else {
        debug!("Unknown menu event id: {:?}", event_id);
    }
}

/// Toggle AI Formatting state
fn handle_toggle_ai_formatting() {
    let new_state = toggle_ai_formatting();
    info!(
        "AI Formatting toggled: {}",
        if new_state { "ON" } else { "OFF" }
    );
}

/// Copy last transcript to clipboard
fn handle_copy_last() {
    send_menu_event(TrayMenuEvent::CopyLast);

    // Get last transcript from history
    if let Some(last_entry) = crate::state::history::latest_entry() {
        if let Ok(text) = std::fs::read_to_string(&last_entry.path) {
            if let Err(e) = crate::clipboard::set_clipboard(&text) {
                info!("Failed to copy to clipboard: {}", e);
            } else {
                info!("Copied last transcript to clipboard ({} chars)", text.len());
            }
        }
    } else {
        info!("No transcript history available");
    }
}

// ============================================================================
// Hold Hotkeys Handlers
// ============================================================================

/// Set hold modifier keys and update menu checkmarks
fn handle_set_hold_mods(mods: HoldMods) {
    info!("Setting hold mods to: {:?}", mods);
    send_menu_event(TrayMenuEvent::SetHoldMods(mods));

    // Update menu checkmarks (radio behavior)
    HOLD_MENU_ITEMS.with(|items_cell| {
        if let Some(ref items) = *items_cell.borrow() {
            items.ctrl.set_checked(mods == HoldMods::Ctrl);
            items.ctrl_opt.set_checked(mods == HoldMods::CtrlAlt);
            items.ctrl_shift.set_checked(mods == HoldMods::CtrlShift);
            items.ctrl_cmd.set_checked(mods == HoldMods::CtrlCmd);
            items.label.set_text(format!("Current: {}", mods.label()));
        }
    });

    // Persist to config
    let config = Config::load();
    let _ = config.save_to_env("HOLD_MODS", mods.as_str());
}

/// Toggle hold exclusive mode
fn handle_toggle_hold_exclusive() {
    send_menu_event(TrayMenuEvent::ToggleHoldExclusive);

    let config = Config::load();
    let new_state = !config.hold_exclusive;
    let _ = config.save_to_env("HOLD_EXCLUSIVE", if new_state { "1" } else { "0" });
    info!(
        "Hold exclusive toggled: {}",
        if new_state { "ON" } else { "OFF" }
    );
}

/// Set toggle trigger and update menu checkmarks
fn handle_set_toggle_trigger(trigger: ToggleTrigger) {
    info!("Setting toggle trigger to: {:?}", trigger);
    send_menu_event(TrayMenuEvent::SetToggleTrigger(trigger));

    // Update menu checkmarks (radio behavior)
    TOGGLE_MENU_ITEMS.with(|items_cell| {
        if let Some(ref items) = *items_cell.borrow() {
            items
                .double_opt
                .set_checked(trigger == ToggleTrigger::DoubleOption);
            items
                .double_ralt
                .set_checked(trigger == ToggleTrigger::DoubleRightOption);
            items.disabled.set_checked(trigger == ToggleTrigger::None);
            items.label.set_text(format!("Toggle: {}", trigger.label()));
        }
    });

    // Persist to config
    let config = Config::load();
    let _ = config.save_to_env("TOGGLE_TRIGGER", trigger.as_str());
}

// ============================================================================
// History Handlers
// ============================================================================

/// Toggle history saving
fn handle_toggle_history() {
    send_menu_event(TrayMenuEvent::ToggleHistory);

    let config = Config::load();
    let new_state = !config.history_enabled;
    let _ = config.save_to_env("HISTORY_ENABLED", if new_state { "1" } else { "0" });
    info!(
        "History saving toggled: {}",
        if new_state { "ON" } else { "OFF" }
    );
}

/// Toggle audio dump (keep audio files)
fn handle_toggle_keep_audio() {
    let config = Config::load();
    let new_state = !config.dump_audio_logs;
    let _ = config.save_to_env("DUMP_AUDIO_LOGS", if new_state { "1" } else { "0" });
    info!(
        "Keep Audio toggled: {}",
        if new_state { "ON" } else { "OFF" }
    );
}

/// Copy latest transcript to clipboard
fn handle_copy_latest_to_clipboard() {
    send_menu_event(TrayMenuEvent::CopyLatestToClipboard);

    if let Some(last_entry) = crate::state::history::latest_entry() {
        if let Ok(text) = std::fs::read_to_string(&last_entry.path) {
            if let Err(e) = crate::clipboard::set_clipboard(&text) {
                info!("Failed to copy to clipboard: {}", e);
            } else {
                info!(
                    "Copied latest transcript to clipboard ({} chars)",
                    text.len()
                );
            }
        }
    } else {
        info!("No transcript history available");
    }
}

/// Open history folder in Finder
fn handle_open_history_folder() {
    send_menu_event(TrayMenuEvent::OpenHistoryFolder);
    crate::state::history::open_history_folder();
    info!("Opening history folder");
}

// ============================================================================
// Settings Handlers
// ============================================================================

/// Open settings file in $EDITOR
fn handle_open_settings() {
    send_menu_event(TrayMenuEvent::OpenSettings);

    #[cfg(target_os = "macos")]
    {
        let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
        let config_path = format!("{}/.codescribe/.env", home);

        // Ensure directory exists
        let config_dir = format!("{}/.codescribe", home);
        let _ = std::fs::create_dir_all(&config_dir);

        // Create default config if missing
        if !std::path::Path::new(&config_path).exists() {
            let default_config = r#"# CodeScribe Configuration
# Created by: codescribe Settings menu

# === STT (Speech-to-Text) ===
STT_ENDPOINT=https://api.libraxis.cloud/v1/audio/transcriptions
STT_API_KEY=your-api-key-here
WHISPER_MODEL=mlx-community/whisper-large-v3-mlx
WHISPER_LANGUAGE=en

# === LLM (AI Formatting) ===
LLM_ENDPOINT=https://api.libraxis.cloud/v1/responses
LLM_API_KEY=your-api-key-here
LLM_MODEL=gpt-oss-120b-mlx
AI_FORMATTING_ENABLED=1

# === Hotkeys ===
HOLD_MODS=ctrl
TOGGLE_TRIGGER=double_option

# === Audio ===
SOUND_VOLUME=0.25

# === Logging ===
LOG_LEVEL=INFO
"#;
            let _ = std::fs::write(&config_path, default_config);
            info!("Created default config: {}", config_path);
        }

        // Use macOS `open -t` for default text editor (works without TTY)
        // Falls back to TextEdit if no default is set
        info!("Opening settings with default text editor: {}", config_path);
        let _ = Command::new("open").arg("-t").arg(&config_path).spawn();
    }
}

/// Open help documentation in browser
fn handle_open_help() {
    send_menu_event(TrayMenuEvent::OpenHelp);

    #[cfg(target_os = "macos")]
    {
        // Try local docs first, fall back to GitHub
        let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
        let local_docs = format!("{}/.codescribe/docs/README.md", home);

        let url = if std::path::Path::new(&local_docs).exists() {
            local_docs
        } else {
            "https://github.com/VetCoders/CodeScribe#readme".to_string()
        };

        info!("Opening help: {}", url);
        let _ = Command::new("open").arg(&url).spawn();
    }
}

/// Show about dialog with version
fn handle_show_about() {
    send_menu_event(TrayMenuEvent::ShowAbout);

    #[cfg(target_os = "macos")]
    {
        let version = env!("CARGO_PKG_VERSION");
        let message = format!(
            "CodeScribe v{}\\n\\nSpeech-to-text for macOS\\n\\nCreated by M&K (c)2026 VetCoders",
            version
        );

        // Use osascript for native dialog
        let script = format!(
            r#"display dialog "{}" buttons {{"OK"}} default button "OK" with title "About CodeScribe" with icon note"#,
            message
        );

        info!("Showing about dialog");
        let _ = Command::new("osascript").arg("-e").arg(&script).spawn();
    }
}

/// Open prompt files for editing
fn handle_edit_prompt() {
    info!("Opening prompt files for editing...");
    crate::config::prompts::open_prompt_file("formatting.txt");
}

/// Reset conversation context
fn handle_reset_context() {
    crate::state::conversation::reset_conversation();
    crate::ai_formatting::reset_ollama_memory();
    info!("Conversation context reset");
}

/// Format last transcript (async in new thread)
fn handle_format_last() {
    info!("Formatting last transcript...");

    std::thread::spawn(|| {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();

        rt.block_on(async {
            if let Some(last_entry) = crate::state::history::latest_entry() {
                if let Ok(text) = std::fs::read_to_string(&last_entry.path) {
                    let _ = crate::tray::update_tray_status(crate::tray::TrayStatus::Thinking);
                    let result =
                        crate::ai_formatting::format_text_with_status(&text, None, false).await;
                    let kind = match result.status {
                        crate::ai_formatting::AiFormatStatus::Applied => {
                            crate::state::history::TranscriptKind::Ai
                        }
                        crate::ai_formatting::AiFormatStatus::Failed => {
                            crate::state::history::TranscriptKind::AiFailed
                        }
                        crate::ai_formatting::AiFormatStatus::Skipped => {
                            crate::state::history::TranscriptKind::Raw
                        }
                    };
                    // Zapisujemy jako nowy wpis, pozostawiając oryginalny raw w historii
                    crate::state::history::save_entry_with_kind(&result.text, kind);
                    let _ = crate::clipboard::set_clipboard(&result.text);

                    let _ = crate::tray::update_tray_status(crate::tray::TrayStatus::Success);
                    tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                    let _ = crate::tray::update_tray_status(crate::tray::TrayStatus::Idle);
                }
            } else {
                info!("No transcript to format");
            }
        });
    });
}

/// Format last 5 transcripts (async batch)
fn handle_format_last_five() {
    info!("Formatting last 5 transcripts...");

    std::thread::spawn(|| {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();

        rt.block_on(async {
            let entries = crate::state::history::recent_entries(5);
            if entries.is_empty() {
                info!("No transcripts to format");
                return;
            }

            let _ = crate::tray::update_tray_status(crate::tray::TrayStatus::Thinking);

            let mut last_formatted: Option<String> = None;

            for entry in entries {
                if let Ok(text) = std::fs::read_to_string(&entry.path) {
                    let result =
                        crate::ai_formatting::format_text_with_status(&text, None, false).await;
                    let kind = match result.status {
                        crate::ai_formatting::AiFormatStatus::Applied => {
                            crate::state::history::TranscriptKind::Ai
                        }
                        crate::ai_formatting::AiFormatStatus::Failed => {
                            crate::state::history::TranscriptKind::AiFailed
                        }
                        crate::ai_formatting::AiFormatStatus::Skipped => {
                            crate::state::history::TranscriptKind::Raw
                        }
                    };
                    // Zapisujemy jako nowy wpis, raw pozostaje w historii
                    crate::state::history::save_entry_with_kind(&result.text, kind);
                    last_formatted = Some(result.text);
                }
            }

            if let Some(formatted) = last_formatted {
                let _ = crate::clipboard::set_clipboard(&formatted);
            }

            let _ = crate::tray::update_tray_status(crate::tray::TrayStatus::Success);
            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
            let _ = crate::tray::update_tray_status(crate::tray::TrayStatus::Idle);
        });
    });
}

/// Open prompts folder
fn handle_open_prompts_folder() {
    crate::config::prompts::open_prompts_folder();
    info!("Opened prompts folder");
}

/// Open GUI (Tauri window)
/// Public so it can be called from dock click handler
pub fn open_gui() {
    info!("Opening GUI...");

    if activate_running_gui() {
        info!("Activated running GUI instance");
        return;
    }

    if open_gui_app_bundle() {
        info!("Opened GUI app bundle");
        return;
    }

    // Try to launch codescribe-gui binary
    // First check if it exists in same directory as codescribe
    let gui_binary = if let Ok(exe_path) = std::env::current_exe() {
        let parent = exe_path.parent().unwrap_or(std::path::Path::new("."));
        let gui_path = parent.join("codescribe-gui");
        if gui_path.exists() {
            gui_path
        } else {
            // Fallback to PATH lookup
            std::path::PathBuf::from("codescribe-gui")
        }
    } else {
        std::path::PathBuf::from("codescribe-gui")
    };

    match Command::new(&gui_binary).spawn() {
        Ok(_) => {
            info!("Launched GUI: {}", gui_binary.display());
        }
        Err(e) => {
            // If codescribe-gui not found, show notification
            info!("Failed to launch GUI: {} - {}", gui_binary.display(), e);

            // Show macOS notification about missing GUI
            let _ = Command::new("osascript")
                .arg("-e")
                .arg(r#"display notification "GUI binary not found. Build with: cd tauri-app && cargo tauri build" with title "CodeScribe""#)
                .spawn();
        }
    }
}

fn activate_running_gui() -> bool {
    Command::new("osascript")
        .arg("-e")
        .arg(r#"tell application "CodeScribe" to activate"#)
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

fn open_gui_app_bundle() -> bool {
    // Try direct path first (more reliable)
    let app_path = "/Applications/CodeScribe.app";
    if std::path::Path::new(app_path).exists() {
        info!("Found app bundle at {}", app_path);
        return Command::new("open")
            .arg(app_path)
            .status()
            .map(|status| {
                info!("open {} returned: {}", app_path, status.success());
                status.success()
            })
            .unwrap_or(false);
    }

    // Fallback to -a flag
    info!("App bundle not in /Applications, trying open -a");
    Command::new("open")
        .arg("-a")
        .arg("CodeScribe")
        .status()
        .map(|status| {
            info!("open -a CodeScribe returned: {}", status.success());
            status.success()
        })
        .unwrap_or(false)
}

// ============================================================================
// Quality Handlers
// ============================================================================

/// Open the latest quality report in browser
fn handle_open_quality_report() {
    info!("Opening quality report...");

    if crate::quality_loop::open_latest_report() {
        info!("Opened quality report");
    } else {
        // No report available - show notification
        info!("No quality report available");
        let _ = Command::new("osascript")
            .arg("-e")
            .arg(r#"display notification "No quality report available. Run: codescribe-loop --daemon" with title "CodeScribe Quality""#)
            .spawn();
    }
}
