#[cfg(target_arch = "wasm32")]
mod ui;

#[cfg(not(target_arch = "wasm32"))]
mod commands;

#[cfg(not(target_arch = "wasm32"))]
mod hotkey_integration;

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
    use std::sync::Arc;
    use tauri::{
        Manager,
        menu::{Menu, MenuItem},
        tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    };

    let state = state::AppState::new().expect("failed to initialize AppState");

    // Clone state for hotkey listener (shares internal Arcs)
    let state_for_hotkeys = Arc::new(state.clone());

    tauri::Builder::default()
        .manage(state)
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_fs::init())
        .setup(move |app| {
            // Start hotkey listener in background thread
            if let Err(e) = hotkey_integration::start_hotkey_listener(
                app.handle().clone(),
                Arc::clone(&state_for_hotkeys),
            ) {
                eprintln!("Warning: Failed to start hotkey listener: {}", e);
                // Continue anyway - GUI still works without hotkeys
            }

            // Build tray menu
            let show = MenuItem::with_id(app, "show", "Show Window", true, None::<&str>)?;
            let settings = MenuItem::with_id(app, "settings", "Settings...", true, None::<&str>)?;
            let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;

            let menu = Menu::with_items(app, &[&show, &settings, &quit])?;

            // Create tray icon
            let _tray = TrayIconBuilder::new()
                .icon(app.default_window_icon().unwrap().clone())
                .tooltip("CodeScribe")
                .menu(&menu)
                .on_menu_event(|app, event| {
                    match event.id.as_ref() {
                        "show" => {
                            if let Some(window) = app.get_webview_window("main") {
                                let _ = window.show();
                                let _ = window.set_focus();
                            }
                        }
                        "settings" => {
                            if let Some(window) = app.get_webview_window("main") {
                                let _ = window.show();
                                let _ = window.set_focus();
                                // TODO: emit event to switch to Settings tab
                            }
                        }
                        "quit" => {
                            app.exit(0);
                        }
                        _ => {}
                    }
                })
                .on_tray_icon_event(|tray, event| {
                    if let TrayIconEvent::Click {
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Up,
                        ..
                    } = event
                    {
                        let app = tray.app_handle();
                        if let Some(window) = app.get_webview_window("main") {
                            let _ = window.show();
                            let _ = window.set_focus();
                        }
                    }
                })
                .build(app)?;

            Ok(())
        })
        .on_window_event(|window, event| {
            // Hide window on close instead of quitting (macOS tray app behavior)
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                window.hide().unwrap_or_default();
                api.prevent_close();
            }
        })
        .invoke_handler(tauri::generate_handler![
            commands::stt::transcribe_audio,
            commands::stt::transcribe_audio_streaming,
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
            commands::recording::start_recording,
            commands::recording::stop_recording,
            commands::recording::is_recording,
            commands::formatting::format_transcript,
            commands::formatting::reset_ai_context,
            commands::formatting::get_ai_prompt,
            commands::formatting::open_prompt_in_editor,
            commands::formatting::reset_ai_prompt,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
