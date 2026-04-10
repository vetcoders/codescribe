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
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::{Mutex, RwLock, broadcast};
use tokio::task::JoinHandle;
use tracing::{debug, error, info, warn};
use uuid::Uuid;

use crate::audio::streaming_recorder::StreamingRecorder;
use crate::config::models::ModelManager;
use crate::config::{Config, UserSettings};
use crate::os::clipboard;
use crate::os::hotkeys::HoldMode;
use crate::os::permissions::{
    PermissionStatus, check_accessibility, check_input_monitoring, check_microphone,
};
use crate::os::selection::{
    AssistiveContext, build_assistive_input, capture_assistive_context, capture_frontmost_app_only,
};
use crate::{BadgeMode, hide_hold_badge, show_badge_for_mode};

// Moshi conversation engine and audio output
use codescribe_core::conversation::{ConversationEngine, MoshiConfig};
use codescribe_core::ipc::{IpcEvent, IpcEventPayload};
use codescribe_core::tts::AudioPlayer;

// UI state for conversation mode
use crate::ui::voice_chat::ConversationModeState;

use helpers::{
    SharedSessionTelemetry, new_session_telemetry, raw_save_enabled,
    reset_agent_runtime_for_new_thread as reset_agent_runtime_for_new_thread_impl,
    reset_session_telemetry, send_assistive_with_agent_runtime, setup_voice_chat_send_callback,
    snapshot_session_telemetry,
};
use types::ValidatedAudioPath;

static OVERLAY_CONTROLLER: OnceLock<Arc<RecordingController>> = OnceLock::new();

const LIVE_PROFILE_BUFFER_DELAY_MS: u64 = 280;
const LIVE_PROFILE_TYPING_CPS: f32 = 90.0;
const LIVE_PROFILE_EMIT_WORDS_MAX: u64 = 2;
const LIVE_PROFILE_INTERIM_SEC: f32 = 1.2;
const NO_OVERLAY_PROFILE_INTERIM_SEC: f32 = 8.0;

#[derive(Debug, Clone, Copy)]
struct ActionQualityProbe {
    raw_chars: usize,
    final_chars: usize,
    raw_final_diff_ratio: f32,
    correction_ratio: f32,
    drop_ratio: f32,
}

impl ActionQualityProbe {
    fn from_transcripts(
        raw_text: &str,
        final_text: &str,
        post_stats: &crate::stream_postprocess::StreamPostProcessStats,
    ) -> Self {
        let raw_chars = raw_text.chars().count();
        let final_chars = final_text.chars().count();

        let (backspaces, inserted_chars) =
            codescribe_core::pipeline::contracts::TranscriptDelta::from_diff(raw_text, final_text)
                .map(|delta| {
                    let backspaces = delta
                        .delta
                        .chars()
                        .filter(|c| *c == codescribe_core::pipeline::contracts::BACKSPACE)
                        .count();
                    let inserted = delta.delta.chars().count().saturating_sub(backspaces);
                    (backspaces, inserted)
                })
                .unwrap_or((0, 0));

        let span = raw_chars.max(final_chars).max(1);
        let raw_final_diff_ratio = ((backspaces + inserted_chars) as f32 / span as f32).min(1.0);
        let correction_ratio = (backspaces as f32 / raw_chars.max(1) as f32).min(1.0);
        let drop_ratio = if post_stats.input_chunks == 0 {
            0.0
        } else {
            post_stats.dropped_chunks as f32 / post_stats.input_chunks as f32
        };

        Self {
            raw_chars,
            final_chars,
            raw_final_diff_ratio,
            correction_ratio,
            drop_ratio,
        }
    }
}

fn apply_runtime_transcription_profile(config: &Config, assistive: bool) -> bool {
    let overlay_enabled = config.transcription_overlay_enabled;
    let settings = UserSettings::load();

    let buffer_delay_ms = settings
        .buffer_delay_ms
        .unwrap_or(LIVE_PROFILE_BUFFER_DELAY_MS);
    let typing_cps = settings.typing_cps.unwrap_or(LIVE_PROFILE_TYPING_CPS);
    let emit_words_max = settings
        .emit_words_max
        .unwrap_or(LIVE_PROFILE_EMIT_WORDS_MAX);
    let interim_sec = if !assistive && !overlay_enabled {
        NO_OVERLAY_PROFILE_INTERIM_SEC
    } else {
        settings
            .buffered_interim_sec
            .unwrap_or(LIVE_PROFILE_INTERIM_SEC)
    };

    unsafe {
        std::env::set_var(
            "TRANSCRIPTION_OVERLAY_ENABLED",
            if overlay_enabled { "1" } else { "0" },
        );
        std::env::set_var("CODESCRIBE_BUFFER_DELAY_MS", buffer_delay_ms.to_string());
        std::env::set_var("CODESCRIBE_TYPING_CPS", format!("{typing_cps:.1}"));
        std::env::set_var("CODESCRIBE_EMIT_WORDS_MAX", emit_words_max.to_string());
        std::env::set_var(
            "CODESCRIBE_BUFFERED_INTERIM_SEC",
            format!("{interim_sec:.1}"),
        );
    }

    overlay_enabled
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RecordingTranscriptSource {
    LocalFinalPass,
    CloudPrimary,
    StreamingFallback,
}

fn non_empty_transcript(text: Option<String>) -> Option<String> {
    text.and_then(|text| {
        if text.trim().is_empty() {
            None
        } else {
            Some(text)
        }
    })
}

fn select_recording_transcript(
    use_local_stt: bool,
    local_final_pass_text: Option<String>,
    streaming_text: String,
    cloud_text: Option<String>,
) -> (
    Option<String>,
    Option<String>,
    Option<RecordingTranscriptSource>,
) {
    let local_final_pass_text = non_empty_transcript(local_final_pass_text);
    let streaming_text = non_empty_transcript(Some(streaming_text));
    let cloud_text = non_empty_transcript(cloud_text);

    if use_local_stt {
        if let Some(text) = local_final_pass_text {
            return (
                Some(text),
                cloud_text,
                Some(RecordingTranscriptSource::LocalFinalPass),
            );
        }
    } else if let Some(text) = cloud_text.clone() {
        return (
            Some(text),
            cloud_text,
            Some(RecordingTranscriptSource::CloudPrimary),
        );
    }

    if let Some(text) = streaming_text {
        return (
            Some(text),
            cloud_text,
            Some(RecordingTranscriptSource::StreamingFallback),
        );
    }

    (None, cloud_text, None)
}

const QUALITY_GATE_MIN_CHARS: usize = 24;
const QUALITY_GATE_DROP_RATIO: f32 = 0.35;
const QUALITY_GATE_DIFF_RATIO: f32 = 0.62;
const QUALITY_GATE_CORRECTION_RATIO: f32 = 0.40;
const RECORDER_RUNTIME_DEGRADED_REASON: &str =
    "Microphone recorder unavailable. Voice capture is disabled.";
const RECOVERY_UI_COOLDOWN_MS: u64 = 3_000;
static RUNTIME_RECOVERY_LAST_SHOWN_MS: AtomicU64 = AtomicU64::new(0);

fn should_attempt_recorder_runtime_recovery(
    microphone_status: PermissionStatus,
    recorder_missing: bool,
) -> bool {
    microphone_status == PermissionStatus::Granted && recorder_missing
}

struct AtomicFlagGuard {
    flag: Arc<AtomicBool>,
}

impl AtomicFlagGuard {
    fn new(flag: Arc<AtomicBool>) -> Self {
        flag.store(true, Ordering::SeqCst);
        Self { flag }
    }
}

impl Drop for AtomicFlagGuard {
    fn drop(&mut self) {
        self.flag.store(false, Ordering::SeqCst);
    }
}

#[derive(Debug, Clone, Default)]
struct ProcessRecordingOutcome {
    no_speech_reason: Option<String>,
    commit_trigger: Option<String>,
    transcript_present: bool,
}

impl ProcessRecordingOutcome {
    fn no_speech(reason: impl Into<String>) -> Self {
        Self {
            no_speech_reason: Some(reason.into()),
            commit_trigger: None,
            transcript_present: false,
        }
    }
}

fn should_allow_full_user_bubble_rewrite(
    skip_user_bubble: bool,
    append_mode: bool,
    live_stream_session: bool,
) -> bool {
    !skip_user_bubble && !append_mode && !live_stream_session
}

#[allow(dead_code)]
fn should_allow_full_assistant_rewrite(append_mode: bool, live_stream_session: bool) -> bool {
    !append_mode && !live_stream_session
}

fn should_apply_transcription_action_contract(assistive: bool, live_stream_session: bool) -> bool {
    !assistive && !live_stream_session
}

fn evaluate_quality_commit_trigger(
    force_raw: bool,
    quality_probe: &ActionQualityProbe,
    output_kind: crate::state::history::TranscriptKind,
) -> Option<&'static str> {
    if force_raw {
        return None;
    }
    if output_kind == crate::state::history::TranscriptKind::AiFailed {
        return Some("ai_failed_fallback");
    }
    if quality_probe.raw_chars < QUALITY_GATE_MIN_CHARS
        && quality_probe.final_chars < QUALITY_GATE_MIN_CHARS
    {
        return None;
    }
    if quality_probe.drop_ratio >= QUALITY_GATE_DROP_RATIO {
        return Some("high_drop_ratio");
    }
    if quality_probe.raw_final_diff_ratio >= QUALITY_GATE_DIFF_RATIO {
        return Some("high_rewrite_ratio");
    }
    if quality_probe.correction_ratio >= QUALITY_GATE_CORRECTION_RATIO {
        return Some("high_correction_ratio");
    }
    None
}

