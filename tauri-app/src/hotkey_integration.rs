//! Hotkey integration for Tauri app
//!
//! Spawns the CGEventTap hotkey listener and routes events to the recording controller.
//! Created by M&K (c)2026 VetCoders

use crate::state::AppState;
use codescribe::hotkeys::{HoldAction, HotkeyEvent, HotkeyManager};
use crossbeam_channel::{Receiver, unbounded};
use std::sync::Arc;
use tauri::AppHandle;
use tracing::{debug, error, info, warn};

/// Start the hotkey listener and event routing
///
/// This function:
/// 1. Creates the HotkeyManager which starts CGEventTap in background thread
/// 2. Spawns a tokio task to receive and process hotkey events
/// 3. Routes events to start/stop recording via AppState
pub fn start_hotkey_listener(app_handle: AppHandle, state: Arc<AppState>) -> Result<(), String> {
    // Create channel for hotkey events
    let (tx, rx) = unbounded::<HotkeyEvent>();

    // Start the hotkey manager (spawns CGEventTap thread)
    let _manager = HotkeyManager::new(tx).map_err(|e| {
        error!("Failed to start hotkey listener: {}", e);
        e
    })?;

    info!("Hotkey listener started successfully");

    // Spawn task to process hotkey events
    let state_clone = Arc::clone(&state);
    std::thread::spawn(move || {
        run_hotkey_event_loop(rx, state_clone, app_handle);
    });

    Ok(())
}

/// Run the hotkey event loop (blocking, runs in dedicated thread)
fn run_hotkey_event_loop(rx: Receiver<HotkeyEvent>, state: Arc<AppState>, _app: AppHandle) {
    // Get tokio runtime handle for async operations
    let rt = match tokio::runtime::Handle::try_current() {
        Ok(h) => h,
        Err(_) => {
            // Create a new runtime if none exists
            let rt = tokio::runtime::Runtime::new().expect("Failed to create tokio runtime");
            rt.handle().clone()
        }
    };

    info!("Hotkey event loop started");

    // Track state for hold mode
    let mut hold_pending = false;
    let mut pending_assistive = false;

    loop {
        match rx.recv() {
            Ok(event) => {
                debug!("Received hotkey event: {:?}", event);

                match event {
                    HotkeyEvent::Hold { action, assistive } => {
                        match action {
                            HoldAction::Down => {
                                // Start recording after delay (handled by schedule_hold_start in controller)
                                // For Tauri, we start immediately but could add delay later
                                hold_pending = true;
                                pending_assistive = assistive;

                                let state = Arc::clone(&state);
                                rt.spawn(async move {
                                    if let Err(e) = handle_start_recording(&state).await {
                                        error!("Failed to start recording: {}", e);
                                    }
                                });
                            }
                            HoldAction::Up => {
                                if hold_pending {
                                    hold_pending = false;

                                    let state = Arc::clone(&state);
                                    let use_assistive = pending_assistive;
                                    rt.spawn(async move {
                                        if let Err(e) =
                                            handle_stop_recording(&state, use_assistive).await
                                        {
                                            error!("Failed to stop recording: {}", e);
                                        }
                                    });
                                }
                            }
                        }
                    }
                    HotkeyEvent::Toggle => {
                        // Toggle mode: start or stop depending on current state
                        let state = Arc::clone(&state);
                        rt.spawn(async move {
                            let recording = state.recording.lock().await;
                            let is_recording = recording.is_recording;
                            drop(recording);

                            if is_recording {
                                // Stop and transcribe
                                if let Err(e) = handle_stop_recording(&state, false).await {
                                    error!("Failed to stop recording (toggle): {}", e);
                                }
                            } else {
                                // Start recording
                                if let Err(e) = handle_start_recording(&state).await {
                                    error!("Failed to start recording (toggle): {}", e);
                                }
                            }
                        });
                    }
                }
            }
            Err(e) => {
                error!("Hotkey channel closed: {}", e);
                break;
            }
        }
    }

    warn!("Hotkey event loop terminated");
}

