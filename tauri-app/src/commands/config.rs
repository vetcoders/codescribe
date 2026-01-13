use crate::state::AppState;

use codescribe::config::{Config, HoldMods, Language, ToggleTrigger};
use serde::Deserialize;
use std::str::FromStr;

#[derive(Debug, Deserialize)]
struct ConfigPatch {
    // Backends
    use_local_stt: Option<bool>,
    local_model: Option<String>,
    stt_endpoint: Option<String>,
    stt_api_key: Option<String>,

    // LLM
    llm_host: Option<String>,
    llm_server_url: Option<String>,
    llm_endpoint: Option<String>,
    llm_api_key: Option<String>,
    ollama_host: Option<String>,
    ollama_model: Option<String>,

    // Hotkeys
    hold_mods: Option<String>,
    hold_exclusive: Option<bool>,
    toggle_trigger: Option<String>,
    hold_start_delay_ms: Option<u64>,

    // Language
    whisper_language: Option<String>,

    // Audio
    audio_input_device: Option<String>,
}

#[tauri::command]
pub fn get_config(state: tauri::State<'_, AppState>) -> Result<serde_json::Value, String> {
    let config = state
        .config
        .lock()
        .map_err(|_| "config mutex poisoned".to_string())?
        .clone();
    serde_json::to_value(config).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn save_config(
    state: tauri::State<'_, AppState>,
    config: serde_json::Value,
) -> Result<(), String> {
    let patch: ConfigPatch = serde_json::from_value(config).map_err(|e| e.to_string())?;

    // Apply patch to the in-memory config and persist to ~/.CodeScribe/.env
    let mut cfg = state
        .config
        .lock()
        .map_err(|_| "config mutex poisoned".to_string())?;

    fn persist(cfg: &Config, key: &str, val: &str) -> Result<(), String> {
        cfg.save_to_env(key, val).map_err(|e| e.to_string())
    }

    if let Some(v) = patch.use_local_stt {
        cfg.use_local_stt = v;
        persist(&cfg, "USE_LOCAL_STT", &v.to_string())?;
    }
    if let Some(v) = patch.local_model {
        cfg.local_model = v.clone();
        persist(&cfg, "LOCAL_MODEL", &v)?;
    }
    if let Some(v) = patch.stt_endpoint {
        // empty string means "clear"
        cfg.stt_endpoint = (!v.trim().is_empty()).then_some(v.clone());
        persist(&cfg, "STT_ENDPOINT", &v)?;
    }
    if let Some(v) = patch.stt_api_key {
        cfg.stt_api_key = (!v.trim().is_empty()).then_some(v.clone());
        persist(&cfg, "STT_API_KEY", &v)?;
    }

    if let Some(v) = patch.llm_host {
        cfg.ollama_host = v.clone();
        persist(&cfg, "LLM_HOST", &v)?;
    }
    if let Some(v) = patch.llm_server_url {
        cfg.llm_server_url = v.clone();
        persist(&cfg, "LLM_SERVER_URL", &v)?;
    }
    if let Some(v) = patch.llm_endpoint {
        cfg.llm_endpoint = (!v.trim().is_empty()).then_some(v.clone());
        persist(&cfg, "LLM_ENDPOINT", &v)?;
    }
    if let Some(v) = patch.llm_api_key {
        cfg.llm_api_key = (!v.trim().is_empty()).then_some(v.clone());
        persist(&cfg, "LLM_API_KEY", &v)?;
    }
    if let Some(v) = patch.ollama_host {
        cfg.ollama_host = v.clone();
        persist(&cfg, "OLLAMA_HOST", &v)?;
    }
    if let Some(v) = patch.ollama_model {
        cfg.ollama_model = v.clone();
        persist(&cfg, "LLM_MODEL", &v)?;
    }

    if let Some(v) = patch.hold_mods {
        let parsed = HoldMods::from_str(&v).map_err(|e| e.to_string())?;
        cfg.hold_mods = parsed;
        persist(&cfg, "HOLD_MODS", parsed.as_str())?;
    }
    if let Some(v) = patch.hold_exclusive {
        cfg.hold_exclusive = v;
        persist(&cfg, "HOLD_EXCLUSIVE", &v.to_string())?;
    }
    if let Some(v) = patch.toggle_trigger {
        let parsed = ToggleTrigger::from_str(&v).map_err(|e| e.to_string())?;
        cfg.toggle_trigger = parsed;
        persist(&cfg, "TOGGLE_TRIGGER", parsed.as_str())?;
    }
    if let Some(v) = patch.hold_start_delay_ms {
        cfg.hold_start_delay_ms = v;
        persist(&cfg, "HOLD_START_DELAY_MS", &v.to_string())?;
    }

    if let Some(v) = patch.whisper_language {
        let parsed = Language::from_str(&v).map_err(|e| e.to_string())?;
        cfg.whisper_language = parsed;
        persist(&cfg, "WHISPER_LANGUAGE", parsed.as_str())?;
    }

    if let Some(v) = patch.audio_input_device {
        cfg.audio_input_device = (!v.trim().is_empty()).then_some(v.clone());
        persist(&cfg, "AUDIO_INPUT_DEVICE", &v)?;
    }

    // Reload from env to ensure consistency with loader rules.
    *cfg = Config::load();
    Ok(())
}

#[tauri::command]
pub fn get_env_var(key: String) -> Option<String> {
    std::env::var(&key).ok()
}
