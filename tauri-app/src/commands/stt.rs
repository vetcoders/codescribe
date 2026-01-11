use crate::state::AppState;

use codescribe::config::Language;
use codescribe::local_stt::LocalWhisperEngine;
use std::path::PathBuf;

#[tauri::command]
pub async fn transcribe_audio(
    state: tauri::State<'_, AppState>,
    audio_path: String,
) -> Result<String, String> {
    let audio_path = PathBuf::from(audio_path);
    if !audio_path.exists() {
        return Err(format!("Audio file not found: {}", audio_path.display()));
    }

    let cfg = state
        .config
        .lock()
        .map_err(|_| "config mutex poisoned".to_string())?
        .clone();

    if !cfg.use_local_stt {
        return Err("USE_LOCAL_STT=false (lokalne STT jest wyłączone)".to_string());
    }

    let model_name = cfg.local_model.clone();
    let model_path = state.model_manager.get_model_path(&model_name);
    if !model_path.exists() {
        return Err(format!(
            "Model '{}' not found at {}",
            model_name,
            model_path.display()
        ));
    }

    let lang: Option<&str> = match cfg.whisper_language {
        Language::Auto => None,
        Language::Polish => Some("pl"),
        Language::English => Some("en"),
    };

    let stt_ptr = state.stt.clone();
    let model_name2 = model_name.clone();
    let model_path2 = model_path.clone();

    let handle = tauri::async_runtime::spawn_blocking(move || {
        let mut stt = stt_ptr.lock().map_err(|_| "stt mutex poisoned".to_string())?;

        let need_reload = stt
            .loaded_model
            .as_ref()
            .map(|m| m != &model_name2)
            .unwrap_or(true);

        if need_reload {
            stt.engine = Some(LocalWhisperEngine::new(&model_path2).map_err(|e| e.to_string())?);
            stt.loaded_model = Some(model_name2);
        }

        let engine = stt.engine.as_mut().ok_or_else(|| "engine missing".to_string())?;
        engine
            .transcribe_file_with_language(&audio_path, lang)
            .map_err(|e| e.to_string())
    });

    handle.await.map_err(|e| e.to_string())?
}

#[tauri::command]
pub fn get_available_models(state: tauri::State<'_, AppState>) -> Vec<String> {
    match state.model_manager.list_models() {
        Ok(v) if !v.is_empty() => v,
        _ => vec![
            "tiny".to_string(),
            "base".to_string(),
            "small".to_string(),
            "medium".to_string(),
            "large-v3".to_string(),
        ],
    }
}

#[tauri::command]
pub fn get_current_model(state: tauri::State<'_, AppState>) -> String {
    state
        .config
        .lock()
        .ok()
        .map(|c| c.local_model.clone())
        .unwrap_or_else(|| "large-v3".to_string())
}