/// Start recording (called from hotkey event)
async fn handle_start_recording(state: &AppState) -> Result<(), String> {
    let mut recording = state.recording.lock().await;

    if recording.is_recording {
        debug!("Recording already in progress, ignoring start");
        return Ok(());
    }

    // Initialize recorder if needed
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
        info!("Recording started via hotkey");

        // Play start beep
        codescribe::sound::play_sound("Tink");

        Ok(())
    } else {
        Err("Recorder not initialized".to_string())
    }
}

/// Stop recording, transcribe, and paste (called from hotkey event)
async fn handle_stop_recording(state: &AppState, assistive: bool) -> Result<(), String> {
    let mut recording = state.recording.lock().await;

    if !recording.is_recording {
        debug!("No recording in progress, ignoring stop");
        return Ok(());
    }

    // Stop recording
    let audio_path = if let Some(ref mut recorder) = recording.recorder {
        let result = recorder
            .stop()
            .await
            .map_err(|e| format!("Failed to stop recording: {e}"))?;
        recording.is_recording = false;
        result
    } else {
        recording.is_recording = false;
        return Err("Recorder not initialized".to_string());
    };

    drop(recording); // Release lock before transcription

    // Transcribe the audio
    let audio_path = match audio_path {
        Some(p) => p,
        None => {
            warn!("No audio captured");
            return Ok(());
        }
    };

    info!("Transcribing audio: {:?}", audio_path);

    // Get language from config
    let config = state.config.lock().map_err(|e| e.to_string())?;
    let language = config.whisper_language;
    let use_local_stt = config.use_local_stt;
    let local_model = config.local_model.clone();
    drop(config);

    let language_opt = match language {
        codescribe::config::Language::Auto => None,
        lang => Some(lang.as_str().to_string()),
    };

    // Transcribe using local STT
    let raw_text = if use_local_stt {
        // Try to use the engine from state
        let mut stt = state.stt.lock().map_err(|e| e.to_string())?;

        // Load engine if not already loaded
        if stt.engine.is_none() || stt.loaded_model.as_ref() != Some(&local_model) {
            let model_path = state.model_manager.get_model_path(&local_model);
            match codescribe::whisper::LocalWhisperEngine::new(&model_path) {
                Ok(engine) => {
                    stt.engine = Some(engine);
                    stt.loaded_model = Some(local_model.clone());
                    info!("Loaded local STT model: {}", local_model);
                }
                Err(e) => {
                    warn!("Failed to load local STT: {}", e);
                    return Err(format!("Local STT unavailable: {e}"));
                }
            }
        }

        // Transcribe
        if let Some(ref mut engine) = stt.engine {
            engine
                .transcribe_file_with_language(&audio_path, language_opt.as_deref())
                .map_err(|e| format!("Transcription failed: {e}"))?
        } else {
            return Err("Local STT engine not available".to_string());
        }
    } else {
        return Err("Cloud STT not implemented in Tauri yet".to_string());
    };

    if raw_text.trim().is_empty() {
        warn!("Empty transcript");
        return Ok(());
    }

    info!("Transcribed: {} chars", raw_text.len());

    // Apply formatting if assistive mode
    let final_text = if assistive {
        // Check for repetition loops and clean up
        if codescribe::ai_formatting::has_repetition_loop(&raw_text) {
            info!("Cleaning up repetition loop");
            codescribe::ai_formatting::remove_simple_repetitions(&raw_text)
        } else {
            // TODO: Add AI formatting via Ollama when configured
            raw_text
        }
    } else {
        raw_text
    };

    // Paste to clipboard and active app
    codescribe::clipboard::paste_text(&final_text).map_err(|e| format!("Failed to paste: {e}"))?;

    info!("Text pasted successfully ({} chars)", final_text.len());

    // Play completion beep
    codescribe::sound::play_sound("Pop");

    Ok(())
}
