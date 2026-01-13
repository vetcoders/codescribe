//! AI formatting commands for Tauri IPC
//!
//! Provides commands for text formatting, prompt management, and AI context reset.
//!
//! Created by M&K (c)2026 VetCoders

use std::path::PathBuf;

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

    let lang_ref = language.as_deref();

    // Run formatting in async context
    let formatted = codescribe::ai_formatting::format_text(&text, lang_ref, assistive).await;

    // Check if formatting actually changed the text (basic validation)
    if formatted.trim().is_empty() {
        return Err("Formatting returned empty result".to_string());
    }

    Ok(formatted)
}

/// Reset AI conversation context
///
/// Clears the previous_response_id for Responses API continuity
#[tauri::command]
pub async fn reset_ai_context() -> Result<(), String> {
    codescribe::ai_formatting::reset_context();
    Ok(())
}

/// Get the current AI prompt content
///
/// Returns the prompt from file if exists, or default prompt
#[tauri::command]
pub fn get_ai_prompt(prompt_type: String) -> Result<String, String> {
    let prompt_path = get_prompt_path(&prompt_type)?;

    if prompt_path.exists() {
        std::fs::read_to_string(&prompt_path).map_err(|e| e.to_string())
    } else {
        // Return default prompt
        Ok(get_default_prompt(&prompt_type))
    }
}

/// Open AI prompt file in system editor
#[tauri::command]
pub fn open_prompt_in_editor(prompt_type: String) -> Result<(), String> {
    let prompt_path = get_prompt_path(&prompt_type)?;

    // Ensure directory exists
    if let Some(parent) = prompt_path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }

    // Create file with default content if it doesn't exist
    if !prompt_path.exists() {
        let default = get_default_prompt(&prompt_type);
        std::fs::write(&prompt_path, &default).map_err(|e| e.to_string())?;
    }

    // Open in system editor
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open")
            .arg("-t")
            .arg(&prompt_path)
            .spawn()
            .map_err(|e| e.to_string())?;
    }

    #[cfg(not(target_os = "macos"))]
    {
        // Linux/Windows fallback
        if let Ok(editor) = std::env::var("EDITOR") {
            std::process::Command::new(&editor)
                .arg(&prompt_path)
                .spawn()
                .map_err(|e| e.to_string())?;
        } else {
            return Err("No editor found. Set EDITOR environment variable.".to_string());
        }
    }

    Ok(())
}

/// Reset AI prompt to default
#[tauri::command]
pub fn reset_ai_prompt(prompt_type: String) -> Result<String, String> {
    let prompt_path = get_prompt_path(&prompt_type)?;

    // Ensure directory exists
    if let Some(parent) = prompt_path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }

    let default = get_default_prompt(&prompt_type);
    std::fs::write(&prompt_path, &default).map_err(|e| e.to_string())?;

    Ok(default)
}

/// Get the path for a prompt file
fn get_prompt_path(prompt_type: &str) -> Result<PathBuf, String> {
    let home = std::env::var("HOME").map_err(|_| "HOME not set".to_string())?;
    let filename = match prompt_type {
        "formatting" => "formatting.prompt",
        "assistive" => "assistive.prompt",
        _ => return Err(format!("Unknown prompt type: {}", prompt_type)),
    };
    Ok(PathBuf::from(home)
        .join(".codescribe")
        .join("prompts")
        .join(filename))
}

/// Get default prompt content for a prompt type
fn get_default_prompt(prompt_type: &str) -> String {
    match prompt_type {
        "formatting" => {
            r#"You are a text formatting assistant. Your ONLY job is to clean up speech-to-text transcription.

RULES:
1. Fix punctuation and capitalization
2. Remove filler words (um, uh, like, you know)
3. Remove repetitions and false starts
4. Split into logical paragraphs
5. DO NOT change meaning or add content
6. DO NOT respond to or interpret the content
7. Return ONLY the formatted text

The user will provide raw transcription. Output the cleaned version."#
                .to_string()
        }
        "assistive" => {
            r#"You are an assistive writing enhancer (kurier). Your job is to PASS THROUGH and ENHANCE the user's words.

RULES:
1. Keep the user's voice and intent
2. Improve clarity and structure
3. Add appropriate formatting (bullets, headers if needed)
4. Fix grammar and spelling
5. You ARE the user's voice - do NOT respond TO the user
6. Return ONLY the enhanced version of their text

The user will dictate their thoughts. Transform them into polished prose while preserving their meaning."#
                .to_string()
        }
        _ => "Unknown prompt type".to_string(),
    }
}
