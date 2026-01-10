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

#[allow(dead_code)] // read() prepared for future voice chat pipeline
impl ValidatedAudioPath {
    /// Create a new ValidatedAudioPath after security validation.
    ///
    /// This prevents path traversal attacks by ensuring the path:
    /// 1. Exists and is a file
    /// 2. Is within an allowed directory (temp dir or ~/.CodeScribe)
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
            .map(|b| b.home_dir().join(".CodeScribe"))
            .unwrap_or_else(|| PathBuf::from(".CodeScribe"));

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

    /// Read the validated file contents.
    ///
    /// This is safe because the path has already been validated.
    async fn read(&self) -> Result<Vec<u8>> {
        tokio::fs::read(&self.0)
            .await
            .with_context(|| format!("Failed to read audio file: {:?}", self.0))
    }
}

use crate::config::Config;
use crate::local_stt::LocalWhisperEngine;
use crate::models::ModelManager;
use crate::tray::{update_tray_status, TrayStatus};
use crate::voice_chat::{VoiceChatClient, VoiceChatEvent};
use crate::voice_chat_ui;
use codescribe::{hide_hold_badge, show_badge_for_mode, BadgeMode};

// TODO: Re-enable when implementing recorder
use crate::audio::Recorder;

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
}

/// Recording controller managing state machine and lifecycle
pub struct RecordingController {
    /// Application configuration
    config: Arc<RwLock<Config>>,

    /// Current state
    state: Arc<RwLock<State>>,

    /// Audio recorder instance
    recorder: Arc<Mutex<Recorder>>,

    /// Whether assistive formatting mode is enabled
    assistive_mode: Arc<RwLock<bool>>,

    /// Current session ID for tracking
    session_id: Arc<RwLock<Option<String>>>,

    /// Task handle for delayed hold-start (800ms default)
    hold_start_task: Arc<Mutex<Option<JoinHandle<()>>>>,

    /// Lock to serialize finish_recording calls
    serial_lock: Arc<Mutex<()>>,

    /// Local STT engine (optional)
    local_stt: Arc<Mutex<Option<LocalWhisperEngine>>>,

}

