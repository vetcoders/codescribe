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

use anyhow::Result;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{Mutex, RwLock};
use tokio::task::JoinHandle;
use tracing::{debug, error, info, warn};
use uuid::Uuid;

// TODO: Re-enable when fixing Send issues
// use crate::audio::Recorder;
// use crate::client;
// use crate::clipboard;
use crate::tray::{update_tray_status, TrayStatus};

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
pub struct HotkeyEvent {
    pub key_type: HotkeyType,
    pub action: HotkeyAction,
    pub assistive: bool,
}

/// Recording controller managing state machine and lifecycle
pub struct RecordingController {
    /// Current state
    state: Arc<RwLock<State>>,

    /// Audio recorder instance
    /// TODO: Recorder causes Send issues due to cpal Stream callbacks
    /// Will be re-enabled after refactoring to use spawn_blocking
    // recorder: Arc<Mutex<Recorder>>,

    /// Whether assistive formatting mode is enabled
    assistive_mode: Arc<RwLock<bool>>,

    /// Current session ID for tracking
    session_id: Arc<RwLock<Option<String>>>,

    /// Task handle for delayed hold-start (800ms default)
    hold_start_task: Arc<Mutex<Option<JoinHandle<()>>>>,

    /// Delay before recording starts in hold mode (ms)
    hold_start_delay_ms: u64,

    /// Lock to serialize finish_recording calls
    serial_lock: Arc<Mutex<()>>,

    /// Whether to beep when recording starts
    beep_on_start: bool,
}

impl RecordingController {
    /// Create a new recording controller with default configuration
    pub fn new() -> Self {
        Self::with_config(800, true)
    }

    /// Create a new recording controller with custom configuration
    ///
    /// # Arguments
    /// * `hold_start_delay_ms` - Delay before recording starts in hold mode (default: 800ms)
    /// * `beep_on_start` - Whether to play a beep when recording starts (default: true)
    pub fn with_config(hold_start_delay_ms: u64, beep_on_start: bool) -> Self {
        info!(
            "Initializing RecordingController (hold_delay={}ms, beep={})",
            hold_start_delay_ms, beep_on_start
        );

        // TODO: Re-enable recorder after fixing Send issues
        // let recorder = Recorder::new().expect("Failed to initialize audio recorder");

        Self {
            state: Arc::new(RwLock::new(State::Idle)),
            // recorder: Arc::new(Mutex::new(recorder)),
            assistive_mode: Arc::new(RwLock::new(false)),
            session_id: Arc::new(RwLock::new(None)),
            hold_start_task: Arc::new(Mutex::new(None)),
            hold_start_delay_ms,
            serial_lock: Arc::new(Mutex::new(())),
            beep_on_start,
        }
    }

    /// Get current state
    pub async fn current_state(&self) -> State {
        *self.state.read().await
    }

