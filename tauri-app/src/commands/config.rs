use crate::ipc_client::IpcClient;

use codescribe::config::{HoldMods, Language, ToggleTrigger};
use codescribe::ipc::{IpcCommand, IpcResponse};
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
pub fn get_config() -> Result<serde_json::Value, String> {
    let mut client = IpcClient::connect().map_err(|e| e.to_string())?;
    let response: IpcResponse = client
        .send(&IpcCommand::GetConfig)
        .map_err(|e| e.to_string())?;

    match response {
        IpcResponse::Config(config) => serde_json::to_value(*config).map_err(|e| e.to_string()),
        IpcResponse::Error(err) => Err(err),
        _ => Err("Unexpected IPC response for GetConfig".to_string()),
    }
}

#[tauri::command]
pub fn save_config(config: serde_json::Value) -> Result<(), String> {
    let patch: ConfigPatch = serde_json::from_value(config).map_err(|e| e.to_string())?;

    let mut client = IpcClient::connect().map_err(|e| e.to_string())?;
    let response: IpcResponse = client
        .send(&IpcCommand::GetConfig)
        .map_err(|e| e.to_string())?;
    let mut cfg = match response {
        IpcResponse::Config(config) => *config,
        IpcResponse::Error(err) => return Err(err),
        _ => return Err("Unexpected IPC response for GetConfig".to_string()),
    };

    // Apply patch to the config, then send it to CLI for persistence.
    if let Some(v) = patch.use_local_stt {
        cfg.use_local_stt = v;
    }
    if let Some(v) = patch.local_model {
        cfg.local_model = v.clone();
    }
    if let Some(v) = patch.stt_endpoint {
        // empty string means "clear"
        cfg.stt_endpoint = (!v.trim().is_empty()).then_some(v.clone());
    }
    if let Some(v) = patch.stt_api_key {
        cfg.stt_api_key = (!v.trim().is_empty()).then_some(v.clone());
    }

    if let Some(v) = patch.llm_host {
        cfg.ollama_host = v.clone();
    }
    if let Some(v) = patch.llm_server_url {
        cfg.llm_server_url = v.clone();
    }
    if let Some(v) = patch.llm_endpoint {
        cfg.llm_endpoint = (!v.trim().is_empty()).then_some(v.clone());
    }
    if let Some(v) = patch.llm_api_key {
        cfg.llm_api_key = (!v.trim().is_empty()).then_some(v.clone());
    }
    if let Some(v) = patch.ollama_host {
        cfg.ollama_host = v.clone();
    }
    if let Some(v) = patch.ollama_model {
        cfg.ollama_model = v.clone();
    }

    if let Some(v) = patch.hold_mods {
        let parsed = HoldMods::from_str(&v).map_err(|e| e.to_string())?;
        cfg.hold_mods = parsed;
    }
    if let Some(v) = patch.hold_exclusive {
        cfg.hold_exclusive = v;
    }
    if let Some(v) = patch.toggle_trigger {
        let parsed = ToggleTrigger::from_str(&v).map_err(|e| e.to_string())?;
        cfg.toggle_trigger = parsed;
    }
    if let Some(v) = patch.hold_start_delay_ms {
        cfg.hold_start_delay_ms = v;
    }

    if let Some(v) = patch.whisper_language {
        let parsed = Language::from_str(&v).map_err(|e| e.to_string())?;
        cfg.whisper_language = parsed;
    }

    if let Some(v) = patch.audio_input_device {
        cfg.audio_input_device = (!v.trim().is_empty()).then_some(v.clone());
    }

    let response: IpcResponse = client
        .send(&IpcCommand::SaveConfig {
            config: Box::new(cfg),
        })
        .map_err(|e| e.to_string())?;

    match response {
        IpcResponse::Ok => Ok(()),
        IpcResponse::Error(err) => Err(err),
        _ => Err("Unexpected IPC response for SaveConfig".to_string()),
    }
}

#[tauri::command]
pub fn get_env_var(key: String) -> Option<String> {
    std::env::var(&key).ok()
}
