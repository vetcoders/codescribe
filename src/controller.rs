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

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tokio::sync::{Mutex, RwLock};
use tokio::task::JoinHandle;
use tracing::{debug, error, info, warn};
use uuid::Uuid;

/// A validated audio file path that is guaranteed to be within allowed directories.
///
/// This newtype wrapper ensures at the type level that the path has been validated
/// against path traversal attacks before any file operations are performed.
#[derive(Debug, Clone)]
struct ValidatedAudioPath(PathBuf);

impl ValidatedAudioPath {
    /// Create a new ValidatedAudioPath after security validation.
    ///
    /// This prevents path traversal attacks by ensuring the path:
    /// 1. Exists and is a file
    /// 2. Is within an allowed directory (temp dir or ~/.codescribe)
    /// 3. After canonicalization, still resolves to an allowed directory
    ///
    /// Returns Ok(ValidatedAudioPath) if valid, or an error if validation fails.
    fn new(path: &Path) -> Result<Self> {
        // Path must exist
        if !path.exists() {
            anyhow::bail!("Audio file does not exist: {:?}", path);
        }

        // Must be a file, not a directory
        if !path.is_file() {
            anyhow::bail!("Audio path is not a file: {:?}", path);
        }

        // Canonicalize to resolve symlinks and get absolute path
        let canonical = path
            .canonicalize()
            .with_context(|| format!("Failed to canonicalize audio path: {:?}", path))?;

        // Define allowed directories
        let temp_dir = std::env::temp_dir();
        let home_codescribe = directories::BaseDirs::new()
            .map(|b| b.home_dir().join(".codescribe"))
            .unwrap_or_else(|| PathBuf::from(".codescribe"));

        // Canonicalize allowed dirs (they might not exist yet)
        let allowed_dirs: Vec<PathBuf> = vec![
            temp_dir.canonicalize().unwrap_or(temp_dir),
            home_codescribe.canonicalize().unwrap_or(home_codescribe),
        ];

        // Check if canonical path starts with any allowed directory
        let is_allowed = allowed_dirs
            .iter()
            .any(|allowed| canonical.starts_with(allowed));

        if !is_allowed {
            anyhow::bail!(
                "Audio path {:?} is outside allowed directories. Canonical: {:?}",
                path,
                canonical
            );
        }

        Ok(Self(canonical))
    }

    /// Get a reference to the validated path.
    fn as_path(&self) -> &Path {
        &self.0
    }
}

use crate::config::Config;
use crate::config::models::ModelManager;
use crate::tray::{TrayStatus, update_tray_status};
use crate::{BadgeMode, hide_hold_badge, show_badge_for_mode};

// TODO: Re-enable when implementing recorder
use crate::audio::streaming_recorder::StreamingRecorder;

/// Application state enum
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    /// Waiting for user input
    Idle,
    /// Recording in hold-to-talk mode
    RecHold,
    /// Recording in toggle mode
    RecToggle,
    /// Processing transcription and formatting
    Busy,
}

impl std::fmt::Display for State {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            State::Idle => write!(f, "IDLE"),
            State::RecHold => write!(f, "REC_HOLD"),
            State::RecToggle => write!(f, "REC_TOGGLE"),
            State::Busy => write!(f, "BUSY"),
        }
    }
}

/// Hotkey event types
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HotkeyType {
    Hold,
    Toggle,
}

/// Hotkey action types
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HotkeyAction {
    Down,
    Up,
    Press,
}

/// Complete hotkey event with metadata
#[derive(Debug, Clone)]
pub struct HotkeyInput {
    pub key_type: HotkeyType,
    pub action: HotkeyAction,
    pub assistive: bool,
    pub force_ai: bool,
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
            crate::append_voice_chat_delta(delta);
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

