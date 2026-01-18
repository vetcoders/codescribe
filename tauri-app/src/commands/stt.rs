//! STT Commands - Speech-to-Text
//!
//! These commands are DEPRECATED - use IPC commands instead.
//! CLI manages the embedded Whisper model, GUI should use:
//! - ipc_get_status() to check CLI availability
//! - Direct recording through CLI's hotkey system
//!
//! Keeping these stubs for backwards compatibility with existing UI.
//!
//! Created by M&K (c)2026 VetCoders

use crate::ipc_client::IpcClient;
use crate::state::AppState;
use codescribe::ipc::{IpcCommand, IpcResponse};
use std::path::PathBuf;

/// Transcribe audio file (DEPRECATED - use IPC)
///
/// This function forwards to the IPC server for consistent
/// Whisper + StreamPostProcess behavior.
#[tauri::command]
pub async fn transcribe_audio(
    _state: tauri::State<'_, AppState>,
    audio_path: String,
) -> Result<String, String> {
    let audio_path = PathBuf::from(&audio_path);
    if !audio_path.exists() {
        return Err(format!("Audio file not found: {}", audio_path.display()));
    }

    let mut client = IpcClient::connect().map_err(|e| e.to_string())?;
    let response: IpcResponse = client
        .send(&IpcCommand::TranscribeFile {
            path: audio_path.to_string_lossy().to_string(),
        })
        .map_err(|e| e.to_string())?;

    match response {
        IpcResponse::Message(text) => Ok(text),
        IpcResponse::Error(err) => Err(err),
        _ => Err("Unexpected IPC response for TranscribeFile".to_string()),
    }
}

/// Transcribe with streaming (DEPRECATED - use IPC)
#[tauri::command]
pub async fn transcribe_audio_streaming(
    _state: tauri::State<'_, AppState>,
    app: tauri::AppHandle,
    audio_path: String,
) -> Result<String, String> {
    use tauri::Emitter;

    let audio_path = PathBuf::from(&audio_path);
    if !audio_path.exists() {
        return Err(format!("Audio file not found: {}", audio_path.display()));
    }

    let mut client = IpcClient::connect().map_err(|e| e.to_string())?;
    let response: IpcResponse = client
        .send(&IpcCommand::TranscribeFile {
            path: audio_path.to_string_lossy().to_string(),
        })
        .map_err(|e| e.to_string())?;

    match response {
        IpcResponse::Message(text) => {
            let _ = app.emit("transcript_chunk", &text);
            let _ = app.emit("transcription_complete", &text);
            Ok(text)
        }
        IpcResponse::Error(err) => Err(err),
        _ => Err("Unexpected IPC response for TranscribeFile".to_string()),
    }
}

/// Get available models (returns embedded model info)
#[tauri::command]
pub fn get_available_models(_state: tauri::State<'_, AppState>) -> Vec<String> {
    // With embedded model, there's only one option
    if codescribe::whisper::embedded::is_embedded_available() {
        vec!["embedded (large-v3-turbo-q8)".to_string()]
    } else {
        vec!["large-v3-turbo".to_string()]
    }
}

/// Get current model name
#[tauri::command]
pub fn get_current_model(_state: tauri::State<'_, AppState>) -> String {
    if codescribe::whisper::embedded::is_embedded_available() {
        "embedded".to_string()
    } else {
        "large-v3-turbo".to_string()
    }
}
