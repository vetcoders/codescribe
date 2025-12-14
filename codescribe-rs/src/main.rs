//! CodeScribe - Speech-to-text tray app for macOS
//!
//! Rust frontend that communicates with Python backend (FastAPI + MLX Whisper)

mod audio;
mod backend;
mod client;
mod clipboard;
mod config;
mod controller;
mod history;
mod hotkeys;
mod launchd;
mod permissions;
mod sound;
mod tray;

use anyhow::Result;
use clap::Parser;
use crossbeam_channel::unbounded;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{error, info, warn, Level};
use tracing_subscriber::FmtSubscriber;

/// CodeScribe - Speech-to-text tray app for macOS
///
/// Hold Ctrl to record, release to transcribe.
/// Double-tap Option to toggle recording.
/// Requires Python backend running (MLX Whisper).
#[derive(Parser)]
#[command(name = "codescribe")]
#[command(version)]
#[command(author = "Loctree <contact@loctree.io>")]
#[command(about = "Speech-to-text tray app for macOS", long_about = None)]
struct Cli {
    /// Enable verbose/debug logging
    #[arg(short, long)]
    verbose: bool,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    // Initialize logging
    let log_level = if cli.verbose {
        Level::DEBUG
    } else {
        Level::INFO
    };

    // Check if we should log to file (when launched from Finder in .app bundle)
    let is_tty = std::io::IsTerminal::is_terminal(&std::io::stdout());
    let exe_path = std::env::current_exe().unwrap_or_default();
    let is_bundled = exe_path
        .to_str()
        .map(|s| s.contains("/CodeScribe.app/"))
        .unwrap_or(false);

