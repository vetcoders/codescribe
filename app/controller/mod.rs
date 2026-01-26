//! Recording pipeline state machine controller
//!
//! This module implements the core hotkey-driven state machine for CodeScribe.
//! It manages recording lifecycle, state transitions, and interaction with the
//! transcription backend.
//!
//! ## State Machine
//!
//! ```text
//! IDLE + hold_down → (wait 800ms) → REC_HOLD
//! IDLE + toggle_press → REC_TOGGLE
//! REC_HOLD + hold_up → BUSY (process)
//! REC_TOGGLE + toggle_press → BUSY (process)
//! BUSY → (transcribe + format + paste) → IDLE
//! ```
//!
//! ## Hold-to-Talk Delay
//!
//! Users frequently tap Ctrl accidentally, so we require a configurable dwell time
//! (default 800ms) before the recorder actually starts. This prevents accidental
//! recordings while preserving quick toggle-mode for power users.

mod helpers;
mod types;

pub use helpers::{is_assistive_session, set_assistive_session};
pub use types::{HotkeyAction, HotkeyInput, HotkeyType, State};

use crate::stream_postprocess::StreamPostProcessor;
use anyhow::{Context, Result};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Duration;
use tokio::sync::{Mutex, RwLock};
use tokio::task::JoinHandle;
use tracing::{debug, error, info, warn};
use uuid::Uuid;

use crate::audio::streaming_recorder::StreamingRecorder;
use crate::config::Config;
use crate::config::models::ModelManager;
use crate::os::clipboard;
use crate::tray::{TrayStatus, update_tray_status};
use crate::{BadgeMode, hide_hold_badge, show_badge_for_mode};

use helpers::{raw_save_enabled, route_transcription_delta, setup_voice_chat_send_callback};
use types::ValidatedAudioPath;

static OVERLAY_CONTROLLER: OnceLock<Arc<RecordingController>> = OnceLock::new();

/// Register the controller for overlay actions (commit/close fragment).
pub fn register_overlay_controller(controller: Arc<RecordingController>) {
    if OVERLAY_CONTROLLER.set(controller).is_err() {
        warn!("Overlay controller already registered");
    }
}

/// Stop the current recording and enter decision mode without waiting for VAD.
pub fn request_recording_commit() {
    let Some(controller) = OVERLAY_CONTROLLER.get().cloned() else {
        warn!("Overlay controller not registered; cannot commit recording");
        return;
    };

    tokio::spawn(async move {
        if let Err(e) = controller.finish_recording().await {
            error!("Overlay commit failed: {}", e);
        }
    });
}

/// Recording controller managing state machine and lifecycle
pub struct RecordingController {
    /// Application configuration
    config: Arc<RwLock<Config>>,

    /// Current state
    state: Arc<RwLock<State>>,

    /// Audio recorder instance
    recorder: Arc<Mutex<StreamingRecorder>>,

    /// Whether assistive formatting mode is enabled (Ctrl+Shift = always AI augmentation)
    assistive_mode: Arc<RwLock<bool>>,

    /// Whether to force RAW mode (Ctrl Hold without Shift = always raw, ignores AI toggle)
    /// Toggle mode (Double Option) keeps this false and respects AI_FORMATTING_ENABLED setting.
    force_raw_mode: Arc<RwLock<bool>>,
    /// Whether to force AI formatting for the current session (e.g., left double Option)
    force_ai_mode: Arc<RwLock<bool>>,

    /// Current session ID for tracking
    session_id: Arc<RwLock<Option<String>>>,

    /// Task handle for delayed hold-start (800ms default)
    hold_start_task: Arc<Mutex<Option<JoinHandle<()>>>>,

    /// Lock to serialize finish_recording calls
    serial_lock: Arc<Mutex<()>>,

    /// Flag set by VAD (silence detection) when recording should auto-stop
    vad_triggered: Arc<AtomicBool>,
}

