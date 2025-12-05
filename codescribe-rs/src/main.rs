//! CodeScribe - Speech-to-text tray app for macOS
//!
//! Rust frontend that communicates with Python backend (FastAPI + MLX Whisper)

mod audio;
mod client;
mod clipboard;
mod config;
mod controller;
mod hotkeys;
mod tray;

use anyhow::Result;
use crossbeam_channel::unbounded;
use std::sync::Arc;
use tracing::{error, info, Level};
use tracing_subscriber::FmtSubscriber;

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize logging
    FmtSubscriber::builder()
        .with_max_level(Level::INFO)
        .with_target(false)
        .compact()
        .init();

    info!("CodeScribe starting...");

    // Check if Python backend is running
    match client::check_health().await {
        Ok(true) => info!("Python backend is healthy"),
        Ok(false) => {
            info!("Python backend not responding - please start it with: ./CodeScribe start backend");
        }
        Err(e) => {
            info!("Could not reach backend: {}", e);
        }
    }

    // Create channel for hotkey events
    let (tx, rx) = unbounded::<hotkeys::HotkeyEvent>();

    // Start hotkey listener in background thread
    info!("Starting hotkey listener...");
    let required_modifiers = hotkeys::ModifierFlags::ctrl_only();
    let exclusive_mode = true;
    hotkeys::start(tx, required_modifiers, exclusive_mode)
        .map_err(|e| anyhow::anyhow!("Failed to start hotkey listener: {}", e))?;

    // Create controller
    let controller = Arc::new(controller::RecordingController::new());

    // Spawn async task to handle hotkey events
    let controller_clone = Arc::clone(&controller);
    tokio::spawn(async move {
        info!("Hotkey event loop started");
        loop {
            // Receive hotkey event from channel (blocking)
            match rx.recv() {
                Ok(raw_event) => {
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

                    // Handle the event
                    if let Err(e) = controller_clone.handle_hotkey_event(controller_event).await {
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

    // Run the tray application (blocking)
    info!("Starting system tray...");
    tray::run()?;

    info!("CodeScribe shutting down...");
    Ok(())
}
