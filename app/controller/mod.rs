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
//! IDLE + toggle_press → REC_TOGGLE (continuous)
//! REC_HOLD + hold_up → BUSY (process)
//! REC_TOGGLE + silence → send (no stop)
//! REC_TOGGLE + toggle_press → IDLE (stop)
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

pub use helpers::{
    is_assistive_session, is_conversation_session, set_assistive_session, set_conversation_session,
};
pub use types::{HotkeyAction, HotkeyInput, HotkeyType, State};

use crate::presentation::emitter::PresentationEmitter;
use crate::stream_postprocess::StreamPostProcessor;
use anyhow::{Context, Result};
#[cfg(target_os = "macos")]
use dispatch::Queue;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Duration;
use tokio::sync::{Mutex, RwLock, broadcast};
use tokio::task::JoinHandle;
use tracing::{debug, error, info, warn};
use uuid::Uuid;

use crate::audio::streaming_recorder::StreamingRecorder;
use crate::config::Config;
use crate::config::models::ModelManager;
use crate::os::clipboard;
use crate::os::hotkeys::HoldMode;
use crate::os::selection::{
    AssistiveContext, build_assistive_input, capture_assistive_context, capture_frontmost_app_only,
};
use crate::{BadgeMode, hide_hold_badge, show_badge_for_mode};

// Moshi conversation engine and audio output
use codescribe_core::conversation::{ConversationEngine, MoshiConfig};
use codescribe_core::ipc::{IpcEvent, IpcEventPayload};
use codescribe_core::llm::edit_blocks;
use codescribe_core::tts::AudioPlayer;

// UI state for conversation mode
use crate::voice_chat_ui::ConversationModeState;

use helpers::{raw_save_enabled, route_transcription_delta, setup_voice_chat_send_callback};
use types::ValidatedAudioPath;

static OVERLAY_CONTROLLER: OnceLock<Arc<RecordingController>> = OnceLock::new();

#[cfg(target_os = "macos")]
fn activate_target_app(app_name: &str) {
    use objc::msg_send;
    use objc::runtime::{Class, Object};
    use objc::{sel, sel_impl};

    let wanted = app_name.trim().to_lowercase();
    if wanted.is_empty() {
        return;
    }

    // Activate via NSWorkspace — no shell usage.
    unsafe {
        let Some(ns_workspace) = Class::get("NSWorkspace") else {
            return;
        };
        let workspace: *mut Object = msg_send![ns_workspace, sharedWorkspace];
        if workspace.is_null() {
            return;
        }

        let running: *mut Object = msg_send![workspace, runningApplications];
        if running.is_null() {
            return;
        }

        let count: usize = msg_send![running, count];
        for i in 0..count {
            let app: *mut Object = msg_send![running, objectAtIndex: i];
            if app.is_null() {
                continue;
            }

            let name: *mut Object = msg_send![app, localizedName];
            if name.is_null() {
                continue;
            }

            let name_cstr: *const std::ffi::c_char = msg_send![name, UTF8String];
            if name_cstr.is_null() {
                continue;
            }

            let name_str = std::ffi::CStr::from_ptr(name_cstr).to_string_lossy();
            let candidate = name_str.trim().to_lowercase();
            if candidate == wanted || candidate.contains(&wanted) || wanted.contains(&candidate) {
                let _: bool = msg_send![app, activateWithOptions: 1u64];
                break;
            }
        }
    }
}

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

