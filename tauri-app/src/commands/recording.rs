//! Recording IPC commands for start/stop audio capture
//! Created by M&K (c)2026 VetCoders

use crate::state::AppState;
use tauri::State;

/// Start audio recording
///
/// Initializes the recorder if needed and begins capturing audio.
/// Returns error if recording is already in progress.
#[tauri::command]
pub async fn start_recording(state: State<'_, AppState>) -> Result<(), String> {
    let mut recording = state.recording.lock().await;

    if recording.is_recording {
        return Err("Recording already in progress".to_string());
    }

    // Initialize recorder if not present
    if recording.recorder.is_none() {
        let recorder = codescribe::audio::Recorder::new()
            .map_err(|e| format!("Failed to init recorder: {e}"))?;
        recording.recorder = Some(recorder);
    }

    // Start recording
    if let Some(ref mut recorder) = recording.recorder {
        recorder
            .start()
            .await
            .map_err(|e| format!("Failed to start recording: {e}"))?;
        recording.is_recording = true;
        Ok(())
    } else {
        Err("Recorder not initialized".to_string())
    }
}

/// Stop audio recording and return path to WAV file
///
/// Stops the active recording and saves the audio to a temporary WAV file.
/// Returns the path to the WAV file, or None if no audio was captured.
#[tauri::command]
pub async fn stop_recording(state: State<'_, AppState>) -> Result<Option<String>, String> {
    let mut recording = state.recording.lock().await;

    if !recording.is_recording {
        return Err("No recording in progress".to_string());
    }

    // Stop recording
    if let Some(ref mut recorder) = recording.recorder {
        let result: Option<std::path::PathBuf> = recorder
            .stop()
            .await
            .map_err(|e| format!("Failed to stop recording: {e}"))?;

        recording.is_recording = false;

        // Convert PathBuf to String for IPC
        Ok(result.map(|p: std::path::PathBuf| p.to_string_lossy().to_string()))
    } else {
        recording.is_recording = false;
        Err("Recorder not initialized".to_string())
    }
}

/// Check if recording is currently active
#[tauri::command]
pub async fn is_recording(state: State<'_, AppState>) -> Result<bool, String> {
    let recording = state.recording.lock().await;
    Ok(recording.is_recording)
}