    // When launched from .app bundle without TTY, set up file logging
    // Otherwise use normal console logging
    if !is_tty && is_bundled {
        use std::fs::OpenOptions;
        use std::io::Write;
        use std::path::PathBuf;

        let log_dir = PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| ".".to_string()))
            .join("Library/Logs");
        std::fs::create_dir_all(&log_dir).ok();
        let log_path = log_dir.join("CodeScribe.log");

        // Note: We use eprintln for logging setup because logger isn't ready yet
        // The actual logs will go to the file after init()
        eprintln!("[CodeScribe] Logging to: {}", log_path.display());

        // Create/append to log file and write startup marker
        if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(&log_path) {
            let _ = writeln!(
                file,
                "\n[{}] CodeScribe starting (from bundle)",
                chrono::Local::now().format("%Y-%m-%d %H:%M:%S")
            );
        }

        // Note: File logging with tracing requires the tracing-appender crate
        // For now, we just use stderr which will be captured by macOS logging
        // Users can view logs with: log stream --predicate 'process == "codescribe"'
        FmtSubscriber::builder()
            .with_max_level(log_level)
            .with_target(false)
            .with_ansi(false)
            .compact()
            .init();
    } else {
        // Normal console logging (development mode)
        FmtSubscriber::builder()
            .with_max_level(log_level)
            .with_target(false)
            .with_ansi(is_tty)
            .compact()
            .init();
    }

    info!("CodeScribe starting...");

    // Check and request macOS permissions (Accessibility, Microphone)
    permissions::request_all_permissions();

    // Start Python backend server
    info!("Starting Python backend server...");
    let _backend = match backend::BackendServer::start() {
        Ok(server) => {
            info!("Python backend started on port {}", server.port());
            Some(server)
        }
        Err(e) => {
            error!("Failed to start Python backend: {}", e);
            error!("Transcription will not work without the backend.");
            error!("Ensure 'uv' is installed and whisper_server.py is accessible.");
            None
        }
    };

    // Longer delay to let backend fully initialize (MLX models take time to load)
    tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;

    // Verify backend is healthy
    match client::check_health().await {
        Ok(true) => info!("Python backend health check passed"),
        Ok(false) => warn!("Python backend health check failed"),
        Err(e) => warn!("Could not verify backend health: {}", e),
    }

    // Create channel for hotkey events
    let (tx, rx) = unbounded::<hotkeys::HotkeyEvent>();

    // Get menu event receiver before starting tray
    let menu_rx = tray::menu_event_receiver().expect("Failed to initialize menu event channel");

    // Create HotkeyManager on main thread (required for macOS)
    // This uses global-hotkey which properly handles macOS threading
    info!("Initializing global hotkeys...");
    let hotkey_manager = match hotkeys::HotkeyManager::new(tx) {
        Ok(manager) => {
            info!("Global hotkeys registered successfully");
            Some(manager)
        }
        Err(e) => {
            error!("Failed to initialize hotkeys: {}", e);
            error!("Continuing in tray-only mode (hotkeys disabled)");
            None
        }
    };

    // Create shared config state for menu event handling
    let shared_config = Arc::new(RwLock::new(config::Config::load()));

    // Create controller with shared config
    let controller = Arc::new(controller::RecordingController::with_config(Arc::clone(
        &shared_config,
    )));

    // Spawn async task to handle menu events
    let config_clone = Arc::clone(&shared_config);
    tokio::spawn(async move {
        info!("Menu event loop started");
        loop {
            match menu_rx.recv() {
                Ok(event) => {
                    info!("Received menu event: {:?}", event);
                    match event {
                        tray::TrayMenuEvent::Quit => {
                            info!("Quit event received - exiting application");
                            std::process::exit(0);
                        }
                        tray::TrayMenuEvent::ToggleHotkeys => {
                            if hotkeys::are_hotkeys_enabled() {
                                info!("Disabling hotkeys");
                                hotkeys::disable_hotkeys();
                            } else {
                                info!("Enabling hotkeys");
                                hotkeys::enable_hotkeys();
                            }
                        }
                        tray::TrayMenuEvent::SetLanguage(lang) => {
                            let new_lang = match lang {
                                tray::Language::Auto => config::Language::Auto,
                                tray::Language::Polish => config::Language::Polish,
                                tray::Language::English => config::Language::English,
                            };
                            info!("Setting language to: {:?}", new_lang);
                            let mut cfg = config_clone.write().await;
                            cfg.whisper_language = new_lang;
                            if let Err(e) = cfg.save_to_env("WHISPER_LANGUAGE", new_lang.as_str()) {
                                error!("Failed to save language setting: {}", e);
                            }
                        }
                        tray::TrayMenuEvent::CopyLatestToClipboard => {
                            info!("Copy latest to clipboard requested");
                            if let Some(entry) = history::latest_entry() {
                                match std::fs::read_to_string(&entry.path) {
                                    Ok(text) => {
                                        if let Err(e) = clipboard::copy(&text) {
                                            error!("Failed to copy to clipboard: {}", e);
                                        } else {
                                            info!(
                                                "Copied latest transcript to clipboard ({} chars)",
                                                text.len()
                                            );
                                        }
                                    }
                                    Err(e) => {
                                        error!("Failed to read history entry: {}", e);
                                    }
                                }
                            } else {
                                warn!("No history entries found");
                            }
                        }
                        tray::TrayMenuEvent::OpenHistoryFolder => {
                            info!("Opening history folder");
                            history::open_history_folder();
                        }
                        tray::TrayMenuEvent::SelectHistoryEntry(index) => {
                            info!("Selecting history entry at index {}", index);
                            let entries = history::recent_entries(10);
                            if let Some(entry) = entries.get(index) {
                                match std::fs::read_to_string(&entry.path) {
                                    Ok(text) => {
                                        if let Err(e) = clipboard::copy(&text) {
                                            error!("Failed to copy to clipboard: {}", e);
                                        } else {
                                            info!(
                                                "Copied history entry {} to clipboard ({} chars)",
                                                index,
                                                text.len()
                                            );
                                        }
                                    }
                                    Err(e) => {
                                        error!("Failed to read history entry: {}", e);
                                    }
                                }
                            } else {
                                warn!("History entry at index {} not found", index);
                            }
                        }
                        tray::TrayMenuEvent::ToggleHistory => {
                            info!("Toggle history save requested (always enabled for now)");
                            // History saving is always enabled in current implementation
                        }
                        tray::TrayMenuEvent::SetWhisperModel(model) => {
                            let variant = match model {
                                tray::WhisperModel::Small => "small",
                                tray::WhisperModel::Medium => "medium",
                                tray::WhisperModel::LargeV3 => "large-v3",
                                tray::WhisperModel::LargeV3Turbo => "large-v3-turbo",
                            };
                            info!("Setting Whisper model to: {}", variant);
                            // Call backend to switch model
                            match client::set_whisper_model(variant).await {
                                Ok(()) => {
                                    info!("Whisper model switched to: {}", variant);
                                    // Update environment for next restart
                                    std::env::set_var("WHISPER_VARIANT", variant);
                                }
                                Err(e) => {
                                    error!("Failed to switch Whisper model: {}", e);
                                }
                            }
                        }
                        tray::TrayMenuEvent::OpenModelsFolder => {
                            info!("Open models folder requested (handled in tray.rs)");
                            // Action is handled directly in tray.rs handle_menu_event
                        }
                        tray::TrayMenuEvent::ToggleAiFormatting => {
                            info!("Toggle AI formatting requested");
                            // Toggle the AI formatting setting
                            let current = std::env::var("FORMAT_ENABLED")
                                .map(|v| v == "1" || v.to_lowercase() == "true")
                                .unwrap_or(false);
                            let new_value = if current { "0" } else { "1" };
                            std::env::set_var("FORMAT_ENABLED", new_value);
                            info!(
                                "AI formatting {}",
                                if new_value == "1" {
                                    "enabled"
                                } else {
                                    "disabled"
                                }
                            );
                        }
                        tray::TrayMenuEvent::SetFormattingProvider(provider) => {
                            let provider_str = match provider {
                                tray::FormattingProvider::Harmony => "harmony",
                                tray::FormattingProvider::Ollama => "ollama",
                            };
                            info!("Setting formatting provider to: {}", provider_str);
                            std::env::set_var("AI_PROVIDER", provider_str);
                        }
                        // Sound settings
                        tray::TrayMenuEvent::ToggleStartSound => {
                            let mut cfg = config_clone.write().await;
                            cfg.beep_on_start = !cfg.beep_on_start;
                            let enabled = cfg.beep_on_start;
                            if let Err(e) = cfg.save_to_env("BEEP_ON_START", if enabled { "1" } else { "0" }) {
                                error!("Failed to save beep setting: {}", e);
                            }
                            info!("Start sound {}", if enabled { "enabled" } else { "disabled" });
                        }
                        tray::TrayMenuEvent::SetSoundType(sound) => {
                            let sound_name = match sound {
                                tray::SoundType::Tink => "Tink",
                                tray::SoundType::Pop => "Pop",
                            };
                            info!("Setting sound type to: {}", sound_name);
                            std::env::set_var("SOUND_TYPE", sound_name);
                            // Play preview
                            sound::play_sound(sound_name);
                        }
                        tray::TrayMenuEvent::SetVolume => {
                            info!("Volume control requested (not yet implemented - needs UI dialog)");
                            // TODO: Implement volume slider dialog
                        }
                        // Hold hotkey settings
                        tray::TrayMenuEvent::SetHoldMods(mods) => {
                            info!("Setting hold modifiers to: {:?}", mods);
                            let mut cfg = config_clone.write().await;
                            cfg.hold_mods = mods;
                            if let Err(e) = cfg.save_to_env("HOLD_MODS", mods.as_str()) {
                                error!("Failed to save hold mods setting: {}", e);
                            }
                            // Note: Hotkey listener reconfiguration requires restart for now
                            info!("Hold modifiers changed - restart to apply");
                        }
                        tray::TrayMenuEvent::ToggleHoldExclusive => {
                            let mut cfg = config_clone.write().await;
                            cfg.hold_exclusive = !cfg.hold_exclusive;
                            let exclusive = cfg.hold_exclusive;
                            if let Err(e) = cfg.save_to_env("HOLD_EXCLUSIVE", if exclusive { "1" } else { "0" }) {
                                error!("Failed to save exclusive setting: {}", e);
                            }
                            info!("Exclusive mode {}", if exclusive { "enabled" } else { "disabled" });
                        }
                        tray::TrayMenuEvent::SetToggleTrigger(trigger) => {
                            info!("Setting toggle trigger to: {:?}", trigger);
                            let mut cfg = config_clone.write().await;
                            cfg.toggle_trigger = trigger;
                            if let Err(e) = cfg.save_to_env("TOGGLE_TRIGGER", trigger.as_str()) {
                                error!("Failed to save toggle trigger setting: {}", e);
                            }
                            // Note: Hotkey listener reconfiguration requires restart for now
                            info!("Toggle trigger changed - restart to apply");
                        }
                        // Permissions
                        tray::TrayMenuEvent::CheckPermissions => {
                            info!("Checking permissions...");
                            permissions::check_all_permissions();
                            // Note: Menu needs refresh to show updated status
                        }
                        tray::TrayMenuEvent::OpenAccessibilitySettings => {
                            info!("Open Accessibility Settings (handled in tray.rs)");
                        }
                        tray::TrayMenuEvent::OpenMicrophoneSettings => {
                            info!("Open Microphone Settings (handled in tray.rs)");
                        }
                        // Appearance
                        tray::TrayMenuEvent::ToggleStatusGlyph => {
                            let new_state = !tray::is_status_glyph_enabled();
                            info!("Toggling status glyph to: {}", if new_state { "enabled" } else { "disabled" });
                            tray::set_status_glyph_enabled(new_state);
                            // Refresh icon to apply change
                            let _ = tray::update_tray_status(tray::TrayStatus::Idle);
                        }
                        tray::TrayMenuEvent::RefreshTrayIcon => {
                            info!("Refreshing tray icon...");
                            let _ = tray::update_tray_status(tray::TrayStatus::Idle);
                        }
                        // System
                        tray::TrayMenuEvent::StartAtLogin(enabled) => {
                            info!("Start at login: {}", enabled);
                            let result = if enabled {
                                launchd::enable_login_item()
                            } else {
                                launchd::disable_login_item()
                            };

                            match result {
                                Ok(()) => {
                                    info!("Successfully {} Start at Login", if enabled { "enabled" } else { "disabled" });
                                }
                                Err(e) => {
                                    error!("Failed to {} Start at Login: {}", if enabled { "enable" } else { "disable" }, e);
                                }
                            }
                        }
                        // Note: ToggleHotkeys is handled above (line ~176)
                    }
                }
                Err(e) => {
                    error!("Menu channel closed: {}", e);
                    break;
                }
            }
        }
        info!("Menu event loop terminated");
    });

    // Spawn blocking task to handle hotkey events (rx.recv() is blocking)
    // Get runtime handle BEFORE spawning thread (must be in tokio context)
    let rt = tokio::runtime::Handle::current();
    let controller_clone = Arc::clone(&controller);
    std::thread::spawn(move || {
        info!("Hotkey event loop started");
        loop {
            // Receive hotkey event from channel (blocking)
            match rx.recv() {
                Ok(raw_event) => {
                    info!("Received hotkey event: {:?}", raw_event);
                    // Convert hotkeys::HotkeyEvent to controller::HotkeyInput
                    let controller_event = match raw_event {
                        hotkeys::HotkeyEvent::Hold { action, assistive } => {
                            let controller_action = match action {
                                hotkeys::HoldAction::Down => controller::HotkeyAction::Down,
                                hotkeys::HoldAction::Up => controller::HotkeyAction::Up,
                            };
                            controller::HotkeyInput {
                                key_type: controller::HotkeyType::Hold,
                                action: controller_action,
                                assistive,
                            }
                        }
                        hotkeys::HotkeyEvent::Toggle => controller::HotkeyInput {
                            key_type: controller::HotkeyType::Toggle,
                            action: controller::HotkeyAction::Press,
                            assistive: false,
                        },
                    };

                    // Handle the event (block_on since we're in std::thread, not tokio)
                    let result =
                        rt.block_on(controller_clone.handle_hotkey_event(controller_event));
                    if let Err(e) = result {
                        error!("Error handling hotkey event: {}", e);
                    }
                }
                Err(e) => {
                    error!("Hotkey channel closed: {}", e);
                    break;
                }
            }
        }
        info!("Hotkey event loop terminated");
    });

    // Run the tray application with hotkey manager (blocking)
    // Both tray and hotkeys run on main thread with shared event loop
    info!("Starting system tray...");
    tray::run_with_hotkeys(hotkey_manager)?;

    info!("CodeScribe shutting down...");
    Ok(())
}