fn resolve_transcription_action_contract_mode(
    force_raw: bool,
    force_ai: bool,
    ai_formatting_enabled: bool,
    ai_key_available: bool,
) -> crate::ui::overlay::TranscriptionActionContractMode {
    if force_raw {
        crate::ui::overlay::TranscriptionActionContractMode::Raw
    } else if force_ai || (ai_formatting_enabled && ai_key_available) {
        crate::ui::overlay::TranscriptionActionContractMode::AiFormat
    } else {
        crate::ui::overlay::TranscriptionActionContractMode::Raw
    }
}

/// Register the controller for overlay actions (commit/close fragment).
pub fn register_overlay_controller(controller: Arc<RecordingController>) {
    if OVERLAY_CONTROLLER.set(controller).is_err() {
        warn!("Overlay controller already registered");
    }
}

pub fn request_permission_runtime_reconcile() {
    let Some(controller) = OVERLAY_CONTROLLER.get().cloned() else {
        debug!("Overlay controller not registered; skipping permission runtime reconcile");
        return;
    };

    std::thread::spawn(move || {
        match tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        {
            Ok(runtime) => runtime.block_on(async move {
                controller.reconcile_runtime_after_permission_grant().await;
            }),
            Err(error) => warn!("Failed to build runtime for permission reconcile: {error}"),
        }
    });
}

/// Stop the current recording and force the finish pipeline without waiting for VAD.
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

/// Rotate the backend agent thread/runtime boundary for a fresh chat thread.
pub fn request_new_agent_thread() {
    tokio::spawn(async {
        match reset_agent_runtime_for_new_thread_impl().await {
            Ok(generation) => {
                debug!("UI requested new agent thread boundary (generation={generation})");
            }
            Err(error) => {
                warn!("Failed to rotate agent thread boundary: {error}");
            }
        }
    });
}

/// Rotate runtime + thread identity and return generation once backend reset completes.
pub async fn reset_agent_runtime_for_new_thread() -> Result<u64> {
    reset_agent_runtime_for_new_thread_impl().await
}

/// Recording controller managing state machine and lifecycle
pub struct RecordingController {
    /// Application configuration
    config: Arc<RwLock<Config>>,

    /// Current state
    state: Arc<RwLock<State>>,

    /// Audio recorder instance
    recorder: Arc<Mutex<Option<StreamingRecorder>>>,

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
    /// Whether to force AI formatting for the current session (e.g., left double Option)
    force_ai_mode: Arc<RwLock<bool>>,

    /// Current session ID for tracking
    session_id: Arc<RwLock<Option<String>>>,

    /// Task handle for delayed hold-start (800ms default)
    hold_start_task: Arc<Mutex<Option<JoinHandle<()>>>>,
    /// Monotonic generation for hold-start tasks.
    ///
    /// Every cancel/reschedule bumps this value. Spawned tasks compare their
    /// captured generation before/after critical awaits to avoid stale-start races.
    #[allow(dead_code)]
    hold_start_generation: Arc<AtomicU64>,
    /// Guard flag used to prevent idle-recovery from killing a freshly-starting session.
    start_transition_in_flight: Arc<AtomicBool>,

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
    /// Per-session telemetry from engine events (`NoSpeech`, `Stats`).
    session_telemetry: SharedSessionTelemetry,
}

impl RecordingController {
    fn recorder_unavailable_error(context: &str) -> anyhow::Error {
        warn!("{context}: streaming recorder unavailable; voice capture is disabled");
        crate::ui::voice_chat::set_voice_chat_runtime_degraded(
            true,
            Some(RECORDER_RUNTIME_DEGRADED_REASON),
        );
        anyhow::anyhow!("{context}: streaming recorder unavailable")
    }

    fn init_streaming_recorder(context: &str) -> Option<StreamingRecorder> {
        match StreamingRecorder::new() {
            Ok(recorder) => Some(recorder),
            Err(error) => {
                warn!("{context}: failed to initialize streaming recorder: {error}");
                crate::ui::voice_chat::set_voice_chat_runtime_degraded(
                    true,
                    Some(RECORDER_RUNTIME_DEGRADED_REASON),
                );
                None
            }
        }
    }

