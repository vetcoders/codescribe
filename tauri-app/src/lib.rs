#[cfg(target_arch = "wasm32")]
mod ui;

#[cfg(not(target_arch = "wasm32"))]
mod commands;

#[cfg(not(target_arch = "wasm32"))]
mod state;

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
        .invoke_handler(tauri::generate_handler![
            commands::stt::transcribe_audio,
            commands::stt::get_available_models,
            commands::stt::get_current_model,
            commands::config::get_config,
            commands::config::save_config,
            commands::config::get_env_var,
            commands::audio::list_audio_devices,
            commands::audio::get_current_audio_device,
            commands::lexicon::get_lexicon_entries,
            commands::lexicon::list_lexicon_topics,
            commands::lexicon::save_lexicon_entry,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
