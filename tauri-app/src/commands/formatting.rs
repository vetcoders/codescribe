//! AI formatting commands for Tauri IPC
//!
//! Provides commands for text formatting, prompt management, and AI context reset.
//!
//! Created by M&K (c)2026 VetCoders

use crate::ipc_client::IpcClient;
use codescribe_core::ipc::{IpcCommand, IpcResponse};

/// Format a transcript using AI
///
/// # Arguments
/// * `text` - Raw transcript text to format
/// * `language` - Optional language code (e.g., "en", "pl")
/// * `assistive` - If true, use assistive/enhancer mode; if false, simple formatting
#[tauri::command]
pub async fn format_transcript(
    text: String,
    language: Option<String>,
    assistive: bool,
) -> Result<String, String> {
    if text.trim().is_empty() {
        return Err("Empty text cannot be formatted".to_string());
    }

    let mut client = IpcClient::connect().map_err(|e| e.to_string())?;
    let response: IpcResponse = client
        .send(&IpcCommand::FormatTranscript {
            text,
            language,
            assistive,
        })
        .map_err(|e| e.to_string())?;

    match response {
        IpcResponse::Message(message) => Ok(message),
        IpcResponse::Error(err) => Err(err),
        _ => Err("Unexpected IPC response for FormatTranscript".to_string()),
    }
}

/// Reset AI conversation context
///
/// Clears the conversation memory/context
#[tauri::command]
pub async fn reset_ai_context() -> Result<(), String> {
    let mut client = IpcClient::connect().map_err(|e| e.to_string())?;
    let response: IpcResponse = client
        .send(&IpcCommand::ResetContext)
        .map_err(|e| e.to_string())?;

    match response {
        IpcResponse::Ok => Ok(()),
        IpcResponse::Error(err) => Err(err),
        _ => Err("Unexpected IPC response for ResetContext".to_string()),
    }
}

/// Get the current AI prompt content
///
/// Returns the prompt from file if exists, or default prompt
#[tauri::command]
pub fn get_ai_prompt(prompt_type: String) -> Result<String, String> {
    let mut client = IpcClient::connect().map_err(|e| e.to_string())?;
    let response: IpcResponse = client
        .send(&IpcCommand::GetPrompt { prompt_type })
        .map_err(|e| e.to_string())?;

    match response {
        IpcResponse::Prompt(content) => Ok(content),
        IpcResponse::Error(err) => Err(err),
        _ => Err("Unexpected IPC response for GetPrompt".to_string()),
    }
}

/// Open AI prompt file in system editor
#[tauri::command]
pub fn open_prompt_in_editor(prompt_type: String) -> Result<(), String> {
    match prompt_type.as_str() {
        "formatting" => {
            codescribe_core::config::open_prompt_file("formatting.txt");
            Ok(())
        }
        "assistive" => {
            codescribe_core::config::open_prompt_file("assistive.txt");
            Ok(())
        }
        _ => Err(format!("Unknown prompt type: {}", prompt_type)),
    }
}

/// Save AI prompt content to file
#[tauri::command]
pub fn save_ai_prompt(prompt_type: String, content: String) -> Result<(), String> {
    let mut client = IpcClient::connect().map_err(|e| e.to_string())?;
    let response: IpcResponse = client
        .send(&IpcCommand::SavePrompt {
            prompt_type,
            content,
        })
        .map_err(|e| e.to_string())?;

    match response {
        IpcResponse::Ok => Ok(()),
        IpcResponse::Error(err) => Err(err),
        _ => Err("Unexpected IPC response for SavePrompt".to_string()),
    }
}

/// Send a message to AI assistant and get response
#[tauri::command]
pub async fn send_message(message: String) -> Result<MessageResponse, String> {
    if message.trim().is_empty() {
        return Err("Empty message".to_string());
    }

    let mut client = IpcClient::connect().map_err(|e| e.to_string())?;
    let response: IpcResponse = client
        .send(&IpcCommand::SendMessage { message })
        .map_err(|e| e.to_string())?;

    match response {
        IpcResponse::Message(content) => Ok(MessageResponse {
            content,
            is_final: true,
        }),
        IpcResponse::Error(err) => Err(err),
        _ => Err("Unexpected IPC response for SendMessage".to_string()),
    }
}

/// Response from send_message command
#[derive(serde::Serialize)]
pub struct MessageResponse {
    pub content: String,
    pub is_final: bool,
}

/// Reset AI prompt to default
#[tauri::command]
pub fn reset_ai_prompt(prompt_type: String) -> Result<String, String> {
    let mut client = IpcClient::connect().map_err(|e| e.to_string())?;
    let response: IpcResponse = client
        .send(&IpcCommand::ResetPrompt { prompt_type })
        .map_err(|e| e.to_string())?;

    match response {
        IpcResponse::Prompt(content) => Ok(content),
        IpcResponse::Error(err) => Err(err),
        _ => Err("Unexpected IPC response for ResetPrompt".to_string()),
    }
}
