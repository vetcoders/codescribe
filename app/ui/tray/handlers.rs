//! Menu action handlers for tray menu events
//! Handles menu item clicks and dispatches appropriate events.

use muda::MenuId;
use std::process::Command;
use tracing::{debug, info};

use crate::config::Config;
use crate::os::clipboard;
use crate::os::permissions;
use crate::tray::state::{NOTES_MENU_ITEMS, send_menu_event};
use crate::tray::types::{MenuIds, TrayMenuEvent};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MenuRoute {
    CopyLast,
    ShowOverlay,
    ContinueOnboarding,
    OpenSettings,
    OpenHistory,
    CopyDiagnostics,
    Help,
    About,
    Quit,
    ToggleQuickNotes,
    ToggleQuickNotesSaveOnly,
    OpenNotesFolder,
    OpenTodayNote,
    OpenQualityReport,
    InstallSileroVad,
}

fn resolve_menu_route(event_id: &MenuId, menu_ids: &MenuIds) -> Option<MenuRoute> {
    if event_id == &menu_ids.copy_last {
        Some(MenuRoute::CopyLast)
    } else if event_id == &menu_ids.show_overlay {
        Some(MenuRoute::ShowOverlay)
    } else if menu_ids
        .continue_onboarding
        .as_ref()
        .is_some_and(|id| event_id == id)
    {
        Some(MenuRoute::ContinueOnboarding)
    } else if event_id == &menu_ids.open_settings {
        Some(MenuRoute::OpenSettings)
    } else if event_id == &menu_ids.open_history {
        Some(MenuRoute::OpenHistory)
    } else if event_id == &menu_ids.copy_diagnostics {
        Some(MenuRoute::CopyDiagnostics)
    } else if event_id == &menu_ids.help {
        Some(MenuRoute::Help)
    } else if event_id == &menu_ids.about {
        Some(MenuRoute::About)
    } else if event_id == &menu_ids.quit {
        Some(MenuRoute::Quit)
    } else if event_id == &menu_ids.notes_toggle_quick_notes {
        Some(MenuRoute::ToggleQuickNotes)
    } else if event_id == &menu_ids.notes_toggle_save_only {
        Some(MenuRoute::ToggleQuickNotesSaveOnly)
    } else if event_id == &menu_ids.notes_open_folder {
        Some(MenuRoute::OpenNotesFolder)
    } else if event_id == &menu_ids.notes_open_today {
        Some(MenuRoute::OpenTodayNote)
    } else if event_id == &menu_ids.quality_open_report {
        Some(MenuRoute::OpenQualityReport)
    } else if event_id == &menu_ids.silero_vad_install {
        Some(MenuRoute::InstallSileroVad)
    } else {
        None
    }
}

/// Handle menu item click and send appropriate event.
pub fn handle_menu_event(event_id: &MenuId, menu_ids: &MenuIds) {
    match resolve_menu_route(event_id, menu_ids) {
        Some(MenuRoute::CopyLast) => handle_copy_last(),
        Some(MenuRoute::ShowOverlay) => crate::ui::voice_chat::show_voice_chat_overlay(),
        Some(MenuRoute::ContinueOnboarding) => crate::ui::onboarding::show_onboarding_wizard(),
        Some(MenuRoute::OpenSettings) => crate::ui::settings::show_settings_window(),
        Some(MenuRoute::OpenHistory) => handle_open_history_folder(),
        Some(MenuRoute::CopyDiagnostics) => handle_copy_diagnostics(),
        Some(MenuRoute::Help) => handle_open_help(),
        Some(MenuRoute::About) => handle_show_about(),
        Some(MenuRoute::Quit) => send_menu_event(TrayMenuEvent::Quit),
        Some(MenuRoute::ToggleQuickNotes) => handle_toggle_quick_notes(),
        Some(MenuRoute::ToggleQuickNotesSaveOnly) => handle_toggle_quick_notes_save_only(),
        Some(MenuRoute::OpenNotesFolder) => handle_open_notes_folder(),
        Some(MenuRoute::OpenTodayNote) => handle_open_today_note(),
        Some(MenuRoute::OpenQualityReport) => handle_open_qube_report(),
        Some(MenuRoute::InstallSileroVad) => handle_install_silero_vad(),
        None => debug!("Unknown menu event id: {:?}", event_id),
    }
}

/// Copy last transcript to clipboard
fn handle_copy_last() {
    send_menu_event(TrayMenuEvent::CopyLast);

    // Get last transcript from history
    if let Some(last_entry) = crate::state::history::latest_copyable_entry() {
        if let Ok(text) = std::fs::read_to_string(&last_entry.path) {
            if let Err(e) = clipboard::set_clipboard(&text) {
                info!("Failed to copy to clipboard: {}", e);
            } else {
                info!("Copied last transcript to clipboard ({} chars)", text.len());
            }
        }
    } else {
        info!("No copyable transcript history available");
    }
}