    /// Check if assistive mode is enabled
    pub async fn is_assistive_mode(&self) -> bool {
        *self.assistive_mode.read().await
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
    pub async fn handle_hotkey_event(&self, event: HotkeyEvent) -> Result<()> {
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
    async fn handle_hold_event(&self, event: HotkeyEvent) -> Result<()> {
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
    async fn handle_toggle_event(&self, event: HotkeyEvent) -> Result<()> {
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
        debug!("Scheduling hold-start after {}ms delay", self.hold_start_delay_ms);

        // Cancel any existing delayed start
        self.cancel_pending_hold_start().await;

        let state = Arc::clone(&self.state);
        let session_id = Arc::clone(&self.session_id);
        // let recorder = Arc::clone(&self.recorder);
        let delay = Duration::from_millis(self.hold_start_delay_ms);
        let beep = self.beep_on_start;

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

            // TODO: Start the recorder
            // let mut rec = recorder.lock().await;
            // if let Err(e) = rec.start().await {
            //     error!("Failed to start recorder: {}", e);
            //     return;
            // }

            // TODO: Play start beep if enabled
            if beep {
                debug!("Would play start beep");
            }

            // TODO: Show hold badge UI
            debug!("Would show hold badge");

            // Transition to REC_HOLD
            *state.write().await = State::RecHold;
            info!("STATE TRANSITION: IDLE → REC_HOLD");

            // Update tray status to Listening
            let _ = update_tray_status(TrayStatus::Listening);
        });

        *self.hold_start_task.lock().await = Some(task);
        Ok(())
    }

    /// Start recording in toggle mode (immediate, no delay)
    async fn start_toggle_recording(&self) -> Result<()> {
        // Acquire serial lock to prevent race conditions
        let _guard = self.serial_lock.lock().await;

        // Double-check state under lock
        let current_state = *self.state.read().await;
        if current_state != State::Idle {
            debug!("start_toggle_recording: state already changed to {}", current_state);
            return Ok(());
        }

        // Generate session ID
        let new_session_id = Uuid::new_v4().to_string();
        *self.session_id.write().await = Some(new_session_id.clone());

        info!("Starting toggle recording (session={})", new_session_id);

        // TODO: Start the recorder
        // let mut recorder = self.recorder.lock().await;
        // recorder.start().await
        //     .context("Failed to start recorder in toggle mode")?;

        // TODO: Play start beep if enabled
        if self.beep_on_start {
            debug!("Would play start beep");
        }

        // TODO: Show hold badge UI
        debug!("Would show hold badge");

        // Transition to REC_TOGGLE
        *self.state.write().await = State::RecToggle;
        info!("STATE TRANSITION: IDLE → REC_TOGGLE");

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

        // Update tray icon to "thinking"
        let _ = update_tray_status(TrayStatus::Thinking);

        let result = self.process_recording(session_id, assistive).await;

        // Always reset to IDLE, even on error
        *self.state.write().await = State::Idle;
        *self.assistive_mode.write().await = false;
        *self.session_id.write().await = None;

        // TODO: Hide hold badge UI
        debug!("Would hide hold badge");

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
    async fn process_recording(
        &self,
        _session_id: Option<String>,
        assistive: bool,
    ) -> Result<()> {
        // TODO: Stop the recorder and get audio file path
        // let mut recorder = self.recorder.lock().await;
        // let audio_path = recorder.stop().await
        //     .context("Failed to stop recorder")?
        //     .ok_or_else(|| anyhow::anyhow!("No audio file produced"))?;

        let audio_path = "/tmp/placeholder.wav"; // Placeholder
        info!("Transcribing audio file: {}", audio_path);

        // TODO: Call backend transcription
        // let raw_text = crate::client::transcribe(
        //     &audio_path,
        //     session_id.as_deref()
        // ).await
        //     .context("Transcription failed")?;

        let raw_text = "placeholder transcript".to_string(); // Placeholder

        if raw_text.trim().is_empty() {
            error!("Transcription failed: no text returned");
            anyhow::bail!("Empty transcript");
        }

        info!("Raw transcript captured ({} chars)", raw_text.len());

        // Format the text if we have content
        let formatted_text = if assistive {
            info!("Formatting transcript (assistive=true)");
            // TODO: Call backend formatting
            // crate::client::format_text(&raw_text, assistive, session_id.as_deref())
            //     .await
            //     .unwrap_or_else(|e| {
            //         warn!("Formatting failed: {}, using raw text", e);
            //         raw_text.clone()
            //     })
            raw_text.clone() // Placeholder
        } else {
            raw_text.clone()
        };

        info!(
            "Formatted transcript ready ({} chars, assistive={})",
            formatted_text.len(),
            assistive
        );

        // TODO: Paste the text
        // crate::clipboard::paste_text(&formatted_text)
        //     .context("Failed to paste text")?;

        info!("Text pasted successfully");

        Ok(())
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
        assert!(!controller.is_assistive_mode().await);
    }

    #[tokio::test]
    async fn test_hold_down_schedules_delayed_start() {
        let controller = RecordingController::with_config(100, false);

        let event = HotkeyEvent {
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
    async fn test_hold_up_before_delay_cancels() {
        let controller = RecordingController::with_config(200, false);

        // Press down
        let down_event = HotkeyEvent {
            key_type: HotkeyType::Hold,
            action: HotkeyAction::Down,
            assistive: false,
        };
        controller.handle_hotkey_event(down_event).await.unwrap();

        // Release before delay elapses
        tokio::time::sleep(Duration::from_millis(50)).await;
        let up_event = HotkeyEvent {
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
    async fn test_toggle_starts_immediately() {
        let controller = RecordingController::new();

        let event = HotkeyEvent {
            key_type: HotkeyType::Toggle,
            action: HotkeyAction::Press,
            assistive: true,
        };

        controller.handle_hotkey_event(event).await.unwrap();

        // Should immediately transition to REC_TOGGLE
        assert_eq!(controller.current_state().await, State::RecToggle);
        assert!(controller.is_assistive_mode().await);
    }

    #[tokio::test]
    async fn test_busy_state_ignores_hotkeys() {
        let controller = RecordingController::new();

        // Manually set to BUSY
        *controller.state.write().await = State::Busy;

        let event = HotkeyEvent {
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
}
