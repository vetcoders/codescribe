use crate::state::AppState;

use cpal::traits::{DeviceTrait, HostTrait};

#[tauri::command]
pub fn list_audio_devices() -> Vec<String> {
    let host = cpal::default_host();
    let Ok(devices) = host.input_devices() else {
        return Vec::new();
    };

    devices
        .filter_map(|d| d.name().ok())
        .collect::<Vec<_>>()
}

#[tauri::command]
pub fn get_current_audio_device(_state: tauri::State<'_, AppState>) -> Option<String> {
    let host = cpal::default_host();
    host.default_input_device().and_then(|d| d.name().ok())
}
