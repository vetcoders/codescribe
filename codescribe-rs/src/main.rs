//! CodeScribe - Speech-to-text tray app for macOS
//!
//! Rust frontend that communicates with Python backend (FastAPI + MLX Whisper)

mod audio;
mod backend;
mod client;
mod clipboard;
mod config;
mod controller;
mod hotkeys;
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
    FmtSubscriber::builder()
        .with_max_level(log_level)
        .with_target(false)
        .compact()
        .init();

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
                            info!("Toggle hotkeys requested (not yet implemented)");
                            // TODO: Wire up hotkey enable/disable
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
                            // TODO: Implement history tracking to get latest transcript
                            warn!("History feature not yet implemented");
                        }
                        _ => {
                            info!("Unhandled menu event: {:?}", event);
                        }
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
                    // Convert hotkeys::HotkeyEvent to controller::HotkeyEvent
                    let controller_event = match raw_event {
                        hotkeys::HotkeyEvent::Hold { action, assistive } => {
                            let controller_action = match action {
                                hotkeys::HoldAction::Down => controller::HotkeyAction::Down,
                                hotkeys::HoldAction::Up => controller::HotkeyAction::Up,
                            };
                            controller::HotkeyEvent {
                                key_type: controller::HotkeyType::Hold,
                                action: controller_action,
                                assistive,
                            }
                        }
                        hotkeys::HotkeyEvent::Toggle => controller::HotkeyEvent {
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
