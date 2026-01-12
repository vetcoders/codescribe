//! Menu action handlers for tray menu events
//!
//! Handles menu item clicks and dispatches appropriate events.

use muda::MenuId;
use std::process::Command;
use tracing::{debug, info};

use crate::tray::menu::toggle_ai_formatting;
use crate::tray::state::send_menu_event;
use crate::tray::types::{MenuIds, TrayMenuEvent};

/// Handle menu item click and send appropriate event
pub fn handle_menu_event(event_id: &MenuId, menu_ids: &MenuIds) {
    if event_id == &menu_ids.ai_formatting {
        handle_toggle_ai_formatting();
    } else if event_id == &menu_ids.copy_last {
        handle_copy_last();
    } else if event_id == &menu_ids.settings {
        handle_open_settings();
    } else if event_id == &menu_ids.help {
        handle_open_help();
    } else if event_id == &menu_ids.about {
        handle_show_about();
    } else if event_id == &menu_ids.quit {
        send_menu_event(TrayMenuEvent::Quit);
    } else {
        debug!("Unknown menu event id: {:?}", event_id);
    }
}

/// Toggle AI Formatting state
fn handle_toggle_ai_formatting() {
    let new_state = toggle_ai_formatting();
    info!("AI Formatting toggled: {}", if new_state { "ON" } else { "OFF" });
}

/// Copy last transcript to clipboard
fn handle_copy_last() {
    send_menu_event(TrayMenuEvent::CopyLast);

    // Get last transcript from history
    if let Some(last_entry) = crate::history::latest_entry() {
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
// Handler Helper Functions
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

        // Try $EDITOR, $VISUAL, then common editors
        let editor = std::env::var("EDITOR")
            .or_else(|_| std::env::var("VISUAL"))
            .unwrap_or_else(|_| {
                for editor in &["code", "nvim", "vim", "nano"] {
                    if Command::new("which")
                        .arg(editor)
                        .output()
                        .map(|o| o.status.success())
                        .unwrap_or(false)
                    {
                        return editor.to_string();
                    }
                }
                "open".to_string() // macOS default
            });

        info!("Opening settings in: {}", editor);
        let _ = Command::new(&editor).arg(&config_path).spawn();
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
