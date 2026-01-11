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
    use tauri::{
        Manager,
        menu::{Menu, MenuItem},
        tray::{TrayIconBuilder, MouseButton, MouseButtonState, TrayIconEvent},
    };

    let state = state::AppState::new().expect("failed to initialize AppState");

    tauri::Builder::default()
        .manage(state)
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_fs::init())
        .setup(|app| {
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
                    } = event {
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