impl RecordingController {
    /// Create a new recording controller with configuration loaded from disk
    pub fn new() -> Self {
        let config = Config::load();

        info!(
            "Initializing RecordingController (hold_delay={}ms, beep={}, language={:?})",
            config.hold_start_delay_ms, config.beep_on_start, config.whisper_language
        );

        let mut recorder =
            StreamingRecorder::new().expect("Failed to initialize streaming recorder");
        recorder.set_delta_callback(Some(Arc::new(|delta| {
            route_transcription_delta(delta);
        })));

        let model_manager = ModelManager::new().expect("Failed to initialize model manager");
        if let Ok(models) = model_manager.list_models()
            && !models.is_empty()
        {
            info!("Available local models: {:?}", models);
        }

        // Initialize Whisper engine (singleton)
        if let Err(e) = crate::whisper::init() {
            warn!("Failed to initialize Whisper engine: {}", e);
        }

        let config = Arc::new(RwLock::new(config));
        setup_voice_chat_send_callback(Arc::clone(&config));

        Self {
            config,
            state: Arc::new(RwLock::new(State::Idle)),
            recorder: Arc::new(Mutex::new(recorder)),
            assistive_mode: Arc::new(RwLock::new(false)),
            force_raw_mode: Arc::new(RwLock::new(false)),
            force_ai_mode: Arc::new(RwLock::new(false)),
            session_id: Arc::new(RwLock::new(None)),
            hold_start_task: Arc::new(Mutex::new(None)),
            serial_lock: Arc::new(Mutex::new(())),
            vad_triggered: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Create a new recording controller with shared configuration
    pub fn with_config(config: Arc<RwLock<Config>>) -> Self {
        let cfg = tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(async { config.read().await.clone() })
        });

        info!(
            "Initializing RecordingController with shared config (hold_delay={}ms, beep={}, language={:?})",
            cfg.hold_start_delay_ms, cfg.beep_on_start, cfg.whisper_language
        );

        let mut recorder =
            StreamingRecorder::new().expect("Failed to initialize streaming recorder");
        recorder.set_delta_callback(Some(Arc::new(|delta| {
            route_transcription_delta(delta);
        })));

        let model_manager = ModelManager::new().expect("Failed to initialize model manager");
        if let Ok(models) = model_manager.list_models()
            && !models.is_empty()
        {
            info!("Available local models: {:?}", models);
        }

        // Initialize Whisper engine (singleton)
        if let Err(e) = crate::whisper::init() {
            warn!("Failed to initialize Whisper engine: {}", e);
        }

        setup_voice_chat_send_callback(Arc::clone(&config));

        Self {
            config,
            state: Arc::new(RwLock::new(State::Idle)),
            recorder: Arc::new(Mutex::new(recorder)),
            assistive_mode: Arc::new(RwLock::new(false)),
            force_raw_mode: Arc::new(RwLock::new(false)),
            force_ai_mode: Arc::new(RwLock::new(false)),
            session_id: Arc::new(RwLock::new(None)),
            hold_start_task: Arc::new(Mutex::new(None)),
            serial_lock: Arc::new(Mutex::new(())),
            vad_triggered: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Get current state
    pub async fn current_state(&self) -> State {
        *self.state.read().await
    }

    /// Replace controller configuration at runtime
    pub async fn set_config(&self, config: Config) {
        *self.config.write().await = config;
    }

    /// Snapshot of current controller configuration
    pub async fn get_config(&self) -> Config {
        self.config.read().await.clone()
    }

    /// Check if VAD (silence detection) has triggered auto-stop
    pub fn is_vad_triggered(&self) -> bool {
        self.vad_triggered.load(Ordering::SeqCst)
    }

    /// Clear the VAD triggered flag
    pub fn clear_vad_triggered(&self) {
        self.vad_triggered.store(false, Ordering::SeqCst);
    }

    /// Cancel any pending delayed hold-start task
    async fn cancel_pending_hold_start(&self) {
        let mut task_guard = self.hold_start_task.lock().await;
        if let Some(task) = task_guard.take()
            && !task.is_finished()
        {
            debug!("Cancelling pending hold-start task");
            task.abort();
            let _ = task.await; // Suppress cancellation errors
        }
    }

    /// Handle hotkey event - main entry point for state machine
    ///
    /// # Arguments
    /// * `event` - The hotkey event to process
    ///
    /// This method implements the state machine logic and delegates to
    /// appropriate handlers based on current state and event type.
    ///
    /// ## Mode Determination (NEW architecture):
    /// - **Hold + assistive=false**: force RAW mode (ignores AI_FORMATTING_ENABLED)
    /// - **Hold + assistive=true**: force Assistive mode (Shift pressed = AI augmentation)
    /// - **Toggle + force_ai=true**: force AI formatting (normal hands-off)
    /// - **Toggle + assistive=true**: force Assistive hands-off
    pub async fn handle_hotkey_event(&self, event: HotkeyInput) -> Result<()> {
        let current_state = self.current_state().await;

        debug!(
            "Hotkey event: type={:?} action={:?} assistive={} force_ai={} state={}",
            event.key_type, event.action, event.assistive, event.force_ai, current_state
        );

        // Update assistive mode from event (can be upgraded mid-hold if Shift added)
        if event.assistive {
            *self.assistive_mode.write().await = true;
            // Shift pressed = NOT force_raw (Assistive takes precedence)
            *self.force_raw_mode.write().await = false;
            *self.force_ai_mode.write().await = false;
        } else if matches!(event.action, HotkeyAction::Down | HotkeyAction::Press) {
            // Only reset on Down/Press, not Up (preserves upgrade during hold)
            *self.assistive_mode.write().await = false;

            match event.key_type {
                HotkeyType::Hold => {
                    // Hold without Shift = force RAW mode
                    *self.force_raw_mode.write().await = true;
                    *self.force_ai_mode.write().await = false;
                }
                HotkeyType::Toggle => {
                    *self.force_raw_mode.write().await = false;
                    *self.force_ai_mode.write().await = event.force_ai;
                }
            }
        }

        // Ignore all hotkeys when busy
        if current_state == State::Busy {
            info!("App busy; ignoring hotkey event");
            return Ok(());
        }

        // Route to appropriate handler
        match event.key_type {
            HotkeyType::Hold => self.handle_hold_event(event).await,
            HotkeyType::Toggle => self.handle_toggle_event(event).await,
        }
    }

    /// Handle hold-type hotkey events
    async fn handle_hold_event(&self, event: HotkeyInput) -> Result<()> {
        match event.action {
            HotkeyAction::Down => {
                let current_state = self.current_state().await;
                if current_state == State::Idle {
                    self.schedule_hold_start().await?;
                }
            }
            HotkeyAction::Up => {
                let current_state = self.current_state().await;
                if current_state == State::RecHold {
                    info!("Hold released; finishing recording");
                    self.finish_recording().await?;
                } else {
                    // Cancel the delayed start if user released before delay elapsed
                    self.cancel_pending_hold_start().await;
                }
            }
            HotkeyAction::Press => {
                // Hold keys don't use press events
            }
        }
        Ok(())
    }

    /// Handle toggle-type hotkey events
    async fn handle_toggle_event(&self, event: HotkeyInput) -> Result<()> {
        if event.action != HotkeyAction::Press {
            return Ok(());
        }

        let current_state = self.current_state().await;

        match current_state {
            State::Idle => {
                self.start_toggle_recording(event.assistive).await?;
            }
            State::RecToggle => {
                info!("Toggle pressed again; finishing recording");
                self.finish_recording().await?;
            }
            _ => {
                debug!("Toggle event ignored in state {}", current_state);
            }
        }

        Ok(())
    }

    /// Schedule delayed recording start for hold mode
    async fn schedule_hold_start(&self) -> Result<()> {
        // Check backend health before starting (skip in tests: no backend available)
        if !cfg!(test) {
            match crate::client::check_health().await {
                Ok(true) => {}
                Ok(false) => {
                    warn!("Whisper engine not ready");
                    let _ = crate::tray::update_tray_status(crate::tray::TrayStatus::Error);
                    return Ok(());
                }
                Err(e) => {
                    error!("Whisper engine unavailable: {}", e);
                    let _ = crate::tray::update_tray_status(crate::tray::TrayStatus::Error);
                    return Ok(());
                }
            }
        }

        let config = self.config.read().await;
        let delay_ms = config.hold_start_delay_ms;
        let beep = config.beep_on_start;
        let language = config.whisper_language;
        drop(config); // Release read lock

        // Capture assistive mode for badge display
        let is_assistive = *self.assistive_mode.read().await;

        debug!(
            "Scheduling hold-start after {}ms delay (assistive={})",
            delay_ms, is_assistive
        );

        // Cancel any existing delayed start
        self.cancel_pending_hold_start().await;

        // Reset VAD flag for new session
        self.vad_triggered.store(false, Ordering::SeqCst);

        let state = Arc::clone(&self.state);
        let session_id = Arc::clone(&self.session_id);
        let recorder = Arc::clone(&self.recorder);
        let delay = Duration::from_millis(delay_ms);
        let vad_flag = Arc::clone(&self.vad_triggered);

        let task = tokio::spawn(async move {
            // Wait for the configured delay
            tokio::time::sleep(delay).await;

            // Check if we're still in IDLE state
            let current_state = *state.read().await;
            if current_state != State::Idle {
                debug!("Hold-start cancelled: state changed to {}", current_state);
                return;
            }

            // Generate session ID
            let new_session_id = Uuid::new_v4().to_string();
            *session_id.write().await = Some(new_session_id.clone());

            info!("Starting hold recording (session={})", new_session_id);

            // Start the recorder (skip in tests: no CoreAudio device needed)
            // hang_sec is configured via CODESCRIBE_VAD_MAX_SILENCE_SEC env var (single source of truth)
            let mut rec = recorder.lock().await;
            rec.recorder.set_on_vad_stop(move || {
                info!("VAD callback: setting vad_triggered flag");
                vad_flag.store(true, Ordering::SeqCst);
            });
            if !cfg!(test)
                && let Err(e) = rec.start(Some(language.as_str().to_string())).await
            {
                error!("Failed to start recorder: {}", e);
                return;
            }

            // Play start beep if enabled
            if beep {
                crate::audio::play_sound("Tink");
            }

            // Show badge with appropriate mode (Hold=red solid, Assistive=purple)
            let badge_mode = if is_assistive {
                BadgeMode::Assistive
            } else {
                BadgeMode::Hold
            };
            show_badge_for_mode(badge_mode);

            // Set session mode for delta routing
            set_assistive_session(is_assistive);

            // ALWAYS show transcription overlay for live preview
            crate::clear_transcription_text();
            crate::show_transcription_overlay();
            if is_assistive {
                crate::show_voice_chat_overlay();
                crate::show_agent_tab();
            }
            crate::enter_recording_mode();

            // Transition to REC_HOLD
            *state.write().await = State::RecHold;
            info!(
                "STATE TRANSITION: IDLE → REC_HOLD (assistive={})",
                is_assistive
            );

            // Update tray status to Listening
            let _ = update_tray_status(TrayStatus::Listening);
        });

        *self.hold_start_task.lock().await = Some(task);
        Ok(())
    }

    /// Start recording in toggle mode (immediate, no delay)
    async fn start_toggle_recording(&self, is_assistive: bool) -> Result<()> {
        // Check backend health before starting (skip in tests: no backend available)
        if !cfg!(test) {
            match crate::client::check_health().await {
                Ok(true) => {}
                Ok(false) => {
                    warn!("Whisper engine not ready");
                    let _ = crate::tray::update_tray_status(crate::tray::TrayStatus::Error);
                    return Ok(());
                }
                Err(e) => {
                    error!("Whisper engine unavailable: {}", e);
                    let _ = crate::tray::update_tray_status(crate::tray::TrayStatus::Error);
                    return Ok(());
                }
            }
        }

        // Acquire serial lock to prevent race conditions
        let _guard = self.serial_lock.lock().await;

        // Double-check state under lock
        let current_state = *self.state.read().await;
        if current_state != State::Idle {
            debug!(
                "start_toggle_recording: state already changed to {}",
                current_state
            );
            return Ok(());
        }

        // Generate session ID
        let new_session_id = Uuid::new_v4().to_string();
        *self.session_id.write().await = Some(new_session_id.clone());

        info!("Starting toggle recording (session={})", new_session_id);

        let config = self.config.read().await;
        let language = config.whisper_language;
        drop(config);

        // Reset VAD flag and set callback
        self.vad_triggered.store(false, Ordering::SeqCst);
        let vad_flag = Arc::clone(&self.vad_triggered);

        // Start the recorder with VAD callback
        // hang_sec is configured via CODESCRIBE_VAD_MAX_SILENCE_SEC env var (single source of truth)
        let mut recorder = self.recorder.lock().await;

        recorder.recorder.set_on_vad_stop(move || {
            info!("VAD callback: setting vad_triggered flag");
            vad_flag.store(true, Ordering::SeqCst);
        });

        // Set streaming callback for overlay updates (routed by session mode)
        recorder.set_delta_callback(Some(Arc::new(|text: &str| {
            route_transcription_delta(text);
        })));

        // Skip actual audio stream in tests (no CoreAudio device needed)
        if !cfg!(test) {
            recorder.start(Some(language.as_str().to_string())).await?;
        }

        // Play start beep if enabled
        let beep_enabled = self.config.read().await.beep_on_start;
        if beep_enabled {
            crate::audio::play_sound("Tink");
        }

        // Show badge with appropriate mode
        let badge_mode = if is_assistive {
            BadgeMode::Assistive
        } else {
            BadgeMode::Toggle
        };
        show_badge_for_mode(badge_mode);

        // Set session mode for delta routing
        set_assistive_session(is_assistive);

        // ALWAYS show transcription overlay for live preview
        crate::clear_transcription_text();
        crate::show_transcription_overlay();
        if is_assistive {
            crate::show_voice_chat_overlay();
            crate::show_agent_tab();
        }
        crate::enter_recording_mode();

        // Transition to REC_TOGGLE
        *self.state.write().await = State::RecToggle;
        info!("STATE TRANSITION: IDLE → REC_TOGGLE (pulsing badge)");

        // Update tray status to Listening
        let _ = update_tray_status(TrayStatus::Listening);

        Ok(())
    }

    /// Stop recording, transcribe, format, and paste the result
    ///
    /// This is the core processing pipeline that:
    /// 1. Stops the audio recorder
    /// 2. Transcribes the audio via backend
    /// 3. Formats the transcript (if assistive mode enabled)
    /// 4. Pastes the result into the active application
    pub async fn finish_recording(&self) -> Result<()> {
        // Cancel any pending hold-start
        self.cancel_pending_hold_start().await;

        // Acquire serial lock to prevent concurrent finish calls
        let _guard = self.serial_lock.lock().await;

        self.finish_recording_locked().await
    }

    /// Internal finish_recording implementation (assumes lock is held)
    async fn finish_recording_locked(&self) -> Result<()> {
        let current_state = *self.state.read().await;

        // Ignore if we're not recording
        if matches!(current_state, State::Idle | State::Busy) {
            warn!(
                "finish_recording called while state={}; ignoring (race?)",
                current_state
            );
            return Ok(());
        }

        info!("Finishing recording (state={})", current_state);

        // Transition to BUSY
        debug!("STATE TRANSITION: {} → BUSY", current_state);
        *self.state.write().await = State::Busy;

        // Get session ID and mode flags before we reset them
        let session_id = self.session_id.read().await.clone();
        let assistive = *self.assistive_mode.read().await;
        let force_raw = *self.force_raw_mode.read().await;
        let force_ai = *self.force_ai_mode.read().await;

        // Switch badge to processing mode (orange, pulsing)
        show_badge_for_mode(BadgeMode::Processing);

        let result = self
            .process_recording(session_id, assistive, force_raw, force_ai)
            .await;

        // Always reset to IDLE, even on error
        *self.state.write().await = State::Idle;
        *self.assistive_mode.write().await = false;
        *self.force_raw_mode.write().await = false;
        *self.force_ai_mode.write().await = false;
        *self.session_id.write().await = None;

        // Hide red dot indicator
        hide_hold_badge();

        // Update tray icon based on result
        match &result {
            Ok(_) => {
                let _ = update_tray_status(TrayStatus::Success);
                info!("Processing finished successfully. State reset to IDLE.");

                // After recording finishes, enter decision mode + auto-hide ONLY for non‑assistive flows.
                // Assistive/chat overlay must stay alive (bubbles persist; user may keep chatting).
                if !assistive {
                    crate::enter_decision_mode();
                    crate::schedule_auto_hide();
                }
            }
            Err(e) => {
                error!("Processing failed: {}", e);
                let _ = update_tray_status(TrayStatus::Idle);

                // Hide overlay immediately on error
                crate::hide_transcription_overlay();
            }
        }

        result
    }

    /// Process the recording: stop, transcribe, format, paste
    ///
    /// ## Mode Logic:
    /// - `assistive=true`: ALWAYS AI augmentation (Ctrl+Shift held)
    /// - `force_raw=true`: ALWAYS raw transcript (Ctrl held without Shift)
    /// - `force_ai=true`: ALWAYS AI formatting (left double Option)
    /// - Neither: Toggle mode - respects AI_FORMATTING_ENABLED setting
    async fn process_recording(
        &self,
        _session_id: Option<String>,
        assistive: bool,
        force_raw: bool,
        force_ai: bool,
    ) -> Result<()> {
        // Stop the recorder and get audio file path
        let mut recorder = self.recorder.lock().await;
        let (streaming_text, raw_audio_path_opt) =
            recorder.stop().await.context("Failed to stop recorder")?;
        drop(recorder); // Release lock

        // Check audio path validity (if present)
        let audio_path = if let Some(path) = raw_audio_path_opt {
            match ValidatedAudioPath::new(&path) {
                Ok(p) => Some(p),
                Err(e) => {
                    warn!("Invalid audio path: {}", e);
                    None
                }
            }
        } else {
            None
        };

        // Capture timestamp NOW for pairing audio with transcript
        let recording_timestamp = chrono::Local::now();

        let config = self.config.read().await.clone();
        let language = config.whisper_language;
        let language_opt = Some(language.as_str());
        let use_local_stt = config.use_local_stt;
        let raw_save_enabled = raw_save_enabled();

        let cloud_config = if use_local_stt {
            None
        } else {
            match (config.stt_endpoint.clone(), config.stt_api_key.clone()) {
                (Some(endpoint), Some(api_key))
                    if !endpoint.trim().is_empty() && !api_key.trim().is_empty() =>
                {
                    Some((endpoint, api_key))
                }
                _ => None,
            }
        };

        let mut raw_text_opt = None;
        let mut cloud_text_opt = None;
        let mut cloud_handle: Option<JoinHandle<Result<String>>> = None;

        // Start cloud transcription in parallel (for early mismatch detection)
        if let Some((cloud_endpoint, cloud_api_key)) = cloud_config {
            if let Some(path) = &audio_path {
                let cloud_path = path.as_path().to_path_buf();
                let cloud_language = language_opt.map(str::to_string);
                cloud_handle = Some(tokio::spawn(async move {
                    crate::client::transcribe_cloud(
                        &cloud_path,
                        cloud_language.as_deref(),
                        &cloud_endpoint,
                        &cloud_api_key,
                    )
                    .await
                }));
            } else {
                warn!("Cloud STT disabled: no audio file available");
            }
        } else if !use_local_stt {
            warn!("Cloud STT disabled: STT_ENDPOINT/STT_API_KEY missing");
        }

        // 1. Try Streaming Result (Local)
        if use_local_stt {
            if !streaming_text.trim().is_empty() {
                info!(
                    "Using streaming transcription result ({} chars)",
                    streaming_text.len()
                );
                raw_text_opt = Some(streaming_text);
            } else {
                warn!("Streaming returned empty text");
            }
        }

        // 2. Fallback to Cloud if needed
        if raw_text_opt.is_none() {
            if let Some(handle) = cloud_handle.take() {
                info!("Falling back to cloud STT (LibraxisAI)");
                match handle.await {
                    Ok(Ok(text)) => {
                        cloud_text_opt = Some(text.clone());
                        raw_text_opt = Some(text);
                    }
                    Ok(Err(e)) => error!("Cloud transcription failed: {}", e),
                    Err(e) => error!("Cloud transcription task failed: {}", e),
                }
            } else if !use_local_stt {
                warn!("Cloud fallback unavailable (cloud disabled or missing credentials)");
            }
        }

        let raw_text = raw_text_opt.ok_or_else(|| anyhow::anyhow!("Empty transcript"))?;

        info!("Raw transcript captured ({} chars)", raw_text.len());

        // ALWAYS-ON: Final post-processing pass (lexicon + cleanup + semantic gate)
        // This ensures ALL output paths receive clean text regardless of mode.
        // Contract: every chunk/transcript passes through StreamPostProcessor before
        // reaching overlay, clipboard, augmentation, or dataset.
        let clean_text = {
            let mut finalizer = StreamPostProcessor::new();
            finalizer
                .process(&raw_text)
                .unwrap_or_else(|| raw_text.clone())
        };
        info!(
            "Post-processed transcript ({} chars, delta={})",
            clean_text.len(),
            raw_text.len() as i64 - clean_text.len() as i64
        );

        if raw_save_enabled {
            let raw_entry = crate::state::history::save_entry_with_timestamp_and_slug(
                &raw_text,
                Some(recording_timestamp),
                crate::state::history::TranscriptKind::Raw,
                Some(&raw_text),
            );
            info!("Raw transcript saved: {}", raw_entry.path.display());
            crate::voice_chat_ui::update_drawer_after_save(raw_entry.path.as_path());
        }

        // Check for repetition loops (Whisper hallucination like "Wielki, Wielki, Wielki...")
        let has_repetition = crate::ai_formatting::has_repetition_loop(&clean_text);
        if has_repetition {
            warn!("Detected repetition loop in transcription - will clean up");
        }

        let chat_active = crate::voice_chat_ui::is_voice_chat_overlay_visible();

        // Determine final text based on mode (NEW architecture):
        //
        // 1. Ctrl+Shift (assistive=true): ALWAYS AI augmentation (expands, creates plans)
        // 2. Ctrl Hold (force_raw=true): ALWAYS raw transcript (ignores AI toggle)
        // 3. Left double Option (force_ai=true): ALWAYS AI formatting
        // 4. Toggle (neither): respects AI_FORMATTING_ENABLED toggle
        //
        // This allows users to choose mode via hotkey:
        // - Quick dictation? → Ctrl (fast, raw)
        // - Need formatting? → Double Option (respects setting)
        // - Need AI help? → Ctrl+Shift (always AI)
        let (formatted_text, output_kind) = if assistive {
            // Ctrl+Shift: ALWAYS augmentation mode (AI expands content)
            info!("Assistive mode (Ctrl+Shift): augmenting transcript via AI");

            if chat_active {
                crate::voice_chat_ui::add_voice_chat_user_message(&clean_text);
                crate::voice_chat_ui::show_agent_tab();
                crate::voice_chat_ui::set_voice_chat_sending(true);
                crate::voice_chat_ui::update_voice_chat_status("Thinking...");
            }

            let lang_str = language_opt.map(String::from);

            // Determine streaming mode from config
            let transcript_mode = config.transcript_send_mode;
            let use_streaming = matches!(
                transcript_mode,
                crate::config::TranscriptSendMode::Streaming
            );

            // Callback for streaming AI response to overlay
            let delta_callback = if use_streaming && chat_active {
                Some(Arc::new(|text: &str| {
                    crate::voice_chat_ui::append_voice_chat_assistant_delta(text);
                }) as Arc<dyn Fn(&str) + Send + Sync>)
            } else {
                None
            };

            let result = crate::ai_formatting::format_text_with_status(
                &clean_text,
                lang_str.as_deref(),
                true,
                delta_callback,
            )
            .await;
            let kind = match result.status {
                crate::ai_formatting::AiFormatStatus::Applied => {
                    if chat_active {
                        // Display AI response in overlay
                        crate::voice_chat_ui::update_voice_chat_status("AI Response:");
                        crate::voice_chat_ui::set_voice_chat_text(&result.text);
                        info!(
                            "Assistive response displayed in overlay ({} chars)",
                            result.text.len()
                        );
                    }
                    crate::state::history::TranscriptKind::Ai
                }
                crate::ai_formatting::AiFormatStatus::Failed => {
                    if chat_active {
                        crate::voice_chat_ui::update_voice_chat_status("AI Failed");
                        crate::voice_chat_ui::add_voice_chat_error_message("AI Failed");
                    }
                    crate::state::history::TranscriptKind::AiFailed
                }
                crate::ai_formatting::AiFormatStatus::Skipped => {
                    if chat_active {
                        crate::voice_chat_ui::set_voice_chat_sending(false);
                    }
                    crate::state::history::TranscriptKind::Raw
                }
            };
            (result.text, kind)
        } else if force_raw {
            // Ctrl Hold: ALWAYS raw transcript (fast dictation mode)
            // Post-processed clean_text is used (lexicon + cleanup already applied)
            if has_repetition {
                info!("Raw mode (Ctrl): applying local repetition cleanup on post-processed text");
                (
                    crate::ai_formatting::remove_simple_repetitions(&clean_text),
                    crate::state::history::TranscriptKind::Raw,
                )
            } else {
                info!("Raw mode (Ctrl): using post-processed transcript");
                (
                    clean_text.clone(),
                    crate::state::history::TranscriptKind::Raw,
                )
            }
        } else if force_ai {
            // Left double Option: ALWAYS formatting (no augmentation)
            let should_use_ai = crate::ai_formatting::has_api_key();
            if should_use_ai {
                info!("Formatting mode (Left Option): correcting transcript via AI");

                if chat_active {
                    crate::voice_chat_ui::add_voice_chat_user_message(&clean_text);
                    crate::voice_chat_ui::set_voice_chat_sending(true);
                    crate::voice_chat_ui::update_voice_chat_status("Formatting...");
                }

                let lang_str = language_opt.map(String::from);

                // Determine streaming mode from config
                let transcript_mode = config.transcript_send_mode;
                let use_streaming = matches!(
                    transcript_mode,
                    crate::config::TranscriptSendMode::Streaming
                );

                // Callback for streaming AI response to overlay
                let delta_callback = if use_streaming && chat_active {
                    Some(Arc::new(|text: &str| {
                        crate::voice_chat_ui::append_voice_chat_assistant_delta(text);
                    }) as Arc<dyn Fn(&str) + Send + Sync>)
                } else {
                    None
                };

                let result = crate::ai_formatting::format_text_with_status(
                    &clean_text,
                    lang_str.as_deref(),
                    false,
                    delta_callback,
                )
                .await;
                let kind = match result.status {
                    crate::ai_formatting::AiFormatStatus::Applied => {
                        if chat_active {
                            // Display formatted text in overlay
                            crate::voice_chat_ui::update_voice_chat_status("Formatted:");
                            crate::voice_chat_ui::set_voice_chat_text(&result.text);
                            info!(
                                "Formatted response displayed in overlay ({} chars)",
                                result.text.len()
                            );
                        }
                        crate::state::history::TranscriptKind::Ai
                    }
                    crate::ai_formatting::AiFormatStatus::Failed => {
                        if chat_active {
                            crate::voice_chat_ui::update_voice_chat_status("Formatting Failed");
                            crate::voice_chat_ui::add_voice_chat_error_message("Formatting Failed");
                        }
                        crate::state::history::TranscriptKind::AiFailed
                    }
                    crate::ai_formatting::AiFormatStatus::Skipped => {
                        if chat_active {
                            crate::voice_chat_ui::set_voice_chat_sending(false);
                        }
                        crate::state::history::TranscriptKind::Raw
                    }
                };
                (result.text, kind)
            } else if has_repetition {
                info!("Formatting mode (Left Option): AI unavailable, cleaning repetitions");
                (
                    crate::ai_formatting::remove_simple_repetitions(&clean_text),
                    crate::state::history::TranscriptKind::Raw,
                )
            } else {
                info!(
                    "Formatting mode (Left Option): AI unavailable, using post-processed transcript"
                );
                (
                    clean_text.clone(),
                    crate::state::history::TranscriptKind::Raw,
                )
            }
        } else {
            // Double Option: respects AI Formatting toggle setting
            let ai_formatting_enabled = config.ai_formatting_enabled;
            let should_use_ai = ai_formatting_enabled && crate::ai_formatting::has_api_key();

            if should_use_ai {
                // Toggle ON: formatting only (no augmentation)
                info!("Formatting mode (Toggle): correcting transcript via AI");

                if chat_active {
                    crate::voice_chat_ui::add_voice_chat_user_message(&clean_text);
                    crate::voice_chat_ui::set_voice_chat_sending(true);
                    crate::voice_chat_ui::update_voice_chat_status("Formatting...");
                }

                let lang_str = language_opt.map(String::from);

                // Determine streaming mode from config
                let transcript_mode = config.transcript_send_mode;
                let use_streaming = matches!(
                    transcript_mode,
                    crate::config::TranscriptSendMode::Streaming
                );

                // Callback for streaming AI response to overlay
                let delta_callback = if use_streaming && chat_active {
                    Some(Arc::new(|text: &str| {
                        crate::voice_chat_ui::append_voice_chat_assistant_delta(text);
                    }) as Arc<dyn Fn(&str) + Send + Sync>)
                } else {
                    None
                };

                let result = crate::ai_formatting::format_text_with_status(
                    &clean_text,
                    lang_str.as_deref(),
                    false,
                    delta_callback,
                )
                .await;
                let kind = match result.status {
                    crate::ai_formatting::AiFormatStatus::Applied => {
                        if chat_active {
                            // Display formatted text in overlay
                            crate::voice_chat_ui::update_voice_chat_status("Formatted:");
                            crate::voice_chat_ui::set_voice_chat_text(&result.text);
                            info!(
                                "Formatted response displayed in overlay ({} chars)",
                                result.text.len()
                            );
                        }
                        crate::state::history::TranscriptKind::Ai
                    }
                    crate::ai_formatting::AiFormatStatus::Failed => {
                        if chat_active {
                            crate::voice_chat_ui::update_voice_chat_status("Formatting Failed");
                            crate::voice_chat_ui::add_voice_chat_error_message("Formatting Failed");
                        }
                        crate::state::history::TranscriptKind::AiFailed
                    }
                    crate::ai_formatting::AiFormatStatus::Skipped => {
                        if chat_active {
                            crate::voice_chat_ui::set_voice_chat_sending(false);
                        }
                        crate::state::history::TranscriptKind::Raw
                    }
                };
                (result.text, kind)
            } else if has_repetition {
                // Toggle OFF with repetition: local cleanup only
                info!("Raw mode (Toggle OFF): applying local repetition cleanup");
                (
                    crate::ai_formatting::remove_simple_repetitions(&clean_text),
                    crate::state::history::TranscriptKind::Raw,
                )
            } else {
                // Toggle OFF: using post-processed transcript
                info!("Raw mode (Toggle OFF): using post-processed transcript");
                (
                    clean_text.clone(),
                    crate::state::history::TranscriptKind::Raw,
                )
            }
        };

        let mode_label = if assistive {
            "assistive"
        } else if force_raw {
            "raw"
        } else if force_ai {
            "format"
        } else {
            "toggle"
        };
        info!(
            "Final transcript ready ({} chars, mode={})",
            formatted_text.len(),
            mode_label
        );

        // Save audio to transcriptions folder if enabled (pair with RAW for reports)
        if config.dump_audio_logs
            && let Some(path) = &audio_path
        {
            crate::state::history::save_audio(
                path.as_path(),
                recording_timestamp,
                Some(&raw_text),
                crate::state::history::TranscriptKind::Raw,
            );
        }

        // Paste the text into the active application
        clipboard::paste_text(&formatted_text).context("Failed to paste text")?;

        info!("Text pasted successfully");

        // Save final transcript (skip duplicate when RAW already stored and unchanged)
        let needs_final_save = !raw_save_enabled
            || output_kind != crate::state::history::TranscriptKind::Raw
            || formatted_text.trim() != raw_text.trim();
        if needs_final_save {
            let entry = crate::state::history::save_entry_with_timestamp_and_slug(
                &formatted_text,
                Some(recording_timestamp),
                output_kind,
                Some(&raw_text),
            );
            info!("Transcript saved: {}", entry.path.display());
            crate::voice_chat_ui::refresh_drawer();
        } else {
            info!("Final transcript matches RAW; skipping duplicate save");
        }

        if let Some(cloud_text) = cloud_text_opt {
            let entry = crate::state::history::save_entry_with_timestamp_and_slug(
                &cloud_text,
                Some(recording_timestamp),
                crate::state::history::TranscriptKind::Cloud,
                Some(&raw_text),
            );
            info!("Cloud transcript saved: {}", entry.path.display());
        } else if let Some(handle) = cloud_handle {
            let slug_hint = raw_text.clone();
            let timestamp = recording_timestamp;
            tokio::spawn(async move {
                match handle.await {
                    Ok(Ok(text)) => {
                        let entry = crate::state::history::save_entry_with_timestamp_and_slug(
                            &text,
                            Some(timestamp),
                            crate::state::history::TranscriptKind::Cloud,
                            Some(&slug_hint),
                        );
                        info!("Cloud transcript saved: {}", entry.path.display());
                    }
                    Ok(Err(e)) => error!("Cloud transcription failed: {}", e),
                    Err(e) => error!("Cloud transcription task failed: {}", e),
                }
            });
        }

        Ok(())
    }

    /// Force reset to IDLE state without stopping recorder.
    ///
    /// This is the nuclear option - use only when state is corrupted
    /// or during crash recovery.
    pub async fn reset(&self) {
        warn!("Forcing state reset to IDLE (recovery mode)");
        self.reset_state().await;
    }

    /// Internal helper to reset all state variables
    async fn reset_state(&self) {
        *self.state.write().await = State::Idle;
        *self.assistive_mode.write().await = false;
        *self.force_raw_mode.write().await = false;
        *self.force_ai_mode.write().await = false;
        *self.session_id.write().await = None;

        // Hide UI indicators
        hide_hold_badge();

        // Update tray status
        let _ = update_tray_status(TrayStatus::Idle);

        info!("State reset to IDLE complete");
    }

    /// Check if controller is in a recording state
    pub async fn is_recording(&self) -> bool {
        matches!(
            self.current_state().await,
            State::RecHold | State::RecToggle
        )
    }

    /// Check if controller is busy processing
    pub async fn is_busy(&self) -> bool {
        self.current_state().await == State::Busy
    }
}

impl Default for RecordingController {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests;