impl RecordingController {
    /// Create a new recording controller with configuration loaded from disk
    pub fn new() -> Self {
        let config = Config::load();

        info!(
            "Initializing RecordingController (hold_delay={}ms, beep={}, language={:?})",
            config.hold_start_delay_ms, config.beep_on_start, config.whisper_language
        );

        let recorder = Recorder::new().expect("Failed to initialize audio recorder");

        let model_manager = ModelManager::new().expect("Failed to initialize model manager");
        let model_path = model_manager.get_model_path(&config.local_model);

        let local_stt_engine = if config.use_local_stt && model_manager.check_model_exists(&config.local_model) {
            match LocalWhisperEngine::new(&model_path) {
                Ok(engine) => {
                    info!("Local STT engine initialized with model: {}", config.local_model);
                    Some(engine)
                },
                Err(e) => {
                    warn!("Failed to initialize local STT engine: {}", e);
                    None
                }
            }
        } else {
             if config.use_local_stt {
                 warn!("Local STT enabled but model not found at: {}", model_path.display());
             }
             None
        };

        Self {
            config: Arc::new(RwLock::new(config)),
            state: Arc::new(RwLock::new(State::Idle)),
            recorder: Arc::new(Mutex::new(recorder)),
            assistive_mode: Arc::new(RwLock::new(false)),
            session_id: Arc::new(RwLock::new(None)),
            hold_start_task: Arc::new(Mutex::new(None)),
            serial_lock: Arc::new(Mutex::new(())),
            local_stt: Arc::new(Mutex::new(local_stt_engine)),
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

        let recorder = Recorder::new().expect("Failed to initialize audio recorder");

        let model_manager = ModelManager::new().expect("Failed to initialize model manager");
        let model_path = model_manager.get_model_path(&cfg.local_model);

        let local_stt_engine = if cfg.use_local_stt && model_manager.check_model_exists(&cfg.local_model) {
            match LocalWhisperEngine::new(&model_path) {
                Ok(engine) => {
                    info!("Local STT engine initialized with model: {}", cfg.local_model);
                    Some(engine)
                },
                Err(e) => {
                    warn!("Failed to initialize local STT engine: {}", e);
                    None
                }
            }
        } else {
             if cfg.use_local_stt {
                 warn!("Local STT enabled but model not found at: {}", model_path.display());
             }
             None
        };

        Self {
            config,
            state: Arc::new(RwLock::new(State::Idle)),
            recorder: Arc::new(Mutex::new(recorder)),
            assistive_mode: Arc::new(RwLock::new(false)),
            session_id: Arc::new(RwLock::new(None)),
            hold_start_task: Arc::new(Mutex::new(None)),
            serial_lock: Arc::new(Mutex::new(())),
            local_stt: Arc::new(Mutex::new(local_stt_engine)),
        }
    }

    /// Get current state
    pub async fn current_state(&self) -> State {
        *self.state.read().await
    }

    /// Cancel any pending delayed hold-start task
    async fn cancel_pending_hold_start(&self) {
        let mut task_guard = self.hold_start_task.lock().await;
        if let Some(task) = task_guard.take() {
            if !task.is_finished() {
                debug!("Cancelling pending hold-start task");
                task.abort();
                let _ = task.await; // Suppress cancellation errors
            }
        }
    }

    /// Handle hotkey event - main entry point for state machine
    ///
    /// # Arguments
    /// * `event` - The hotkey event to process
    ///
    /// This method implements the state machine logic and delegates to
    /// appropriate handlers based on current state and event type.
    pub async fn handle_hotkey_event(&self, event: HotkeyInput) -> Result<()> {
        let current_state = self.current_state().await;

        debug!(
            "Hotkey event: type={:?} action={:?} assistive={} state={}",
            event.key_type, event.action, event.assistive, current_state
        );

        // Update assistive mode on down/press
        if matches!(event.action, HotkeyAction::Down | HotkeyAction::Press) {
            *self.assistive_mode.write().await = event.assistive;
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
            if let Err(e) = rec.start().await {
                error!("Failed to start recorder: {}", e);
                return;
            }

            // Play start beep if enabled
            if beep {
                crate::sound::play_sound("Tink");
            }

            // Show badge with appropriate mode (Hold=red solid, Assistive=purple)
            let badge_mode = if is_assistive {
                BadgeMode::Assistive
            } else {
                BadgeMode::Hold
            };
            show_badge_for_mode(badge_mode);

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

        // Start the recorder
        let mut recorder = self.recorder.lock().await;
        recorder.start().await?;

        // Play start beep if enabled
        let beep_enabled = self.config.read().await.beep_on_start;
        if beep_enabled {
            crate::sound::play_sound("Tink");
        }

        // Show pulsing red badge for toggle mode (hands-off recording)
        show_badge_for_mode(BadgeMode::Toggle);

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

        // Get session ID and assistive mode before we reset them
        let session_id = self.session_id.read().await.clone();
        let assistive = *self.assistive_mode.read().await;

        // Switch badge to processing mode (orange, pulsing)
        show_badge_for_mode(BadgeMode::Processing);

        let result = self.process_recording(session_id, assistive).await;

        // Always reset to IDLE, even on error
        *self.state.write().await = State::Idle;
        *self.assistive_mode.write().await = false;
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
    async fn process_recording(&self, _session_id: Option<String>, assistive: bool) -> Result<()> {
        // Stop the recorder and get audio file path
        let mut recorder = self.recorder.lock().await;
        let raw_audio_path = recorder
            .stop()
            .await
            .context("Failed to stop recorder")?
            .ok_or_else(|| anyhow::anyhow!("No audio file produced"))?;
        drop(recorder); // Release lock

        // Validate audio path to prevent path traversal attacks
        let audio_path = ValidatedAudioPath::new(&raw_audio_path)?;

        // Capture timestamp NOW for pairing audio with transcript
        let recording_timestamp = chrono::Local::now();

        // Save audio to transcriptions folder if enabled
        if self.config.read().await.dump_audio_logs {
            crate::history::save_audio(audio_path.as_path(), recording_timestamp);
        }

        // Normal transcription flow
        info!(
            "Transcribing audio file: {}",
            audio_path.as_path().display()
        );

        // Get language from config
        let language = self.config.read().await.whisper_language;

        // Call backend transcription with language parameter
        let mut raw_text_opt = None;
        let use_local_stt = self.config.read().await.use_local_stt;

        let language_opt = match language {
            crate::config::Language::Auto => None,
            lang => Some(lang.as_str()),
        };

        if use_local_stt {
            let local_engine = self.local_stt.clone();
            let path_owned = audio_path.as_path().to_path_buf();
            let language_owned = language_opt.map(|s| s.to_string());

            // Run in blocking task to avoid blocking async runtime
            // Since we use spawn_blocking, we need to handle JoinError too, but we use ?
            let local_result = tokio::task::spawn_blocking(move || {
                let mut guard = local_engine.blocking_lock();
                match guard.as_mut().as_mut() {
                    Some(engine) => engine.transcribe_file_with_language(&path_owned, language_owned.as_deref()),
                    None => Err(anyhow::anyhow!("Local engine not initialized")),
                }
            }).await;

            match local_result {
                Ok(Ok(text)) => {
                    info!("Local transcription successful");
                    raw_text_opt = Some(text);
                },
                Ok(Err(e)) => {
                    warn!("Local STT failed: {}", e);
                },
                Err(e) => {
                     warn!("Local STT task failed: {}", e);
                }
            }
        }

        let raw_text = if let Some(text) = raw_text_opt {
            text
        } else {
            crate::client::transcribe(audio_path.as_path(), language_opt)
                .await
                .context("Transcription failed")?
        };

        if raw_text.trim().is_empty() {
            error!("Transcription failed: no text returned");
            anyhow::bail!("Empty transcript");
        }

        info!("Raw transcript captured ({} chars)", raw_text.len());

        // Check for repetition loops (Whisper hallucination like "Wielki, Wielki, Wielki...")
        let has_repetition = crate::ai_formatting::has_repetition_loop(&raw_text);
        if has_repetition {
            warn!("Detected repetition loop in transcription - will clean up");
        }

        // Determine final text based on assistive mode:
        // - assistive=false (Ctrl only): raw transcript, only local repetition cleanup if needed
        // - assistive=true (Ctrl+Shift): AI formatting if configured
        let formatted_text = if assistive {
            // AI mode: use AI formatting if enabled and configured
            let ai_formatting_enabled = self.config.read().await.ai_formatting_enabled;
            let should_use_ai = ai_formatting_enabled && crate::ai_formatting::has_api_key();

            if should_use_ai {
                info!("Assistive mode: formatting transcript via AI");
                let lang_str = language_opt.map(String::from);
                crate::ai_formatting::format_text(&raw_text, lang_str.as_deref(), true).await
            } else if has_repetition {
                info!("Assistive mode: AI not available, using local repetition cleanup");
                crate::ai_formatting::remove_simple_repetitions(&raw_text)
            } else {
                info!("Assistive mode: AI not configured, using raw text");
                raw_text.clone()
            }
        } else if has_repetition {
            // Raw mode with repetition: only local cleanup
            info!("Raw mode: applying local repetition cleanup");
            crate::ai_formatting::remove_simple_repetitions(&raw_text)
        } else {
            // Raw mode: return transcript as-is
            info!("Raw mode: using raw transcript");
            raw_text.clone()
        };

        info!(
            "Final transcript ready ({} chars, mode={})",
            formatted_text.len(),
            if assistive { "AI" } else { "raw" }
        );

        // Paste the text into the active application
        crate::clipboard::paste_text(&formatted_text).context("Failed to paste text")?;

        info!("Text pasted successfully");

        // Save to history with same timestamp as audio file
        let entry =
            crate::history::save_entry_with_timestamp(&formatted_text, Some(recording_timestamp));
        info!("Transcript saved: {}", entry.path.display());

        // Update tray menu history label
        crate::tray::update_history_label(&formatted_text);

        Ok(())
    }

    /// Process recording through Voice Chat WebSocket pipeline.
    ///
    /// This sends the audio to the backend via WebSocket, which:
    /// 1. Transcribes the audio
    /// 2. Detects sentence boundaries
    /// 3. Streams LLM response tokens back
    ///
    /// The response is displayed in an overlay and copied to clipboard.
    #[allow(dead_code)] // Prepared for voice chat feature
    async fn process_voice_chat(
        &self,
        audio_path: &ValidatedAudioPath,
        recording_timestamp: chrono::DateTime<chrono::Local>,
    ) -> Result<()> {
        use crossbeam_channel::unbounded;
        use std::time::Duration;

        // Show the overlay
        voice_chat_ui::show_voice_chat_overlay();
        voice_chat_ui::update_voice_chat_status("Connecting...");

        // Create event channel for voice chat
        let (event_tx, event_rx) = unbounded::<VoiceChatEvent>();

        // Connect to voice chat WebSocket
        let client = match VoiceChatClient::connect(event_tx).await {
            Ok(client) => client,
            Err(e) => {
                error!("Failed to connect to voice chat: {}", e);
                voice_chat_ui::update_voice_chat_status("Connection failed");
                tokio::time::sleep(Duration::from_secs(2)).await;
                voice_chat_ui::hide_voice_chat_overlay();

                // Fall back to normal assistive formatting
                warn!("Falling back to standard assistive formatting");
                return self
                    .fallback_assistive_processing(audio_path, recording_timestamp)
                    .await;
            }
        };

        voice_chat_ui::update_voice_chat_status("Sending audio...");

        // Read and send audio file in chunks (path already validated)
        let audio_data = audio_path.read().await?;

        const CHUNK_SIZE: usize = 16 * 1024; // 16KB chunks
        let chunks: Vec<&[u8]> = audio_data.chunks(CHUNK_SIZE).collect();
        let total_chunks = chunks.len();

        info!(
            "Sending {} audio chunks ({} bytes total)",
            total_chunks,
            audio_data.len()
        );

        for (i, chunk) in chunks.iter().enumerate() {
            let is_last = i == total_chunks - 1;
            if let Err(e) = client.send_audio_chunk(chunk, 16000, is_last).await {
                error!("Failed to send audio chunk: {}", e);
                voice_chat_ui::update_voice_chat_status("Send failed");
                tokio::time::sleep(Duration::from_secs(2)).await;
                voice_chat_ui::hide_voice_chat_overlay();
                return Err(e);
            }

            // Small delay between chunks
            if !is_last {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        }

        // Signal end of audio
        if let Err(e) = client.send_end().await {
            error!("Failed to send end signal: {}", e);
        }

        voice_chat_ui::update_voice_chat_status("Processing...");

        // Process events from the voice chat
        let mut full_response = String::new();
        let mut got_response = false;
        let timeout = Duration::from_secs(60); // 60 second timeout for LLM response
        let start = std::time::Instant::now();

        loop {
            if start.elapsed() > timeout {
                warn!("Voice chat timeout");
                voice_chat_ui::update_voice_chat_status("Timeout");
                break;
            }

            match event_rx.recv_timeout(Duration::from_millis(100)) {
                Ok(event) => match event {
                    VoiceChatEvent::Connected => {
                        debug!("Voice chat connected");
                    }
                    VoiceChatEvent::Transcript(text) => {
                        info!("Transcript received: {}", text);
                        voice_chat_ui::update_voice_chat_status("Thinking...");
                    }
                    VoiceChatEvent::SentenceComplete(text) => {
                        info!("Sentence complete: {}", text);
                    }
                    VoiceChatEvent::LlmDelta(delta) => {
                        voice_chat_ui::append_voice_chat_delta(&delta);
                        full_response.push_str(&delta);
                    }
                    VoiceChatEvent::LlmDone {
                        text,
                        response_id: _,
                    } => {
                        info!("LLM response complete: {} chars", text.len());
                        full_response = text;
                        got_response = true;

                        // Copy to clipboard
                        if let Err(e) = crate::clipboard::copy(&full_response) {
                            error!("Failed to copy to clipboard: {}", e);
                        } else {
                            info!("Response copied to clipboard");
                        }

                        voice_chat_ui::update_voice_chat_status("Copied to clipboard!");
                        break;
                    }
                    VoiceChatEvent::Error(msg) => {
                        error!("Voice chat error: {}", msg);
                        voice_chat_ui::update_voice_chat_status(&format!("Error: {}", msg));
                        tokio::time::sleep(Duration::from_secs(2)).await;
                        break;
                    }
                    VoiceChatEvent::Disconnected => {
                        info!("Voice chat disconnected");
                        break;
                    }
                },
                Err(crossbeam_channel::RecvTimeoutError::Timeout) => {
                    // Continue waiting
                    continue;
                }
                Err(crossbeam_channel::RecvTimeoutError::Disconnected) => {
                    info!("Voice chat event channel closed");
                    break;
                }
            }
        }

        // Close the WebSocket
        let _ = client.close().await;

        // Keep overlay visible for a moment, then hide
        if got_response {
            tokio::time::sleep(Duration::from_secs(3)).await;
        } else {
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
        voice_chat_ui::hide_voice_chat_overlay();

        // Save to history if we got a response (with same timestamp as audio)
        if !full_response.is_empty() {
            let entry = crate::history::save_entry_with_timestamp(
                &full_response,
                Some(recording_timestamp),
            );
            info!("Voice chat response saved: {}", entry.path.display());
            // Update tray menu history label
            crate::tray::update_history_label(&full_response);
        }

        Ok(())
    }

    /// Fallback to standard assistive processing when voice chat fails.
    #[allow(dead_code)] // Called from process_voice_chat (voice chat feature)
    async fn fallback_assistive_processing(
        &self,
        audio_path: &ValidatedAudioPath,
        recording_timestamp: chrono::DateTime<chrono::Local>,
    ) -> Result<()> {
        info!("Using fallback assistive processing");

        let language = self.config.read().await.whisper_language;
        let language_opt = match language {
            crate::config::Language::Auto => None,
            lang => Some(lang.as_str()),
        };

        // Transcribe (path already validated)
        let raw_text = crate::client::transcribe(audio_path.as_path(), language_opt)
            .await
            .context("Transcription failed")?;

        if raw_text.trim().is_empty() {
            error!("Transcription failed: no text returned");
            anyhow::bail!("Empty transcript");
        }

        // Format with assistive mode
        let lang_str = language_opt.map(String::from);
        let formatted_text =
            crate::ai_formatting::format_text(&raw_text, lang_str.as_deref(), true).await;

        // Paste
        crate::clipboard::paste_text(&formatted_text).context("Failed to paste text")?;

        // Save to history with same timestamp as audio
        let entry =
            crate::history::save_entry_with_timestamp(&formatted_text, Some(recording_timestamp));
        info!("Transcript saved: {}", entry.path.display());

        // Update tray menu history label
        crate::tray::update_history_label(&formatted_text);

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
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_initial_state() {
        let controller = RecordingController::new();
        assert_eq!(controller.current_state().await, State::Idle);
    }

    #[tokio::test]
    #[ignore = "requires audio hardware"]
    async fn test_hold_down_schedules_delayed_start() {
        let controller = RecordingController::new();
        // Override hold delay for faster test
        controller.config.write().await.hold_start_delay_ms = 100;

        let event = HotkeyInput {
            key_type: HotkeyType::Hold,
            action: HotkeyAction::Down,
            assistive: false,
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
        };
        controller.handle_hotkey_event(down_event).await.unwrap();

        // Release before delay elapses
        tokio::time::sleep(Duration::from_millis(50)).await;
        let up_event = HotkeyInput {
            key_type: HotkeyType::Hold,
            action: HotkeyAction::Up,
            assistive: false,
        };
        controller.handle_hotkey_event(up_event).await.unwrap();

        // Wait past the original delay
        tokio::time::sleep(Duration::from_millis(200)).await;

        // Should still be IDLE (start was cancelled)
        assert_eq!(controller.current_state().await, State::Idle);
    }

    #[tokio::test]
    #[ignore = "requires audio hardware"]
    async fn test_toggle_starts_immediately() {
        let controller = RecordingController::new();

        let event = HotkeyInput {
            key_type: HotkeyType::Toggle,
            action: HotkeyAction::Press,
            assistive: true,
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
}
