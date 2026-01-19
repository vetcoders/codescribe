//! CodeScribe Tauri App - GUI Window
//!
//! GUI-only app (tray managed by CLI):
//! - Opens from CLI tray menu "Open GUI..." or dock click
//! - Hides on close (doesn't quit)
//! - Tabs: Voice Lab, Teacher, Settings, Assistant, Prompts
//!
//! Created by M&K (c)2026 VetCoders

#[cfg(target_arch = "wasm32")]
mod ui;

#[cfg(not(target_arch = "wasm32"))]
mod commands;

#[cfg(not(target_arch = "wasm32"))]
mod ipc_client;

#[cfg(not(target_arch = "wasm32"))]
mod state;

#[cfg(not(target_arch = "wasm32"))]
mod window;

#[cfg(target_arch = "wasm32")]
use wasm_bindgen::prelude::*;

#[cfg(target_arch = "wasm32")]
#[wasm_bindgen(start)]
pub fn start_frontend() {
    console_error_panic_hook::set_once();
    let _ = console_log::init_with_level(log::Level::Debug);
    leptos::mount::mount_to_body(ui::app::App);
}

#[cfg(not(target_arch = "wasm32"))]
pub fn run_backend() {
    let state = state::AppState::new().expect("failed to initialize AppState");

    tauri::Builder::default()
        .manage(state)
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_fs::init())
        .on_window_event(|window, event| {
            // Hide window on close instead of quitting (tray app behavior)
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                window.hide().unwrap_or_default();
                api.prevent_close();
            }
        })
        .invoke_handler(tauri::generate_handler![
            // STT commands
            commands::stt::transcribe_audio,
            commands::stt::transcribe_audio_streaming,
            commands::stt::get_available_models,
            commands::stt::get_current_model,
            // Config commands
            commands::config::get_config,
            commands::config::save_config,
            commands::config::get_env_var,
            // Audio commands
            commands::audio::list_audio_devices,
            commands::audio::get_current_audio_device,
            // Lexicon commands
            commands::lexicon::get_lexicon_entries,
            commands::lexicon::list_lexicon_topics,
            commands::lexicon::save_lexicon_entry,
            // Recording commands
            commands::recording::start_recording,
            commands::recording::stop_recording,
            commands::recording::is_recording,
            // Formatting commands
            commands::formatting::format_transcript,
            commands::formatting::reset_ai_context,
            commands::formatting::get_ai_prompt,
            commands::formatting::save_ai_prompt,
            commands::formatting::send_message,
            commands::formatting::open_prompt_in_editor,
            commands::formatting::reset_ai_prompt,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