/// Start a toggle recording session from the UI (CTA).
pub fn request_toggle_recording_start(assistive: bool) {
    let Some(controller) = OVERLAY_CONTROLLER.get().cloned() else {
        warn!("Overlay controller not registered; cannot start recording");
        return;
    };

    tokio::spawn(async move {
        let event = HotkeyInput {
            key_type: HotkeyType::Toggle,
            action: HotkeyAction::Press,
            assistive,
            hold_mode: HoldMode::Raw,
            force_raw: !assistive,
            force_ai: assistive,
        };
        if let Err(e) = controller.handle_hotkey_event(event).await {
            error!("CTA start recording failed: {}", e);
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

    /// Whether AI assistive mode is enabled for the current session.
    ///
    /// This is true for:
    /// - Hold modes: Chat (Shift) / Selection (Cmd)
    /// - Assistive toggle (right Option double-tap, if enabled)
    assistive_mode: Arc<RwLock<bool>>,
    /// Current hold intent (Raw/Chat/Selection) for the active session.
    hold_mode: Arc<RwLock<HoldMode>>,

    /// Whether to force RAW mode (Ctrl Hold without Shift = always raw, ignores AI toggle)
    /// Toggle mode (Double Option) keeps this false and respects AI_FORMATTING_ENABLED setting.
    force_raw_mode: Arc<RwLock<bool>>,
    /// Whether to force AI formatting for the current session (explicit force path)
    force_ai_mode: Arc<RwLock<bool>>,

    /// Current session ID for tracking
    session_id: Arc<RwLock<Option<String>>>,

    /// Task handle for delayed hold-start (800ms default)
    hold_start_task: Arc<Mutex<Option<JoinHandle<()>>>>,

    /// Lock to serialize finish_recording calls
    serial_lock: Arc<Mutex<()>>,

    /// Flag set by VAD (silence detection) when recording should auto-stop
    vad_triggered: Arc<AtomicBool>,

    /// Assistive hands-off loop active (Right Option toggle)
    assistive_loop_active: Arc<AtomicBool>,

    /// Toggle session: track whether we've already appended user/assistant text
    toggle_user_has_text: Arc<AtomicBool>,
    toggle_assistant_has_text: Arc<AtomicBool>,

    /// Best-effort selected-text/app context captured for assistive sessions.
    ///
    /// Must be captured BEFORE showing any overlay window, because overlays
    /// may steal focus and destroy the user's selection context.
    assistive_context: Arc<RwLock<Option<AssistiveContext>>>,
    /// True when we opened the unified overlay solely to show a raw transcription preview.
    ///
    /// This lets us preserve the old behavior:
    /// - If the user had the overlay already open (Drawer/Agent), don't close it after dictation.
    /// - If we popped it open just for raw dictation, auto-hide it after processing.
    opened_voice_chat_overlay_for_transcription: Arc<AtomicBool>,

    // ═══════════════════════════════════════════════════════════
    // Conversation mode (Moshi full-duplex)
    // ═══════════════════════════════════════════════════════════
    /// Moshi conversation engine (lazy-initialized on first use)
    conversation_engine: Arc<Mutex<Option<ConversationEngine>>>,

    /// Audio player for conversation responses (lazy-initialized)
    audio_player: Arc<Mutex<Option<AudioPlayer>>>,

    /// Flag to signal conversation mode should stop
    conversation_stop_flag: Arc<AtomicBool>,

    /// Session generation counter - increments on each conversation start.
    /// Spawn tasks capture this value and compare before UI updates to prevent
    /// cross-session race conditions (old tasks updating new session's UI).
    conversation_generation: Arc<AtomicU64>,

    /// Task handle for conversation audio processing loop
    conversation_task: Arc<Mutex<Option<JoinHandle<()>>>>,

    /// Broadcast stream for IPC subscribers.
    event_broadcast: broadcast::Sender<IpcEvent>,
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
        recorder.set_delta_callback(Some(Arc::new(
            codescribe_core::pipeline::sinks::CallbackSink::new(Arc::new(|delta: &str| {
                route_transcription_delta(delta);
            })),
        )));

        if !cfg!(test) {
            let model_manager = ModelManager::new().expect("Failed to initialize model manager");
            if let Ok(models) = model_manager.list_models()
                && !models.is_empty()
            {
                info!("Available local models: {:?}", models);
            }

            // Initialize Whisper engine if not already done (daemon pre-inits)
            if !crate::whisper::is_initialized()
                && let Err(e) = crate::whisper::init()
            {
                warn!("Failed to initialize Whisper engine: {}", e);
            }
        }

        let config = Arc::new(RwLock::new(config));
        setup_voice_chat_send_callback(Arc::clone(&config));
        let (event_broadcast, _) = broadcast::channel::<IpcEvent>(256);

        Self {
            config,
            state: Arc::new(RwLock::new(State::Idle)),
            recorder: Arc::new(Mutex::new(recorder)),
            assistive_mode: Arc::new(RwLock::new(false)),
            hold_mode: Arc::new(RwLock::new(HoldMode::Raw)),
            force_raw_mode: Arc::new(RwLock::new(false)),
            force_ai_mode: Arc::new(RwLock::new(false)),
            session_id: Arc::new(RwLock::new(None)),
            hold_start_task: Arc::new(Mutex::new(None)),
            serial_lock: Arc::new(Mutex::new(())),
            vad_triggered: Arc::new(AtomicBool::new(false)),
            assistive_loop_active: Arc::new(AtomicBool::new(false)),
            toggle_user_has_text: Arc::new(AtomicBool::new(false)),
            toggle_assistant_has_text: Arc::new(AtomicBool::new(false)),
            assistive_context: Arc::new(RwLock::new(None)),
            opened_voice_chat_overlay_for_transcription: Arc::new(AtomicBool::new(false)),
            // Conversation mode (lazy init)
            conversation_engine: Arc::new(Mutex::new(None)),
            audio_player: Arc::new(Mutex::new(None)),
            conversation_stop_flag: Arc::new(AtomicBool::new(false)),
            conversation_generation: Arc::new(AtomicU64::new(0)),
            conversation_task: Arc::new(Mutex::new(None)),
            event_broadcast,
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
        recorder.set_delta_callback(Some(Arc::new(
            codescribe_core::pipeline::sinks::CallbackSink::new(Arc::new(|delta: &str| {
                route_transcription_delta(delta);
            })),
        )));

        if !cfg!(test) {
            let model_manager = ModelManager::new().expect("Failed to initialize model manager");
            if let Ok(models) = model_manager.list_models()
                && !models.is_empty()
            {
                info!("Available local models: {:?}", models);
            }
        }

        // Initialize Whisper engine if not already done (daemon pre-inits)
        if !cfg!(test)
            && !crate::whisper::is_initialized()
            && let Err(e) = crate::whisper::init()
        {
            warn!("Failed to initialize Whisper engine: {}", e);
        }

        setup_voice_chat_send_callback(Arc::clone(&config));
        let (event_broadcast, _) = broadcast::channel::<IpcEvent>(256);

        Self {
            config,
            state: Arc::new(RwLock::new(State::Idle)),
            recorder: Arc::new(Mutex::new(recorder)),
            assistive_mode: Arc::new(RwLock::new(false)),
            hold_mode: Arc::new(RwLock::new(HoldMode::Raw)),
            force_raw_mode: Arc::new(RwLock::new(false)),
            force_ai_mode: Arc::new(RwLock::new(false)),
            session_id: Arc::new(RwLock::new(None)),
            hold_start_task: Arc::new(Mutex::new(None)),
            serial_lock: Arc::new(Mutex::new(())),
            vad_triggered: Arc::new(AtomicBool::new(false)),
            assistive_loop_active: Arc::new(AtomicBool::new(false)),
            toggle_user_has_text: Arc::new(AtomicBool::new(false)),
            toggle_assistant_has_text: Arc::new(AtomicBool::new(false)),
            assistive_context: Arc::new(RwLock::new(None)),
            opened_voice_chat_overlay_for_transcription: Arc::new(AtomicBool::new(false)),
            // Conversation mode (lazy init)
            conversation_engine: Arc::new(Mutex::new(None)),
            audio_player: Arc::new(Mutex::new(None)),
            conversation_stop_flag: Arc::new(AtomicBool::new(false)),
            conversation_generation: Arc::new(AtomicU64::new(0)),
            conversation_task: Arc::new(Mutex::new(None)),
            event_broadcast,
        }
    }

    /// Get current state
    pub async fn current_state(&self) -> State {
        *self.state.read().await
    }

    pub fn subscribe_events(&self) -> broadcast::Receiver<IpcEvent> {
        self.event_broadcast.subscribe()
    }

    #[cfg(test)]
    pub(crate) fn publish_ipc_event_for_test(&self, payload: IpcEventPayload) {
        let _ = self.event_broadcast.send(IpcEvent {
            timestamp: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
            payload,
        });
    }

    async fn set_state(&self, new_state: State) {
        Self::set_state_with_broadcast(&self.state, &self.event_broadcast, new_state).await;
    }

    async fn set_state_with_broadcast(
        state: &Arc<RwLock<State>>,
        event_broadcast: &broadcast::Sender<IpcEvent>,
        new_state: State,
    ) {
        let old_state = {
            let mut guard = state.write().await;
            let old = *guard;
            *guard = new_state;
            old
        };

        if old_state != new_state {
            let _ = event_broadcast.send(IpcEvent {
                timestamp: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
                payload: IpcEventPayload::StateChange {
                    from: old_state.to_ipc_str().to_string(),
                    to: new_state.to_ipc_str().to_string(),
                },
            });
        }
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

    fn is_already_in_progress_error(error: &anyhow::Error) -> bool {
        error
            .to_string()
            .contains("Recording is already in progress")
    }

    async fn recover_stale_recorder_if_idle(&self) {
        if self.current_state().await != State::Idle {
            return;
        }

        let mut recorder = self.recorder.lock().await;
        if !recorder.recorder.is_active() {
            return;
        }

        warn!("Recorder recovery: detected active stream while controller is IDLE; forcing stop");
        if let Err(e) = recorder.stop_without_saving().await {
            warn!("Recorder recovery: forced stop failed: {e}");
        }
        recorder.set_utterance_callback(None);
        recorder.set_utterance_silence_sec(None);
        recorder.set_event_sink(None);
        drop(recorder);

        *self.session_id.write().await = None;
        self.assistive_loop_active.store(false, Ordering::SeqCst);
        set_assistive_session(false);
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
    /// - **Toggle + force_ai=true**: force AI formatting (explicit force path)
    /// - **Toggle + assistive=true**: force Assistive hands-off
    pub async fn handle_hotkey_event(&self, event: HotkeyInput) -> Result<()> {
        let current_state = self.current_state().await;

        if current_state == State::Idle {
            self.recover_stale_recorder_if_idle().await;
        }

        debug!(
            "Hotkey event: type={:?} action={:?} assistive={} hold_mode={:?} force_raw={} force_ai={} state={}",
            event.key_type,
            event.action,
            event.assistive,
            event.hold_mode,
            event.force_raw,
            event.force_ai,
            current_state
        );

        // Update mode flags from event (supports mid-hold mode changes via Press events).
        if matches!(event.action, HotkeyAction::Down | HotkeyAction::Press) {
            match event.key_type {
                HotkeyType::Hold => {
                    match event.hold_mode {
                        HoldMode::Raw => {
                            // If we're already in an assistive session (Chat/Selection) and the user
                            // releases Shift/Cmd while still holding Ctrl, the event tap will emit a
                            // HoldUpdate back to Raw. We *do not* want to flip the UI back to the
                            // transcription overlay mid-session (it looks like the chat "blinks"
                            // and then disappears).
                            //
                            // We treat assistive mode as "latched" for the duration of a recording.
                            // NOTE: hold_mode write is deferred to AFTER this check to prevent
                            // corrupting the mode while the latch rejects the update.
                            if matches!(current_state, State::RecHold | State::RecToggle)
                                && *self.assistive_mode.read().await
                            {
                                debug!("Ignoring Raw hold-mode update during assistive session");
                                return Ok(());
                            }

                            *self.hold_mode.write().await = event.hold_mode;
                            *self.assistive_mode.write().await = false;
                            *self.assistive_context.write().await = None;
                            *self.force_raw_mode.write().await = !event.force_ai;
                            *self.force_ai_mode.write().await = event.force_ai;

                            if matches!(current_state, State::RecHold | State::RecToggle) {
                                set_assistive_session(false);
                                self.opened_voice_chat_overlay_for_transcription
                                    .store(false, Ordering::SeqCst);
                                crate::show_transcription_overlay();
                                crate::enter_recording_mode();
                                crate::clear_transcription_text();
                            }
                        }
                        HoldMode::Chat => {
                            *self.hold_mode.write().await = event.hold_mode;
                            *self.assistive_mode.write().await = true;
                            *self.force_raw_mode.write().await = false;
                            *self.force_ai_mode.write().await = false;
                            *self.assistive_context.write().await = None;

                            // If we switch modes while already recording, update UI immediately.
                            if matches!(current_state, State::RecHold | State::RecToggle) {
                                let ctx = tokio::task::spawn_blocking(capture_frontmost_app_only)
                                    .await
                                    .unwrap_or_default();
                                *self.assistive_context.write().await = Some(ctx);
                                crate::voice_chat_ui::set_voice_chat_target_app(
                                    self.assistive_context
                                        .read()
                                        .await
                                        .clone()
                                        .unwrap_or_default()
                                        .frontmost_app,
                                );
                                set_assistive_session(true);
                                crate::hide_transcription_overlay_with_reason(
                                    "switch_to_assistive_chat",
                                );
                                crate::show_voice_chat_overlay();
                                crate::show_agent_tab();
                                crate::voice_chat_ui::update_voice_chat_status("Listening...");
                            }
                        }
                        HoldMode::Selection => {
                            *self.hold_mode.write().await = event.hold_mode;
                            *self.assistive_mode.write().await = true;
                            *self.force_raw_mode.write().await = false;
                            *self.force_ai_mode.write().await = false;
                            *self.assistive_context.write().await = None;

                            // If we switch modes while already recording, update UI immediately.
                            if matches!(current_state, State::RecHold | State::RecToggle) {
                                let ctx = tokio::task::spawn_blocking(capture_assistive_context)
                                    .await
                                    .unwrap_or_default();
                                *self.assistive_context.write().await = Some(ctx);
                                crate::voice_chat_ui::set_voice_chat_target_app(
                                    self.assistive_context
                                        .read()
                                        .await
                                        .clone()
                                        .unwrap_or_default()
                                        .frontmost_app,
                                );
                                set_assistive_session(true);
                                crate::hide_transcription_overlay_with_reason(
                                    "switch_to_assistive_selection",
                                );
                                crate::show_voice_chat_overlay();
                                crate::show_agent_tab();
                                crate::voice_chat_ui::update_voice_chat_status("Listening...");
                            }
                        }
                    }
                }
                HotkeyType::Toggle => {
                    *self.hold_mode.write().await = HoldMode::Raw;
                    *self.assistive_context.write().await = None;

                    *self.assistive_mode.write().await = event.assistive;
                    *self.force_raw_mode.write().await = event.force_raw;
                    *self.force_ai_mode.write().await = event.force_ai;
                }
                HotkeyType::Conversation => {
                    *self.hold_mode.write().await = HoldMode::Raw;
                    *self.assistive_context.write().await = None;
                    // Conversation mode - full-duplex (no raw/ai flags)
                    *self.assistive_mode.write().await = false;
                    *self.force_raw_mode.write().await = false;
                    *self.force_ai_mode.write().await = false;
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
            HotkeyType::Conversation => self.handle_conversation_event(event).await,
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
                info!("Toggle pressed again; stopping recording");
                self.assistive_loop_active.store(false, Ordering::SeqCst);
                self.stop_toggle_recording().await?;
            }
            State::RecHold => {
                // Safety/UX: if a hands-off toggle is triggered while in hold recording
                // (e.g., due to short HOLD_START_DELAY_MS or user timing), allow it to stop.
                // We only do this for RAW toggle to avoid surprising behavior for Option toggles.
                if event.force_raw {
                    info!("RAW toggle pressed during hold recording; finishing recording");
                    self.assistive_loop_active.store(false, Ordering::SeqCst);
                    self.finish_recording().await?;
                } else {
                    debug!("Toggle event ignored in REC_HOLD (force_raw=false)");
                }
            }
            _ => {
                debug!("Toggle event ignored in state {}", current_state);
            }
        }

        Ok(())
    }

    /// Handle conversation-mode hotkey events (Ctrl+Option)
    ///
    /// Conversation mode is full-duplex: simultaneous mic → Moshi → speaker.
    async fn handle_conversation_event(&self, event: HotkeyInput) -> Result<()> {
        match event.action {
            HotkeyAction::Down => {
                let current_state = self.current_state().await;
                if current_state == State::Idle {
                    self.start_conversation_mode().await?;
                }
            }
            HotkeyAction::Up => {
                let current_state = self.current_state().await;
                if current_state == State::Conversation {
                    info!("Conversation mode key released; stopping");
                    self.stop_conversation_mode().await?;
                }
            }
            HotkeyAction::Press => {
                // Conversation keys don't use press events
            }
        }
        Ok(())
    }

    /// Start conversation mode (full-duplex Moshi)
    ///
    /// Initializes ConversationEngine and AudioPlayer, then starts the audio
    /// processing loop that feeds mic input to Moshi and plays responses.
    async fn start_conversation_mode(&self) -> Result<()> {
        info!("Starting conversation mode (Moshi full-duplex)");

        // 1. Initialize ConversationEngine if needed (lazy init)
        {
            let mut engine_guard = self.conversation_engine.lock().await;
            if engine_guard.is_none() {
                info!("Lazy-initializing ConversationEngine...");
                let config = MoshiConfig::default();
                match ConversationEngine::new(config) {
                    Ok(mut engine) => {
                        // Pre-initialize to load models now (rather than on first audio)
                        if let Err(e) = engine.init() {
                            error!("ConversationEngine init failed: {}", e);
                            crate::voice_chat_ui::add_voice_chat_error_message(&format!(
                                "Moshi init failed: {}",
                                e
                            ));
                            return Err(e);
                        }
                        *engine_guard = Some(engine);
                        info!("ConversationEngine initialized successfully");
                    }
                    Err(e) => {
                        error!("Failed to create ConversationEngine: {}", e);
                        crate::voice_chat_ui::add_voice_chat_error_message(&format!(
                            "Moshi unavailable: {}",
                            e
                        ));
                        return Err(e);
                    }
                }
            }
        }

        // 2. Initialize AudioPlayer if needed (lazy init)
        {
            let mut player_guard = self.audio_player.lock().await;
            if player_guard.is_none() {
                info!("Lazy-initializing AudioPlayer...");
                match AudioPlayer::new() {
                    Ok(player) => {
                        *player_guard = Some(player);
                        info!("AudioPlayer initialized");
                    }
                    Err(e) => {
                        warn!("AudioPlayer init failed, using dummy: {}", e);
                        *player_guard = Some(AudioPlayer::dummy());
                    }
                }
            }
        }

        // 3. Reset stop flag and increment session generation
        self.conversation_stop_flag.store(false, Ordering::SeqCst);
        let generation = self.conversation_generation.fetch_add(1, Ordering::SeqCst) + 1;
        info!("Starting conversation session generation {}", generation);

        // 4. Set conversation session flag
        helpers::set_conversation_session(true);

        // 5. Transition to CONVERSATION state
        self.set_state(State::Conversation).await;
        info!("STATE TRANSITION: IDLE → CONVERSATION");

        // 6. Update UI
        show_badge_for_mode(BadgeMode::Assistive);
        crate::show_voice_chat_overlay();
        crate::voice_chat_ui::show_agent_tab();
        crate::voice_chat_ui::update_voice_chat_status("Listening...");
        crate::voice_chat_ui::update_conversation_state(ConversationModeState::Listening);

        // 7. Start the conversation audio processing task
        let engine = Arc::clone(&self.conversation_engine);
        let player = Arc::clone(&self.audio_player);
        let stop_flag = Arc::clone(&self.conversation_stop_flag);
        let generation_arc = Arc::clone(&self.conversation_generation);
        let state = Arc::clone(&self.state);
        let recorder = Arc::clone(&self.recorder);
        let event_broadcast = self.event_broadcast.clone();

        let task = tokio::spawn(async move {
            Self::conversation_audio_loop(
                engine,
                player,
                recorder,
                stop_flag,
                generation_arc,
                generation,
                state,
                event_broadcast,
            )
            .await;
        });

        *self.conversation_task.lock().await = Some(task);

        Ok(())
    }

    /// The main conversation audio processing loop
    ///
    /// Runs in a background task: captures audio → ConversationEngine → speaker
    #[allow(clippy::too_many_arguments)]
    async fn conversation_audio_loop(
        engine: Arc<Mutex<Option<ConversationEngine>>>,
        player: Arc<Mutex<Option<AudioPlayer>>>,
        recorder: Arc<Mutex<StreamingRecorder>>,
        stop_flag: Arc<AtomicBool>,
        generation_counter: Arc<AtomicU64>,
        my_generation: u64,
        state: Arc<RwLock<State>>,
        event_broadcast: broadcast::Sender<IpcEvent>,
    ) {
        info!(
            "Conversation audio loop started (generation {})",
            my_generation
        );

        // Create audio channel for conversation mode
        let (tx, mut rx) = tokio::sync::mpsc::channel::<Vec<f32>>(100);

        // Guard against concurrent playback
        let playback_active = Arc::new(AtomicBool::new(false));

        // Start recorder with callback that sends to our channel
        let tx_clone = tx.clone();
        {
            let mut rec = recorder.lock().await;
            rec.recorder.set_callback(Box::new(move |data| {
                let _ = tx_clone.try_send(data.to_vec());
            }));

            if let Err(e) = rec.recorder.start().await {
                error!("Failed to start recorder for conversation: {}", e);
                // Full cleanup on failure: state, session flag, badge, UI
                Self::set_state_with_broadcast(&state, &event_broadcast, State::Idle).await;
                helpers::set_conversation_session(false);
                hide_hold_badge();
                crate::voice_chat_ui::update_voice_chat_status("Recorder error");
                crate::voice_chat_ui::update_conversation_state(ConversationModeState::Inactive);
                crate::voice_chat_ui::add_voice_chat_error_message(&format!("Mic error: {}", e));
                return;
            }
        }

        // Get actual sample rate from recorder
        let sample_rate = {
            let rec = recorder.lock().await;
            rec.recorder.actual_sample_rate()
        };
        info!("Conversation mode: recording at {}Hz", sample_rate);

        // Processing loop
        let mut last_response_check = std::time::Instant::now();
        let response_check_interval = Duration::from_millis(100);

        while !stop_flag.load(Ordering::SeqCst) {
            // Process incoming audio chunks
            match tokio::time::timeout(Duration::from_millis(50), rx.recv()).await {
                Ok(Some(samples)) => {
                    // Feed audio to ConversationEngine
                    let mut engine_guard = engine.lock().await;
                    if let Some(ref mut eng) = *engine_guard {
                        if let Err(e) = eng.process_audio_any_rate(&samples, sample_rate) {
                            warn!("ConversationEngine.process_audio error: {}", e);
                        }

                        // Update UI based on conversation state (only if still current session)
                        let current_gen = generation_counter.load(Ordering::SeqCst);
                        if current_gen == my_generation {
                            let conv_state = eng.state();
                            let (status, ui_state) = match conv_state {
                                codescribe_core::conversation::context::ConversationState::UserSpeaking => {
                                    ("You're speaking...", ConversationModeState::UserSpeaking)
                                }
                                codescribe_core::conversation::context::ConversationState::AssistantSpeaking => {
                                    ("Moshi responding...", ConversationModeState::AssistantSpeaking)
                                }
                                codescribe_core::conversation::context::ConversationState::Processing => {
                                    ("Processing...", ConversationModeState::Processing)
                                }
                                _ => ("Listening...", ConversationModeState::Listening),
                            };
                            crate::voice_chat_ui::update_voice_chat_status(status);
                            crate::voice_chat_ui::update_conversation_state(ui_state);
                        }
                    }
                }
                Ok(None) => {
                    // Channel closed
                    break;
                }
                Err(_) => {
                    // Timeout - check for responses
                }
            }

            // Periodically check for and play responses
            if last_response_check.elapsed() >= response_check_interval {
                last_response_check = std::time::Instant::now();

                let mut engine_guard = engine.lock().await;
                if let Some(ref mut eng) = *engine_guard
                    && let Some(response_samples) = eng.get_response()
                {
                    let response_len = response_samples.len();
                    let response_rate = eng.sample_rate();
                    drop(engine_guard); // Release lock before blocking playback

                    info!(
                        "Playing response: {} samples ({:.2}s @ {}Hz)",
                        response_len,
                        response_len as f32 / response_rate as f32,
                        response_rate
                    );

                    // Guard: skip if playback already in progress
                    if playback_active.swap(true, Ordering::SeqCst) {
                        info!("Skipping response - playback already active");
                        continue;
                    }

                    crate::voice_chat_ui::update_voice_chat_status("Moshi speaking...");
                    crate::voice_chat_ui::update_conversation_state(
                        ConversationModeState::AssistantSpeaking,
                    );

                    // Play response audio in separate blocking task (non-blocking for loop)
                    // This preserves full-duplex: we can still process mic while playing
                    let player_clone = Arc::clone(&player);
                    let stop_flag_clone = Arc::clone(&stop_flag);
                    let generation_clone = Arc::clone(&generation_counter);
                    let playback_active_clone = Arc::clone(&playback_active);
                    let playback_active_reset = Arc::clone(&playback_active);

                    // Wrap spawn in catch_unwind to reset playback_active if spawn itself fails
                    let spawn_result =
                        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                            let handle = tokio::runtime::Handle::current();
                            tokio::task::spawn_blocking(move || {
                                // Drop guard ensures playback_active is reset even on panic
                                struct PlaybackGuard(Arc<AtomicBool>);
                                impl Drop for PlaybackGuard {
                                    fn drop(&mut self) {
                                        self.0.store(false, Ordering::SeqCst);
                                    }
                                }
                                let _guard = PlaybackGuard(Arc::clone(&playback_active_clone));

                                // Block this thread for playback, but don't block the async loop
                                let player_guard = handle.block_on(player_clone.lock());
                                if let Some(ref p) = *player_guard
                                    && let Err(e) = p.play(&response_samples, response_rate)
                                {
                                    warn!("AudioPlayer.play error: {}", e);
                                }
                                // Only update UI if:
                                // 1. Conversation wasn't stopped (stop_flag)
                                // 2. This is still the current session (generation matches)
                                // This prevents cross-session UI races
                                let current_gen = generation_clone.load(Ordering::SeqCst);
                                if !stop_flag_clone.load(Ordering::SeqCst)
                                    && current_gen == my_generation
                                {
                                    crate::voice_chat_ui::update_voice_chat_status("Listening...");
                                    crate::voice_chat_ui::update_conversation_state(
                                        ConversationModeState::Listening,
                                    );
                                }
                                // _guard dropped here, resets playback_active even on panic
                            })
                        }));

                    if spawn_result.is_err() {
                        warn!("spawn_blocking panicked - resetting playback_active");
                        playback_active_reset.store(false, Ordering::SeqCst);
                    }
                }
            }
        }

        // Cleanup: stop recorder
        {
            let mut rec = recorder.lock().await;
            let _ = rec.recorder.stop().await;
        }

        // Full cleanup if loop exits unexpectedly (e.g., channel closed)
        // This ensures state/UI consistency even without stop_conversation_mode()
        // CRITICAL: Only cleanup if THIS is still the current session (generation check)
        // This prevents "old loop kills new session" race when stop_conversation_mode() times out
        let current_gen = generation_counter.load(Ordering::SeqCst);
        let current_state = *state.read().await;

        if current_state == State::Conversation && current_gen == my_generation {
            // This loop owns the current session - safe to cleanup
            stop_flag.store(true, Ordering::SeqCst);

            Self::set_state_with_broadcast(&state, &event_broadcast, State::Idle).await;
            helpers::set_conversation_session(false);
            hide_hold_badge();
            crate::voice_chat_ui::update_voice_chat_status("Conversation ended");
            crate::voice_chat_ui::update_conversation_state(ConversationModeState::Inactive);
            info!(
                "Loop cleanup: conversation ended unexpectedly (gen {})",
                my_generation
            );
        } else if current_gen != my_generation {
            // New session started - don't touch anything
            info!(
                "Loop cleanup skipped: new session started (my_gen={}, current_gen={})",
                my_generation, current_gen
            );
        }

        info!("Conversation audio loop ended (gen {})", my_generation);
    }

    /// Stop conversation mode
    ///
    /// Signals the audio loop to stop and waits for cleanup.
    async fn stop_conversation_mode(&self) -> Result<()> {
        info!("Stopping conversation mode");

        // 1. Signal stop
        self.conversation_stop_flag.store(true, Ordering::SeqCst);

        // 2. Clear conversation session flag (before any cleanup)
        helpers::set_conversation_session(false);

        // 3. Stop recorder BEFORE waiting for task (prevents leak on abort)
        {
            let mut rec = self.recorder.lock().await;
            let _ = rec.recorder.stop().await;
            info!("Recorder stopped in stop_conversation_mode");
        }

        // 4. Wait for conversation task to finish (with timeout)
        let task = self.conversation_task.lock().await.take();
        if let Some(handle) = task {
            match tokio::time::timeout(Duration::from_secs(3), handle).await {
                Ok(Ok(())) => info!("Conversation task finished cleanly"),
                Ok(Err(e)) => warn!("Conversation task panicked: {}", e),
                Err(_) => {
                    warn!("Conversation task timeout - task will be aborted");
                    // Task aborted, but recorder already stopped above - no leak
                }
            }
        }

        // 6. Reset ConversationEngine state
        {
            let mut engine_guard = self.conversation_engine.lock().await;
            if let Some(ref mut eng) = *engine_guard {
                eng.reset();
            }
        }

        // 7. Transition back to IDLE
        self.set_state(State::Idle).await;
        info!("STATE TRANSITION: CONVERSATION → IDLE");

        // 8. Update UI
        hide_hold_badge();
        crate::voice_chat_ui::update_voice_chat_status("Conversation ended");
        crate::voice_chat_ui::update_conversation_state(ConversationModeState::Inactive);

        Ok(())
    }

    /// Schedule delayed recording start for hold mode
    async fn schedule_hold_start(&self) -> Result<()> {
        // Hold mode never runs the assistive loop
        self.assistive_loop_active.store(false, Ordering::SeqCst);
        // Check backend health before starting (skip in tests: no backend available)
        if !cfg!(test) {
            match crate::client::check_health().await {
                Ok(true) => {}
                Ok(false) => {
                    warn!("Whisper engine not ready");
                    crate::voice_chat_ui::update_voice_chat_status("Backend unavailable");
                    return Ok(());
                }
                Err(e) => {
                    error!("Whisper engine unavailable: {}", e);
                    crate::voice_chat_ui::update_voice_chat_status("Backend unavailable");
                    return Ok(());
                }
            }
        }

        let config = self.config.read().await;
        let delay_ms = config.hold_start_delay_ms;
        let beep = config.beep_on_start;
        let language = config.whisper_language;
        drop(config); // Release read lock

        let hold_mode = Arc::clone(&self.hold_mode);

        debug!(
            "Scheduling hold-start after {}ms delay (hold_mode={:?})",
            delay_ms,
            *hold_mode.read().await
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
        let assistive_context = Arc::clone(&self.assistive_context);
        let event_broadcast = self.event_broadcast.clone();
        let serial_lock = Arc::clone(&self.serial_lock);
        let opened_overlay_for_transcription =
            Arc::clone(&self.opened_voice_chat_overlay_for_transcription);

        let task = tokio::spawn(async move {
            // Wait for the configured delay
            tokio::time::sleep(delay).await;

            // Serialize with other start/stop operations.
            let _serial_guard = serial_lock.lock().await;

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

            let hold_mode = *hold_mode.read().await;
            let is_assistive = matches!(hold_mode, HoldMode::Chat | HoldMode::Selection);

            // Capture context IMMEDIATELY, before starting recorder/badge/beep.
            // Any of those can transiently make CodeScribe frontmost, which breaks
            // the osascript frontmost-app query and skips selection capture.
            let captured_ctx = match (is_assistive, hold_mode) {
                (true, HoldMode::Selection) => {
                    tokio::task::spawn_blocking(capture_assistive_context)
                        .await
                        .unwrap_or_default()
                }
                _ => {
                    tokio::task::spawn_blocking(capture_frontmost_app_only)
                        .await
                        .unwrap_or_default()
                }
            };

            // Start the recorder (skip in tests: no CoreAudio device needed)
            // hang_sec is configured via CODESCRIBE_VAD_MAX_SILENCE_SEC env var (single source of truth)
            let mut rec = recorder.lock().await;
            // Hold-to-talk: the key-down is the source of truth. Don't auto-stop mid-hold.
            rec.recorder.config.auto_silence = false;
            rec.set_utterance_callback(None);
            rec.set_utterance_silence_sec(None);
            rec.recorder.set_on_vad_stop(move || {
                info!("VAD callback: setting vad_triggered flag");
                vad_flag.store(true, Ordering::SeqCst);
            });

            // Set session mode for delta routing BEFORE starting the pipeline,
            // so the very first deltas route to the correct overlay.
            set_assistive_session(is_assistive);

            let use_event_pipeline = std::env::var("CODESCRIBE_EVENT_PIPELINE")
                .ok()
                .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
                .unwrap_or(false);

            if use_event_pipeline {
                // Event-based pipeline: PresentationEmitter routes through BufferedEmitter
                // (Backspace Magic). Hold mode has no utterance callback — text is collected on key-up.
                let tb = rec.transcript_buffer_handle();
                let delta_sink: Arc<dyn codescribe_core::pipeline::contracts::DeltaSink> =
                    Arc::new(helpers::RoutingDeltaSink);
                let pe: Arc<dyn codescribe_core::pipeline::contracts::EventSink> =
                    Arc::new(PresentationEmitter::new(tb, Some(delta_sink), None));
                let ipc_sink: Arc<dyn codescribe_core::pipeline::contracts::EventSink> =
                    Arc::new(helpers::IpcBroadcastSink::new(event_broadcast.clone()));
                rec.set_event_sink(Some(
                    codescribe_core::pipeline::sinks::FanoutEventSink::pair(pe, ipc_sink),
                ));
            }
            if !cfg!(test) {
                let start_result = if use_event_pipeline {
                    rec.start_event_session(Some(language.as_str().to_string()))
                        .await
                } else {
                    rec.start(Some(language.as_str().to_string())).await
                };
                if let Err(e) = start_result {
                    if Self::is_already_in_progress_error(&e) {
                        warn!("Hold-start hit stale recorder lock; forcing stop and retrying once");
                        if let Err(stop_err) = rec.stop_without_saving().await {
                            warn!("Hold-start stale-recorder recovery failed: {stop_err}");
                        }
                        rec.set_utterance_callback(None);
                        rec.set_utterance_silence_sec(None);

                        let retry_result = if use_event_pipeline {
                            let tb = rec.transcript_buffer_handle();
                            let delta_sink: Arc<
                                dyn codescribe_core::pipeline::contracts::DeltaSink,
                            > = Arc::new(helpers::RoutingDeltaSink);
                            let pe: Arc<dyn codescribe_core::pipeline::contracts::EventSink> =
                                Arc::new(PresentationEmitter::new(tb, Some(delta_sink), None));
                            let ipc_sink: Arc<dyn codescribe_core::pipeline::contracts::EventSink> =
                                Arc::new(helpers::IpcBroadcastSink::new(event_broadcast.clone()));
                            rec.set_event_sink(Some(
                                codescribe_core::pipeline::sinks::FanoutEventSink::pair(
                                    pe, ipc_sink,
                                ),
                            ));
                            rec.start_event_session(Some(language.as_str().to_string()))
                                .await
                        } else {
                            rec.set_event_sink(None);
                            rec.start(Some(language.as_str().to_string())).await
                        };
                        if let Err(retry_err) = retry_result {
                            error!("Failed to start recorder after recovery: {retry_err}");
                            *session_id.write().await = None;
                            set_assistive_session(false);
                            return;
                        }
                    } else {
                        error!("Failed to start recorder: {e}");
                        *session_id.write().await = None;
                        set_assistive_session(false);
                        return;
                    }
                }
            }

            // Transition to REC_HOLD as soon as recorder starts to avoid IDLE/active races.
            Self::set_state_with_broadcast(&state, &event_broadcast, State::RecHold).await;
            info!(
                "STATE TRANSITION: IDLE → REC_HOLD (assistive={}, hold_mode={:?})",
                is_assistive, hold_mode
            );

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

            if is_assistive {
                opened_overlay_for_transcription.store(false, Ordering::SeqCst);
                *assistive_context.write().await = Some(captured_ctx);
                crate::voice_chat_ui::set_voice_chat_target_app(
                    assistive_context
                        .read()
                        .await
                        .clone()
                        .unwrap_or_default()
                        .frontmost_app,
                );

                crate::hide_transcription_overlay_with_reason("start_assistive_hold");
                crate::show_voice_chat_overlay();
                crate::show_agent_tab();
                crate::voice_chat_ui::update_voice_chat_status("Listening...");
            } else {
                *assistive_context.write().await = Some(captured_ctx);
                crate::voice_chat_ui::set_voice_chat_target_app(
                    assistive_context
                        .read()
                        .await
                        .clone()
                        .unwrap_or_default()
                        .frontmost_app,
                );
                opened_overlay_for_transcription.store(false, Ordering::SeqCst);
                crate::show_transcription_overlay();
                crate::enter_recording_mode();
                crate::clear_transcription_text();
            }
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
                    crate::voice_chat_ui::update_voice_chat_status("Backend unavailable");
                    return Ok(());
                }
                Err(e) => {
                    error!("Whisper engine unavailable: {}", e);
                    crate::voice_chat_ui::update_voice_chat_status("Backend unavailable");
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

        if is_assistive {
            *self.assistive_mode.write().await = true;
            *self.force_raw_mode.write().await = false;
            *self.force_ai_mode.write().await = false;
        }
        self.assistive_loop_active
            .store(is_assistive, Ordering::SeqCst);
        self.toggle_user_has_text.store(false, Ordering::SeqCst);
        self.toggle_assistant_has_text
            .store(false, Ordering::SeqCst);

        info!("Starting toggle recording (session={})", new_session_id);

        let config = self.config.read().await.clone();
        let language = config.whisper_language;
        let toggle_silence_sec = config.toggle_silence_sec;
        let beep_enabled = config.beep_on_start;
        let use_buffered_stream = std::env::var("CODESCRIBE_BUFFERED_STREAM")
            .ok()
            .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
            .unwrap_or(true);

        // Start the recorder
        let mut recorder = self.recorder.lock().await;

        // Toggle mode: continuous recording; silence only triggers per-utterance send.
        recorder.recorder.config.auto_silence = false;
        recorder.recorder.set_on_vad_stop(|| {});
        recorder.set_utterance_silence_sec(Some(toggle_silence_sec));

        // Set session mode for delta routing BEFORE starting the pipeline,
        // so the very first deltas route to the correct overlay.
        set_assistive_session(is_assistive);

        let use_event_pipeline = std::env::var("CODESCRIBE_EVENT_PIPELINE")
            .ok()
            .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
            .unwrap_or(false);

        if use_event_pipeline {
            // Event-based pipeline: PresentationEmitter routes through BufferedEmitter
            // (Backspace Magic). Toggle mode gets utterance callback for auto-send.
            let controller = OVERLAY_CONTROLLER.get().cloned();
            let expected_session = new_session_id.clone();
            let is_assistive_session = is_assistive;
            let event_broadcast = self.event_broadcast.clone();

            let tb = recorder.transcript_buffer_handle();
            let delta_sink: Arc<dyn codescribe_core::pipeline::contracts::DeltaSink> =
                Arc::new(helpers::RoutingDeltaSink);
            let mut pe = PresentationEmitter::new(tb, Some(delta_sink), None);
            pe.set_utterance_callback(Some(Arc::new(move |text: String| {
                if is_assistive_session {
                    // Close the current streaming user bubble immediately at utterance boundary
                    // to prevent next preview from appending into the previous message.
                    crate::voice_chat_ui::finalize_voice_chat_user_message();
                }
                let controller = controller.clone();
                let expected_session = expected_session.clone();
                tokio::spawn(async move {
                    if let Some(controller) = controller
                        && let Err(e) = controller
                            .handle_toggle_utterance(
                                text,
                                expected_session,
                                is_assistive_session,
                                true, // skip_user_bubble: Preview already streams into bubble
                            )
                            .await
                    {
                        warn!("Toggle utterance processing failed: {}", e);
                    }
                });
            })));

            let pe: Arc<dyn codescribe_core::pipeline::contracts::EventSink> = Arc::new(pe);
            let ipc_sink: Arc<dyn codescribe_core::pipeline::contracts::EventSink> =
                Arc::new(helpers::IpcBroadcastSink::new(event_broadcast));
            recorder.set_event_sink(Some(
                codescribe_core::pipeline::sinks::FanoutEventSink::pair(pe, ipc_sink),
            ));

            if !cfg!(test)
                && let Err(e) = recorder
                    .start_event_session(Some(language.as_str().to_string()))
                    .await
            {
                if Self::is_already_in_progress_error(&e) {
                    warn!("Toggle start hit stale recorder lock; forcing stop and retrying once");
                    if let Err(stop_err) = recorder.stop_without_saving().await {
                        warn!("Toggle stale-recorder recovery failed: {stop_err}");
                    }
                    recorder.set_utterance_callback(None);
                    recorder.set_utterance_silence_sec(None);

                    let retry_controller = OVERLAY_CONTROLLER.get().cloned();
                    let retry_expected_session = new_session_id.clone();
                    let retry_assistive_session = is_assistive;
                    let retry_event_broadcast = self.event_broadcast.clone();
                    let tb = recorder.transcript_buffer_handle();
                    let delta_sink: Arc<dyn codescribe_core::pipeline::contracts::DeltaSink> =
                        Arc::new(helpers::RoutingDeltaSink);
                    let mut retry_pe = PresentationEmitter::new(tb, Some(delta_sink), None);
                    retry_pe.set_utterance_callback(Some(Arc::new(move |text: String| {
                        if retry_assistive_session {
                            crate::voice_chat_ui::finalize_voice_chat_user_message();
                        }
                        let controller = retry_controller.clone();
                        let expected_session = retry_expected_session.clone();
                        tokio::spawn(async move {
                            if let Some(controller) = controller
                                && let Err(e) = controller
                                    .handle_toggle_utterance(
                                        text,
                                        expected_session,
                                        retry_assistive_session,
                                        true,
                                    )
                                    .await
                            {
                                warn!("Toggle utterance processing failed: {}", e);
                            }
                        });
                    })));
                    let retry_pe: Arc<dyn codescribe_core::pipeline::contracts::EventSink> =
                        Arc::new(retry_pe);
                    let retry_ipc_sink: Arc<dyn codescribe_core::pipeline::contracts::EventSink> =
                        Arc::new(helpers::IpcBroadcastSink::new(retry_event_broadcast));
                    recorder.set_event_sink(Some(
                        codescribe_core::pipeline::sinks::FanoutEventSink::pair(
                            retry_pe,
                            retry_ipc_sink,
                        ),
                    ));
                    if let Err(retry_err) = recorder
                        .start_event_session(Some(language.as_str().to_string()))
                        .await
                    {
                        *self.session_id.write().await = None;
                        self.assistive_loop_active.store(false, Ordering::SeqCst);
                        set_assistive_session(false);
                        return Err(anyhow::anyhow!(
                            "Failed to start event session after recovery: {retry_err}"
                        ));
                    }
                } else {
                    *self.session_id.write().await = None;
                    self.assistive_loop_active.store(false, Ordering::SeqCst);
                    set_assistive_session(false);
                    return Err(e);
                }
            }
        } else {
            // Legacy pipeline: separate delta_callback + utterance_callback
            let controller = OVERLAY_CONTROLLER.get().cloned();
            let expected_session = new_session_id.clone();
            let is_assistive_session = is_assistive;
            recorder.set_utterance_callback(Some(Arc::new(move |text: String| {
                if is_assistive_session {
                    // Keep utterance boundaries explicit in assistive mode.
                    crate::voice_chat_ui::finalize_voice_chat_user_message();
                }
                let controller = controller.clone();
                let expected_session = expected_session.clone();
                tokio::spawn(async move {
                    if let Some(controller) = controller
                        && let Err(e) = controller
                            .handle_toggle_utterance(
                                text,
                                expected_session,
                                is_assistive_session,
                                true, // skip_user_bubble: delta_callback already streams into bubble
                            )
                            .await
                    {
                        warn!("Toggle utterance processing failed: {}", e);
                    }
                });
            })));

            // Set streaming callback for overlay updates (routed by session mode)
            recorder.set_delta_callback(Some(Arc::new(
                codescribe_core::pipeline::sinks::CallbackSink::new(Arc::new(|text: &str| {
                    route_transcription_delta(text);
                })),
            )));

            debug!(
                "Legacy toggle pipeline using buffered_stream={}",
                use_buffered_stream
            );

            // Skip actual audio stream in tests (no CoreAudio device needed)
            if !cfg!(test)
                && let Err(e) = recorder
                    .start_with_buffered(Some(language.as_str().to_string()), use_buffered_stream)
                    .await
            {
                if Self::is_already_in_progress_error(&e) {
                    warn!("Toggle start hit stale recorder lock; forcing stop and retrying once");
                    if let Err(stop_err) = recorder.stop_without_saving().await {
                        warn!("Toggle stale-recorder recovery failed: {stop_err}");
                    }
                    recorder.set_utterance_callback(None);
                    recorder.set_utterance_silence_sec(None);
                    recorder.set_event_sink(None);
                    if let Err(retry_err) = recorder
                        .start_with_buffered(
                            Some(language.as_str().to_string()),
                            use_buffered_stream,
                        )
                        .await
                    {
                        *self.session_id.write().await = None;
                        self.assistive_loop_active.store(false, Ordering::SeqCst);
                        set_assistive_session(false);
                        return Err(anyhow::anyhow!(
                            "Failed to start buffered recorder after recovery: {retry_err}"
                        ));
                    }
                } else {
                    *self.session_id.write().await = None;
                    self.assistive_loop_active.store(false, Ordering::SeqCst);
                    set_assistive_session(false);
                    return Err(e);
                }
            }
        }

        // Transition to REC_TOGGLE immediately after recorder starts.
        self.set_state(State::RecToggle).await;
        info!("STATE TRANSITION: IDLE → REC_TOGGLE (pulsing badge)");

        // Play start beep if enabled
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

        if is_assistive {
            self.opened_voice_chat_overlay_for_transcription
                .store(false, Ordering::SeqCst);
            // Toggle-assistive is a hands-off chat loop with optional selection context.
            // Capture selection when available (best-effort), otherwise just app name.
            let ctx = tokio::task::spawn_blocking(capture_assistive_context)
                .await
                .unwrap_or_default();
            *self.assistive_context.write().await = Some(ctx);
            crate::voice_chat_ui::set_voice_chat_target_app(
                self.assistive_context
                    .read()
                    .await
                    .clone()
                    .unwrap_or_default()
                    .frontmost_app,
            );

            crate::hide_transcription_overlay_with_reason("start_assistive_toggle");
            crate::show_voice_chat_overlay();
            crate::show_agent_tab();
            crate::voice_chat_ui::update_voice_chat_status("Listening...");
        } else {
            // Capture frontmost app for paste actions (no selection/clipboard).
            let ctx = tokio::task::spawn_blocking(capture_frontmost_app_only)
                .await
                .unwrap_or_default();
            *self.assistive_context.write().await = Some(ctx);
            crate::voice_chat_ui::set_voice_chat_target_app(
                self.assistive_context
                    .read()
                    .await
                    .clone()
                    .unwrap_or_default()
                    .frontmost_app,
            );
            self.opened_voice_chat_overlay_for_transcription
                .store(false, Ordering::SeqCst);
            crate::show_transcription_overlay();
            crate::enter_recording_mode();
            crate::clear_transcription_text();
        }

        Ok(())
    }

    async fn handle_toggle_utterance(
        &self,
        raw_text: String,
        expected_session: String,
        is_assistive: bool,
        skip_user_bubble: bool,
    ) -> Result<()> {
        if raw_text.trim().is_empty() {
            if is_assistive {
                crate::voice_chat_ui::set_voice_chat_sending(false);
                crate::voice_chat_ui::update_voice_chat_status("Listening...");
            }
            return Ok(());
        }

        // Skip if another session is active. If session_id is None, allow final flush.
        if let Some(current) = self.session_id.read().await.clone()
            && current != expected_session
        {
            debug!("Ignoring stale toggle utterance (session changed)");
            return Ok(());
        }

        let _guard = self.serial_lock.lock().await;

        // Snapshot mode flags
        let hold_mode = *self.hold_mode.read().await;
        let force_raw = *self.force_raw_mode.read().await;
        let force_ai = *self.force_ai_mode.read().await;

        if is_assistive {
            let ctx = tokio::task::spawn_blocking(capture_assistive_context)
                .await
                .unwrap_or_default();
            *self.assistive_context.write().await = Some(ctx);
        } else {
            let ctx = tokio::task::spawn_blocking(capture_frontmost_app_only)
                .await
                .unwrap_or_default();
            *self.assistive_context.write().await = Some(ctx);
        }

        crate::voice_chat_ui::set_voice_chat_target_app(
            self.assistive_context
                .read()
                .await
                .clone()
                .unwrap_or_default()
                .frontmost_app,
        );

        let config = self.config.read().await.clone();
        let language_opt = Some(config.whisper_language.as_str().to_string());
        let user_needs_separator = false;
        let assistant_needs_separator = false;

        let result = self
            .process_transcript_text_pipeline(types::TranscriptPipelineParams {
                raw_text,
                recording_timestamp: chrono::Local::now(),
                assistive: is_assistive,
                hold_mode,
                force_raw,
                force_ai,
                config,
                language_opt,
                raw_save_enabled: raw_save_enabled(),
                audio_path: None,
                cloud_text_opt: None,
                cloud_handle: None,
                append_mode: false,
                user_needs_separator,
                assistant_needs_separator,
                skip_user_bubble,
            })
            .await;

        if *self.state.read().await == State::RecToggle && is_assistive {
            crate::voice_chat_ui::set_voice_chat_sending(false);
            crate::voice_chat_ui::update_voice_chat_status("Listening...");
        }

        result
    }

    async fn stop_toggle_recording(&self) -> Result<()> {
        // Ignore if not recording
        if *self.state.read().await != State::RecToggle {
            return Ok(());
        }

        info!("Stopping toggle recording");

        // Stop recording and flush buffered worker
        let mut recorder = self.recorder.lock().await;
        if !cfg!(test) {
            let _ = recorder
                .stop_without_saving()
                .await
                .context("Failed to stop recorder")?;
        }
        recorder.set_utterance_callback(None);
        recorder.set_utterance_silence_sec(None);
        drop(recorder);

        // Reset state
        self.set_state(State::Idle).await;
        *self.assistive_mode.write().await = false;
        *self.hold_mode.write().await = HoldMode::Raw;
        *self.force_raw_mode.write().await = false;
        *self.force_ai_mode.write().await = false;
        *self.session_id.write().await = None;
        self.assistive_loop_active.store(false, Ordering::SeqCst);
        if self.toggle_user_has_text.load(Ordering::SeqCst) {
            crate::voice_chat_ui::finalize_voice_chat_user_message();
        }
        if self.toggle_assistant_has_text.load(Ordering::SeqCst) {
            crate::voice_chat_ui::finalize_voice_chat_assistant_message();
        }
        self.toggle_user_has_text.store(false, Ordering::SeqCst);
        self.toggle_assistant_has_text
            .store(false, Ordering::SeqCst);
        set_assistive_session(false);

        hide_hold_badge();
        crate::voice_chat_ui::update_voice_chat_status("Ready");

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
        self.set_state(State::Busy).await;

        // Get session ID and mode flags before we reset them
        let session_id = self.session_id.read().await.clone();
        let assistive = *self.assistive_mode.read().await;
        let hold_mode = *self.hold_mode.read().await;
        let force_raw = *self.force_raw_mode.read().await;
        let force_ai = *self.force_ai_mode.read().await;

        // Switch badge to processing mode (orange, pulsing)
        show_badge_for_mode(BadgeMode::Processing);

        let result = self
            .process_recording(session_id, assistive, hold_mode, force_raw, force_ai)
            .await;

        // Always reset to IDLE, even on error
        self.set_state(State::Idle).await;
        *self.assistive_mode.write().await = false;
        *self.hold_mode.write().await = HoldMode::Raw;
        *self.force_raw_mode.write().await = false;
        *self.force_ai_mode.write().await = false;
        *self.session_id.write().await = None;
        *self.assistive_context.write().await = None;
        self.assistive_loop_active.store(false, Ordering::SeqCst);
        // Keep event-router sink selection in sync with controller state after finish.
        set_assistive_session(false);

        // Hide red dot indicator
        hide_hold_badge();

        // Update tray icon based on result
        match &result {
            Ok(_) => {
                crate::voice_chat_ui::update_voice_chat_status("Ready");
                info!("Processing finished successfully. State reset to IDLE.");

                // Overlay policy:
                // - Inline edit: auto-hide (text replaced in-place, overlay not needed).
                // - Assistive: keep chat overlay alive (bubbles persist).
                // - Dictation (RAW/AI/save-only): keep final transcript briefly, then auto-hide.
                let is_inline_edit = self.config.read().await.inline_edit_enabled
                    && assistive
                    && matches!(hold_mode, HoldMode::Selection);
                if !assistive || is_inline_edit {
                    let opened = self
                        .opened_voice_chat_overlay_for_transcription
                        .swap(false, Ordering::SeqCst);
                    if opened {
                        crate::voice_chat_ui::hide_voice_chat_overlay();
                    }

                    // Keep final transcript visible briefly (also for RAW/save-only),
                    // then auto-hide. This gives user feedback even when we auto-paste.
                    crate::enter_decision_mode();
                    crate::schedule_auto_hide();
                }
            }
            Err(e) => {
                error!("Processing failed: {}", e);
                crate::voice_chat_ui::update_voice_chat_status("Processing failed");

                // Hide overlay immediately on error
                let opened = self
                    .opened_voice_chat_overlay_for_transcription
                    .swap(false, Ordering::SeqCst);
                if opened {
                    crate::voice_chat_ui::hide_voice_chat_overlay();
                }
                crate::hide_transcription_overlay_with_reason("finish_error");
            }
        }

        result
    }

    /// Process the recording: stop, transcribe, format, paste
    ///
    /// ## Mode Logic:
    /// - `assistive=true`: ALWAYS AI augmentation (HoldMode::Chat / HoldMode::Selection)
    /// - `force_raw=true`: ALWAYS raw transcript (HoldMode::Raw)
    /// - `force_ai=true`: ALWAYS AI formatting (explicit force path)
    /// - Neither: Toggle mode - respects AI_FORMATTING_ENABLED setting
    async fn process_recording(
        &self,
        _session_id: Option<String>,
        assistive: bool,
        hold_mode: HoldMode,
        force_raw: bool,
        force_ai: bool,
    ) -> Result<()> {
        if cfg!(test) {
            info!(
                "process_recording: skipped in tests (assistive={}, hold_mode={:?}, force_raw={}, force_ai={})",
                assistive, hold_mode, force_raw, force_ai
            );
            return Ok(());
        }

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

        // In assistive mode, we want to update overlay state even if the window hasn't been
        // realized on the main thread yet. This avoids "dead" overlays due to timing.
        let chat_active = assistive;
        let assistive_loop = assistive && self.assistive_loop_active.load(Ordering::SeqCst);

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

        // Optional "final pass" local STT:
        // Streaming can be great for live UX, but it can also produce duplicates/truncations
        // depending on chunking/VAD. For final output (paste/save), prefer transcribing the
        // full recorded audio file when available.
        //
        // Default: enabled (set CODESCRIBE_LOCAL_STT_FINAL_PASS=0 to disable).
        let local_final_pass_enabled = std::env::var("CODESCRIBE_LOCAL_STT_FINAL_PASS")
            .ok()
            .map(|v| !matches!(v.to_lowercase().as_str(), "0" | "false" | "no" | "off"))
            .unwrap_or(true);

        if use_local_stt && local_final_pass_enabled {
            if let Some(path) = &audio_path {
                let wav_path = path.as_path().to_path_buf();
                let lang = language_opt.map(str::to_string);

                if chat_active {
                    crate::voice_chat_ui::update_voice_chat_status("Finalizing…");
                }

                info!(
                    "Running final-pass local STT from audio file (overrides streaming): {}",
                    wav_path.display()
                );

                match tokio::task::spawn_blocking(move || {
                    crate::whisper::transcribe_file(&wav_path, lang.as_deref())
                })
                .await
                {
                    Ok(Ok(text)) if !text.trim().is_empty() => {
                        info!("Final-pass transcription captured ({} chars)", text.len());
                        raw_text_opt = Some(text);
                    }
                    Ok(Ok(_)) => warn!("Final-pass transcription returned empty text"),
                    Ok(Err(e)) => warn!("Final-pass transcription failed: {}", e),
                    Err(e) => warn!("Final-pass transcription task failed: {}", e),
                }
            } else {
                warn!("Final-pass local STT skipped: no audio file available");
            }
        }

        // 1. Try Streaming Result (Local)
        if raw_text_opt.is_none() {
            if !streaming_text.trim().is_empty() {
                if !use_local_stt {
                    warn!("Using streaming transcript fallback (USE_LOCAL_STT=0)");
                }
                info!(
                    "Using streaming transcription result ({} chars)",
                    streaming_text.len()
                );
                raw_text_opt = Some(streaming_text);
            } else if use_local_stt {
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

        let raw_text = match raw_text_opt {
            Some(text) if !text.trim().is_empty() => text,
            Some(_) | None => {
                if assistive_loop {
                    if chat_active {
                        crate::voice_chat_ui::set_voice_chat_sending(false);
                        crate::voice_chat_ui::update_voice_chat_status("Listening...");
                    }
                    warn!("Empty transcript in assistive loop; skipping");
                    return Ok(());
                }
                return Err(anyhow::anyhow!("Empty transcript"));
            }
        };

        info!("Raw transcript captured ({} chars)", raw_text.len());

        let language_opt = Some(language.as_str().to_string());
        self.process_transcript_text_pipeline(types::TranscriptPipelineParams {
            raw_text,
            recording_timestamp,
            assistive,
            hold_mode,
            force_raw,
            force_ai,
            config,
            language_opt,
            raw_save_enabled,
            audio_path,
            cloud_text_opt,
            cloud_handle,
            append_mode: false,
            user_needs_separator: false,
            assistant_needs_separator: false,
            skip_user_bubble: false,
        })
        .await
    }

    async fn process_transcript_text_pipeline(
        &self,
        p: types::TranscriptPipelineParams,
    ) -> Result<()> {
        let types::TranscriptPipelineParams {
            raw_text,
            recording_timestamp,
            assistive,
            hold_mode,
            force_raw,
            force_ai,
            config,
            language_opt,
            raw_save_enabled,
            audio_path,
            cloud_text_opt,
            cloud_handle,
            append_mode,
            user_needs_separator,
            assistant_needs_separator,
            skip_user_bubble,
        } = p;
        let language_opt = language_opt.as_deref();

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

        let chat_active = assistive;

        let mut effective_hold_mode = if assistive && matches!(hold_mode, HoldMode::Raw) {
            // Toggle-assistive path doesn't have a meaningful hold-mode; treat as Chat
            // but allow optional selection context if it was captured.
            HoldMode::Chat
        } else {
            hold_mode
        };

        // Determine final text based on mode (NEW architecture):
        //
        // 1. HoldMode::Chat / HoldMode::Selection (assistive=true): ALWAYS AI augmentation
        // 2. Ctrl Hold (force_raw=true): ALWAYS raw transcript (ignores AI toggle)
        // 3. Explicit force_ai=true: ALWAYS AI formatting
        // 4. Toggle (neither): respects AI_FORMATTING_ENABLED toggle
        //
        // This allows users to choose mode via hotkey:
        // - Quick dictation? → Ctrl (fast, raw)
        // - Need formatting? → Double Option (respects setting)
        // - AI chat? → Hold + Shift (Chat)
        // - AI on selection? → Hold + Cmd (Selection)
        let (mut formatted_text, output_kind, mut should_auto_paste) = if assistive {
            info!(
                "Assistive mode ({:?}): augmenting transcript via AI",
                effective_hold_mode
            );

            // Inline edit candidate: skip overlay entirely — result goes in-place.
            // If selection is missing (detected later), mode falls to Chat and overlay is shown then.
            let inline_edit_candidate = config.inline_edit_enabled
                && matches!(effective_hold_mode, HoldMode::Selection);

            if chat_active && !inline_edit_candidate {
                crate::show_voice_chat_overlay();
                if skip_user_bubble {
                    // Event pipeline: Preview already streamed text into the bubble.
                    // Just finalize the user message (stop streaming indicator)
                    // without re-writing the text.
                    crate::voice_chat_ui::finalize_voice_chat_user_message();
                    self.toggle_user_has_text.store(true, Ordering::SeqCst);
                } else if append_mode {
                    if user_needs_separator {
                        crate::voice_chat_ui::append_voice_chat_user_delta("\n\n");
                    }
                    crate::voice_chat_ui::append_voice_chat_user_delta(&clean_text);
                    self.toggle_user_has_text.store(true, Ordering::SeqCst);
                } else {
                    crate::voice_chat_ui::set_voice_chat_user_text(&clean_text);
                }
                crate::voice_chat_ui::show_agent_tab();
                crate::voice_chat_ui::set_voice_chat_sending(true);
                crate::voice_chat_ui::update_voice_chat_status("Thinking...");
            }

            let mut ctx = self
                .assistive_context
                .read()
                .await
                .clone()
                .unwrap_or_default();

            // Ensure we have a target app label (best-effort, no selection, no clipboard).
            if ctx.frontmost_app.is_none() {
                ctx.frontmost_app = tokio::task::spawn_blocking(capture_frontmost_app_only)
                    .await
                    .ok()
                    .and_then(|c| c.frontmost_app);
            }

            {
                let app = ctx
                    .frontmost_app
                    .as_deref()
                    .unwrap_or("?")
                    .trim()
                    .to_string();
                let sel_len = ctx.selected_text.as_deref().unwrap_or("").len();
                crate::voice_chat_ui::update_voice_chat_context_summary(&format!(
                    "ctx: {} | sel: {}",
                    app, sel_len
                ));
            }

            let missing_selection = matches!(effective_hold_mode, HoldMode::Selection)
                && ctx.selected_text.as_deref().unwrap_or("").trim().is_empty();
            if missing_selection {
                warn!(
                    "Selection mode requested, but no selected text captured; falling back to Chat mode"
                );
                effective_hold_mode = HoldMode::Chat;
                if chat_active {
                    // Inline edit skipped overlay earlier; now that we fell back to Chat, show it.
                    if inline_edit_candidate {
                        crate::show_voice_chat_overlay();
                        crate::voice_chat_ui::set_voice_chat_user_text(&clean_text);
                        crate::voice_chat_ui::show_agent_tab();
                        crate::voice_chat_ui::set_voice_chat_sending(true);
                    }
                    crate::voice_chat_ui::update_voice_chat_status(
                        "Selection unavailable - chat fallback",
                    );
                    crate::voice_chat_ui::add_voice_chat_system_message(
                        "Selection was not detected. Continuing without selected-text context.",
                    );
                }
            }

            // Split behavior:
            // - Chat: ignore selection.
            // - Selection: if no selection was captured, we already downgraded to Chat mode.
            let assistive_input = build_assistive_input(&clean_text, &ctx);

            let lang_str = language_opt.map(String::from);

            // Assistive/chat responses should always stream to preserve progressive feedback.
            let use_streaming = chat_active;

            // Callback for streaming AI response to overlay (assistant channel only).
            let streamed_any_delta = Arc::new(AtomicBool::new(false));
            let delta_callback = if use_streaming && chat_active {
                let needs_prefix = append_mode && assistant_needs_separator;
                let prefix_sent = Arc::new(AtomicBool::new(false));
                let assistant_has_text = self.toggle_assistant_has_text.clone();
                let streamed_any_delta = Arc::clone(&streamed_any_delta);
                Some(Arc::new(move |text: &str| {
                    if needs_prefix && !prefix_sent.swap(true, Ordering::SeqCst) {
                        crate::voice_chat_ui::append_voice_chat_assistant_delta("\n\n");
                    }
                    streamed_any_delta.store(true, Ordering::SeqCst);
                    crate::voice_chat_ui::append_voice_chat_assistant_delta(text);
                    assistant_has_text.store(true, Ordering::SeqCst);
                }) as Arc<dyn Fn(&str) + Send + Sync>)
            } else {
                None
            };

            let result = crate::ai_formatting::format_text_with_status_channels(
                &assistive_input,
                lang_str.as_deref(),
                true,
                delta_callback,
                None,
            )
            .await;
            let kind = match result.status {
                crate::ai_formatting::AiFormatStatus::Applied => {
                    // Inline edit: text is replaced in-place in the target app.
                    // Skip overlay display to avoid stealing focus and freezing.
                    let is_inline = config.inline_edit_enabled
                        && matches!(effective_hold_mode, HoldMode::Selection);
                    if chat_active && !is_inline {
                        let streamed = use_streaming && streamed_any_delta.load(Ordering::SeqCst);
                        // Display AI response in overlay
                        crate::show_voice_chat_overlay();
                        crate::voice_chat_ui::update_voice_chat_status("AI Response:");
                        if streamed {
                            crate::voice_chat_ui::finalize_voice_chat_assistant_message();
                        } else if append_mode {
                            if assistant_needs_separator {
                                crate::voice_chat_ui::append_voice_chat_assistant_delta("\n\n");
                            }
                            crate::voice_chat_ui::append_voice_chat_assistant_delta(&result.text);
                            self.toggle_assistant_has_text.store(true, Ordering::SeqCst);
                        } else {
                            crate::voice_chat_ui::set_voice_chat_text(&result.text);
                        }
                        info!(
                            "Assistive response displayed in overlay ({} chars)",
                            result.text.len()
                        );

                        if let Some(reasoning_text) = result.reasoning_text.clone() {
                            crate::voice_chat_ui::add_voice_chat_system_message(&format!(
                                "Reasoning summary:\n{}",
                                reasoning_text
                            ));
                        }
                    } else if is_inline {
                        info!(
                            "Inline edit: skipping overlay display ({} chars)",
                            result.text.len()
                        );
                    }
                    crate::state::history::TranscriptKind::Ai
                }
                crate::ai_formatting::AiFormatStatus::Failed => {
                    if chat_active {
                        crate::show_voice_chat_overlay();
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
            // Inline edit: Selection mode auto-pastes back (replaces the selection in-place).
            let inline_paste = config.inline_edit_enabled
                && matches!(effective_hold_mode, HoldMode::Selection)
                && matches!(result.status, crate::ai_formatting::AiFormatStatus::Applied);
            (result.text, kind, inline_paste)
        } else if force_raw {
            // Ctrl Hold: ALWAYS raw transcript (fast dictation mode)
            // Post-processed clean_text is used (lexicon + cleanup already applied)
            if has_repetition {
                info!("Raw mode (Ctrl): applying local repetition cleanup on post-processed text");
                (
                    crate::ai_formatting::remove_simple_repetitions(&clean_text),
                    crate::state::history::TranscriptKind::Raw,
                    true,
                )
            } else {
                info!("Raw mode (Ctrl): using post-processed transcript");
                (
                    clean_text.clone(),
                    crate::state::history::TranscriptKind::Raw,
                    true,
                )
            }
        } else if force_ai {
            // Left double Option: ALWAYS formatting (no augmentation)
            // Auto-paste like hold mode — formatted text goes where the cursor is.
            let should_use_ai = crate::ai_formatting::has_api_key();
            if should_use_ai {
                info!("Formatting mode (force_ai): correcting transcript via AI");

                let lang_str = language_opt.map(String::from);
                let result = crate::ai_formatting::format_text_with_status(
                    &clean_text,
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
                (result.text, kind, true)
            } else if has_repetition {
                info!("Formatting mode (force_ai): AI unavailable, cleaning repetitions");
                (
                    crate::ai_formatting::remove_simple_repetitions(&clean_text),
                    crate::state::history::TranscriptKind::Raw,
                    true,
                )
            } else {
                info!(
                    "Formatting mode (force_ai): AI unavailable, using post-processed transcript"
                );
                (
                    clean_text.clone(),
                    crate::state::history::TranscriptKind::Raw,
                    true,
                )
            }
        } else {
            // Double Option: respects AI Formatting toggle setting
            let ai_formatting_enabled = config.ai_formatting_enabled;
            let should_use_ai = ai_formatting_enabled && crate::ai_formatting::has_api_key();

            if should_use_ai {
                // Toggle ON: formatting only (no augmentation)
                info!("Formatting mode (Toggle): correcting transcript via AI");

                let lang_str = language_opt.map(String::from);
                let result = crate::ai_formatting::format_text_with_status(
                    &clean_text,
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
                (result.text, kind, false)
            } else if has_repetition {
                // Toggle OFF with repetition: local cleanup only
                info!("Raw mode (Toggle OFF): applying local repetition cleanup");
                (
                    crate::ai_formatting::remove_simple_repetitions(&clean_text),
                    crate::state::history::TranscriptKind::Raw,
                    true,
                )
            } else {
                // Toggle OFF: using post-processed transcript
                info!("Raw mode (Toggle OFF): using post-processed transcript");
                (
                    clean_text.clone(),
                    crate::state::history::TranscriptKind::Raw,
                    true,
                )
            }
        };

        let mode_label = if assistive {
            match effective_hold_mode {
                HoldMode::Chat => "chat",
                HoldMode::Selection => "selection",
                HoldMode::Raw => "assistive",
            }
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
        if !assistive {
            // Keep the ephemeral transcription overlay in sync with what we will paste/save.
            // This makes it easier to understand differences between streaming preview and final-pass output.
            crate::set_transcription_text(&formatted_text);
        }

        // Quick Notes: optionally save to daily note file (dictation-only).
        if !assistive && config.quick_notes_enabled {
            let frontmost_app = tokio::task::spawn_blocking(capture_frontmost_app_only)
                .await
                .ok()
                .and_then(|ctx| ctx.frontmost_app);

            match crate::state::notes::append_quick_note(
                &formatted_text,
                recording_timestamp,
                frontmost_app.as_deref(),
            ) {
                Ok(path) => {
                    info!("Quick note saved: {}", path.display());
                    #[cfg(target_os = "macos")]
                    crate::os::notifications::notify(
                        "CodeScribe",
                        &format!(
                            "Saved note: {}",
                            path.file_name().and_then(|s| s.to_str()).unwrap_or("note")
                        ),
                    );
                }
                Err(e) => {
                    warn!("Quick note save failed: {}", e);
                }
            }

            // Optional: make Quick Notes "save-only".
            // Keep hold-to-talk RAW behavior predictable: raw hold should still paste at cursor.
            if config.quick_notes_save_only {
                if force_raw {
                    info!(
                        "Quick Notes save-only enabled, but keeping auto-paste for force_raw session"
                    );
                } else {
                    should_auto_paste = false;
                    info!("Auto-paste disabled by Quick Notes save-only mode");
                }
            }
        }

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

        if cfg!(test) {
            info!("Skipping paste in tests (mode={})", mode_label);
        } else if should_auto_paste {
            // Paste the text into the app that was frontmost when recording started.
            #[cfg(target_os = "macos")]
            {
                let target_app = self
                    .assistive_context
                    .read()
                    .await
                    .clone()
                    .and_then(|ctx| ctx.frontmost_app)
                    .map(|v| v.trim().to_string())
                    .filter(|name| !name.is_empty() && !name.eq_ignore_ascii_case("CodeScribe"));

                let current_frontmost = tokio::task::spawn_blocking(capture_frontmost_app_only)
                    .await
                    .ok()
                    .and_then(|ctx| ctx.frontmost_app)
                    .unwrap_or_default()
                    .trim()
                    .to_string();

                let should_reactivate_target = current_frontmost.eq_ignore_ascii_case("CodeScribe");
                info!(
                    "Auto-paste focus check: current='{}' target='{}' reactivate={}",
                    if current_frontmost.is_empty() {
                        "<unknown>"
                    } else {
                        &current_frontmost
                    },
                    target_app.as_deref().unwrap_or("<none>"),
                    should_reactivate_target
                );

                if should_reactivate_target && let Some(app_name) = target_app {
                    Queue::main().exec_async(move || activate_target_app(&app_name));
                    tokio::time::sleep(Duration::from_millis(160)).await;
                }
            }

            // Inline edit: use replace_selected_text (AX write → clipboard fallback)
            // to replace the selection in-place. Regular paste for all other modes.
            let is_inline_edit = config.inline_edit_enabled
                && assistive
                && matches!(effective_hold_mode, HoldMode::Selection);

            if is_inline_edit {
                // Retrieve original selection for surgical SEARCH/REPLACE editing.
                let original_selection = self
                    .assistive_context
                    .read()
                    .await
                    .clone()
                    .and_then(|ctx| ctx.selected_text)
                    .unwrap_or_default();

                // Try SEARCH/REPLACE blocks for surgical edit; fall back to full replacement.
                let mut skip_ax_write = false;
                let final_text =
                    if let Some(blocks) = edit_blocks::parse_edit_blocks(&formatted_text) {
                        match edit_blocks::validate_and_apply(&original_selection, &blocks) {
                            Ok(result) => {
                                info!(
                                    "Inline edit: applied {} SEARCH/REPLACE block(s) ({} -> {} chars)",
                                    blocks.len(),
                                    original_selection.len(),
                                    result.len()
                                );
                                result
                            }
                            Err(e) => {
                                warn!("Inline edit: SEARCH/REPLACE apply failed: {e}");
                                // Show full AI response in overlay + clipboard — user decides.
                                let display_text =
                                    edit_blocks::strip_markdown_fences(&formatted_text);
                                let _ = clipboard::set_clipboard(&display_text);
                                crate::show_voice_chat_overlay();
                                crate::voice_chat_ui::set_voice_chat_text(&display_text);
                                crate::voice_chat_ui::update_voice_chat_status(
                                    "Edit failed \u{2014} copied to clipboard",
                                );
                                info!(
                                    "Inline edit: overlay fallback ({} chars, error: {e})",
                                    display_text.len()
                                );
                                skip_ax_write = true;
                                display_text
                            }
                        }
                    } else {
                        info!(
                            "Inline edit: no SEARCH/REPLACE blocks, using full replacement"
                        );
                        edit_blocks::strip_markdown_fences(&formatted_text)
                    };

                if !skip_ax_write {
                    let text_for_replace = final_text.clone();
                    let inline_ok = match tokio::task::spawn_blocking(move || {
                        crate::os::selection::replace_selected_text(&text_for_replace)
                    })
                    .await
                    {
                        Ok(Ok(method)) => {
                            info!("Inline edit succeeded (method={})", method);
                            // Safety net: some apps (e.g. Taio) report AX write
                            // success but silently ignore it. Copy to clipboard so
                            // user can Cmd+V if the app didn't apply the change.
                            let _ = clipboard::set_clipboard(&final_text);
                            crate::audio::play_sound("Tink");
                            true
                        }
                        Ok(Err(e)) => {
                            warn!("Inline edit failed: {e}. Falling back to overlay + clipboard.");
                            false
                        }
                        Err(e) => {
                            warn!("Inline edit task join error: {e}. Falling back to overlay + clipboard.");
                            false
                        }
                    };

                    if !inline_ok {
                        // Target is read-only (e.g. terminal output): show result in overlay
                        // and copy to clipboard so user can Cmd+V manually.
                        let _ = clipboard::set_clipboard(&final_text);
                        crate::show_voice_chat_overlay();
                        crate::voice_chat_ui::set_voice_chat_text(&final_text);
                        crate::voice_chat_ui::update_voice_chat_status("Copied to clipboard");
                        info!(
                            "Inline edit fallback: overlay shown + clipboard ({} chars)",
                            final_text.len()
                        );
                    }
                }
                // Update formatted_text so transcript save records the actual
                // result that was inserted, not the raw AI response with markers.
                formatted_text = final_text;
            } else if let Err(e) = clipboard::paste_text(&formatted_text) {
                warn!("Auto-paste failed: {e}. Falling back to clipboard copy.");
                let _ = clipboard::set_clipboard(&formatted_text);
            } else {
                info!("Text pasted successfully");
            }
        } else {
            info!("Auto-paste skipped (mode={})", mode_label);
        }

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
        self.set_state(State::Idle).await;
        *self.assistive_mode.write().await = false;
        *self.hold_mode.write().await = HoldMode::Raw;
        *self.force_raw_mode.write().await = false;
        *self.force_ai_mode.write().await = false;
        *self.session_id.write().await = None;
        *self.assistive_context.write().await = None;

        // Hide UI indicators
        hide_hold_badge();

        // Update shared UI status
        crate::voice_chat_ui::update_voice_chat_status("Idle");

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