fn handle_copy_diagnostics() {
    send_menu_event(TrayMenuEvent::CopyDiagnostics);

    let report = permissions::diagnostics_report();
    if let Err(e) = clipboard::set_clipboard(&report) {
        info!("Failed to copy diagnostics to clipboard: {}", e);
        return;
    }

    #[cfg(target_os = "macos")]
    {
        let _ = Command::new("osascript")
            .arg("-e")
            .arg(
                r#"display notification "Copied diagnostics to clipboard" with title "CodeScribe""#,
            )
            .spawn();
    }
}

fn handle_install_silero_vad() {
    send_menu_event(TrayMenuEvent::InstallSileroVad);
}

// ============================================================================
// Notes Handlers
// ============================================================================

fn handle_toggle_quick_notes() {
    let config = Config::load();
    let new_state = !config.quick_notes_enabled;

    info!(
        "Toggling Quick Notes: {}",
        if new_state { "ON" } else { "OFF" }
    );

    let _ = config.save_to_env("QUICK_NOTES_ENABLED", if new_state { "1" } else { "0" });
    send_menu_event(TrayMenuEvent::SetQuickNotesEnabled(new_state));

    NOTES_MENU_ITEMS.with(|items_cell| {
        if let Some(ref items) = *items_cell.borrow() {
            items.quick_notes_toggle.set_checked(new_state);
            // If disabled, also uncheck "save-only" in the UI (config remains on disk).
            if !new_state {
                items.quick_notes_save_only.set_checked(false);
            }
        }
    });

    #[cfg(target_os = "macos")]
    {
        let _ = Command::new("osascript")
            .arg("-e")
            .arg(format!(
                r#"display notification "Quick Notes: {}" with title "CodeScribe""#,
                if new_state { "ON" } else { "OFF" }
            ))
            .spawn();
    }
}

fn handle_toggle_quick_notes_save_only() {
    let config = Config::load();
    let enabled = config.quick_notes_enabled;
    let new_state = !config.quick_notes_save_only;

    if !enabled && new_state {
        // UX: turning "save-only" ON implies Quick Notes ON.
        let _ = config.save_to_env("QUICK_NOTES_ENABLED", "1");
        send_menu_event(TrayMenuEvent::SetQuickNotesEnabled(true));
    }

    let _ = config.save_to_env("QUICK_NOTES_SAVE_ONLY", if new_state { "1" } else { "0" });
    send_menu_event(TrayMenuEvent::SetQuickNotesSaveOnly(new_state));

    NOTES_MENU_ITEMS.with(|items_cell| {
        if let Some(ref items) = *items_cell.borrow() {
            if !enabled && new_state {
                items.quick_notes_toggle.set_checked(true);
            }
            items.quick_notes_save_only.set_checked(new_state);
        }
    });
}

fn handle_open_notes_folder() {
    crate::state::notes::open_notes_folder();
}

fn handle_open_today_note() {
    crate::state::notes::open_today_note();
}

/// Open history folder in Finder
fn handle_open_history_folder() {
    send_menu_event(TrayMenuEvent::OpenHistoryFolder);
    crate::state::history::open_history_folder();
    info!("Opening history folder");
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
            "CodeScribe v{}\n\nSpeech-to-text for macOS\n\nCreated by M&K (c)2026 VetCoders",
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

// ============================================================================
// Quality Handlers
// ============================================================================

/// Open the latest quality report in browser
fn handle_open_qube_report() {
    info!("Opening quality report...");

    if crate::qube_daemon::open_latest_report() {
        info!("Opened quality report");
    } else {
        // No report available - show notification
        info!("No quality report available");
        let _ = Command::new("osascript")
            .arg("-e")
            .arg(r#"display notification "No quality report available. Run: qube-daemon --daemon" with title "CodeScribe Quality""#)
            .spawn();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use muda::MenuId;

    fn menu_ids_for_test() -> MenuIds {
        MenuIds {
            copy_last: MenuId::new("copy-last"),
            show_overlay: MenuId::new("show-overlay"),
            open_settings: MenuId::new("open-settings"),
            continue_onboarding: Some(MenuId::new("continue-onboarding")),
            open_history: MenuId::new("open-history"),
            copy_diagnostics: MenuId::new("copy-diagnostics"),
            help: MenuId::new("help"),
            about: MenuId::new("about"),
            quit: MenuId::new("quit"),
            quality_open_report: MenuId::new("quality-open-report"),
            silero_vad_install: MenuId::new("silero-vad-install"),
            notes_toggle_quick_notes: MenuId::new("notes-toggle-quick-notes"),
            notes_toggle_save_only: MenuId::new("notes-toggle-save-only"),
            notes_open_folder: MenuId::new("notes-open-folder"),
            notes_open_today: MenuId::new("notes-open-today"),
        }
    }

    #[test]
    fn resolve_menu_route_separates_onboarding_from_settings() {
        let menu_ids = menu_ids_for_test();

        assert_eq!(
            resolve_menu_route(&menu_ids.open_settings, &menu_ids),
            Some(MenuRoute::OpenSettings)
        );
        assert_eq!(
            resolve_menu_route(
                menu_ids
                    .continue_onboarding
                    .as_ref()
                    .expect("test menu ids should include onboarding"),
                &menu_ids
            ),
            Some(MenuRoute::ContinueOnboarding)
        );
    }
}