    fn recorder_from_guard_mut<'a>(
        recorder_guard: &'a mut Option<StreamingRecorder>,
        context: &str,
    ) -> Result<&'a mut StreamingRecorder> {
        recorder_guard
            .as_mut()
            .ok_or_else(|| Self::recorder_unavailable_error(context))
    }

    fn recorder_from_guard<'a>(
        recorder_guard: &'a Option<StreamingRecorder>,
        context: &str,
    ) -> Result<&'a StreamingRecorder> {
        recorder_guard
            .as_ref()
            .ok_or_else(|| Self::recorder_unavailable_error(context))
    }

    fn should_emit_runtime_recovery_message() -> bool {
        let now_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_millis() as u64)
            .unwrap_or_default();
        loop {
            let last_ms = RUNTIME_RECOVERY_LAST_SHOWN_MS.load(Ordering::SeqCst);
            if now_ms.saturating_sub(last_ms) < RECOVERY_UI_COOLDOWN_MS {
                return false;
            }
            if RUNTIME_RECOVERY_LAST_SHOWN_MS
                .compare_exchange(last_ms, now_ms, Ordering::SeqCst, Ordering::SeqCst)
                .is_ok()
            {
                return true;
            }
        }
    }

    fn format_recorder_recovery_message(
        missing_permissions: &[&str],
        dictation_binding: &str,
        formatting_binding: &str,
        assistive_binding: &str,
    ) -> String {
        if missing_permissions.is_empty() {
            format!(
                "Mic unavailable: recorder failed to initialize. Open Settings to review hotkeys, input device, and runtime services, then retry. Configured shortcuts: Dictation={} • Formatting={} • Assistive={}.",
                dictation_binding, formatting_binding, assistive_binding
            )
        } else {
            format!(
                "Mic unavailable: recorder failed to initialize. Missing permissions: {}. Open Settings to grant access, then retry your hotkey. Configured shortcuts: Dictation={} • Formatting={} • Assistive={}.",
                missing_permissions.join(", "),
                dictation_binding,
                formatting_binding,
                assistive_binding
            )
        }
    }

    fn format_backend_recovery_message(detail: Option<&str>) -> String {
        let mut message =
            "Speech backend unavailable. Open Settings to verify the transcription provider, endpoint, and runtime service, then retry."
                .to_string();
        if let Some(detail) = detail.map(str::trim).filter(|text| !text.is_empty()) {
            message.push_str(" Details: ");
            message.push_str(detail);
        }
        message
    }

    fn recording_recovery_guidance() -> String {
        let settings = crate::config::UserSettings::load();
        let dictation_binding = settings
            .mode_binding_for(crate::config::WorkMode::Dictation)
            .label();
        let formatting_binding = settings
            .mode_binding_for(crate::config::WorkMode::Formatting)
            .label();
        let assistive_binding = settings
            .mode_binding_for(crate::config::WorkMode::Assistive)
            .label();
        let mut missing_permissions = Vec::new();
        if check_accessibility() != PermissionStatus::Granted {
            missing_permissions.push("Accessibility");
        }
        if check_input_monitoring() != PermissionStatus::Granted {
            missing_permissions.push("Input Monitoring");
        }
        if check_microphone() != PermissionStatus::Granted {
            missing_permissions.push("Microphone");
        }

        Self::format_recorder_recovery_message(
            &missing_permissions,
            dictation_binding,
            formatting_binding,
            assistive_binding,
        )
    }

    fn present_runtime_recovery_ui(status: &str, message: &str) {
        let emit_recovery_message = Self::should_emit_runtime_recovery_message();
        let overlay_visible = crate::ui::voice_chat::is_voice_chat_overlay_visible();
        if emit_recovery_message || !overlay_visible {
            crate::ui::voice_chat::show_voice_chat_overlay();
            crate::ui::voice_chat::show_agent_tab();
        }
        crate::ui::voice_chat::update_voice_chat_status(status);
        if emit_recovery_message {
            crate::ui::voice_chat::add_voice_chat_error_message(message);
            crate::ui::settings::show_settings_window();
        } else {
            debug!("Runtime recovery UI throttled (cooldown active)");
        }
    }

    fn present_recorder_unavailable(context: &str) {
        warn!("{context}: recorder unavailable; routing to settings recovery");
        crate::ui::voice_chat::set_voice_chat_runtime_degraded(
            true,
            Some(RECORDER_RUNTIME_DEGRADED_REASON),
        );
        let message = Self::recording_recovery_guidance();
        Self::present_runtime_recovery_ui("Recorder unavailable", &message);
    }

    fn present_backend_unavailable(context: &str, detail: Option<&str>) {
        warn!("{context}: backend unavailable; routing to settings recovery");
        let message = Self::format_backend_recovery_message(detail);
        Self::present_runtime_recovery_ui("Backend unavailable", &message);
    }

    /// Create a new recording controller with configuration loaded from disk
    pub fn new() -> Self {
        let config = Config::load();

        info!(
            "Initializing RecordingController (hold_delay={}ms, beep={}, language={:?})",
            config.hold_start_delay_ms, config.beep_on_start, config.whisper_language
        );

        let recorder = Self::init_streaming_recorder("RecordingController::new");

        if !cfg!(test) {
            match ModelManager::new() {
                Ok(model_manager) => {
                    if let Ok(models) = model_manager.list_models()
                        && !models.is_empty()
                    {
                        info!("Available local models: {:?}", models);
                    }
                }
                Err(error) => warn!("Model manager unavailable during startup: {error}"),
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
        if recorder.is_none() {
            crate::ui::voice_chat::set_voice_chat_runtime_degraded(
                true,
                Some(RECORDER_RUNTIME_DEGRADED_REASON),
            );
        }
        let (event_broadcast, _) = broadcast::channel::<IpcEvent>(256);
        let session_telemetry = new_session_telemetry();

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
            hold_start_generation: Arc::new(AtomicU64::new(0)),
            start_transition_in_flight: Arc::new(AtomicBool::new(false)),
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
            session_telemetry,
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

        let recorder = Self::init_streaming_recorder("RecordingController::with_config");

        if !cfg!(test) {
            match ModelManager::new() {
                Ok(model_manager) => {
                    if let Ok(models) = model_manager.list_models()
                        && !models.is_empty()
                    {
                        info!("Available local models: {:?}", models);
                    }
                }
                Err(error) => warn!("Model manager unavailable during startup: {error}"),
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
        if recorder.is_none() {
            crate::ui::voice_chat::set_voice_chat_runtime_degraded(
                true,
                Some(RECORDER_RUNTIME_DEGRADED_REASON),
            );
        }
        let (event_broadcast, _) = broadcast::channel::<IpcEvent>(256);
        let session_telemetry = new_session_telemetry();

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
            hold_start_generation: Arc::new(AtomicU64::new(0)),
            start_transition_in_flight: Arc::new(AtomicBool::new(false)),
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
            session_telemetry,
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

    async fn reconcile_runtime_after_permission_grant(&self) {
        let mut recorder_guard = self.recorder.lock().await;
        if !should_attempt_recorder_runtime_recovery(check_microphone(), recorder_guard.is_none()) {
            return;
        }

        *recorder_guard = Self::init_streaming_recorder("Permission runtime reconcile");
        if recorder_guard.is_some() {
            crate::ui::voice_chat::set_voice_chat_runtime_degraded(false, None);
            info!("Permission runtime reconcile: recorder runtime recovered after grant");
        }
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
        let generation = self.hold_start_generation.fetch_add(1, Ordering::SeqCst) + 1;
        let mut task_guard = self.hold_start_task.lock().await;
        if let Some(task) = task_guard.take() {
            if task.is_finished() {
                let _ = task.await;
            } else {
                debug!("Invalidated pending hold-start task (generation={generation})");
            }
        }
    }

    fn clear_recorder_callbacks(recorder: &mut StreamingRecorder) {
        recorder.set_utterance_callback(None);
        recorder.set_utterance_silence_sec(None);
        recorder.set_event_sink(None);
    }

    #[allow(dead_code)]
    async fn ensure_recorder_ready_for_start(
        recorder: &mut StreamingRecorder,
        context: &str,
    ) -> Result<()> {
        if recorder.recorder.is_active() {
            warn!("{context}: recorder already active before start; forcing stale-session stop");
            recorder
                .stop_without_saving()
                .await
                .with_context(|| format!("{context}: failed stale-session stop"))?;
            info!("{context}: stale recorder stopped before start");
        }

        Self::clear_recorder_callbacks(recorder);
        Ok(())
    }

    #[allow(dead_code)]
    async fn reset_session_after_start_failure(&self, context: &str) {
        warn!("{context}: resetting controller flags after failed start");
        self.set_state(State::Idle).await;
        *self.assistive_mode.write().await = false;
        *self.hold_mode.write().await = HoldMode::Raw;
        *self.force_raw_mode.write().await = false;
        *self.force_ai_mode.write().await = false;
        *self.session_id.write().await = None;
        *self.assistive_context.write().await = None;
        self.start_transition_in_flight
            .store(false, Ordering::SeqCst);
        self.assistive_loop_active.store(false, Ordering::SeqCst);
        self.toggle_user_has_text.store(false, Ordering::SeqCst);
        self.toggle_assistant_has_text
            .store(false, Ordering::SeqCst);
        set_assistive_session(false);
        reset_session_telemetry(&self.session_telemetry);
        hide_hold_badge();
        crate::ui::voice_chat::update_voice_chat_status("Ready");
    }

    fn is_already_in_progress_error(error: &anyhow::Error) -> bool {
        error
            .to_string()
            .contains("Recording is already in progress")
    }

    async fn recover_stale_recorder_if_idle(&self) {
        if self.start_transition_in_flight.load(Ordering::SeqCst) {
            debug!("RECOVERY decision: skip idle-recovery while start transition is in-flight");
            return;
        }

        let _serial_guard = self.serial_lock.lock().await;

        if self.start_transition_in_flight.load(Ordering::SeqCst) {
            debug!(
                "RECOVERY decision: skip idle-recovery after lock (start transition still active)"
            );
            return;
        }

        if *self.state.read().await != State::Idle {
            return;
        }

        let mut recorder_guard = self.recorder.lock().await;
        let Some(recorder) = recorder_guard.as_mut() else {
            return;
        };
        if !recorder.recorder.is_active() {
            return;
        }

        warn!("Recorder recovery: detected active stream while controller is IDLE; forcing stop");
        if let Err(e) = recorder.stop_without_saving().await {
            warn!("Recorder recovery: forced stop failed: {e}");
        }
        Self::clear_recorder_callbacks(recorder);
        drop(recorder_guard);

        *self.assistive_mode.write().await = false;
        *self.hold_mode.write().await = HoldMode::Raw;
        *self.force_raw_mode.write().await = false;
        *self.force_ai_mode.write().await = false;
        *self.assistive_context.write().await = None;
        *self.session_id.write().await = None;
        self.assistive_loop_active.store(false, Ordering::SeqCst);
        self.toggle_user_has_text.store(false, Ordering::SeqCst);
        self.toggle_assistant_has_text
            .store(false, Ordering::SeqCst);
        set_assistive_session(false);
        reset_session_telemetry(&self.session_telemetry);
        hide_hold_badge();
        crate::ui::voice_chat::update_voice_chat_status("Ready");
        info!("RECOVERY decision: stale active stream cleared, controller remains IDLE");
    }

    fn configure_hold_event_sink(
        recorder: &mut StreamingRecorder,
        preview_deltas_enabled: bool,
        event_broadcast: broadcast::Sender<IpcEvent>,
        session_telemetry: SharedSessionTelemetry,
    ) {
        let tb = recorder.transcript_buffer_handle();
        let delta_sink = preview_deltas_enabled.then(|| {
            Arc::new(helpers::RoutingDeltaSink)
                as Arc<dyn codescribe_core::pipeline::contracts::DeltaSink>
        });
        let pe: Arc<dyn codescribe_core::pipeline::contracts::EventSink> =
            Arc::new(PresentationEmitter::new(tb, delta_sink, None));
        let ipc_sink: Arc<dyn codescribe_core::pipeline::contracts::EventSink> =
            Arc::new(helpers::IpcBroadcastSink::new(event_broadcast));
        let telemetry_sink: Arc<dyn codescribe_core::pipeline::contracts::EventSink> =
            Arc::new(helpers::SessionTelemetrySink::new(session_telemetry));
        recorder.set_event_sink(Some(Arc::new(
            codescribe_core::pipeline::sinks::FanoutEventSink::new(vec![
                pe,
                ipc_sink,
                telemetry_sink,
            ]),
        )));
    }

    fn configure_toggle_event_sink(
        recorder: &mut StreamingRecorder,
        preview_deltas_enabled: bool,
        controller: Option<Arc<RecordingController>>,
        expected_session: String,
        is_assistive_session: bool,
        event_broadcast: broadcast::Sender<IpcEvent>,
        session_telemetry: SharedSessionTelemetry,
    ) {
        let tb = recorder.transcript_buffer_handle();
        let delta_sink = preview_deltas_enabled.then(|| {
            Arc::new(helpers::RoutingDeltaSink)
                as Arc<dyn codescribe_core::pipeline::contracts::DeltaSink>
        });
        let mut pe = PresentationEmitter::new(tb, delta_sink, None);

        pe.set_utterance_callback(Some(Arc::new(move |text: String| {
            if is_assistive_session {
                // Close current streaming user bubble at utterance boundary
                // so next preview starts a fresh user message.
                crate::ui::voice_chat::finalize_voice_chat_user_message();
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
        let telemetry_sink: Arc<dyn codescribe_core::pipeline::contracts::EventSink> =
            Arc::new(helpers::SessionTelemetrySink::new(session_telemetry));
        recorder.set_event_sink(Some(Arc::new(
            codescribe_core::pipeline::sinks::FanoutEventSink::new(vec![
                pe,
                ipc_sink,
                telemetry_sink,
            ]),
        )));
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
        let mut current_state = self.current_state().await;

        if current_state == State::Idle {
            self.recover_stale_recorder_if_idle().await;
            current_state = self.current_state().await;
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
                    *self.hold_mode.write().await = event.hold_mode;
                    match event.hold_mode {
                        HoldMode::Raw => {
                            // If we're already in an assistive session (Chat/Selection) and the user
                            // releases Shift/Cmd while still holding Ctrl, the event tap will emit a
                            // HoldUpdate back to Raw. We *do not* want to flip the UI back to the
                            // transcription overlay mid-session (it looks like the chat "blinks"
                            // and then disappears).
                            //
                            // We treat assistive mode as "latched" for the duration of a recording.
                            if matches!(current_state, State::RecHold | State::RecToggle)
                                && *self.assistive_mode.read().await
                            {
                                debug!("Ignoring Raw hold-mode update during assistive session");
                                return Ok(());
                            }

                            *self.assistive_mode.write().await = false;
                            *self.assistive_context.write().await = None;
                            *self.force_raw_mode.write().await = !event.force_ai;
                            *self.force_ai_mode.write().await = event.force_ai;

                            if matches!(current_state, State::RecHold | State::RecToggle) {
                                let overlay_enabled =
                                    self.config.read().await.transcription_overlay_enabled;
                                set_assistive_session(false);
                                self.opened_voice_chat_overlay_for_transcription
                                    .store(false, Ordering::SeqCst);
                                crate::ui::overlay::clear_transcription_text();
                                if overlay_enabled {
                                    crate::ui::overlay::show_transcription_overlay();
                                    crate::ui::overlay::enter_recording_mode();
                                } else {
                                    crate::ui::overlay::hide_transcription_overlay();
                                }
                            }
                        }
                        HoldMode::Chat => {
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
                                crate::ui::voice_chat::set_voice_chat_target_app(
                                    self.assistive_context
                                        .read()
                                        .await
                                        .clone()
                                        .unwrap_or_default()
                                        .frontmost_app,
                                );
                                set_assistive_session(true);
                                crate::ui::overlay::hide_transcription_overlay();
                                crate::ui::voice_chat::show_voice_chat_overlay();
                                crate::ui::voice_chat::show_agent_tab();
                                crate::ui::voice_chat::update_voice_chat_status("Listening...");
                            }
                        }
                        HoldMode::Selection => {
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
                                crate::ui::voice_chat::set_voice_chat_target_app(
                                    self.assistive_context
                                        .read()
                                        .await
                                        .clone()
                                        .unwrap_or_default()
                                        .frontmost_app,
                                );
                                set_assistive_session(true);
                                crate::ui::overlay::hide_transcription_overlay();
                                crate::ui::voice_chat::show_voice_chat_overlay();
                                crate::ui::voice_chat::show_agent_tab();
                                crate::ui::voice_chat::update_voice_chat_status("Listening...");
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

        {
            let recorder_guard = self.recorder.lock().await;
            if recorder_guard.is_none() {
                let error = Self::recorder_unavailable_error("Conversation-start");
                Self::present_recorder_unavailable("Conversation-start");
                return Err(error);
            }
        }

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
                            crate::ui::voice_chat::add_voice_chat_error_message(&format!(
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
                        crate::ui::voice_chat::add_voice_chat_error_message(&format!(
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
        crate::ui::voice_chat::show_voice_chat_overlay();
        crate::ui::voice_chat::show_agent_tab();
        crate::ui::voice_chat::update_voice_chat_status("Listening...");
        crate::ui::voice_chat::update_conversation_state(ConversationModeState::Listening);

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
        recorder: Arc<Mutex<Option<StreamingRecorder>>>,
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
            let mut rec_guard = recorder.lock().await;
            let rec = match Self::recorder_from_guard_mut(&mut rec_guard, "Conversation-loop start")
            {
                Ok(rec) => rec,
                Err(error) => {
                    error!("Conversation mode unavailable: {error}");
                    drop(rec_guard);
                    // Full cleanup on failure: state, session flag, badge, UI
                    Self::set_state_with_broadcast(&state, &event_broadcast, State::Idle).await;
                    helpers::set_conversation_session(false);
                    hide_hold_badge();
                    crate::ui::voice_chat::update_conversation_state(
                        ConversationModeState::Inactive,
                    );
                    Self::present_recorder_unavailable("Conversation-loop start");
                    return;
                }
            };
            rec.recorder.set_callback(Box::new(move |data: &[f32]| {
                let _ = tx_clone.try_send(data.to_vec());
            }));

            if let Err(e) = rec.recorder.start().await {
                error!("Failed to start recorder for conversation: {}", e);
                // Full cleanup on failure: state, session flag, badge, UI
                Self::set_state_with_broadcast(&state, &event_broadcast, State::Idle).await;
                helpers::set_conversation_session(false);
                hide_hold_badge();
                crate::ui::voice_chat::update_voice_chat_status("Recorder error");
                crate::ui::voice_chat::update_conversation_state(ConversationModeState::Inactive);
                crate::ui::voice_chat::add_voice_chat_error_message(&format!("Mic error: {}", e));
                return;
            }
        }

        // Get actual sample rate from recorder
        let sample_rate = {
            let rec_guard = recorder.lock().await;
            let rec = match Self::recorder_from_guard(&rec_guard, "Conversation-loop sample rate") {
                Ok(rec) => rec,
                Err(error) => {
                    error!("Conversation mode aborted: {error}");
                    drop(rec_guard);
                    Self::set_state_with_broadcast(&state, &event_broadcast, State::Idle).await;
                    helpers::set_conversation_session(false);
                    hide_hold_badge();
                    crate::ui::voice_chat::update_conversation_state(
                        ConversationModeState::Inactive,
                    );
                    Self::present_recorder_unavailable("Conversation-loop sample rate");
                    return;
                }
            };
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
                            crate::ui::voice_chat::update_voice_chat_status(status);
                            crate::ui::voice_chat::update_conversation_state(ui_state);
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

                    crate::ui::voice_chat::update_voice_chat_status("Moshi speaking...");
                    crate::ui::voice_chat::update_conversation_state(
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
                                    crate::ui::voice_chat::update_voice_chat_status("Listening...");
                                    crate::ui::voice_chat::update_conversation_state(
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
            let mut rec_guard = recorder.lock().await;
            if let Some(rec) = rec_guard.as_mut() {
                let _ = rec.recorder.stop().await;
            }
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
            crate::ui::voice_chat::update_voice_chat_status("Conversation ended");
            crate::ui::voice_chat::update_conversation_state(ConversationModeState::Inactive);
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
            let mut rec_guard = self.recorder.lock().await;
            if let Some(rec) = rec_guard.as_mut() {
                let _ = rec.recorder.stop().await;
                info!("Recorder stopped in stop_conversation_mode");
            } else {
                warn!("stop_conversation_mode: recorder unavailable during stop");
            }
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
        crate::ui::voice_chat::update_voice_chat_status("Conversation ended");
        crate::ui::voice_chat::update_conversation_state(ConversationModeState::Inactive);

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
                    Self::present_backend_unavailable("Hold-start health check", None);
                    return Ok(());
                }
                Err(e) => {
                    error!("Whisper engine unavailable: {}", e);
                    let detail = e.to_string();
                    Self::present_backend_unavailable(
                        "Hold-start health check",
                        Some(detail.as_str()),
                    );
                    return Ok(());
                }
            }
        }

        let config = self.config.read().await.clone();
        let delay_ms = config.hold_start_delay_ms;
        let beep = config.beep_on_start;
        let sound_volume = config.sound_volume;
        let language = config.whisper_language;

        let hold_mode = Arc::clone(&self.hold_mode);

        debug!(
            "Scheduling hold-start after {}ms delay (hold_mode={:?})",
            delay_ms,
            *hold_mode.read().await
        );

        // Cancel any existing delayed start
        self.cancel_pending_hold_start().await;
        let task_generation = self.hold_start_generation.load(Ordering::SeqCst);

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
        let hold_start_generation = Arc::clone(&self.hold_start_generation);
        let start_transition_in_flight = Arc::clone(&self.start_transition_in_flight);
        let session_telemetry = Arc::clone(&self.session_telemetry);
        let opened_overlay_for_transcription =
            Arc::clone(&self.opened_voice_chat_overlay_for_transcription);

        let task = tokio::spawn(async move {
            // Wait for the configured delay
            tokio::time::sleep(delay).await;

            if hold_start_generation.load(Ordering::SeqCst) != task_generation {
                debug!("Hold-start cancelled: superseded generation before lock");
                return;
            }

            // Serialize with other start/stop operations.
            let _serial_guard = serial_lock.lock().await;

            if hold_start_generation.load(Ordering::SeqCst) != task_generation {
                debug!("Hold-start cancelled: superseded generation while waiting for lock");
                return;
            }

            // Check if we're still in IDLE state
            let current_state = *state.read().await;
            if current_state != State::Idle {
                debug!("Hold-start cancelled: state changed to {}", current_state);
                return;
            }
            let _start_guard = AtomicFlagGuard::new(Arc::clone(&start_transition_in_flight));

            // Generate session ID
            let new_session_id = Uuid::new_v4().to_string();
            *session_id.write().await = Some(new_session_id.clone());

            info!("Starting hold recording (session={})", new_session_id);

            let hold_mode = *hold_mode.read().await;
            let is_assistive = matches!(hold_mode, HoldMode::Chat | HoldMode::Selection);
            let overlay_enabled = apply_runtime_transcription_profile(&config, is_assistive);

            // Start the recorder (skip in tests: no CoreAudio device needed)
            // hang_sec is derived from hardcoded VAD defaults (single source of truth).
            let mut rec_guard = recorder.lock().await;
            let rec = match Self::recorder_from_guard_mut(&mut rec_guard, "Hold-start") {
                Ok(rec) => rec,
                Err(error) => {
                    error!("Hold-start aborted: {error}");
                    drop(rec_guard);
                    *session_id.write().await = None;
                    set_assistive_session(false);
                    Self::present_recorder_unavailable("Hold-start");
                    return;
                }
            };
            if let Err(e) = Self::ensure_recorder_ready_for_start(rec, "Hold-start preflight").await
            {
                error!("Hold-start aborted: {e}");
                drop(rec_guard);
                *session_id.write().await = None;
                set_assistive_session(false);
                return;
            }
            // Hold-to-talk: the key-down is the source of truth. Don't auto-stop mid-hold.
            rec.recorder.config.auto_silence = false;
            rec.recorder.set_on_vad_stop(move || {
                info!("VAD callback: setting vad_triggered flag");
                vad_flag.store(true, Ordering::SeqCst);
            });

            // Set session mode for delta routing BEFORE starting the pipeline,
            // so the very first deltas route to the correct overlay.
            set_assistive_session(is_assistive);
            reset_session_telemetry(&session_telemetry);

            // Runtime pipeline is always event-based. Hold mode has no utterance callback;
            // text is finalized on key-up in `finish_recording`.
            Self::configure_hold_event_sink(
                rec,
                is_assistive || overlay_enabled,
                event_broadcast.clone(),
                Arc::clone(&session_telemetry),
            );
            if !cfg!(test) {
                let start_result = rec
                    .start_event_session(Some(language.as_str().to_string()))
                    .await;
                if let Err(e) = start_result {
                    if Self::is_already_in_progress_error(&e) {
                        warn!("Hold-start hit stale recorder lock; forcing stop and retrying once");
                        if let Err(stop_err) = rec.stop_without_saving().await {
                            warn!("Hold-start stale-recorder recovery failed: {stop_err}");
                        }
                        Self::clear_recorder_callbacks(rec);
                        Self::configure_hold_event_sink(
                            rec,
                            is_assistive || overlay_enabled,
                            event_broadcast.clone(),
                            Arc::clone(&session_telemetry),
                        );
                        let retry_result = rec
                            .start_event_session(Some(language.as_str().to_string()))
                            .await;
                        if let Err(retry_err) = retry_result {
                            error!("Failed to start recorder after recovery: {retry_err}");
                            Self::clear_recorder_callbacks(rec);
                            *session_id.write().await = None;
                            set_assistive_session(false);
                            return;
                        }
                    } else {
                        error!("Failed to start recorder: {e}");
                        Self::clear_recorder_callbacks(rec);
                        *session_id.write().await = None;
                        set_assistive_session(false);
                        return;
                    }
                }
            }

            if hold_start_generation.load(Ordering::SeqCst) != task_generation {
                warn!("Hold-start superseded after recorder start; stopping stale session");
                if rec.recorder.is_active()
                    && let Err(stop_err) = rec.stop_without_saving().await
                {
                    warn!("Hold-start stale-session stop failed: {stop_err}");
                }
                Self::clear_recorder_callbacks(rec);
                *session_id.write().await = None;
                set_assistive_session(false);
                return;
            }
            drop(rec_guard);

            // Transition to REC_HOLD as soon as recorder starts to avoid IDLE/active races.
            Self::set_state_with_broadcast(&state, &event_broadcast, State::RecHold).await;
            info!(
                "STATE TRANSITION: IDLE → REC_HOLD (assistive={})",
                is_assistive
            );

            // Play start beep if enabled
            if beep {
                crate::audio::play_sound_with_volume("Tink", sound_volume);
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
                // Capture context BEFORE showing any overlay (overlays can steal focus).
                let ctx = match hold_mode {
                    HoldMode::Selection => tokio::task::spawn_blocking(capture_assistive_context)
                        .await
                        .unwrap_or_default(),
                    HoldMode::Chat => tokio::task::spawn_blocking(capture_frontmost_app_only)
                        .await
                        .unwrap_or_default(),
                    HoldMode::Raw => tokio::task::spawn_blocking(capture_frontmost_app_only)
                        .await
                        .unwrap_or_default(),
                };
                *assistive_context.write().await = Some(ctx);
                crate::ui::voice_chat::set_voice_chat_target_app(
                    assistive_context
                        .read()
                        .await
                        .clone()
                        .unwrap_or_default()
                        .frontmost_app,
                );

                crate::ui::overlay::hide_transcription_overlay();
                crate::ui::voice_chat::show_voice_chat_overlay();
                crate::ui::voice_chat::show_agent_tab();
                crate::ui::voice_chat::update_voice_chat_status("Listening...");
            } else {
                // Capture frontmost app for paste actions (no selection/clipboard).
                let ctx = tokio::task::spawn_blocking(capture_frontmost_app_only)
                    .await
                    .unwrap_or_default();
                *assistive_context.write().await = Some(ctx);
                crate::ui::voice_chat::set_voice_chat_target_app(
                    assistive_context
                        .read()
                        .await
                        .clone()
                        .unwrap_or_default()
                        .frontmost_app,
                );
                opened_overlay_for_transcription.store(false, Ordering::SeqCst);
                crate::ui::overlay::clear_transcription_text();
                if overlay_enabled {
                    crate::ui::overlay::show_transcription_overlay();
                    crate::ui::overlay::enter_recording_mode();
                } else {
                    crate::ui::overlay::hide_transcription_overlay();
                }
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
                    Self::present_backend_unavailable("Toggle-start health check", None);
                    return Ok(());
                }
                Err(e) => {
                    error!("Whisper engine unavailable: {}", e);
                    let detail = e.to_string();
                    Self::present_backend_unavailable(
                        "Toggle-start health check",
                        Some(detail.as_str()),
                    );
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
        let _start_guard = AtomicFlagGuard::new(Arc::clone(&self.start_transition_in_flight));

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
        let sound_volume = config.sound_volume;
        let overlay_enabled = apply_runtime_transcription_profile(&config, is_assistive);

        // Start the recorder
        let mut recorder_guard = self.recorder.lock().await;
        let recorder = match Self::recorder_from_guard_mut(&mut recorder_guard, "Toggle-start") {
            Ok(recorder) => recorder,
            Err(error) => {
                drop(recorder_guard);
                self.reset_session_after_start_failure("Toggle-start").await;
                Self::present_recorder_unavailable("Toggle-start");
                return Err(error);
            }
        };
        if let Err(e) =
            Self::ensure_recorder_ready_for_start(recorder, "Toggle-start preflight").await
        {
            drop(recorder_guard);
            self.reset_session_after_start_failure("Toggle-start preflight")
                .await;
            return Err(e);
        }

        // Toggle mode: continuous recording; silence only triggers per-utterance send.
        recorder.recorder.config.auto_silence = false;
        recorder.recorder.set_on_vad_stop(|| {});
        recorder.set_utterance_silence_sec(Some(toggle_silence_sec));

        // Set session mode for delta routing BEFORE starting the pipeline,
        // so the very first deltas route to the correct overlay.
        set_assistive_session(is_assistive);
        reset_session_telemetry(&self.session_telemetry);

        // Runtime pipeline is always event-based.
        Self::configure_toggle_event_sink(
            recorder,
            is_assistive || overlay_enabled,
            OVERLAY_CONTROLLER.get().cloned(),
            new_session_id.clone(),
            is_assistive,
            self.event_broadcast.clone(),
            Arc::clone(&self.session_telemetry),
        );

        // Skip actual audio stream in tests (no CoreAudio device needed)
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
                Self::clear_recorder_callbacks(recorder);
                Self::configure_toggle_event_sink(
                    recorder,
                    is_assistive || overlay_enabled,
                    OVERLAY_CONTROLLER.get().cloned(),
                    new_session_id.clone(),
                    is_assistive,
                    self.event_broadcast.clone(),
                    Arc::clone(&self.session_telemetry),
                );
                if let Err(retry_err) = recorder
                    .start_event_session(Some(language.as_str().to_string()))
                    .await
                {
                    drop(recorder_guard);
                    self.reset_session_after_start_failure("Toggle-start retry")
                        .await;
                    return Err(anyhow::anyhow!(
                        "Failed to start event session after recovery: {retry_err}"
                    ));
                }
            } else {
                drop(recorder_guard);
                self.reset_session_after_start_failure("Toggle-start").await;
                return Err(e);
            }
        }
        drop(recorder_guard);

        // Transition to REC_TOGGLE immediately after recorder starts.
        self.set_state(State::RecToggle).await;
        info!("STATE TRANSITION: IDLE → REC_TOGGLE (pulsing badge)");

        // Play start beep if enabled
        if beep_enabled {
            crate::audio::play_sound_with_volume("Tink", sound_volume);
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
            crate::ui::voice_chat::set_voice_chat_target_app(
                self.assistive_context
                    .read()
                    .await
                    .clone()
                    .unwrap_or_default()
                    .frontmost_app,
            );

            crate::ui::overlay::hide_transcription_overlay();
            crate::ui::voice_chat::show_voice_chat_overlay();
            crate::ui::voice_chat::show_agent_tab();
            crate::ui::voice_chat::update_voice_chat_status("Listening...");
        } else {
            // Capture frontmost app for paste actions (no selection/clipboard).
            let ctx = tokio::task::spawn_blocking(capture_frontmost_app_only)
                .await
                .unwrap_or_default();
            *self.assistive_context.write().await = Some(ctx);
            crate::ui::voice_chat::set_voice_chat_target_app(
                self.assistive_context
                    .read()
                    .await
                    .clone()
                    .unwrap_or_default()
                    .frontmost_app,
            );
            self.opened_voice_chat_overlay_for_transcription
                .store(false, Ordering::SeqCst);
            crate::ui::overlay::clear_transcription_text();
            if overlay_enabled {
                crate::ui::overlay::show_transcription_overlay();
                crate::ui::overlay::enter_recording_mode();
            } else {
                crate::ui::overlay::hide_transcription_overlay();
            }
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
                crate::ui::voice_chat::set_voice_chat_sending(false);
                crate::ui::voice_chat::update_voice_chat_status("Listening...");
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

        crate::ui::voice_chat::set_voice_chat_target_app(
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
                raw_save_enabled: raw_save_enabled(is_assistive),
                audio_path: None,
                cloud_text_opt: None,
                cloud_handle: None,
                append_mode: false,
                live_stream_session: true,
                user_needs_separator,
                assistant_needs_separator,
                skip_user_bubble,
            })
            .await
            .map(|_| ());

        if *self.state.read().await == State::RecToggle && is_assistive {
            crate::ui::voice_chat::set_voice_chat_sending(false);
            crate::ui::voice_chat::update_voice_chat_status("Listening...");
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
        let mut recorder_guard = self.recorder.lock().await;
        let mut stop_error: Option<anyhow::Error> = None;
        if let Some(recorder) = recorder_guard.as_mut() {
            if !cfg!(test)
                && let Err(e) = recorder.stop_without_saving().await
            {
                warn!("Toggle stop: recorder stop failed; continuing cleanup: {e}");
                stop_error = Some(e);
            }
            Self::clear_recorder_callbacks(recorder);
        } else {
            let error = Self::recorder_unavailable_error("Toggle-stop");
            warn!("Toggle stop: {error}; continuing cleanup");
            stop_error = Some(error);
        }
        drop(recorder_guard);

        // Reset state
        self.set_state(State::Idle).await;
        *self.assistive_mode.write().await = false;
        *self.hold_mode.write().await = HoldMode::Raw;
        *self.force_raw_mode.write().await = false;
        *self.force_ai_mode.write().await = false;
        *self.session_id.write().await = None;
        self.start_transition_in_flight
            .store(false, Ordering::SeqCst);
        self.assistive_loop_active.store(false, Ordering::SeqCst);
        if self.toggle_user_has_text.load(Ordering::SeqCst) {
            crate::ui::voice_chat::finalize_voice_chat_user_message();
        }
        if self.toggle_assistant_has_text.load(Ordering::SeqCst) {
            crate::ui::voice_chat::finalize_voice_chat_assistant_message();
        }
        self.toggle_user_has_text.store(false, Ordering::SeqCst);
        self.toggle_assistant_has_text
            .store(false, Ordering::SeqCst);
        set_assistive_session(false);

        hide_hold_badge();
        crate::ui::voice_chat::update_voice_chat_status("Ready");

        if let Some(e) = stop_error {
            return Err(anyhow::anyhow!("Failed to stop recorder: {e}"));
        }

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
        self.start_transition_in_flight
            .store(false, Ordering::SeqCst);
        self.assistive_loop_active.store(false, Ordering::SeqCst);
        // Keep event-router sink selection in sync with controller state after finish.
        set_assistive_session(false);

        // Hide red dot indicator
        hide_hold_badge();

        // Update tray icon based on result
        match &result {
            Ok(outcome) => {
                crate::ui::voice_chat::update_voice_chat_status("Ready");
                info!("Processing finished successfully. State reset to IDLE.");

                if let Some(reason) = outcome.no_speech_reason.as_deref() {
                    info!("NoSpeech outcome in finish_recording: reason={reason}");
                    if !assistive {
                        let opened = self
                            .opened_voice_chat_overlay_for_transcription
                            .swap(false, Ordering::SeqCst);
                        if opened {
                            crate::ui::voice_chat::hide_voice_chat_overlay();
                        }
                        crate::ui::overlay::hide_transcription_overlay();
                    }
                } else if !assistive {
                    let cfg = self.config.read().await.clone();
                    let show_decision_overlay = outcome.transcript_present
                        && cfg.transcription_overlay_enabled
                        && !(cfg.quick_notes_enabled && cfg.quick_notes_save_only);

                    let opened = self
                        .opened_voice_chat_overlay_for_transcription
                        .swap(false, Ordering::SeqCst);
                    if opened {
                        crate::ui::voice_chat::hide_voice_chat_overlay();
                    }

                    if show_decision_overlay {
                        let reason = outcome
                            .commit_trigger
                            .as_deref()
                            .unwrap_or("quality_gate_clean");
                        info!(
                            "COMMIT decision: trigger={reason} force_ai={force_ai} force_raw={force_raw}"
                        );
                        crate::ui::overlay::enter_decision_mode();
                        crate::ui::overlay::schedule_auto_hide();
                    } else if cfg.quick_notes_enabled && cfg.quick_notes_save_only {
                        info!("COMMIT decision: skipped (quick_notes_save_only)");
                        crate::ui::overlay::hide_transcription_overlay();
                    } else {
                        info!("COMMIT decision: skipped (quality gate clean)");
                        crate::ui::overlay::hide_transcription_overlay();
                    }
                }
            }
            Err(e) => {
                error!("Processing failed: {}", e);
                crate::ui::voice_chat::update_voice_chat_status("Processing failed");

                // Hide overlay immediately on error
                let opened = self
                    .opened_voice_chat_overlay_for_transcription
                    .swap(false, Ordering::SeqCst);
                if opened {
                    crate::ui::voice_chat::hide_voice_chat_overlay();
                }
                crate::ui::overlay::hide_transcription_overlay();
            }
        }

        result.map(|_| ())
    }

    /// Process the recording: stop, transcribe, format, paste
    ///
    /// ## Mode Logic:
    /// - `assistive=true`: ALWAYS AI augmentation (HoldMode::Chat / HoldMode::Selection)
    /// - `force_raw=true`: ALWAYS raw transcript (HoldMode::Raw)
    /// - `force_ai=true`: ALWAYS AI formatting (left double Option)
    /// - Neither: Toggle mode - respects AI_FORMATTING_ENABLED setting
    async fn process_recording(
        &self,
        _session_id: Option<String>,
        assistive: bool,
        hold_mode: HoldMode,
        force_raw: bool,
        force_ai: bool,
    ) -> Result<ProcessRecordingOutcome> {
        if cfg!(test) {
            info!(
                "process_recording: skipped in tests (assistive={}, hold_mode={:?}, force_raw={}, force_ai={})",
                assistive, hold_mode, force_raw, force_ai
            );
            return Ok(ProcessRecordingOutcome::default());
        }

        // Stop the recorder and get audio file path
        let mut recorder_guard = self.recorder.lock().await;
        let recorder = Self::recorder_from_guard_mut(&mut recorder_guard, "Process-recording")?;
        let (streaming_text, raw_audio_path_opt) =
            recorder.stop().await.context("Failed to stop recorder")?;
        drop(recorder_guard); // Release lock

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
        let raw_save_enabled = raw_save_enabled(assistive);

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

        let mut local_final_pass_text = None;
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
        // Streaming is the source of truth for live UX and final output by default.
        // Enable this only when explicitly requested for diagnostics/experiments.
        //
        // Default: disabled (set CODESCRIBE_LOCAL_STT_FINAL_PASS=1 to enable).
        let local_final_pass_enabled = std::env::var("CODESCRIBE_LOCAL_STT_FINAL_PASS")
            .ok()
            .map(|v| matches!(v.to_lowercase().as_str(), "1" | "true" | "yes" | "on"))
            .unwrap_or(false);

        if use_local_stt && local_final_pass_enabled {
            if let Some(path) = &audio_path {
                let wav_path = path.as_path().to_path_buf();
                let lang = language_opt.map(str::to_string);

                if chat_active {
                    crate::ui::voice_chat::update_voice_chat_status("Finalizing… (20%)");
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
                        local_final_pass_text = Some(text);
                    }
                    Ok(Ok(_)) => warn!("Final-pass transcription returned empty text"),
                    Ok(Err(e)) => warn!("Final-pass transcription failed: {}", e),
                    Err(e) => warn!("Final-pass transcription task failed: {}", e),
                }
            } else {
                warn!("Final-pass local STT skipped: no audio file available");
            }
        }

        if !use_local_stt {
            if let Some(handle) = cloud_handle.take() {
                info!("Awaiting cloud STT as selected transcript backend");
                match handle.await {
                    Ok(Ok(text)) => cloud_text_opt = Some(text),
                    Ok(Err(e)) => error!("Cloud transcription failed: {}", e),
                    Err(e) => error!("Cloud transcription task failed: {}", e),
                }
            } else {
                warn!("Cloud backend unavailable (cloud disabled or missing credentials)");
            }
        }

        let (raw_text_opt, cloud_text_opt, transcript_source) = select_recording_transcript(
            use_local_stt,
            local_final_pass_text,
            streaming_text,
            cloud_text_opt,
        );
        match transcript_source {
            Some(RecordingTranscriptSource::LocalFinalPass) => {
                if let Some(text) = raw_text_opt.as_ref() {
                    info!(
                        "Using final-pass local transcription result ({} chars)",
                        text.len()
                    );
                }
            }
            Some(RecordingTranscriptSource::CloudPrimary) => {
                if let Some(text) = raw_text_opt.as_ref() {
                    info!(
                        "Using cloud transcription result as selected backend ({} chars)",
                        text.len()
                    );
                }
            }
            Some(RecordingTranscriptSource::StreamingFallback) => {
                if !use_local_stt {
                    warn!("Cloud backend unavailable; using streaming transcript fallback");
                }
                if let Some(text) = raw_text_opt.as_ref() {
                    info!(
                        "Using streaming transcription result ({} chars)",
                        text.len()
                    );
                }
            }
            None => {
                if use_local_stt {
                    warn!("Streaming returned empty text");
                }
            }
        }
        let session_telemetry = snapshot_session_telemetry(&self.session_telemetry);

        let raw_text = match raw_text_opt {
            Some(text) if !text.trim().is_empty() => text,
            Some(_) | None => {
                let reason = session_telemetry
                    .no_speech_reason
                    .clone()
                    .unwrap_or_else(|| "empty_transcript_without_no_speech_event".to_string());
                if let Some(stats) = session_telemetry.stats.as_ref() {
                    info!(
                        "NoSpeech outcome: reason={} utterances={} hallu_drops={} semantic_drops={} filtered_empty={} corrections={} dropped_chunks={} partial_runs={} partial_trigger_utt={} partial_trigger_speech={} partial_trigger_watchdog={} partial_stale={} partial_coalesced={} partial_dropped={}",
                        reason,
                        stats.total_utterances,
                        stats.hallucination_drops,
                        stats.semantic_gate_drops,
                        stats.filtered_empty_drops,
                        stats.corrections_applied,
                        stats.dropped_audio_chunks,
                        stats.partial_runs_total,
                        stats.trigger_utterance_count,
                        stats.trigger_speech_count,
                        stats.trigger_watchdog_count,
                        stats.partial_stale_count,
                        stats.partial_coalesced_count,
                        stats.partial_dropped_count
                    );
                } else {
                    info!("NoSpeech outcome: reason={} stats=unavailable", reason);
                }
                if assistive_loop {
                    if chat_active {
                        crate::ui::voice_chat::set_voice_chat_sending(false);
                        crate::ui::voice_chat::update_voice_chat_status("Listening...");
                    }
                    warn!("NoSpeech in assistive loop; continuing hands-off listening");
                }
                return Ok(ProcessRecordingOutcome::no_speech(reason));
            }
        };

        info!("Raw transcript captured ({} chars)", raw_text.len());
        let transcript_present = !raw_text.trim().is_empty();

        let language_opt = Some(language.as_str().to_string());
        let pipeline_outcome = self
            .process_transcript_text_pipeline(types::TranscriptPipelineParams {
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
                live_stream_session: false,
                user_needs_separator: false,
                assistant_needs_separator: false,
                skip_user_bubble: false,
            })
            .await?;

        Ok(ProcessRecordingOutcome {
            no_speech_reason: None,
            commit_trigger: pipeline_outcome.commit_trigger,
            transcript_present,
        })
    }

    async fn process_transcript_text_pipeline(
        &self,
        p: types::TranscriptPipelineParams,
    ) -> Result<types::TranscriptProcessOutcome> {
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
            live_stream_session,
            user_needs_separator,
            assistant_needs_separator: _assistant_needs_separator,
            skip_user_bubble,
        } = p;
        let language_opt = language_opt.as_deref();

        // ALWAYS-ON: Final post-processing pass (lexicon + cleanup + semantic gate)
        // This ensures ALL output paths receive clean text regardless of mode.
        // Contract: every chunk/transcript passes through StreamPostProcessor before
        // reaching overlay, clipboard, augmentation, or dataset.
        let (clean_text, postprocess_stats) = {
            let mut finalizer = StreamPostProcessor::new();
            let clean_text = finalizer
                .process(&raw_text)
                .unwrap_or_else(|| raw_text.clone());
            let stats = finalizer.stats();
            (clean_text, stats)
        };
        info!(
            "Post-processed transcript ({} chars, delta={}, drops={}/{}, gate_drops={}, lexicon_rewrites={})",
            clean_text.len(),
            raw_text.len() as i64 - clean_text.len() as i64,
            postprocess_stats.dropped_chunks,
            postprocess_stats.input_chunks,
            postprocess_stats.gate_drops,
            postprocess_stats.lexicon_rewrites
        );

        if raw_save_enabled {
            let raw_entry = crate::state::history::save_entry_with_timestamp_and_slug(
                &raw_text,
                Some(recording_timestamp),
                crate::state::history::TranscriptKind::Raw,
                Some(&raw_text),
            );
            info!("Raw transcript saved: {}", raw_entry.path.display());
            crate::ui::voice_chat::update_drawer_after_save(raw_entry.path.as_path());
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
        let ai_key_available = crate::ai_formatting::has_api_key();

        // Determine final text based on mode (NEW architecture):
        //
        // 1. HoldMode::Chat / HoldMode::Selection (assistive=true): ALWAYS AI augmentation
        // 2. Ctrl Hold (force_raw=true): ALWAYS raw transcript (ignores AI toggle)
        // 3. Left double Option (force_ai=true): ALWAYS AI formatting
        // 4. Toggle (neither): respects AI_FORMATTING_ENABLED toggle
        //
        // This allows users to choose mode via hotkey:
        // - Quick dictation? → Ctrl (fast, raw)
        // - Need formatting? → Double Option (respects setting)
        // - AI chat? → Hold + Shift (Chat)
        // - AI on selection? → Hold + Cmd (Selection)
        let (formatted_text, output_kind, mut should_auto_paste) = if assistive {
            info!(
                "Assistive mode ({:?}): augmenting transcript via AI",
                effective_hold_mode
            );

            if chat_active {
                crate::ui::voice_chat::show_voice_chat_overlay();
                if skip_user_bubble {
                    // Event pipeline: Preview already streamed text into the bubble.
                    // Just finalize the user message (stop streaming indicator)
                    // without re-writing the text.
                    crate::ui::voice_chat::finalize_voice_chat_user_message();
                    self.toggle_user_has_text.store(true, Ordering::SeqCst);
                } else if !should_allow_full_user_bubble_rewrite(
                    skip_user_bubble,
                    append_mode,
                    live_stream_session,
                ) {
                    // Delta-first path: avoid full rewrites while stream is active.
                    if user_needs_separator {
                        crate::ui::voice_chat::append_voice_chat_user_delta("\n\n");
                    }
                    crate::ui::voice_chat::append_voice_chat_user_delta(&clean_text);
                    self.toggle_user_has_text.store(true, Ordering::SeqCst);
                } else {
                    crate::ui::voice_chat::set_voice_chat_user_text(&clean_text);
                }
                crate::ui::voice_chat::show_agent_tab();
                crate::ui::voice_chat::set_voice_chat_sending(true);
                crate::ui::voice_chat::update_voice_chat_status("Thinking… (35%)");
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
                crate::ui::voice_chat::update_voice_chat_context_summary(&format!(
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
                    crate::ui::voice_chat::update_voice_chat_status(
                        "Selection unavailable - chat fallback",
                    );
                    crate::ui::voice_chat::add_voice_chat_system_message(
                        "Selection was not detected. Continuing without selected-text context.",
                    );
                }
            }

            // Split behavior:
            // - Chat: ignore selection.
            // - Selection: if no selection was captured, we already downgraded to Chat mode.
            let assistive_input = build_assistive_input(&clean_text, &ctx);
            if chat_active {
                crate::ui::voice_chat::finalize_voice_chat_user_message();
                crate::ui::voice_chat::set_voice_chat_sending(true);
                send_assistive_with_agent_runtime(
                    assistive_input,
                    config.whisper_language,
                    config.ai_assistive_max_tokens,
                )
                .await;
            }
            // Agent runtime path persists full conversation in ThreadStore.
            (
                clean_text.clone(),
                crate::state::history::TranscriptKind::Raw,
                false,
            )
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
            let should_use_ai = ai_key_available;
            if should_use_ai {
                info!("Formatting mode (Left Option): correcting transcript via AI");

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
                info!("Formatting mode (Left Option): AI unavailable, cleaning repetitions");
                (
                    crate::ai_formatting::remove_simple_repetitions(&clean_text),
                    crate::state::history::TranscriptKind::Raw,
                    true,
                )
            } else {
                info!(
                    "Formatting mode (Left Option): AI unavailable, using post-processed transcript"
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
            let should_use_ai = ai_formatting_enabled && ai_key_available;

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
        let quality_probe =
            ActionQualityProbe::from_transcripts(&raw_text, &formatted_text, &postprocess_stats);
        info!(
            "Action quality guardrail: mode={} assistive={} raw_chars={} final_chars={} diff_raw_final={:.3} correction_ratio={:.3} drop_ratio={:.3} route_independent=true",
            mode_label,
            assistive,
            quality_probe.raw_chars,
            quality_probe.final_chars,
            quality_probe.raw_final_diff_ratio,
            quality_probe.correction_ratio,
            quality_probe.drop_ratio
        );
        let commit_trigger = if !assistive && !live_stream_session {
            evaluate_quality_commit_trigger(force_raw, &quality_probe, output_kind)
                .map(str::to_string)
        } else {
            None
        };
        if let Some(reason) = commit_trigger.as_deref() {
            info!(
                "COMMIT decision: trigger={} mode={} diff_raw_final={:.3} correction_ratio={:.3} drop_ratio={:.3}",
                reason,
                mode_label,
                quality_probe.raw_final_diff_ratio,
                quality_probe.correction_ratio,
                quality_probe.drop_ratio
            );
        } else if !assistive && !live_stream_session {
            info!("COMMIT decision: not required by quality gate (mode={mode_label})");
        }

        if should_apply_transcription_action_contract(assistive, live_stream_session)
            && config.transcription_overlay_enabled
        {
            let action_contract_mode = resolve_transcription_action_contract_mode(
                force_raw,
                force_ai,
                config.ai_formatting_enabled,
                ai_key_available,
            );
            // Keep the ephemeral transcription overlay in sync with what we will paste/save.
            // This makes it easier to understand differences between streaming preview and final-pass output.
            crate::ui::overlay::set_transcription_action_contract(
                &raw_text,
                &formatted_text,
                action_contract_mode,
            );
        } else if !assistive {
            debug!(
                "Skipping transcription action contract rewrite during live stream (mode={mode_label})"
            );
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
            if config.quick_notes_save_only {
                should_auto_paste = false;
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
            // Paste the text into the active application
            clipboard::paste_text(&formatted_text).context("Failed to paste text")?;
            info!("Text pasted successfully");
        } else {
            info!("Auto-paste skipped (mode={})", mode_label);
        }

        // Save final transcript (skip duplicate when RAW already stored and unchanged)
        let needs_final_save = !assistive
            && (!raw_save_enabled
                || output_kind != crate::state::history::TranscriptKind::Raw
                || formatted_text.trim() != raw_text.trim());
        if needs_final_save {
            let entry = crate::state::history::save_entry_with_timestamp_and_slug(
                &formatted_text,
                Some(recording_timestamp),
                output_kind,
                Some(&raw_text),
            );
            info!("Transcript saved: {}", entry.path.display());
            crate::ui::voice_chat::refresh_drawer();
        } else if assistive {
            info!(
                "Assistive flow: skipping legacy final transcript save (ThreadStore is source of truth)"
            );
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

        Ok(types::TranscriptProcessOutcome { commit_trigger })
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
        crate::ui::voice_chat::update_voice_chat_status("Idle");

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