        Self {
            config: Arc::new(RwLock::new(config)),
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
            crate::append_voice_chat_delta(delta);
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
                self.start_toggle_recording().await?;
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
        // Check backend health before starting
        match crate::client::check_health().await {
            Ok(true) => {}
            Ok(false) => {
                warn!("Backend not ready (model loading)");
                let _ = crate::tray::update_tray_status(crate::tray::TrayStatus::Error);
                return Ok(());
            }
            Err(e) => {
                error!("Backend unavailable: {}", e);
                let _ = crate::tray::update_tray_status(crate::tray::TrayStatus::Error);
                return Ok(());
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

        let state = Arc::clone(&self.state);
        let session_id = Arc::clone(&self.session_id);
        let recorder = Arc::clone(&self.recorder);
        let delay = Duration::from_millis(delay_ms);

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

            // Start the recorder
            let mut rec = recorder.lock().await;
            if let Err(e) = rec.start(Some(language.as_str().to_string())).await {
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

            // Show live transcription overlay
            crate::clear_voice_chat_text();
            crate::show_voice_chat_overlay();

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
    async fn start_toggle_recording(&self) -> Result<()> {
        // Check backend health before starting
        match crate::client::check_health().await {
            Ok(true) => {}
            Ok(false) => {
                warn!("Backend not ready (model loading)");
                let _ = crate::tray::update_tray_status(crate::tray::TrayStatus::Error);
                return Ok(());
            }
            Err(e) => {
                error!("Backend unavailable: {}", e);
                let _ = crate::tray::update_tray_status(crate::tray::TrayStatus::Error);
                return Ok(());
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

        let language = self.config.read().await.whisper_language;

        // Reset VAD flag and set callback
        self.vad_triggered.store(false, Ordering::SeqCst);
        let vad_flag = Arc::clone(&self.vad_triggered);

        // Start the recorder with VAD callback
        let mut recorder = self.recorder.lock().await;
        recorder.recorder.set_on_vad_stop(move || {
            info!("VAD callback: setting vad_triggered flag");
            vad_flag.store(true, Ordering::SeqCst);
        });

        // Set streaming callback for overlay updates
        recorder.set_delta_callback(Some(Arc::new(|text: &str| {
            crate::voice_chat_ui::append_voice_chat_delta(text);
        })));

        recorder.start(Some(language.as_str().to_string())).await?;

        // Play start beep if enabled
        let beep_enabled = self.config.read().await.beep_on_start;
        if beep_enabled {
            crate::audio::play_sound("Tink");
        }

        // Show pulsing red badge for toggle mode (hands-off recording)
        show_badge_for_mode(BadgeMode::Toggle);

        // Show live transcription overlay
        crate::clear_voice_chat_text();
        crate::show_voice_chat_overlay();

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
            }
            Err(e) => {
                error!("Processing failed: {}", e);
                let _ = update_tray_status(TrayStatus::Idle);
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

        // Get language from config
        let language = self.config.read().await.whisper_language;
        let language_opt = Some(language.as_str());
        let use_local_stt = self.config.read().await.use_local_stt;
        let cloud_enabled = cloud_stt_enabled();
        let raw_save_enabled = raw_save_enabled();

        let mut raw_text_opt = None;
        let mut cloud_text_opt = None;
        let mut cloud_handle: Option<JoinHandle<Result<String>>> = None;

        // Start cloud transcription in parallel (for early mismatch detection)
        if cloud_enabled {
            if let Some(path) = &audio_path {
                if cloud_credentials_available() {
                    let cloud_path = path.as_path().to_path_buf();
                    let cloud_language = language_opt.map(str::to_string);
                    cloud_handle = Some(tokio::spawn(async move {
                        crate::client::transcribe(&cloud_path, cloud_language.as_deref()).await
                    }));
                } else {
                    warn!("Cloud STT disabled: STT_ENDPOINT/STT_API_KEY missing");
                }
            } else {
                warn!("Cloud STT disabled: no audio file available");
            }
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
            } else if cloud_enabled {
                warn!("Cloud fallback unavailable (cloud disabled or missing credentials)");
            }
        }

        let raw_text = raw_text_opt.ok_or_else(|| anyhow::anyhow!("Empty transcript"))?;

        info!("Raw transcript captured ({} chars)", raw_text.len());

        if raw_save_enabled {
            let raw_entry = crate::state::history::save_entry_with_timestamp_and_slug(
                &raw_text,
                Some(recording_timestamp),
                crate::state::history::TranscriptKind::Raw,
                Some(&raw_text),
            );
            info!("Raw transcript saved: {}", raw_entry.path.display());
        }

        // Check for repetition loops (Whisper hallucination like "Wielki, Wielki, Wielki...")
        let has_repetition = crate::ai_formatting::has_repetition_loop(&raw_text);
        if has_repetition {
            warn!("Detected repetition loop in transcription - will clean up");
        }

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

            // Update overlay status to show AI is thinking
            crate::voice_chat_ui::update_voice_chat_status("Thinking...");

            let lang_str = language_opt.map(String::from);

            // Callback for streaming AI response to overlay
            let delta_callback = Arc::new(|text: &str| {
                crate::voice_chat_ui::append_voice_chat_delta(text);
            });

            let result = crate::ai_formatting::format_text_with_status(
                &raw_text,
                lang_str.as_deref(),
                true,
                Some(delta_callback),
            )
            .await;
            let kind = match result.status {
                crate::ai_formatting::AiFormatStatus::Applied => {
                    // Display AI response in overlay
                    crate::voice_chat_ui::update_voice_chat_status("AI Response:");
                    crate::voice_chat_ui::set_voice_chat_text(&result.text);
                    info!(
                        "Assistive response displayed in overlay ({} chars)",
                        result.text.len()
                    );

                    // Auto-hide overlay after 10 seconds
                    // Created by M&K (c)2026 VetCoders
                    tokio::spawn(async {
                        tokio::time::sleep(Duration::from_secs(10)).await;
                        crate::voice_chat_ui::hide_voice_chat_overlay();
                    });

                    crate::state::history::TranscriptKind::Ai
                }
                crate::ai_formatting::AiFormatStatus::Failed => {
                    crate::voice_chat_ui::update_voice_chat_status("AI Failed");

                    // Auto-hide overlay after 3 seconds on failure
                    tokio::spawn(async {
                        tokio::time::sleep(Duration::from_secs(3)).await;
                        crate::voice_chat_ui::hide_voice_chat_overlay();
                    });

                    crate::state::history::TranscriptKind::AiFailed
                }
                crate::ai_formatting::AiFormatStatus::Skipped => {
                    crate::state::history::TranscriptKind::Raw
                }
            };
            (result.text, kind)
        } else if force_raw {
            // Ctrl Hold: ALWAYS raw transcript (fast dictation mode)
            if has_repetition {
                info!("Raw mode (Ctrl): applying local repetition cleanup only");
                (
                    crate::ai_formatting::remove_simple_repetitions(&raw_text),
                    crate::state::history::TranscriptKind::Raw,
                )
            } else {
                info!("Raw mode (Ctrl): using raw transcript");
                (raw_text.clone(), crate::state::history::TranscriptKind::Raw)
            }
        } else if force_ai {
            // Left double Option: ALWAYS formatting (no augmentation)
            let should_use_ai = crate::ai_formatting::has_api_key();
            if should_use_ai {
                info!("Formatting mode (Left Option): correcting transcript via AI");
                let lang_str = language_opt.map(String::from);
                let result = crate::ai_formatting::format_text_with_status(
                    &raw_text,
                    lang_str.as_deref(),
                    false,
                    None,
                )
                .await;
                let kind = match result.status {
                    crate::ai_formatting::AiFormatStatus::Applied => {
                        crate::state::history::TranscriptKind::Ai
                    }
                    crate::ai_formatting::AiFormatStatus::Failed => {
                        crate::state::history::TranscriptKind::AiFailed
                    }
                    crate::ai_formatting::AiFormatStatus::Skipped => {
                        crate::state::history::TranscriptKind::Raw
                    }
                };
                (result.text, kind)
            } else if has_repetition {
                info!("Formatting mode (Left Option): AI unavailable, cleaning repetitions");
                (
                    crate::ai_formatting::remove_simple_repetitions(&raw_text),
                    crate::state::history::TranscriptKind::Raw,
                )
            } else {
                info!("Formatting mode (Left Option): AI unavailable, using raw transcript");
                (raw_text.clone(), crate::state::history::TranscriptKind::Raw)
            }
        } else {
            // Double Option: respects AI Formatting toggle setting
            let ai_formatting_enabled = self.config.read().await.ai_formatting_enabled;
            let should_use_ai = ai_formatting_enabled && crate::ai_formatting::has_api_key();

            if should_use_ai {
                // Toggle ON: formatting only (no augmentation)
                info!("Formatting mode (Toggle): correcting transcript via AI");
                let lang_str = language_opt.map(String::from);
                let result = crate::ai_formatting::format_text_with_status(
                    &raw_text,
                    lang_str.as_deref(),
                    false,
                    None,
                )
                .await;
                let kind = match result.status {
                    crate::ai_formatting::AiFormatStatus::Applied => {
                        crate::state::history::TranscriptKind::Ai
                    }
                    crate::ai_formatting::AiFormatStatus::Failed => {
                        crate::state::history::TranscriptKind::AiFailed
                    }
                    crate::ai_formatting::AiFormatStatus::Skipped => {
                        crate::state::history::TranscriptKind::Raw
                    }
                };
                (result.text, kind)
            } else if has_repetition {
                // Toggle OFF with repetition: local cleanup only
                info!("Raw mode (Toggle OFF): applying local repetition cleanup");
                (
                    crate::ai_formatting::remove_simple_repetitions(&raw_text),
                    crate::state::history::TranscriptKind::Raw,
                )
            } else {
                // Toggle OFF: raw transcript
                info!("Raw mode (Toggle OFF): using raw transcript");
                (raw_text.clone(), crate::state::history::TranscriptKind::Raw)
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
        if self.config.read().await.dump_audio_logs
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
        crate::clipboard::paste_text(&formatted_text).context("Failed to paste text")?;

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
}

impl RecordingController {
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

fn env_bool(key: &str) -> bool {
    std::env::var(key)
        .ok()
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false)
}

fn raw_save_enabled() -> bool {
    !env_bool("CODESCRIBE_QUALITY_DISABLE_RAW_SAVE")
}

fn cloud_stt_enabled() -> bool {
    !env_bool("CODESCRIBE_QUALITY_DISABLE_CLOUD")
}

fn cloud_credentials_available() -> bool {
    std::env::var("STT_ENDPOINT").is_ok() && std::env::var("STT_API_KEY").is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;

    #[tokio::test]
    async fn test_initial_state() {
        let controller = RecordingController::new();
        assert_eq!(controller.current_state().await, State::Idle);
    }

    #[tokio::test]
    #[serial]
    #[ignore = "requires audio hardware"]
    async fn test_hold_down_schedules_delayed_start() {
        let controller = RecordingController::new();
        // Override hold delay for faster test
        controller.config.write().await.hold_start_delay_ms = 100;

        let event = HotkeyInput {
            key_type: HotkeyType::Hold,
            action: HotkeyAction::Down,
            assistive: false,
            force_ai: false,
        };

        controller.handle_hotkey_event(event).await.unwrap();

        // Should still be IDLE (delay not elapsed)
        assert_eq!(controller.current_state().await, State::Idle);

        // Wait for delay to elapse
        tokio::time::sleep(Duration::from_millis(150)).await;

        // Should now be REC_HOLD
        assert_eq!(controller.current_state().await, State::RecHold);
    }

    #[tokio::test]
    #[serial]
    #[ignore = "requires audio hardware"]
    async fn test_hold_up_before_delay_cancels() {
        let controller = RecordingController::new();
        // Override hold delay for faster test
        controller.config.write().await.hold_start_delay_ms = 200;

        // Press down
        let down_event = HotkeyInput {
            key_type: HotkeyType::Hold,
            action: HotkeyAction::Down,
            assistive: false,
            force_ai: false,
        };
        controller.handle_hotkey_event(down_event).await.unwrap();

        // Release before delay elapses
        tokio::time::sleep(Duration::from_millis(50)).await;
        let up_event = HotkeyInput {
            key_type: HotkeyType::Hold,
            action: HotkeyAction::Up,
            assistive: false,
            force_ai: false,
        };
        controller.handle_hotkey_event(up_event).await.unwrap();

        // Wait past the original delay
        tokio::time::sleep(Duration::from_millis(200)).await;

        // Should still be IDLE (start was cancelled)
        assert_eq!(controller.current_state().await, State::Idle);
    }

    #[tokio::test]
    #[serial]
    #[ignore = "requires audio hardware"]
    async fn test_toggle_starts_immediately() {
        let controller = RecordingController::new();

        let event = HotkeyInput {
            key_type: HotkeyType::Toggle,
            action: HotkeyAction::Press,
            assistive: true,
            force_ai: false,
        };

        controller.handle_hotkey_event(event).await.unwrap();

        // Should immediately transition to REC_TOGGLE
        assert_eq!(controller.current_state().await, State::RecToggle);
    }

    #[tokio::test]
    async fn test_busy_state_ignores_hotkeys() {
        let controller = RecordingController::new();

        // Manually set to BUSY
        *controller.state.write().await = State::Busy;

        let event = HotkeyInput {
            key_type: HotkeyType::Toggle,
            action: HotkeyAction::Press,
            assistive: false,
            force_ai: false,
        };

        controller.handle_hotkey_event(event).await.unwrap();

        // Should remain BUSY
        assert_eq!(controller.current_state().await, State::Busy);
    }

    #[tokio::test]
    async fn test_state_display() {
        assert_eq!(State::Idle.to_string(), "IDLE");
        assert_eq!(State::RecHold.to_string(), "REC_HOLD");
        assert_eq!(State::RecToggle.to_string(), "REC_TOGGLE");
        assert_eq!(State::Busy.to_string(), "BUSY");
    }

    #[tokio::test]
    async fn test_reset_from_busy() {
        let controller = RecordingController::new();

        // Manually set to BUSY (simulating stuck state)
        *controller.state.write().await = State::Busy;
        assert!(controller.is_busy().await);

        // Reset should force back to IDLE
        controller.reset().await;
        assert_eq!(controller.current_state().await, State::Idle);
        assert!(!controller.is_busy().await);
    }

    #[tokio::test]
    async fn test_is_recording_states() {
        let controller = RecordingController::new();

        // IDLE - not recording
        assert!(!controller.is_recording().await);

        // REC_HOLD - recording
        *controller.state.write().await = State::RecHold;
        assert!(controller.is_recording().await);

        // REC_TOGGLE - recording
        *controller.state.write().await = State::RecToggle;
        assert!(controller.is_recording().await);

        // BUSY - not recording (processing)
        *controller.state.write().await = State::Busy;
        assert!(!controller.is_recording().await);
    }

    // ============================================================
    // NEW HOTKEY ARCHITECTURE TESTS (force_raw_mode logic)
    // ============================================================
    //
    // These tests verify the new mode determination logic:
    // - Ctrl Hold (no Shift) → force_raw=true, assistive=false → RAW mode
    // - Ctrl+Shift Hold → force_raw=false, assistive=true → Assistive mode
    // - Left Double Option → force_ai=true, assistive=false → Formatting mode
    // - Toggle (no force_ai) → respects AI_FORMATTING_ENABLED setting

    #[tokio::test]
    async fn test_hold_down_sets_force_raw_mode() {
        let controller = RecordingController::new();

        // Verify initial state
        assert!(!*controller.force_raw_mode.read().await);
        assert!(!*controller.assistive_mode.read().await);

        // Hold Down without Shift → force_raw=true
        let event = HotkeyInput {
            key_type: HotkeyType::Hold,
            action: HotkeyAction::Down,
            assistive: false,
            force_ai: false,
        };
        controller.handle_hotkey_event(event).await.unwrap();

        // force_raw should be true, assistive should be false
        assert!(
            *controller.force_raw_mode.read().await,
            "Hold Down should set force_raw_mode=true"
        );
        assert!(
            !*controller.assistive_mode.read().await,
            "Hold Down without Shift should keep assistive_mode=false"
        );
    }

    #[tokio::test]
    async fn test_toggle_press_does_not_set_force_raw_mode() {
        let controller = RecordingController::new();

        // Toggle Press → force_raw=false (respects AI_FORMATTING_ENABLED)
        let event = HotkeyInput {
            key_type: HotkeyType::Toggle,
            action: HotkeyAction::Press,
            assistive: false,
            force_ai: false,
        };
        controller.handle_hotkey_event(event).await.unwrap();

        // force_raw should be false
        assert!(
            !*controller.force_raw_mode.read().await,
            "Toggle should NOT set force_raw_mode (respects setting)"
        );
        assert!(
            !*controller.assistive_mode.read().await,
            "Toggle without assistive should keep assistive_mode=false"
        );
    }

    #[tokio::test]
    async fn test_toggle_press_sets_force_ai_mode() {
        let controller = RecordingController::new();

        let event = HotkeyInput {
            key_type: HotkeyType::Toggle,
            action: HotkeyAction::Press,
            assistive: false,
            force_ai: true,
        };
        controller.handle_hotkey_event(event).await.unwrap();

        assert!(
            *controller.force_ai_mode.read().await,
            "Toggle with force_ai should set force_ai_mode=true"
        );
    }

    #[tokio::test]
    async fn test_hold_with_shift_sets_assistive_not_force_raw() {
        let controller = RecordingController::new();

        // Hold Down WITH Shift → assistive=true, force_raw=false
        let event = HotkeyInput {
            key_type: HotkeyType::Hold,
            action: HotkeyAction::Down,
            assistive: true,
            force_ai: false, // Shift was held from the start (Ctrl+Shift)
        };
        controller.handle_hotkey_event(event).await.unwrap();

        // assistive should be true, force_raw should be false
        assert!(
            *controller.assistive_mode.read().await,
            "Hold with Shift should set assistive_mode=true"
        );
        assert!(
            !*controller.force_raw_mode.read().await,
            "Hold with Shift should NOT set force_raw_mode (Assistive takes precedence)"
        );
    }

    #[tokio::test]
    async fn test_shift_upgrade_mid_hold_overrides_force_raw() {
        let controller = RecordingController::new();

        // First: Hold Down without Shift (starts as RAW mode)
        let down_event = HotkeyInput {
            key_type: HotkeyType::Hold,
            action: HotkeyAction::Down,
            assistive: false,
            force_ai: false,
        };
        controller.handle_hotkey_event(down_event).await.unwrap();

        // Verify RAW mode is set
        assert!(*controller.force_raw_mode.read().await);
        assert!(!*controller.assistive_mode.read().await);

        // Now: User adds Shift mid-hold (upgrade to Assistive)
        // This comes as another event with assistive=true
        let upgrade_event = HotkeyInput {
            key_type: HotkeyType::Hold,
            action: HotkeyAction::Down, // Still "down" - modifier flags changed
            assistive: true,
            force_ai: false,
        };
        controller.handle_hotkey_event(upgrade_event).await.unwrap();

        // Should upgrade to Assistive, force_raw should be cleared
        assert!(
            *controller.assistive_mode.read().await,
            "Shift added mid-hold should upgrade to assistive_mode=true"
        );
        assert!(
            !*controller.force_raw_mode.read().await,
            "Shift upgrade should clear force_raw_mode"
        );
    }

    #[tokio::test]
    async fn test_hold_up_preserves_mode_flags_when_idle() {
        let controller = RecordingController::new();

        // Set up flags manually (simulating mid-session state)
        *controller.force_raw_mode.write().await = true;
        *controller.assistive_mode.write().await = false;
        // Keep state IDLE - Up event in IDLE just cancels pending hold start

        // Hold Up when IDLE should NOT modify the flags
        let up_event = HotkeyInput {
            key_type: HotkeyType::Hold,
            action: HotkeyAction::Up,
            assistive: false,
            force_ai: false,
        };
        controller.handle_hotkey_event(up_event).await.unwrap();

        // Flags should still be set (Up action doesn't touch them in IDLE state)
        assert!(
            *controller.force_raw_mode.read().await,
            "Hold Up in IDLE should preserve force_raw_mode"
        );
    }

    #[tokio::test]
    #[serial]
    #[ignore = "requires audio hardware"]
    async fn test_hold_up_triggers_finish_recording() {
        // This test verifies that Hold Up in REC_HOLD state triggers finish_recording
        // which reads force_raw_mode and assistive_mode before processing.
        // Requires audio hardware to actually record/transcribe.
        let controller = RecordingController::new();
        *controller.state.write().await = State::RecHold;
        *controller.force_raw_mode.write().await = true;

        let up_event = HotkeyInput {
            key_type: HotkeyType::Hold,
            action: HotkeyAction::Up,
            assistive: false,
            force_ai: false,
        };
        let result = controller.handle_hotkey_event(up_event).await;
        if let Err(err) = result {
            assert!(
                err.to_string().contains("Empty transcript"),
                "Unexpected error: {err}"
            );
        }

        // After finish_recording, flags should be reset to false
        assert_eq!(controller.current_state().await, State::Idle);
        assert!(!*controller.force_raw_mode.read().await);
    }

    #[tokio::test]
    async fn test_reset_clears_all_mode_flags() {
        let controller = RecordingController::new();

        // Set up various flags
        *controller.state.write().await = State::RecHold;
        *controller.force_raw_mode.write().await = true;
        *controller.force_ai_mode.write().await = true;
        *controller.assistive_mode.write().await = true;
        *controller.session_id.write().await = Some("test-session".to_string());

        // Reset should clear everything
        controller.reset().await;

        assert_eq!(controller.current_state().await, State::Idle);
        assert!(
            !*controller.force_raw_mode.read().await,
            "reset should clear force_raw_mode"
        );
        assert!(
            !*controller.assistive_mode.read().await,
            "reset should clear assistive_mode"
        );
        assert!(
            !*controller.force_ai_mode.read().await,
            "reset should clear force_ai_mode"
        );
        assert!(
            controller.session_id.read().await.is_none(),
            "reset should clear session_id"
        );
    }

    #[tokio::test]
    async fn test_mode_matrix_coverage() {
        // This test documents all possible mode combinations:
        //
        // | Hotkey          | force_raw | assistive | Result                    |
        // |-----------------|-----------|-----------|---------------------------|
        // | Ctrl Hold       | true      | false     | RAW (ignore AI setting)   |
        // | Ctrl+Shift Hold | false     | true      | Assistive (always AI)     |
        // | Left Double Opt | false     | false     | Formatting (force AI)     |

        let controller = RecordingController::new();

        // Case 1: Ctrl Hold
        let ctrl_hold = HotkeyInput {
            key_type: HotkeyType::Hold,
            action: HotkeyAction::Down,
            assistive: false,
            force_ai: false,
        };
        controller.handle_hotkey_event(ctrl_hold).await.unwrap();
        assert!(*controller.force_raw_mode.read().await);
        assert!(!*controller.assistive_mode.read().await);

        // Reset for next case
        *controller.force_raw_mode.write().await = false;
        *controller.assistive_mode.write().await = false;

        // Case 2: Ctrl+Shift Hold
        let ctrl_shift_hold = HotkeyInput {
            key_type: HotkeyType::Hold,
            action: HotkeyAction::Down,
            assistive: true,
            force_ai: false,
        };
        controller
            .handle_hotkey_event(ctrl_shift_hold)
            .await
            .unwrap();
        assert!(!*controller.force_raw_mode.read().await);
        assert!(*controller.assistive_mode.read().await);

        // Reset for next case
        *controller.force_raw_mode.write().await = false;
        *controller.assistive_mode.write().await = false;

        // Case 3: Left Double Option (force AI)
        let double_option = HotkeyInput {
            key_type: HotkeyType::Toggle,
            action: HotkeyAction::Press,
            assistive: false,
            force_ai: true,
        };
        controller.handle_hotkey_event(double_option).await.unwrap();
        assert!(!*controller.force_raw_mode.read().await);
        assert!(!*controller.assistive_mode.read().await);
        assert!(*controller.force_ai_mode.read().await);
    }
}
