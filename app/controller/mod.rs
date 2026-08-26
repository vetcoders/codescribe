//! Recording pipeline state machine controller
//!
//! This module implements the core hotkey-driven state machine for Codescribe.
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
//! (default 800ms) before the recorder actually starts. Assistive hold bindings
//! keep a 400ms floor even if settings lower the generic hold delay. This prevents
//! accidental Emil sessions while preserving quick toggle-mode for power users.

/// Admission readiness: the precondition of beginning a product recording.
pub mod admission;
/// Per-session assistive context bag (selection, app, images).
mod context_bucket;
/// One destination throne: intent → Agent / Orient / paste. Focus is not king.
mod delivery_route;
/// Session telemetry, image attach helpers, assistive send wiring.
mod helpers;
/// Hold/toggle timing, agent-send vetoes, stop adjudication policy.
mod hotkey_policy;
/// Production-owned, content-private PCM replay of the overlay engine cone.
pub mod production_replay;
/// Public serving-status surface for tray/UI consumers.
pub mod serving_status;
/// Controller state, hotkey types, and recording truth metadata.
mod types;

pub use helpers::{
    is_assistive_session, is_conversation_session, publish_recording_indicator,
    set_assistive_session, set_assistive_target_thread, set_conversation_session,
};
pub use types::{HotkeyAction, HotkeyInput, HotkeyType, State, TranscriptionActionContractMode};
pub use delivery_route::{OverlayPasteDelivery, OverlayPasteResult};

use crate::presentation::{PresentationEmitter, TranscriptBus, TranscriptMode, TranscriptSession};
use anyhow::{Context, Result};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::time::Duration;
use tokio::sync::{Mutex, RwLock, broadcast, mpsc};
use tokio::task::JoinHandle;
use tracing::{debug, error, info, warn};
use uuid::Uuid;

use crate::audio::streaming_recorder::StreamingRecorder;
#[cfg(test)]
use crate::config::DeferredInsertShortcut;
use crate::config::models::ModelManager;
use crate::config::{Config, RuntimeSettingsSnapshot, UserSettings};
use crate::os::clipboard;
use crate::os::hold_badge::BadgeMode;
use crate::os::hotkeys::{self, HoldMode};
use crate::os::selection::{
    AssistiveContext, capture_assistive_context,
    capture_assistive_context_with_image_with_prior_frontmost,
    capture_frontmost_app_only_with_prior_frontmost,
};
use crate::os::shortcut_registry;
use context_bucket::ContextBucket;
#[cfg(test)]
pub(crate) use context_bucket::ContextMarker;

// Moshi conversation engine and audio output
use codescribe_core::conversation::{ConversationEngine, MoshiConfig};
use codescribe_core::ipc::{EngineEventWire, IpcEvent, IpcEventPayload};
use codescribe_core::tts::AudioPlayer;

#[cfg(test)]
pub(crate) use codescribe_core::pipeline::contracts::TranscriptionConfidenceFlag;

use delivery_route::{
    DeliveryFacts, DeliveryIntent, DeliveryRoute, format_delivery_route_line,
    overlay_insert_facts, resolve_delivery_route, target_is_self_app,
};
#[cfg(test)]
use helpers::SessionEngineStats;
#[cfg(test)]
pub(crate) use helpers::SessionTelemetrySnapshot;
#[cfg(test)]
use helpers::build_image_attachments_from_text;
use helpers::{
    SharedSessionTelemetry, new_session_telemetry, reset_session_telemetry,
    send_assistive_with_agent_runtime_lane,
};
use hotkey_policy::{
    STOP_TIMEOUT, effective_hold_start_delay_ms, should_apply_incoming_mode_flags,
    should_block_hotkey_during_agent_send, should_use_toggle_adjudicated_stop,
    toggle_final_pass_enabled,
};
#[cfg(test)]
use hotkey_policy::{is_assistive_start_event, toggle_stop_adjudicate_timeout};

/// Live overlay: ms of audio held before the first interim emit.
const LIVE_PROFILE_BUFFER_DELAY_MS: u64 = 280;
/// Live overlay typing animation speed in characters per second.
const LIVE_PROFILE_TYPING_CPS: f32 = 90.0;
/// Cap words emitted per live interim chunk (smooth typing feel).
const LIVE_PROFILE_EMIT_WORDS_MAX: u64 = 2;
/// Seconds between interim emissions when the overlay is visible.
const LIVE_PROFILE_INTERIM_SEC: f32 = 1.2;
/// Longer interim interval when no overlay is watching partials.
const NO_OVERLAY_PROFILE_INTERIM_SEC: f32 = 8.0;
/// At most one level sample may wait behind the controller worker. The capture
/// thread never constructs IPC events or timestamps and never accumulates a
/// backlog when the bridge/UI is slower than CoreAudio.
const AUDIO_LEVEL_QUEUE_CAPACITY: usize = 1;

/// Test-only latch: when true, process_recording blocks forever.
/// Armed by hang_process_recording_for_test; cleared by its Drop guard.
#[cfg(test)]
static PROCESS_RECORDING_TEST_HANG: AtomicBool = AtomicBool::new(false);

/// Clears [`PROCESS_RECORDING_TEST_HANG`] on drop, so a test that arms the hang
/// cannot leak it into the next test in the same process.
#[cfg(test)]
struct ProcessRecordingHangGuard;

/// Make `process_recording` block forever, so the stuck-stop watchdog can be
/// exercised without a real recorder or a real stall.
#[cfg(test)]
fn hang_process_recording_for_test() -> ProcessRecordingHangGuard {
    PROCESS_RECORDING_TEST_HANG.store(true, Ordering::SeqCst);
    ProcessRecordingHangGuard
}

#[cfg(test)]
impl Drop for ProcessRecordingHangGuard {
    /// Clear PROCESS_RECORDING_TEST_HANG so the hang cannot leak across tests.
    fn drop(&mut self) {
        PROCESS_RECORDING_TEST_HANG.store(false, Ordering::SeqCst);
    }
}

/// Publish the live-transcription tuning for the session that is about to start
/// and report whether the overlay is enabled.
///
/// The knobs cross into the core pipeline as process env vars, which is why
/// this runs at every session start rather than once at boot: user settings can
/// change between recordings. A session with no overlay to feed uses a much
/// longer interim window — nobody is watching the partials, so paying for
/// frequent interim emissions would be waste.
fn apply_runtime_transcription_profile(
    config: &Config,
    settings: &UserSettings,
    assistive: bool,
) -> bool {
    let overlay_enabled = config.transcription_overlay_enabled;

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

/// Holds a shared flag `true` for a scope and clears it on drop — including on
/// an early `return` out of a start path, which is exactly where a hand-written
/// reset gets forgotten.
struct AtomicFlagGuard {
    flag: Arc<AtomicBool>,
}

impl AtomicFlagGuard {
    /// Raise the flag; it falls when the returned guard is dropped.
    fn new(flag: Arc<AtomicBool>) -> Self {
        flag.store(true, Ordering::SeqCst);
        Self { flag }
    }
}

impl Drop for AtomicFlagGuard {
    /// Lower the shared AtomicFlagGuard flag when the scope ends.
    fn drop(&mut self) {
        self.flag.store(false, Ordering::SeqCst);
    }
}

/// Keep the last session WAV at a stable path so overlay Retranscribe
/// (Full HQ / Cloud) can re-run without depending on a temp file still
/// being there after stop.
fn retain_last_session_audio(path: &std::path::Path) {
    let dest = crate::config::Config::config_dir().join("last_session.wav");
    match std::fs::copy(path, &dest) {
        Ok(_) => info!("last_session.wav retained at {}", dest.display()),
        Err(err) => warn!("last_session.wav retain failed: {err:#}"),
    }
}

/// What one stop-and-process pass produced: the delivery decision plus the
/// per-phase wall clock that the stop-path budget line reports.
#[derive(Debug, Clone, Default)]
struct ProcessRecordingOutcome {
    no_speech_reason: Option<String>,
    commit_trigger: Option<String>,
    transcript_present: bool,
}

impl ProcessRecordingOutcome {
    /// Outcome for a stop that produced no usable transcript. Timings are zero
    /// because this path exits before the delivery cone.
    fn no_speech(reason: impl Into<String>) -> Self {
        Self {
            no_speech_reason: Some(reason.into()),
            commit_trigger: None,
            transcript_present: false,
        }
    }
}

/// Whether a finalized transcript may replace the whole user bubble rather than
/// append to it. Append-mode and live-stream sessions own their canvas
/// incrementally, so a full rewrite there would erase committed text.
#[cfg(test)]
fn should_allow_full_user_bubble_rewrite(
    skip_user_bubble: bool,
    append_mode: bool,
    live_stream_session: bool,
) -> bool {
    !skip_user_bubble && !append_mode && !live_stream_session
}

/// The action contract applies only to plain dictation: assistive sessions
/// deliver through the agent lane, and live-stream sessions bypass formatting.
fn should_apply_transcription_action_contract(assistive: bool, live_stream_session: bool) -> bool {
    !assistive && !live_stream_session
}

/// Recording controller managing state machine and lifecycle
pub struct RecordingController {
    /// The one mutable controller generation handle. Each take clones this Arc
    /// once and every config, user-settings, LLM and recorder fact comes from it.
    /// The Arc inside is replaced only when idle so an active take keeps its
    /// generation even if Settings writes a later snapshot.
    runtime_settings: RwLock<Arc<RuntimeSettingsSnapshot>>,

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
    /// The one observer bus for the active recording. Presentation may publish
    /// mutable drafts through it, but only the stop controller publishes the
    /// immutable product seal after every automatic stage completes.
    active_transcript_bus: Arc<RwLock<Option<Arc<TranscriptBus>>>>,

    /// Task handle for delayed hold-start (800ms default)
    hold_start_task: Arc<Mutex<Option<JoinHandle<()>>>>,
    /// Monotonic generation for hold-start tasks.
    ///
    /// Every cancel/reschedule bumps this value. Spawned tasks compare their
    /// captured generation before/after critical awaits to avoid stale-start races.
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
    /// Trigger-time context retained after recording cleanup until the overlay
    /// either auto-sends the untouched final transcript or the user explicitly
    /// sends an edited transcript.
    pending_assistive_context: Arc<RwLock<Option<AssistiveContext>>>,
    /// Combo-collected selections for the active dictation. The bucket survives
    /// recording cleanup and is consumed only at the agent-send seam.
    context_bucket: Arc<Mutex<ContextBucket>>,
    /// App that was frontmost when the user initiated a hold session, before
    /// Codescribe badge/overlay UI can become frontmost.
    pre_overlay_frontmost_app: Arc<RwLock<Option<String>>>,

    /// Sample offset (in the recorder buffer) marking the start of the next
    /// incremental segment. Advances on each `commit_segment` call so segment
    /// snapshots don't overlap. Resets to 0 on new toggle session start.
    ///
    /// Used by Commit / Augment overlay buttons to clip a WAV slice from the
    /// active recorder without stopping the stream.
    last_segment_audio_offset: Arc<AtomicUsize>,

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
    /// One phrasing for "there is no recorder", logged and returned together so
    /// a caller cannot report the failure in a way the log does not corroborate.
    fn recorder_unavailable_error(context: &str) -> anyhow::Error {
        warn!("{context}: streaming recorder unavailable; voice capture is disabled");
        anyhow::anyhow!("{context}: streaming recorder unavailable")
    }

    /// Best-effort recorder construction at controller init. A missing audio
    /// device disables voice capture but must not prevent the app from starting,
    /// so the failure degrades to `None` plus a warning.
    fn init_streaming_recorder(context: &str) -> Option<StreamingRecorder> {
        match StreamingRecorder::new() {
            Ok(recorder) => Some(recorder),
            Err(error) => {
                warn!("{context}: failed to initialize streaming recorder: {error}");
                None
            }
        }
    }

    /// Mutable recorder out of a held lock guard, or the unavailable error.
    fn recorder_from_guard_mut<'a>(
        recorder_guard: &'a mut Option<StreamingRecorder>,
        context: &str,
    ) -> Result<&'a mut StreamingRecorder> {
        recorder_guard
            .as_mut()
            .ok_or_else(|| Self::recorder_unavailable_error(context))
    }

    /// Shared recorder out of a held lock guard, or the unavailable error.
    fn recorder_from_guard<'a>(
        recorder_guard: &'a Option<StreamingRecorder>,
        context: &str,
    ) -> Result<&'a StreamingRecorder> {
        recorder_guard
            .as_ref()
            .ok_or_else(|| Self::recorder_unavailable_error(context))
    }

    /// Create a new recording controller with configuration loaded from disk
    pub fn new() -> Self {
        let snapshot = Config::load_runtime_snapshot()
            .unwrap_or_else(|error| panic!("runtime settings snapshot refused: {error:?}"));
        Self::with_runtime_settings(snapshot, "RecordingController::new")
    }

    /// Create a new recording controller without populating secrets from Keychain.
    ///
    /// Used by the SwiftUI redesign dictation bridge: starting local recording must
    /// not ask for API-key access as an incidental side effect.
    pub fn new_without_keychain() -> Self {
        let snapshot = Config::load_runtime_snapshot_without_keychain()
            .unwrap_or_else(|error| panic!("runtime settings snapshot refused: {error:?}"));
        Self::with_runtime_settings(
            snapshot,
            "RecordingController::new_without_keychain",
        )
    }

    /// Shared constructor behind both public entry points.
    ///
    /// Outside tests this also kicks off a background STT prewarm. The product
    /// invariant it protects is that **recording readiness is not engine
    /// readiness**: capture must start the instant the user presses record, so
    /// the prewarm runs on its own thread and a failure is a warning, never a
    /// blocked recording.
    fn with_runtime_settings(
        runtime_settings: RuntimeSettingsSnapshot,
        recorder_context: &str,
    ) -> Self {
        let config = runtime_settings.values().clone();
        info!(
            "Initializing RecordingController (hold_delay={}ms, beep={}, language={:?})",
            config.hold_start_delay_ms, config.beep_on_start, config.whisper_language
        );

        let recorder = Self::init_streaming_recorder(recorder_context);

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

            if !crate::whisper::is_initialized() {
                // Best-effort BACKGROUND prewarm — never block recording readiness.
                //
                // Product invariant: recording readiness is NOT engine readiness.
                // Audio capture must start the moment the user presses record; the
                // live local refinement and explicit Retranscribe lazy-load the
                // engine on first use.
                // A failed prewarm is a warning, not an app or recording failure.
                // The idle-unload reaper (commit 2b8bb1f) may legitimately drop the
                // engine later and the next call reloads it — pinning it here would
                // undo that GPU/host-memory reclaim.
                //
                // Warm the ACTIVE router engine (Apple SpeechAnalyzer on macOS 26+,
                // Candle on fallback/older macOS) AND run a synthetic warmup
                // inference, so the first dictation pays neither model-load nor
                // Metal kernel-compilation latency — matching the old always-instant
                // behaviour where the long-lived daemon was warm before first use.
                std::thread::Builder::new()
                    .name("stt-prewarm".into())
                    .spawn(|| {
                        if let Err(e) = crate::stt::prewarm_active_engine() {
                            warn!(
                                "STT background prewarm failed (will lazy-load on first use): {}",
                                e
                            );
                        }
                    })
                    .ok();
            }
        }

        let runtime_settings = RwLock::new(Arc::new(runtime_settings));
        if recorder.is_none() {
            warn!("Recorder unavailable at controller init; voice capture is disabled");
        }
        let (event_broadcast, _) = broadcast::channel::<IpcEvent>(256);
        let session_telemetry = new_session_telemetry();

        Self {
            runtime_settings,
            state: Arc::new(RwLock::new(State::Idle)),
            recorder: Arc::new(Mutex::new(recorder)),
            assistive_mode: Arc::new(RwLock::new(false)),
            hold_mode: Arc::new(RwLock::new(HoldMode::Raw)),
            force_raw_mode: Arc::new(RwLock::new(false)),
            force_ai_mode: Arc::new(RwLock::new(false)),
            session_id: Arc::new(RwLock::new(None)),
            active_transcript_bus: Arc::new(RwLock::new(None)),
            hold_start_task: Arc::new(Mutex::new(None)),
            hold_start_generation: Arc::new(AtomicU64::new(0)),
            start_transition_in_flight: Arc::new(AtomicBool::new(false)),
            serial_lock: Arc::new(Mutex::new(())),
            vad_triggered: Arc::new(AtomicBool::new(false)),
            assistive_loop_active: Arc::new(AtomicBool::new(false)),
            toggle_user_has_text: Arc::new(AtomicBool::new(false)),
            toggle_assistant_has_text: Arc::new(AtomicBool::new(false)),
            assistive_context: Arc::new(RwLock::new(None)),
            pending_assistive_context: Arc::new(RwLock::new(None)),
            context_bucket: Arc::new(Mutex::new(ContextBucket::for_codescribe_data_dir(
                Config::config_dir(),
            ))),
            pre_overlay_frontmost_app: Arc::new(RwLock::new(None)),
            last_segment_audio_offset: Arc::new(AtomicUsize::new(0)),
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

    /// Forward one host sleep/wake boundary to the active recording session.
    ///
    /// This never creates a recorder or starts an engine. When capture is not
    /// active it is a normal no-op; otherwise the per-recording lifecycle
    /// channel wakes the session loop and degrades Layer 1 fail-closed.
    pub async fn note_sleep_wake(&self) -> bool {
        self.recorder
            .lock()
            .await
            .as_ref()
            .is_some_and(StreamingRecorder::note_sleep_wake)
    }

    /// Subscribe to the controller's IPC event stream. Each subscriber gets its
    /// own receiver; a slow consumer lags rather than stalling the producer.
    pub fn subscribe_events(&self) -> broadcast::Receiver<IpcEvent> {
        self.event_broadcast.subscribe()
    }

    /// Transition state and broadcast the change (see
    /// [`Self::set_state_with_broadcast`] for the invariants).
    async fn set_state(&self, new_state: State) {
        Self::set_state_with_broadcast(&self.state, &self.event_broadcast, new_state).await;
    }

    /// Flip the cursor badge to "processing" while a stop pipeline runs.
    async fn show_processing_badge_if_enabled(&self) {
        let hold_indicator = self.get_config().await.hold_indicator;
        publish_recording_indicator(BadgeMode::Processing, hold_indicator);
    }

    /// Character offset the live transcript has reached, used to anchor a
    /// context marker at the point in the dictation where the user pressed the
    /// combo. Zero outside an active recording, and zero when no recorder or
    /// buffer exists — an unanchored marker is better than a wrong anchor.
    async fn current_live_transcript_position(&self, state: State) -> usize {
        if !matches!(state, State::RecHold | State::RecToggle) {
            return 0;
        }
        let transcript_buffer = {
            let recorder = self.recorder.lock().await;
            recorder
                .as_ref()
                .map(StreamingRecorder::transcript_buffer_handle)
        };
        let Some(transcript_buffer) = transcript_buffer else {
            return 0;
        };
        transcript_buffer.lock().await.chars().count()
    }

    /// Capture selection + frontmost app for an assistive combo pressed mid
    /// session, and drop it into the context bucket as a marked item.
    ///
    /// Ordering is deliberate: the OS capture is launched first and the
    /// transcript position is read while it is already in flight, because
    /// focus and caret state can vanish the moment the combo changes the UI.
    /// A bucket failure degrades to the plain selection context rather than
    /// losing the capture entirely.
    async fn capture_assistive_combo_context(
        &self,
        state: State,
        prior_frontmost_app: Option<String>,
    ) -> AssistiveContext {
        // Start selection capture first: focus/caret state may disappear as soon
        // as the combo changes the UI. Snapshot the live transcript position
        // concurrently while the OS capture is already in flight.
        let capture_task = tokio::task::spawn_blocking(move || {
            capture_assistive_context_with_image_with_prior_frontmost(prior_frontmost_app)
        });
        let position = self.current_live_transcript_position(state).await;
        let captured_payload = capture_task.await.unwrap_or_default();
        let captured = captured_payload.context;
        let fallback = captured.clone();
        // A selected image is retained before clipboard restoration. If Cmd+C
        // produced no image, preserve the existing clipboard-image behavior.
        let image_png = match captured_payload.image_png {
            some @ Some(_) => some,
            None => tokio::task::spawn_blocking(clipboard::get_image_png_best_effort)
                .await
                .unwrap_or(None),
        };
        let mut bucket = self.context_bucket.lock().await;
        let result = (|| -> anyhow::Result<_> {
            let mut context = captured;
            let marker = match context.selected_text.take() {
                Some(selected_text) => bucket.add_selection(position, selected_text)?,
                None => None,
            };
            if let Some(png) = image_png {
                let _ = bucket.add_image_png(&png)?;
            }
            Ok((context, marker))
        })();

        match result {
            Ok((context, Some(marker))) => {
                let marker = format!("{{{}}}", marker.label);
                let _ = self.event_broadcast.send(IpcEvent {
                    timestamp: chrono::Utc::now()
                        .to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
                    payload: IpcEventPayload::ContextMarker {
                        position: u64::try_from(position).unwrap_or(u64::MAX),
                        marker,
                    },
                });
                context
            }
            Ok((context, None)) => context,
            Err(error) => {
                warn!("Context bucket capture failed; retaining legacy selection context: {error}");
                fallback
            }
        }
    }

    /// Attach the current OS selection as `{selection_N}` during an in-flight
    /// hold. Destination, overlay visibility, and Agent UI stay unchanged.
    pub async fn attach_hold_selection(&self) -> Result<()> {
        let current_state = self.current_state().await;
        let pending_hold = self.hold_start_task.lock().await.is_some();
        if !matches!(current_state, State::RecHold | State::RecToggle) && !pending_hold {
            debug!("attach_hold_selection ignored: no in-flight hold");
            return Ok(());
        }

        let prior_frontmost_app = self.pre_overlay_frontmost_app.read().await.clone();
        let _ctx = self
            .capture_assistive_combo_context(current_state, prior_frontmost_app)
            .await;
        Ok(())
    }

    /// Deliver the overlay's current transcript with the context captured at
    /// trigger time. Taking the context makes delivery one-shot.
    pub async fn deliver_pending_assistive_transcript(&self, transcript: String) -> Result<bool> {
        let runtime_settings = self.runtime_settings_arc().await;
        self.deliver_pending_assistive_transcript_with(
            transcript,
            move |wire, language, max_tokens, persona| {
                Box::pin(send_assistive_with_agent_runtime_lane(
                    runtime_settings,
                    wire,
                    language,
                    max_tokens,
                    persona,
                ))
                    as std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>>
            },
        )
        .await
    }

    /// Production body with an injectable send adapter. The harness executes
    /// this exact instrumentation boundary with a fake adapter — moving the
    /// timer off the real send breaks the harness, not just a formatter test.
    pub(crate) async fn deliver_pending_assistive_transcript_with<F>(
        &self,
        transcript: String,
        send: F,
    ) -> Result<bool>
    where
        F: FnOnce(
            String,
            crate::config::Language,
            i32,
            bool,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>>,
    {
        let delivery_started = std::time::Instant::now();
        if transcript.trim().is_empty() {
            info!(elapsed_secs = delivery_started.elapsed().as_secs_f64(), "assistive delivery skipped: empty transcript");
            return Ok(false);
        }
        let to_agent = resolve_delivery_route(
            DeliveryIntent::OverlayToAgent,
            DeliveryFacts {
                has_text: true,
                no_speech: false,
                auto_paste_enabled: false,
                overlay_enabled: true,
                live_stream_session: false,
                commit_required: false,
                latched_target_is_self: false,
            },
        );
        info!(
            "{}",
            format_delivery_route_line(DeliveryIntent::OverlayToAgent, to_agent, None,)
        );
        // Dictation/formatting sessions never run the assistive pipeline branch
        // that arms `pending_assistive_context`, so the overlay's explicit
        // "To Agent" used to fail closed behind a live button (review P0-03).
        // The session trigger context (frontmost app, captured at every session
        // start) is truthful for an explicit send; taking it keeps delivery
        // one-shot either way.
        let _ = self.pending_assistive_context.write().await.take();
        let _ = self.assistive_context.write().await.take();
        let config = self.get_config().await;
        {
            let mut bucket = self.context_bucket.lock().await;
            match bucket.archive_and_reset("assistive-delivery") {
                Ok(Some(dir)) => info!("Context bucket archived: {}", dir.display()),
                Ok(None) => {}
                Err(err) => warn!("Context bucket archive failed (items kept): {err:#}"),
            }
        }
        send(
            transcript,
            config.whisper_language,
            config.ai_assistive_max_tokens,
            true,
        )
        .await;
        info!(elapsed_secs = delivery_started.elapsed().as_secs_f64(), "assistive delivery completed");
        Ok(true)
    }

    /// Swap the state and broadcast the transition, on `Arc` handles so spawned
    /// tasks can drive it without borrowing the controller.
    ///
    /// The write guard is released before the broadcast, and the event fires
    /// only on a real change. Any arrival at `Idle` tears down the cursor badge,
    /// which is what makes finalize, cancel, error and no-speech all end with a
    /// clean cursor instead of each path remembering to hide it.
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
            // Recording ended → always tear down the cursor badge (covers finalize,
            // cancel, error, no-speech — any path back to Idle).
            if new_state == State::Idle {
                crate::os::hold_badge::hide_hold_badge();
            }
            let _ = event_broadcast.send(IpcEvent {
                timestamp: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
                payload: IpcEventPayload::StateChange {
                    from: old_state.to_ipc_str().to_string(),
                    to: new_state.to_ipc_str().to_string(),
                },
            });
        }
    }

    /// Replace the immutable settings generation when no take is active.
    ///
    /// Scheduling, starts, stops and refresh all cross `serial_lock`. A live
    /// delayed-hold task counts as active ownership; a finished stale handle is
    /// consumed so it cannot block refresh forever.
    pub async fn replace_runtime_settings_when_idle(
        &self,
        runtime_settings: RuntimeSettingsSnapshot,
    ) -> bool {
        let _serial_guard = self.serial_lock.lock().await;

        {
            let mut hold_task = self.hold_start_task.lock().await;
            let finished = hold_task.as_ref().map(|task| task.is_finished());
            match finished {
                Some(false) => return false,
                Some(true) => {
                    let _ = hold_task.take();
                }
                None => {}
            }
        }

        if self.current_state().await != State::Idle {
            return false;
        }
        *self.runtime_settings.write().await = Arc::new(runtime_settings);
        true
    }

    /// Borrow the current settings generation as an Arc (may differ from an
    /// in-flight take that already cloned an older generation).
    pub async fn runtime_settings_arc(&self) -> Arc<RuntimeSettingsSnapshot> {
        Arc::clone(&*self.runtime_settings.read().await)
    }

    /// Snapshot of current controller configuration
    pub async fn get_config(&self) -> Config {
        self.runtime_settings_arc().await.values().clone()
    }

    /// Admission readiness of the next product recording, decided against
    /// this controller's current settings generation. Probes the device that
    /// would open and the seal lane; opens no stream, invents no floor.
    pub async fn admission_readiness(
        &self,
    ) -> Result<admission::AdmissionGrant, admission::AdmissionBlocker> {
        let snapshot = self.runtime_settings_arc().await;
        tokio::task::spawn_blocking(move || admission::evaluate_live_admission_arc(&snapshot))
            .await
            .unwrap_or_else(|join| {
                Err(admission::AdmissionBlocker::CaptureDeviceUnavailable {
                    reason: format!("admission probe panicked: {join}"),
                })
            })
    }

    /// Surface a refused start on the one engine `Warning` channel the bridge
    /// already forwards to `listener.on_error` (user-terminal code), and flag
    /// the tray. The refusal happened before any microphone opened, so no
    /// state transition exists to carry it — this is the only signal.
    fn broadcast_admission_refusal(
        event_broadcast: &broadcast::Sender<IpcEvent>,
        blocker: &admission::AdmissionBlocker,
    ) {
        crate::os::tray_status::update_tray_status(crate::os::tray_status::TrayStatus::Error);
        let _ = event_broadcast.send(IpcEvent {
            timestamp: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
            payload: IpcEventPayload::Engine(EngineEventWire::Warning {
                code: codescribe_core::pipeline::contracts::ADMISSION_REFUSED_WARNING_CODE
                    .to_string(),
                message: blocker.to_string(),
            }),
        });
    }

    /// Guided acoustic calibration: capture `duration` of the operator speaking
    /// through the exact recorder path a take would open, derive a device
    /// profile from the capture-level receipt (ITU-T P.56 margin), persist it
    /// beside `settings.json`, and report the measured figures. Levels and
    /// counts only — the temporary WAV the recorder writes is deleted and no
    /// audio is retained. Refuses unless the controller is idle.
    pub async fn capture_energy_calibration(
        &self,
        duration: Duration,
    ) -> Result<admission::EnergyCalibrationReport> {
        use codescribe_core::audio::capture_receipt::{CaptureLevelAccumulator, CapturePathMeta};
        use codescribe_core::config::energy_calibration::{
            EnergyCalibrationArtifact, EnergyCalibrationProfile, SOURCE_GUIDED_CAPTURE,
            energy_calibration_path,
        };

        let _guard = self.serial_lock.lock().await;
        let current_state = *self.state.read().await;
        if current_state != State::Idle {
            anyhow::bail!("calibration_busy: cannot calibrate while state={current_state}");
        }
        let mut recorder_guard = self.recorder.lock().await;
        let recorder = Self::recorder_from_guard_mut(&mut recorder_guard, "Calibration")?;
        Self::ensure_recorder_ready_for_start(recorder, "Calibration preflight").await?;

        let accumulator = Arc::new(std::sync::Mutex::new(CaptureLevelAccumulator::new()));
        let sink = Arc::clone(&accumulator);
        recorder.recorder.config.auto_silence = false;
        recorder.recorder.set_callback(Box::new(move |data| {
            sink.lock()
                .unwrap_or_else(|error| error.into_inner())
                .push_samples(data);
        }));
        info!(
            duration_secs = duration.as_secs_f32(),
            "acoustic calibration capture starting"
        );
        recorder.recorder.start().await?;
        tokio::time::sleep(duration).await;
        let stopped = recorder.recorder.stop().await;
        Self::clear_recorder_callbacks(recorder);
        let temp_wav = stopped?;
        if let Some(path) = temp_wav
            && let Err(error) = std::fs::remove_file(&path)
        {
            warn!(path = %path.display(), %error, "calibration temp WAV could not be removed");
        }
        let meta = CapturePathMeta::from_open_path(
            recorder.recorder.actual_sample_rate(),
            recorder.recorder.last_native_channels(),
            recorder.recorder.last_input_device(),
        );
        drop(recorder_guard);

        let receipt = accumulator
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .finalize(meta);
        receipt.log();
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or_default();
        let profile = EnergyCalibrationProfile::derive(&receipt, now_ms, SOURCE_GUIDED_CAPTURE)
            .map_err(|refusal| anyhow::anyhow!("calibration_refused: {refusal}"))?;
        let path = energy_calibration_path();
        EnergyCalibrationArtifact::record_profile(&path, profile.clone(), now_ms)
            .map_err(|refusal| anyhow::anyhow!("calibration_store_failed: {refusal}"))?;
        info!(
            device = %profile.capture_path.device_name,
            version = %profile.version,
            existence_threshold_dbfs = profile.floors.existence_threshold_dbfs,
            path = %path.display(),
            "acoustic calibration profile stored"
        );
        Ok(admission::EnergyCalibrationReport {
            device_name: profile.capture_path.device_name.clone(),
            sample_rate: profile.capture_path.sample_rate,
            measured_seconds: receipt.active_speech_samples as f32
                / receipt.sample_rate.max(1) as f32,
            active_speech_median_dbfs: profile.measurement.active_speech_median_dbfs,
            noise_floor_dbfs: profile.measurement.noise_floor_dbfs,
            peak_dbfs: profile.measurement.peak_dbfs,
            existence_threshold_dbfs: profile.floors.existence_threshold_dbfs,
            version: profile.version,
            path,
        })
    }

    /// App name latched before the current overlay session took focus.
    ///
    /// This is a read-only snapshot for UI copy. Delivery continues to read the
    /// same field inside `paste_text_from_overlay`; exposing it does not alter the
    /// focus restoration or clipboard path.
    pub async fn paste_target_app_name(&self) -> Option<String> {
        self.pre_overlay_frontmost_app.read().await.clone()
    }

    /// Paste user-edited overlay text through the delivery throne, then restore
    /// the latched target and synthesize Cmd+V via clipboard.
    ///
    /// `resolve_delivery_route(OverlayInsert)` picks the destination. Overlay
    /// **caret** (Swift `defer_text_from_overlay`) arms Paste Here. Agent
    /// window, Alacritty, and every other latched caret get Cmd+V. Unconfirmed
    /// ambulances park Paste Here and leave the user's clipboard alone.
    pub async fn paste_text_from_overlay(&self, text: String) -> Result<OverlayPasteResult> {
        let trimmed = text.trim();
        let target_app = self.pre_overlay_frontmost_app.read().await.clone();
        let intent = DeliveryIntent::OverlayInsert;
        let decision =
            resolve_delivery_route(intent, overlay_insert_facts(!trimmed.is_empty(), false));
        info!(
            "{}",
            format_delivery_route_line(intent, decision, target_app.as_deref())
        );
        if trimmed.is_empty() || decision.route == DeliveryRoute::ArchiveOnly {
            return Ok(OverlayPasteResult {
                delivery: OverlayPasteDelivery::Noop,
                target_app_name: None,
                frontmost_app_name: None,
                deferred_insert_shortcut: None,
                deferred_insert_failure: None,
            });
        }
        if decision.route == DeliveryRoute::DeferredInsert {
            return self
                .arm_overlay_text(trimmed, target_app, Some("Codescribe".to_string()))
                .await;
        }

        let focus_confirmed = target_app
            .as_deref()
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .is_some_and(|name| {
                target_is_self_app(name)
                    || (crate::os::selection::activate_app_by_name(name)
                        && crate::os::selection::wait_for_frontmost_app(
                            name,
                            Duration::from_millis(250),
                        ))
            });
        debug!(
            target = ?target_app,
            focus_confirmed,
            "Overlay paste target activation"
        );

        let config = self.get_config().await;
        let paste_text = trimmed.to_string();
        let frontmost = crate::os::selection::current_frontmost_app_name();
        let preflight = clipboard::synthetic_paste_preflight();

        let mut deferred_insert_shortcut = None;
        let mut deferred_insert_failure = None;
        let delivery = if focus_confirmed && preflight.can_post_events() {
            clipboard::paste_and_restore(&paste_text).context("Failed to paste overlay text")?;
            OverlayPasteDelivery::Pasted
        } else {
            warn!(
                target_app = ?target_app,
                frontmost_app = ?frontmost,
                cg_post_event_access = preflight.cg_post_event_access,
                ax_trusted = preflight.ax_trusted,
                focus_confirmed,
                "Overlay paste could not execute the selected clipboard route; arming deferred insert"
            );
            self.arm_or_copy_deferred_payload(
                paste_text,
                &config,
                &mut deferred_insert_shortcut,
                &mut deferred_insert_failure,
            )?
        };

        Ok(OverlayPasteResult {
            delivery,
            target_app_name: target_app,
            frontmost_app_name: frontmost,
            deferred_insert_shortcut,
            deferred_insert_failure,
        })
    }

    /// Degrade path when a synthetic paste is not safe to post: park the payload
    /// in the process-local Paste Here slot. Never writes the system pasteboard.
    ///
    /// The out-params carry back what the UI must tell the user — which
    /// shortcut is now armed, or why the chord is not bound. The transcript
    /// still sits in-process either way; the user's clipboard stays put.
    fn arm_or_copy_deferred_payload(
        &self,
        payload: String,
        config: &Config,
        shortcut_label: &mut Option<String>,
        registration_failure: &mut Option<String>,
    ) -> Result<OverlayPasteDelivery> {
        if !clipboard::arm_deferred_insert(payload) {
            return Ok(OverlayPasteDelivery::Noop);
        }
        let collision =
            shortcut_registry::deferred_insert_shortcut_conflict(config.deferred_insert_shortcut);
        if !config.deferred_insert_shortcut.is_enabled() {
            *registration_failure = Some("Paste Here shortcut is disabled".to_string());
        } else if !hotkeys::is_global_hotkey_manager_active() {
            *registration_failure = Some("Paste Here hotkey registration failed".to_string());
        } else if let Some(reason) = collision {
            *registration_failure = Some(reason);
        } else {
            *shortcut_label = Some(config.deferred_insert_shortcut.label().to_string());
        }
        Ok(OverlayPasteDelivery::DeferredInsertArmed)
    }

    /// Arm tagged overlay text for Paste Here. Shared by the throne's
    /// `DeferredInsert` verdict and by the explicit defer click.
    async fn arm_overlay_text(
        &self,
        trimmed: &str,
        target_app: Option<String>,
        frontmost_app_name: Option<String>,
    ) -> Result<OverlayPasteResult> {
        let config = self.get_config().await;
        let payload = trimmed.to_string();
        let mut deferred_insert_shortcut = None;
        let mut deferred_insert_failure = None;
        let delivery = self.arm_or_copy_deferred_payload(
            payload,
            &config,
            &mut deferred_insert_shortcut,
            &mut deferred_insert_failure,
        )?;
        Ok(OverlayPasteResult {
            delivery,
            target_app_name: target_app,
            frontmost_app_name,
            deferred_insert_shortcut,
            deferred_insert_failure,
        })
    }

    /// Arm the edited overlay transcript without attempting target activation.
    /// Used when the caret is known to still be inside Codescribe.
    pub async fn defer_text_from_overlay(&self, text: String) -> Result<OverlayPasteResult> {
        let trimmed = text.trim();
        let target_app = self.pre_overlay_frontmost_app.read().await.clone();
        let intent = DeliveryIntent::OverlayInsert;
        // This entry exists because Swift already knows the caret is inside
        // Codescribe. That is a latched-self fact, not a focus-at-click fact.
        let decision =
            resolve_delivery_route(intent, overlay_insert_facts(!trimmed.is_empty(), true));
        info!(
            "{}",
            format_delivery_route_line(intent, decision, target_app.as_deref())
        );
        if trimmed.is_empty() || decision.route == DeliveryRoute::ArchiveOnly {
            return Ok(OverlayPasteResult {
                delivery: OverlayPasteDelivery::Noop,
                target_app_name: None,
                frontmost_app_name: None,
                deferred_insert_shortcut: None,
                deferred_insert_failure: None,
            });
        }
        self.arm_overlay_text(trimmed, target_app, Some("Codescribe".to_string()))
            .await
    }

    /// Explicit overlay Copy: write the tagged transcript to the system
    /// pasteboard. This is the only automatic-adjacent verb allowed to replace
    /// the user's clipboard. Insert / stop-path refuse must not call this.
    pub async fn copy_text_from_overlay(&self, text: String) -> Result<()> {
        let trimmed = text.trim();
        if trimmed.is_empty() {
            return Ok(());
        }
        clipboard::set_clipboard(trimmed).context("Failed to copy overlay text")?;
        Ok(())
    }

    /// Cancel any pending delayed hold-start task
    async fn cancel_pending_hold_start(&self) {
        let generation = self.hold_start_generation.fetch_add(1, Ordering::SeqCst) + 1;
        let mut task_guard = self.hold_start_task.lock().await;
        if let Some(task) = task_guard.take() {
            if task.is_finished() {
                debug!("Cleared finished hold-start task (generation={generation})");
            } else {
                debug!("Invalidated pending hold-start task (generation={generation})");
            }
        }
        *self.pre_overlay_frontmost_app.write().await = None;
    }

    /// Detach every sink and callback from the recorder.
    ///
    /// Run on each stop and before each start: a callback left over from a
    /// finished session would route the next session's audio and deltas into
    /// the previous session's overlay.
    fn clear_recorder_callbacks(recorder: &mut StreamingRecorder) {
        recorder.set_utterance_callback(None);
        recorder.set_utterance_silence_sec(None);
        recorder.set_event_sink(None);
        recorder.set_level_callback(None);
    }

    /// Bring the recorder to a clean pre-start state: force-stop a stream left
    /// active by a previous session, then clear its callbacks. Refusing to
    /// start here would strand the user behind a session they cannot see.
    async fn ensure_recorder_ready_for_start(
        recorder: &mut StreamingRecorder,
        context: &str,
    ) -> Result<()> {
        if recorder.recorder.is_active() {
            warn!("{context}: recorder already active before start; forcing stale-session stop");
            recorder
                .stop_and_discard_path()
                .await
                .with_context(|| format!("{context}: failed stale-session stop"))?;
            info!("{context}: stale recorder stopped before start");
        }

        Self::clear_recorder_callbacks(recorder);
        Ok(())
    }

    /// Atomically reset the full set of session-lifecycle fields owned by the
    /// controller and flip `state` to Idle as the final mutation.
    ///
    /// This is the single source of truth for which fields constitute "session
    /// state" so the various reset entry points (start-failure, finished
    /// recording, toggle-stop, nuclear reset) can no longer drift apart in the
    /// subset of fields they clear (P3.1). Each caller keeps its own UI /
    /// telemetry / status-string tail.
    ///
    /// Ordering note (P2.2): every satellite flag is cleared before
    /// `set_state(State::Idle)` so cross-thread readers (e.g. the VAD monitor
    /// polling `current_state`) never observe Idle alongside stale flags.
    async fn reset_session_fields(&self) {
        *self.assistive_mode.write().await = false;
        *self.hold_mode.write().await = HoldMode::Raw;
        *self.force_raw_mode.write().await = false;
        *self.force_ai_mode.write().await = false;
        *self.session_id.write().await = None;
        // Every path back to Idle ends the Bus session exactly once (text-free
        // lifecycle line), so an observer can tell "the take is over" apart
        // from "the take is live" even when zero occurrences sealed.
        let ended_bus = self.active_transcript_bus.write().await.take();
        if let Some(bus) = ended_bus {
            bus.publish_ended();
        }
        *self.assistive_context.write().await = None;
        *self.pre_overlay_frontmost_app.write().await = None;
        self.start_transition_in_flight
            .store(false, Ordering::SeqCst);
        self.assistive_loop_active.store(false, Ordering::SeqCst);
        self.toggle_user_has_text.store(false, Ordering::SeqCst);
        self.toggle_assistant_has_text
            .store(false, Ordering::SeqCst);
        // `state` becomes Idle only once the rest of the session state is consistent.
        self.set_state(State::Idle).await;
    }

    /// Unwind session state after a start that never produced a recording, so a
    /// failed start leaves nothing behind for the next hotkey press.
    async fn reset_session_after_start_failure(&self, context: &str) {
        warn!("{context}: resetting controller flags after failed start");
        self.reset_session_fields().await;
        set_assistive_session(false);
        reset_session_telemetry(&self.session_telemetry);
    }

    /// Unwind session state after a recording that completed. Telemetry is kept
    /// (unlike the start-failure path) — the finished session's stats are still
    /// being read by the result handler.
    async fn reset_finished_recording_state(&self) {
        self.reset_session_fields().await;
        set_assistive_session(false);
    }

    /// Post-pipeline epilogue: log the commit decision on success, and surface a
    /// failure to the user instead of leaving it in the log.
    ///
    /// On success it also hands freed pages back to the OS while the app is
    /// idle, so a long session does not accumulate footprint across recordings.
    /// A failure is routed through the engine `Warning` channel, which the
    /// bridge turns into a visible error and a tray state — a silently failed
    /// transcription reads to the user as a lost recording.
    async fn handle_processed_recording_result(
        &self,
        assistive: bool,
        result: &Result<ProcessRecordingOutcome>,
    ) {
        match result {
            Ok(outcome) => {
                info!("Processing finished successfully. State reset to IDLE.");

                // The transcription just freed large transient buffers (audio,
                // mel, model scratch). Hand those freed-but-retained pages back
                // to the OS now, while idle, instead of letting phys_footprint
                // creep up across a long session.
                codescribe_core::memory::release_freed_heap();

                if let Some(reason) = outcome.no_speech_reason.as_deref() {
                    info!("NoSpeech outcome in finish_recording: reason={reason}");
                } else if !assistive {
                    let cfg = self.get_config().await;

                    if outcome.transcript_present
                        && cfg.transcription_overlay_enabled
                        && !(cfg.quick_notes_enabled && cfg.quick_notes_save_only)
                    {
                        let reason = outcome
                            .commit_trigger
                            .as_deref()
                            .unwrap_or("transcript_present");
                        info!("COMMIT decision: trigger={reason}");
                    } else if cfg.quick_notes_enabled && cfg.quick_notes_save_only {
                        info!("COMMIT decision: skipped (quick_notes_save_only)");
                    } else {
                        info!("COMMIT decision: skipped (delivery conditions not met)");
                    }
                }
            }
            Err(e) => {
                error!("Processing failed: {}", e);
                // Surface the failure to the user instead of leaving it as a
                // log-only event. Reuse the existing engine `Warning` channel:
                // the bridge forwarder (forward_event_to_listener) turns it into
                // `listener.on_error(...)` + a tray Error state, so the SwiftUI
                // surface reflects the failed transcription.
                let _ = self.event_broadcast.send(IpcEvent {
                    timestamp: chrono::Utc::now()
                        .to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
                    payload: IpcEventPayload::Engine(EngineEventWire::Warning {
                        code: "transcription_failed".to_string(),
                        message: format!("Transcription failed: {e}"),
                    }),
                });
            }
        }
    }

    /// Recognize the recorder's "already in progress" refusal, which is the one
    /// start failure worth a single force-stop-and-retry rather than an abort.
    fn is_already_in_progress_error(error: &anyhow::Error) -> bool {
        error
            .to_string()
            .contains("Recording is already in progress")
    }

    /// Reconcile a controller that believes it is `Idle` with a recorder that is
    /// still streaming, by force-stopping the orphaned stream.
    ///
    /// The in-flight start flag is checked twice — before and after taking the
    /// serial lock — because a session that is mid-start legitimately looks like
    /// "Idle plus an active recorder", and killing it there would turn recovery
    /// into the very bug it exists to fix.
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
        if let Err(e) = recorder.stop_and_discard_path().await {
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
        info!("RECOVERY decision: stale active stream cleared, controller remains IDLE");
    }

    /// Fan one pipeline event stream out to the three consumers a session needs:
    /// the presentation emitter (transcript assembly, optional preview deltas),
    /// the IPC broadcast, and session telemetry.
    ///
    /// Preview deltas are wired only when something is actually watching them;
    /// otherwise the delta sink is absent rather than emitting into the void.
    fn build_recording_event_sink(
        transcript_buffer: Arc<tokio::sync::Mutex<String>>,
        preview_deltas_enabled: bool,
        event_broadcast: broadcast::Sender<IpcEvent>,
        session_telemetry: SharedSessionTelemetry,
        transcript_bus: Option<Arc<TranscriptBus>>,
        acoustic_ledger: Option<
            Arc<std::sync::Mutex<codescribe_core::pipeline::acoustic_ledger::AcousticLedger>>,
        >,
    ) -> Arc<dyn codescribe_core::pipeline::contracts::EventSink> {
        let delta_sink = preview_deltas_enabled.then(|| {
            Arc::new(helpers::RoutingDeltaSink)
                as Arc<dyn codescribe_core::pipeline::contracts::DeltaSink>
        });
        let projection_broadcast = event_broadcast.clone();
        let projection_callback = Arc::new(
            move |event: &crate::presentation::transcript_bus::TranscriptBusEvidenceEvent| {
                match serde_json::to_string(event) {
                    Ok(json) => {
                        let _ = projection_broadcast.send(IpcEvent {
                            timestamp: chrono::Utc::now()
                                .to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
                            payload: IpcEventPayload::TranscriptProjection { json },
                        });
                    }
                    Err(error) => {
                        tracing::warn!(%error, "transcript projection serialization failed");
                    }
                }
            },
        );
        let pe: Arc<dyn codescribe_core::pipeline::contracts::EventSink> =
            Arc::new(PresentationEmitter::new_with_authority(
                transcript_buffer,
                delta_sink,
                None,
                transcript_bus,
                acoustic_ledger,
                Some(projection_callback),
            ));
        let ipc_sink: Arc<dyn codescribe_core::pipeline::contracts::EventSink> =
            Arc::new(helpers::IpcBroadcastSink::new(event_broadcast));
        let telemetry_sink: Arc<dyn codescribe_core::pipeline::contracts::EventSink> =
            Arc::new(helpers::SessionTelemetrySink::new(session_telemetry));
        Arc::new(codescribe_core::pipeline::sinks::FanoutEventSink::new(
            vec![pe, ipc_sink, telemetry_sink],
        ))
    }

    /// Feed the overlay's live level meter without doing allocation or timestamp
    /// formatting on CoreAudio's capture thread. The callback only attempts a
    /// bounded, non-blocking send; the controller worker constructs and
    /// broadcasts the typed IPC event. When the worker is behind, the new sample
    /// is dropped instead of growing a queue or delaying audio capture.
    fn configure_level_broadcast(
        recorder: &mut StreamingRecorder,
        event_broadcast: broadcast::Sender<IpcEvent>,
    ) {
        let (level_tx, mut level_rx) = mpsc::channel::<f32>(AUDIO_LEVEL_QUEUE_CAPACITY);
        recorder.set_level_callback(Some(Arc::new(move |rms| {
            let _ = level_tx.try_send(rms);
        })));

        tokio::spawn(async move {
            while let Some(rms) = level_rx.recv().await {
                // Cleanup drops the callback sender. Do not drain a buffered
                // sample after that boundary: it belongs to the closed session
                // and must never animate a subsequently prepared overlay.
                if level_rx.is_closed() {
                    break;
                }
                let _ = event_broadcast.send(IpcEvent {
                    timestamp: chrono::Utc::now()
                        .to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
                    payload: IpcEventPayload::AudioLevel { rms },
                });
            }
        });
    }

    /// Wire level metering and the event sink for a hold session. Hold has no
    /// utterance callback: text is finalized on key-up in `finish_recording`.
    fn configure_hold_event_sink(
        recorder: &mut StreamingRecorder,
        preview_deltas_enabled: bool,
        event_broadcast: broadcast::Sender<IpcEvent>,
        session_telemetry: SharedSessionTelemetry,
        transcript_bus: Option<Arc<TranscriptBus>>,
    ) {
        Self::configure_level_broadcast(recorder, event_broadcast.clone());
        let acoustic_ledger = recorder.acoustic_ledger_handle();
        recorder.set_event_sink(Some(Self::build_recording_event_sink(
            recorder.transcript_buffer_handle(),
            preview_deltas_enabled,
            event_broadcast,
            session_telemetry,
            transcript_bus,
            acoustic_ledger,
        )));
    }

    /// Wire level metering and the event sink for a toggle / hands-off session,
    /// which is ONE continuous recorder session (ADR 2026-05-28 Faza 1).
    fn configure_toggle_event_sink(
        recorder: &mut StreamingRecorder,
        preview_deltas_enabled: bool,
        _flush_voice_chat_on_vad_end: bool,
        event_broadcast: broadcast::Sender<IpcEvent>,
        session_telemetry: SharedSessionTelemetry,
        transcript_bus: Option<Arc<TranscriptBus>>,
    ) {
        // Hands-off is ONE continuous recorder session (ADR 2026-05-28 Faza 1).
        // Normal hands-off uses cumulative SessionRendered deltas in the transcription overlay.
        //
        // Assistive hands-off is intentionally callback-driven: every finalized utterance
        // appends into the current chat user bubble, and VAD end commits that bubble to the
        // agent without stopping the recorder. Do not route assistive live preview deltas
        // into the same bubble, or previews and finals will duplicate.
        Self::configure_level_broadcast(recorder, event_broadcast.clone());
        let acoustic_ledger = recorder.acoustic_ledger_handle();
        recorder.set_event_sink(Some(Self::build_recording_event_sink(
            recorder.transcript_buffer_handle(),
            preview_deltas_enabled,
            event_broadcast,
            session_telemetry,
            transcript_bus,
            acoustic_ledger,
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

        if should_block_hotkey_during_agent_send(
            current_state,
            &event,
            helpers::is_agent_send_in_flight(),
        ) {
            info!("Agent response is still streaming; ignoring hotkey start");
            return Ok(());
        }

        if current_state == State::Idle
            && event.key_type == HotkeyType::Hold
            && matches!(event.action, HotkeyAction::Down)
        {
            // Leftovers from a previous session are archived, never destroyed
            // (operator law 2026-07-21: reproducible moment-of-truth in store).
            match self
                .context_bucket
                .lock()
                .await
                .archive_and_reset("session-start-discard")
            {
                Ok(Some(dir)) => info!("Context bucket archived: {}", dir.display()),
                Ok(None) => {}
                Err(err) => warn!("Context bucket archive failed (items kept): {err:#}"),
            }
        }

        // Update mode flags from event (supports mid-hold mode changes via Press events).
        // A toggle press while already in RecToggle means "stop this session"; it must not
        // rewrite the active session identity with the key that happened to stop it.
        if should_apply_incoming_mode_flags(current_state, &event) {
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
                                set_assistive_session(false);
                            }
                        }
                        HoldMode::Chat => {
                            *self.assistive_mode.write().await = true;
                            *self.force_raw_mode.write().await = false;
                            *self.force_ai_mode.write().await = false;
                            let prior_frontmost_app =
                                self.pre_overlay_frontmost_app.read().await.clone();
                            let ctx = self
                                .capture_assistive_combo_context(current_state, prior_frontmost_app)
                                .await;
                            *self.assistive_context.write().await = Some(ctx);

                            if matches!(current_state, State::RecHold | State::RecToggle) {
                                publish_recording_indicator(
                                    BadgeMode::Assistive,
                                    self.get_config().await.hold_indicator,
                                );
                            }
                        }
                        HoldMode::Selection => {
                            *self.assistive_mode.write().await = true;
                            *self.force_raw_mode.write().await = false;
                            *self.force_ai_mode.write().await = false;
                            let prior_frontmost_app =
                                self.pre_overlay_frontmost_app.read().await.clone();
                            let ctx = self
                                .capture_assistive_combo_context(current_state, prior_frontmost_app)
                                .await;
                            *self.assistive_context.write().await = Some(ctx);

                            if matches!(current_state, State::RecHold | State::RecToggle) {
                                publish_recording_indicator(
                                    BadgeMode::Assistive,
                                    self.get_config().await.hold_indicator,
                                );
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
        } else if matches!(event.action, HotkeyAction::Press)
            && event.key_type == HotkeyType::Toggle
            && current_state == State::RecToggle
        {
            debug!(
                "Preserving active toggle session flags during stop event (event assistive={} force_raw={} force_ai={})",
                event.assistive, event.force_raw, event.force_ai
            );
        }

        // Ignore all hotkeys when busy. `State::Busy` covers the active audio
        // pipeline: recorder drain → transcription → (for the hold/toggle
        // dictation path) the final assistive agent turn, which is awaited while
        // `serial_lock` is held. Letting a second start through here would race a
        // live audio/transcription pipeline, so it stays blocked unconditionally
        // (acceptance: "non-assistive busy/audio/transcription paths remain
        // protected; do not run two audio pipelines concurrently").
        //
        // Assistive "Talk Anytime" is handled one gate up, at the `Idle` agent-
        // send gate (`should_block_hotkey_during_agent_send`): once a turn is
        // dispatched in the background the controller returns to `Idle` and the
        // mic is free, which is the only state where overlapping a new recording
        // is safe.
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
                    self.schedule_hold_start(event.assistive).await?;
                    // Fn down with a live OS selection attaches `{selection_1}`
                    // immediately. Mid-hold arm pulses add `{selection_2..n}`.
                    // Destination stays dictation — do not arm Chat/Agent.
                    if !event.assistive && matches!(event.hold_mode, HoldMode::Raw) {
                        self.attach_hold_selection().await?;
                    }
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
                info!("Toggle pressed; entering stop flow (state=REC_TOGGLE)");
                self.assistive_loop_active.store(false, Ordering::SeqCst);
                self.stop_toggle_and_adjudicate().await?;
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
            State::Busy => {
                warn!(
                    "Toggle pressed while previous stop is still processing (state=BUSY). \
                     If recording badge persists, stop watchdog will force recovery within {}s.",
                    STOP_TIMEOUT.as_secs()
                );
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
                            return Err(e);
                        }
                        *engine_guard = Some(engine);
                        info!("ConversationEngine initialized successfully");
                    }
                    Err(e) => {
                        error!("Failed to create ConversationEngine: {}", e);
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
    // allow(too_many_arguments): spawn boundary of the conversation loop — each
    // Arc/channel is moved into the task; bundling into a struct would hide
    // which shared handles cross the thread boundary.
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
                    // Full cleanup on failure: state, session flag, badge
                    Self::set_state_with_broadcast(&state, &event_broadcast, State::Idle).await;
                    helpers::set_conversation_session(false);
                    codescribe_core::memory::release_freed_heap();
                    return;
                }
            };
            rec.recorder.set_callback(Box::new(move |data: &[f32]| {
                let _ = tx_clone.try_send(data.to_vec());
            }));

            if let Err(e) = rec.recorder.start().await {
                error!("Failed to start recorder for conversation: {}", e);
                // Full cleanup on failure: state, session flag, badge
                Self::set_state_with_broadcast(&state, &event_broadcast, State::Idle).await;
                helpers::set_conversation_session(false);
                codescribe_core::memory::release_freed_heap();
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
                    codescribe_core::memory::release_freed_heap();
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
                    if let Some(ref mut eng) = *engine_guard
                        && let Err(e) = eng.process_audio_any_rate(&samples, sample_rate)
                    {
                        warn!("ConversationEngine.process_audio error: {}", e);
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

                    // Play response audio in separate blocking task (non-blocking for loop)
                    // This preserves full-duplex: we can still process mic while playing
                    let player_clone = Arc::clone(&player);
                    let playback_active_clone = Arc::clone(&playback_active);

                    let handle = tokio::runtime::Handle::current();
                    // Run the playback body on a blocking worker. catch_unwind is
                    // placed INSIDE the closure so it actually wraps the playback
                    // body that runs on the worker thread (the previous version
                    // wrapped only the spawn_blocking() call, which never panics
                    // synchronously, so a panic in p.play()/block_on/UI update was
                    // never caught). On Err we log the panic payload as the root
                    // cause (P1.2).
                    //
                    // Reliability caveat: under panic="abort" (release builds) a
                    // panic aborts the process before catch_unwind or the
                    // PlaybackGuard Drop can run, so this recovery is effective
                    // only under panic="unwind" (debug/tests). The real fix for
                    // the release crash symptom is owned by the panic group
                    // (panic hook P0.1 + abort/unwind decision P1.1).
                    tokio::task::spawn_blocking(move || {
                        // Resets playback_active when this scope exits (also on an
                        // unwinding panic; NOT under panic="abort", see above).
                        /// Clears `playback_active` on exit (unwind path; not panic=abort).
                        struct PlaybackGuard(Arc<AtomicBool>);
                        impl Drop for PlaybackGuard {
                            /// Clear the playback-active flag when play scope ends.
                            fn drop(&mut self) {
                                self.0.store(false, Ordering::SeqCst);
                            }
                        }
                        let _guard = PlaybackGuard(Arc::clone(&playback_active_clone));

                        let body = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                            // Block this thread for playback, but don't block the async loop
                            let player_guard = handle.block_on(player_clone.lock());
                            if let Some(ref p) = *player_guard
                                && let Err(e) = p.play(&response_samples, response_rate)
                            {
                                warn!("AudioPlayer.play error: {}", e);
                            }
                        }));

                        if let Err(panic_payload) = body {
                            let root_cause = panic_payload
                                .downcast_ref::<&str>()
                                .map(|s| s.to_string())
                                .or_else(|| panic_payload.downcast_ref::<String>().cloned())
                                .unwrap_or_else(|| "<non-string panic payload>".to_string());
                            warn!(
                                "Playback task panicked (root cause: {root_cause}); \
                                 playback_active reset by guard"
                            );
                        }
                        // _guard dropped here, resetting playback_active.
                    });
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
            // Return freed host memory to the OS after a conversation session
            // (the dictation stop path already does this; conversation exits did
            // not, leaving malloc retention). Memory-lifecycle only.
            codescribe_core::memory::release_freed_heap();
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
        // Return freed host memory after a conversation session (see note above).
        codescribe_core::memory::release_freed_heap();
        info!("STATE TRANSITION: CONVERSATION → IDLE");

        Ok(())
    }

    /// Schedule delayed recording start for hold mode
    async fn schedule_hold_start(&self, assistive: bool) -> Result<()> {
        // Scheduling selects the take generation. Refresh and every actual
        // start/stop transition cross this same boundary, while the spawned
        // task itself is never awaited under the guard.
        let _scheduling_guard = self.serial_lock.lock().await;

        // Cancel any existing delayed start before selecting the next Arc.
        self.cancel_pending_hold_start().await;
        let task_generation = self.hold_start_generation.load(Ordering::SeqCst);
        let runtime_settings = self.runtime_settings_arc().await;
        let config = runtime_settings.values().clone();

        // Hold mode never runs the assistive loop
        self.assistive_loop_active.store(false, Ordering::SeqCst);
        let configured_delay_ms = config.hold_start_delay_ms;
        let delay_ms = effective_hold_start_delay_ms(configured_delay_ms, assistive);
        let beep = config.beep_on_start;
        let sound_volume = config.sound_volume;
        let language = config.whisper_language;

        let hold_mode = Arc::clone(&self.hold_mode);

        debug!(
            "Scheduling hold-start after {}ms delay (configured={}ms, assistive={}, hold_mode={:?})",
            delay_ms,
            configured_delay_ms,
            assistive,
            *hold_mode.read().await
        );

        *self.pending_assistive_context.write().await = None;
        let initial_hold_mode = *hold_mode.read().await;
        let trigger_context = if matches!(initial_hold_mode, HoldMode::Chat | HoldMode::Selection) {
            self.assistive_context
                .read()
                .await
                .clone()
                .unwrap_or_default()
        } else {
            let prior = self.pre_overlay_frontmost_app.read().await.clone();
            tokio::task::spawn_blocking(move || {
                capture_frontmost_app_only_with_prior_frontmost(prior)
            })
            .await
            .unwrap_or_default()
        };
        *self.pre_overlay_frontmost_app.write().await = trigger_context.frontmost_app.clone();
        *self.assistive_context.write().await = Some(trigger_context);

        // Reset VAD flag for new session
        self.vad_triggered.store(false, Ordering::SeqCst);

        let state = Arc::clone(&self.state);
        let session_id = Arc::clone(&self.session_id);
        let recorder = Arc::clone(&self.recorder);
        let delay = Duration::from_millis(delay_ms);
        let vad_flag = Arc::clone(&self.vad_triggered);
        let event_broadcast = self.event_broadcast.clone();
        let serial_lock = Arc::clone(&self.serial_lock);
        let hold_start_generation = Arc::clone(&self.hold_start_generation);
        let start_transition_in_flight = Arc::clone(&self.start_transition_in_flight);
        let session_telemetry = Arc::clone(&self.session_telemetry);
        let active_transcript_bus = Arc::clone(&self.active_transcript_bus);

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

            if hold_start_generation.load(Ordering::SeqCst) != task_generation {
                debug!("Hold-start cancelled: superseded generation before recorder start");
                return;
            }

            let current_state = *state.read().await;
            if current_state != State::Idle {
                debug!(
                    "Hold-start cancelled before recorder start: state changed to {}",
                    current_state
                );
                return;
            }

            let _start_guard = AtomicFlagGuard::new(Arc::clone(&start_transition_in_flight));

            // Generate session ID
            let new_session_id = Uuid::new_v4().to_string();
            *session_id.write().await = Some(new_session_id.clone());

            info!("Starting hold recording (session={})", new_session_id);

            let hold_mode = *hold_mode.read().await;
            let is_assistive = matches!(hold_mode, HoldMode::Chat | HoldMode::Selection);
            // Cursor-following recording badge (config-gated): red for hold dictation,
            // purple for assistive/agent. Works headless — no overlay needed.
            publish_recording_indicator(
                if is_assistive {
                    BadgeMode::Assistive
                } else {
                    BadgeMode::Hold
                },
                config.hold_indicator,
            );
            let overlay_enabled = apply_runtime_transcription_profile(
                &config,
                runtime_settings.user_settings(),
                is_assistive,
            );

            // Apple live must-have: refuse start before audio when Speech is not
            // ready (empty mid-take death is not an acceptable product mode).
            // Runs BEFORE the recorder lock and on the blocking pool: the probe
            // spawns a bridge child and can block on the Speech TCC dialog for
            // as long as the user takes — holding the recorder mutex (or a
            // runtime worker) for that window froze stop/tray/second-hotkey.
            if !cfg!(test) {
                let preflight =
                    tokio::task::spawn_blocking(codescribe_core::stt::preflight_apple_live_ready)
                        .await
                        .unwrap_or_else(|join| {
                            Err(anyhow::anyhow!("Apple STT preflight task panicked: {join}"))
                        });
                if let Err(e) = preflight {
                    error!("Hold-start aborted (Apple STT preflight): {e:#}");
                    *session_id.write().await = None;
                    set_assistive_session(false);
                    crate::os::hold_badge::hide_hold_badge();
                    return;
                }
            }

            // Acoustic admission must-have (same gate as toggle): refuse before
            // the recorder lock and before any microphone opens. Hold has no
            // return channel to the UI, so the refusal rides the engine
            // Warning channel the bridge forwards as a terminal error.
            if !cfg!(test) {
                // `.clone()` (not `Arc::clone`) on purpose: C15D counts one
                // recorder binding per start body; this is the same Arc.
                let admission_settings = runtime_settings.clone();
                let verdict = tokio::task::spawn_blocking(move || {
                    admission::evaluate_live_admission_arc(&admission_settings)
                })
                .await
                .unwrap_or_else(|join| {
                    Err(admission::AdmissionBlocker::CaptureDeviceUnavailable {
                        reason: format!("admission probe panicked: {join}"),
                    })
                });
                match verdict {
                    Ok(grant) => info!(
                        device = %grant.device_name,
                        sample_rate = grant.sample_rate,
                        calibration_version = %grant.calibration_version,
                        "acoustic admission granted for hold start"
                    ),
                    Err(blocker) => {
                        error!("Hold-start refused (acoustic admission): {blocker}");
                        Self::broadcast_admission_refusal(&event_broadcast, &blocker);
                        *session_id.write().await = None;
                        set_assistive_session(false);
                        crate::os::hold_badge::hide_hold_badge();
                        return;
                    }
                }
            }

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
            // Hold-to-talk: the key-down is the source of truth. Don't auto-stop
            // the session mid-hold. Silence still closes an SFSpeech epoch so
            // Layer 1 can be fed — same knob as toggle (`TOGGLE_SILENCE_SEC`).
            rec.recorder.config.auto_silence = false;
            rec.set_utterance_silence_sec(Some(config.toggle_silence_sec));
            rec.recorder.set_on_vad_stop(move || {
                info!("VAD callback: setting vad_triggered flag");
                vad_flag.store(true, Ordering::SeqCst);
            });

            // Set session mode for delta routing BEFORE starting the pipeline,
            // so the very first deltas route to the correct overlay.
            set_assistive_session(is_assistive);
            reset_session_telemetry(&session_telemetry);
            rec.bind_session_authority(
                new_session_id.clone(),
                Arc::clone(&runtime_settings),
            );
            let transcript_bus = TranscriptBus::open(TranscriptSession {
                session_id: new_session_id,
                mode: if is_assistive {
                    TranscriptMode::Assistive
                } else {
                    TranscriptMode::Dictation
                },
            })
            .map(Arc::new);

            // Runtime pipeline is always event-based. Hold mode has no utterance callback;
            // text is finalized on key-up in `finish_recording`.
            Self::configure_hold_event_sink(
                rec,
                is_assistive || overlay_enabled,
                event_broadcast.clone(),
                Arc::clone(&session_telemetry),
                transcript_bus.clone(),
            );
            if !cfg!(test) {
                let language_hint = language.whisper_hint().map(str::to_string);
                // Audio-first cold start: do not preflight Whisper here. The
                // recorder starts feedback now while STT lazy-loads behind the
                // StreamingRecorder backlog.
                let start_result = rec.start_event_session(language_hint.clone()).await;
                if let Err(e) = start_result {
                    if Self::is_already_in_progress_error(&e) {
                        warn!("Hold-start hit stale recorder lock; forcing stop and retrying once");
                        if let Err(stop_err) = rec.stop_and_discard_path().await {
                            warn!("Hold-start stale-recorder recovery failed: {stop_err}");
                        }
                        Self::clear_recorder_callbacks(rec);
                        Self::configure_hold_event_sink(
                            rec,
                            is_assistive || overlay_enabled,
                            event_broadcast.clone(),
                            Arc::clone(&session_telemetry),
                            transcript_bus.clone(),
                        );
                        let retry_result = rec.start_event_session(language_hint).await;
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

            *active_transcript_bus.write().await = transcript_bus.clone();
            if let Some(bus) = &transcript_bus {
                bus.publish_started();
            }

            if hold_start_generation.load(Ordering::SeqCst) != task_generation {
                warn!("Hold-start superseded after recorder start; stopping stale session");
                if rec.recorder.is_active()
                    && let Err(stop_err) = rec.stop_and_discard_path().await
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
        });

        *self.hold_start_task.lock().await = Some(task);
        Ok(())
    }

    /// Start recording in toggle mode (immediate, no delay)
    async fn start_toggle_recording(&self, is_assistive: bool) -> Result<()> {
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
        let runtime_settings = self.runtime_settings_arc().await;
        let config = runtime_settings.values();
        let _start_guard = AtomicFlagGuard::new(Arc::clone(&self.start_transition_in_flight));

        *self.pending_assistive_context.write().await = None;
        match self
            .context_bucket
            .lock()
            .await
            .archive_and_reset("session-start-discard")
        {
            Ok(Some(dir)) => info!("Context bucket archived: {}", dir.display()),
            Ok(None) => {}
            Err(err) => warn!("Context bucket archive failed (items kept): {err:#}"),
        }
        let trigger_context = if is_assistive {
            tokio::task::spawn_blocking(capture_assistive_context)
                .await
                .unwrap_or_default()
        } else {
            let prior = self.pre_overlay_frontmost_app.read().await.clone();
            tokio::task::spawn_blocking(move || {
                capture_frontmost_app_only_with_prior_frontmost(prior)
            })
            .await
            .unwrap_or_default()
        };
        *self.pre_overlay_frontmost_app.write().await = trigger_context.frontmost_app.clone();
        *self.assistive_context.write().await = Some(trigger_context);

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

        // Cursor-following recording badge (config-gated): pulsing red for toggle /
        // hands-off, purple for assistive/agent.
        publish_recording_indicator(
            if is_assistive {
                BadgeMode::Assistive
            } else {
                BadgeMode::Toggle
            },
            config.hold_indicator,
        );
        let language = config.whisper_language;
        let toggle_silence_sec = config.toggle_silence_sec;
        let beep_enabled = config.beep_on_start;
        let sound_volume = config.sound_volume;
        let overlay_enabled = apply_runtime_transcription_profile(
            config,
            runtime_settings.user_settings(),
            is_assistive,
        );

        // Apple live must-have preflight, BEFORE the recorder lock and on the
        // blocking pool: the probe spawns a bridge child and can block on the
        // Speech TCC dialog indefinitely — holding the recorder mutex (or a
        // runtime worker) for that window froze every other recorder surface.
        if !cfg!(test) {
            let preflight =
                tokio::task::spawn_blocking(codescribe_core::stt::preflight_apple_live_ready)
                    .await
                    .unwrap_or_else(|join| {
                        Err(anyhow::anyhow!("Apple STT preflight task panicked: {join}"))
                    });
            if let Err(e) = preflight {
                // Must log the actual cause — silent "resetting flags" made padaka undiagnosable.
                error!("Toggle-start aborted (Apple STT preflight): {e:#}");
                self.reset_session_after_start_failure("Toggle-start Apple STT preflight")
                    .await;
                return Err(e);
            }
        }

        // Acoustic admission must-have: a take whose occurrences can never
        // qualify (no measured calibration, seal lane disarmed) must be refused
        // HERE, before the recorder lock and before any microphone opens — not
        // recorded into a WAV that grows while the Bus stays on session_started.
        if !cfg!(test) {
            match self.admission_readiness().await {
                Ok(grant) => info!(
                    device = %grant.device_name,
                    sample_rate = grant.sample_rate,
                    calibration_version = %grant.calibration_version,
                    "acoustic admission granted for toggle start"
                ),
                Err(blocker) => {
                    error!("Toggle-start refused (acoustic admission): {blocker}");
                    Self::broadcast_admission_refusal(&self.event_broadcast, &blocker);
                    self.reset_session_after_start_failure("Toggle-start admission")
                        .await;
                    return Err(anyhow::anyhow!("{blocker}"));
                }
            }
        }

        // Start the recorder
        let mut recorder_guard = self.recorder.lock().await;
        let recorder = match Self::recorder_from_guard_mut(&mut recorder_guard, "Toggle-start") {
            Ok(recorder) => recorder,
            Err(error) => {
                drop(recorder_guard);
                self.reset_session_after_start_failure("Toggle-start").await;
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
        recorder.bind_session_authority(
            new_session_id.clone(),
            Arc::clone(&runtime_settings),
        );
        let transcript_bus = TranscriptBus::open(TranscriptSession {
            session_id: new_session_id,
            mode: if is_assistive {
                TranscriptMode::Agent
            } else {
                TranscriptMode::Dictation
            },
        })
        .map(Arc::new);

        // Runtime pipeline is always event-based.
        Self::configure_toggle_event_sink(
            recorder,
            overlay_enabled,
            is_assistive,
            self.event_broadcast.clone(),
            Arc::clone(&self.session_telemetry),
            transcript_bus.clone(),
        );
        // Skip actual audio stream in tests (no CoreAudio device needed)
        let language_hint = language.whisper_hint().map(str::to_string);
        // Audio-first cold start: do not preflight Whisper here. The recorder
        // starts feedback now while STT lazy-loads behind the StreamingRecorder backlog.
        if !cfg!(test)
            && let Err(e) = recorder.start_event_session(language_hint.clone()).await
        {
            if Self::is_already_in_progress_error(&e) {
                warn!("Toggle start hit stale recorder lock; forcing stop and retrying once");
                if let Err(stop_err) = recorder.stop_and_discard_path().await {
                    warn!("Toggle stale-recorder recovery failed: {stop_err}");
                }
                Self::clear_recorder_callbacks(recorder);
                Self::configure_toggle_event_sink(
                    recorder,
                    overlay_enabled,
                    is_assistive,
                    self.event_broadcast.clone(),
                    Arc::clone(&self.session_telemetry),
                    transcript_bus.clone(),
                );
                if let Err(retry_err) = recorder.start_event_session(language_hint).await {
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
        *self.active_transcript_bus.write().await = transcript_bus.clone();
        if let Some(bus) = &transcript_bus {
            bus.publish_started();
        }
        drop(recorder_guard);

        // Transition to REC_TOGGLE immediately after recorder starts.
        self.set_state(State::RecToggle).await;
        info!("STATE TRANSITION: IDLE → REC_TOGGLE (pulsing badge)");

        // Reset incremental segment marker — the next Commit/Augment clips
        // from sample 0 of this new toggle session, not from any leftover
        // offset of a prior session.
        self.last_segment_audio_offset.store(0, Ordering::SeqCst);

        // Play start beep if enabled
        if beep_enabled {
            crate::audio::play_sound_with_volume("Tink", sound_volume);
        }

        Ok(())
    }

    /// Stop a toggle session under a watchdog.
    ///
    /// The full stop — recorder drain, live-session adjudication, post-process,
    /// delivery — must finish within `STOP_TIMEOUT`. When it does not (lock
    /// contention, a `stop` blocked in a CoreAudio
    /// callback), recovery forces `Idle` so the next toggle press registers, the
    /// badge clears, and the tray stops claiming idle over a hung recording.
    async fn stop_toggle_and_adjudicate(&self) -> Result<()> {
        if *self.state.read().await != State::RecToggle {
            return Ok(());
        }

        // Watchdog: full stop+adjudicate (recorder.stop + live truth + post-process
        // + paste) must complete within STOP_TIMEOUT. If it stalls — RwLock
        // contention or recorder.stop blocked on a cpal callback —
        // force recovery to Idle so subsequent toggle presses register, badge clears,
        // and tray reflects truth instead of showing Idle while recording is hung.
        match tokio::time::timeout(STOP_TIMEOUT, self.stop_toggle_and_adjudicate_inner()).await {
            Ok(result) => result,
            Err(_) => {
                error!(
                    "Toggle stop+adjudicate stalled >{}s — forcing recovery to Idle. \
                     Recording session abandoned; future toggle presses will start fresh.",
                    STOP_TIMEOUT.as_secs()
                );
                self.recover_from_stuck_stop().await;
                Err(anyhow::anyhow!(
                    "Toggle stop timeout after {}s; state forced to Idle",
                    STOP_TIMEOUT.as_secs()
                ))
            }
        }
    }

    /// The stop body the watchdog above wraps.
    ///
    /// Phases are timed and logged at `info!` on purpose: the watchdog could say
    /// *that* a stop hung but never *where*, and `debug!` is filtered out in
    /// release, exactly where the hang was reported. The phase numbers in the
    /// log lines are the diagnostic contract.
    ///
    /// The session-id snapshot before the rename is a self-deadlock guard: under
    /// Rust 2024 a read guard held as an if-let scrutinee outlives the body and
    /// would block this same task's write.
    async fn stop_toggle_and_adjudicate_inner(&self) -> Result<()> {
        // Phase-timed instrumentation: the watchdog above wraps this entire fn
        // in STOP_TIMEOUT, but until now we couldn't tell WHICH await hung.
        // Operator reported "hands-off, double option, który potrafi wywołać
        // nagrywanie, ale nie potrafi zakończyć nagrywania" — confirmed in
        // ~/.codescribe/logs/codescribe.log @ 2026-05-13 23:03:22 PDT
        // where "Stopping toggle recording with final-pass adjudication" was
        // followed by 41s of silence before watchdog forced recovery.
        // These per-phase elapsed logs will identify the exact hang point next
        // time it reproduces. Logs MUST stay info! so they survive at default
        // tracing level — debug! gets filtered out in release.
        let stop_start = std::time::Instant::now();
        info!("stop_toggle_inner: PHASE 0 — acquiring serial_lock");
        let _guard = self.serial_lock.lock().await;
        info!(
            "stop_toggle_inner: PHASE 0 — serial_lock acquired in {:?}",
            stop_start.elapsed()
        );

        if *self.state.read().await != State::RecToggle {
            return Ok(());
        }

        info!("Stopping toggle recording with final-pass adjudication");

        let assistive = *self.assistive_mode.read().await;

        // Self-deadlock guard (Rust 2024): the read guard temporary from an
        // if-let chain scrutinee outlives the chain body. Inlining the read
        // would keep the guard alive across `.write().await`, blocking the
        // write on this same task's read guard → STOP_TIMEOUT hang reproduced in
        // ~/.codescribe/logs/codescribe.log 2026-05-14T00:16:23 (PHASE 1
        // never reached; watchdog forced recovery). Materialize the snapshot
        // first so the read guard drops at the semicolon.
        let session_id_snapshot = self.session_id.read().await.clone();
        if let Some(session_id) = session_id_snapshot {
            *self.session_id.write().await = Some(format!("{session_id}:stopping"));
        }

        self.set_state(State::Busy).await;
        self.show_processing_badge_if_enabled().await;

        let (result, rec_stop_secs, phase3_secs) = {
            let phase1 = std::time::Instant::now();
            info!("stop_toggle_inner: PHASE 1 — locking recorder mutex");
            let mut recorder_guard = self.recorder.lock().await;
            info!(
                "stop_toggle_inner: PHASE 1 — recorder mutex acquired in {:?}",
                phase1.elapsed()
            );

            let recorder = Self::recorder_from_guard_mut(&mut recorder_guard, "Toggle-adjudicate")?;

            let phase2 = std::time::Instant::now();
            info!("stop_toggle_inner: PHASE 2 — calling recorder.stop() (cpal drain + WAV save)");
            let (streaming_text, raw_audio_path_opt) =
                recorder.stop().await.context("Failed to stop recorder")?;
            let rec_stop_secs = phase2.elapsed().as_secs_f64();
            info!(
                "stop_toggle_inner: PHASE 2 — recorder.stop() returned in {:?} (streaming_text={} chars, has_wav={})",
                phase2.elapsed(),
                streaming_text.len(),
                raw_audio_path_opt.is_some()
            );

            Self::clear_recorder_callbacks(recorder);
            drop(recorder_guard);

            let phase3 = std::time::Instant::now();
            info!("stop_toggle_inner: PHASE 3 — reducer-owned transcript already delivered");
            if let Some(path) = raw_audio_path_opt.as_deref() {
                retain_last_session_audio(path);
            }
            let r = Ok(ProcessRecordingOutcome {
                transcript_present: !streaming_text.trim().is_empty(),
                ..ProcessRecordingOutcome::default()
            });
            let phase3_secs = phase3.elapsed().as_secs_f64();
            info!(
                "stop_toggle_inner: PHASE 3 — reducer handoff completed in {:?} (ok={})",
                phase3.elapsed(),
                r.is_ok()
            );
            (r, rec_stop_secs, phase3_secs)
        };

        let phase4 = std::time::Instant::now();
        self.toggle_user_has_text.store(false, Ordering::SeqCst);
        self.toggle_assistant_has_text
            .store(false, Ordering::SeqCst);
        self.reset_finished_recording_state().await;
        self.handle_processed_recording_result(assistive, &result)
            .await;
        let cleanup_secs = phase4.elapsed().as_secs_f64();
        let total_secs = stop_start.elapsed().as_secs_f64();
        info!(
            "stop_toggle_inner: PHASE 4 — cleanup + result handler completed in {:?} (total stop time: {:?}, cleanup={cleanup_secs:.3}s)",
            phase4.elapsed(),
            stop_start.elapsed()
        );
        info!(total_secs, rec_stop_secs, phase3_secs, cleanup_secs, "stop_toggle_inner: mechanical stop timing");

        result.map(|_| ())
    }

    /// Recovery path when stop_toggle_and_adjudicate exceeds STOP_TIMEOUT.
    ///
    /// Forces state to Idle and clears all toggle-related flags so subsequent
    /// toggle presses register cleanly. Does NOT attempt to recover the recorder —
    /// it may be in arbitrary state; a fresh `start_toggle_recording` reinitializes
    /// through the normal path. UI surfaces (badge, voice-chat status, overlay)
    /// are restored to Idle visuals so the user gets honest feedback that recording
    /// is no longer alive.
    async fn recover_from_stuck_stop(&self) {
        warn!("Recovery: forcing controller to Idle after stuck stop");
        self.reset_finished_recording_state().await;
    }

    /// Stop the current recording on behalf of a non-hotkey surface (tray, the
    /// SwiftUI overlay), routing to the same stop path the hotkey would have
    /// taken for this session shape.
    pub async fn stop_recording_from_external_surface(&self) -> Result<()> {
        let current_state = self.current_state().await;
        let assistive = *self.assistive_mode.read().await;
        if should_use_toggle_adjudicated_stop(current_state, assistive, toggle_final_pass_enabled())
        {
            self.stop_toggle_and_adjudicate().await
        } else {
            self.finish_recording().await
        }
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
        self.show_processing_badge_if_enabled().await;

        // Get session ID and mode flags before we reset them
        let session_id = self.session_id.read().await.clone();
        let assistive = *self.assistive_mode.read().await;
        let hold_mode = *self.hold_mode.read().await;
        let force_raw = *self.force_raw_mode.read().await;
        let force_ai = *self.force_ai_mode.read().await;

        let result = match tokio::time::timeout(
            STOP_TIMEOUT,
            self.process_recording(session_id, assistive, hold_mode, force_raw, force_ai),
        )
        .await
        {
            Ok(result) => result,
            Err(_) => {
                error!(
                    "Hold stop processing stalled >{}s — forcing recovery to Idle. \
                     Recording session abandoned; future hotkeys will start fresh.",
                    STOP_TIMEOUT.as_secs()
                );
                self.recover_from_stuck_stop().await;
                return Err(anyhow::anyhow!(
                    "Hold stop timeout after {}s; state forced to Idle",
                    STOP_TIMEOUT.as_secs()
                ));
            }
        };

        self.reset_finished_recording_state().await;
        self.handle_processed_recording_result(assistive, &result)
            .await;

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
        #[cfg(test)]
        if PROCESS_RECORDING_TEST_HANG.load(Ordering::SeqCst) {
            info!("process_recording: hanging in test until stuck-stop watchdog cancels it");
            std::future::pending::<()>().await;
        }

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
        Self::clear_recorder_callbacks(recorder);
        drop(recorder_guard); // Release lock

        if let Some(path) = raw_audio_path_opt.as_deref() {
            retain_last_session_audio(path);
        }
        let _ = (assistive, hold_mode, force_raw, force_ai);
        Ok(ProcessRecordingOutcome {
            transcript_present: !streaming_text.trim().is_empty(),
            ..ProcessRecordingOutcome::default()
        })
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
        self.reset_session_fields().await;

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
    /// Build a controller with production defaults (`RecordingController::new`).
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod c15d_settings_one_path_falsifiers {
    use super::*;
    use crate::config::Config;

    /// C15D-A structural falsifier: the controller source has one mutable
    /// settings handle, one write site and no compatibility setter.
    #[test]
    fn controller_has_exactly_one_mutable_settings_generation() {
        let source = include_str!("mod.rs");
        let retired_config_field = ["config: Arc<RwLock<", "Config>>"].concat();
        let runtime_arc_field = [
            "runtime_settings: RwLock<Arc<",
            "RuntimeSettingsSnapshot>>",
        ]
        .concat();
        let runtime_arc_write = ["*self.runtime_settings", ".write().await"].concat();
        let retired_setter = ["pub async ", "fn set_", "config"].concat();

        assert!(!source.contains(&retired_config_field));
        assert_eq!(source.matches(&runtime_arc_field).count(), 1);
        assert_eq!(source.matches(&runtime_arc_write).count(), 1);
        assert!(!source.contains(&retired_setter));
    }

    /// C15D-A projection falsifier: public config is a value projected from
    /// the current Arc, not separately stored controller state.
    #[tokio::test]
    async fn get_config_projects_current_runtime_snapshot_values() {
        let controller = RecordingController::new_without_keychain();
        let snapshot = controller.runtime_settings_arc().await;
        let projected = controller.get_config().await;

        assert_eq!(
            projected.hold_start_delay_ms,
            snapshot.values().hold_start_delay_ms
        );
        assert_eq!(projected.beep_on_start, snapshot.values().beep_on_start);
        assert_eq!(
            projected.transcription_overlay_enabled,
            snapshot.values().transcription_overlay_enabled
        );
    }

    /// C15D-A generation falsifier: idle refresh performs one Arc replacement
    /// and cannot mutate a previously selected Arc.
    #[tokio::test]
    async fn idle_refresh_replaces_one_arc_and_preserves_previous_generation() {
        let controller = RecordingController::new_without_keychain();
        let before = controller.runtime_settings_arc().await;
        let before_digest = before.digest().as_str().to_string();
        let before_delay = before.values().hold_start_delay_ms;
        let next = Config::load_runtime_snapshot_without_keychain()
            .expect("seal next generation");
        assert!(
            controller
                .replace_runtime_settings_when_idle(next.clone())
                .await
        );
        let after = controller.runtime_settings_arc().await;
        assert!(!Arc::ptr_eq(&before, &after));
        assert_eq!(after.digest().as_str(), next.digest().as_str());
        assert_eq!(
            controller.get_config().await.hold_start_delay_ms,
            after.values().hold_start_delay_ms
        );
        assert_eq!(before.digest().as_str(), before_digest.as_str());
        assert_eq!(before.values().hold_start_delay_ms, before_delay);
    }

    /// C15D-A lifecycle falsifier: an unfinished delayed hold owns its selected
    /// generation, so refresh defers and leaves the Arc untouched.
    #[tokio::test]
    async fn pending_hold_rejects_refresh_and_preserves_generation() {
        let controller = RecordingController::new_without_keychain();
        let before = controller.runtime_settings_arc().await;
        let (release, wait) = tokio::sync::oneshot::channel::<()>();
        let pending = tokio::spawn(async move {
            let _ = wait.await;
        });
        *controller.hold_start_task.lock().await = Some(pending);

        let next = Config::load_runtime_snapshot_without_keychain()
            .expect("seal deferred generation");
        assert!(!controller.replace_runtime_settings_when_idle(next).await);
        let after = controller.runtime_settings_arc().await;
        assert!(Arc::ptr_eq(&before, &after));

        let _ = release.send(());
        controller.cancel_pending_hold_start().await;
    }

    /// C15D-A stale-handle falsifier: a completed delayed task is cleanup, not
    /// live ownership, and therefore cannot block an idle refresh forever.
    #[tokio::test]
    async fn finished_hold_handle_does_not_block_idle_refresh() {
        let controller = RecordingController::new_without_keychain();
        let finished = tokio::spawn(async {});
        while !finished.is_finished() {
            tokio::task::yield_now().await;
        }
        *controller.hold_start_task.lock().await = Some(finished);

        let next = Config::load_runtime_snapshot_without_keychain()
            .expect("seal post-hold generation");
        assert!(controller.replace_runtime_settings_when_idle(next).await);
        assert!(controller.hold_start_task.lock().await.is_none());
    }

    /// C15D-A active-take falsifier: both recording states keep their current
    /// Arc until the existing serialized stop/finalize path returns to Idle.
    #[tokio::test]
    async fn active_hold_and_toggle_reject_refresh() {
        let controller = RecordingController::new_without_keychain();
        for active_state in [State::RecHold, State::RecToggle] {
            *controller.state.write().await = active_state;
            let before = controller.runtime_settings_arc().await;
            let next = Config::load_runtime_snapshot_without_keychain()
                .expect("seal later generation");
            assert!(!controller.replace_runtime_settings_when_idle(next).await);
            let after = controller.runtime_settings_arc().await;
            assert!(Arc::ptr_eq(&before, &after));
        }
        *controller.state.write().await = State::Idle;
    }

    /// C15D-A source falsifier: each start body selects exactly one Arc under
    /// `serial_lock`, then derives config, UserSettings and recorder binding
    /// from that named Arc. This is structural evidence, not a scheduler proof.
    #[test]
    fn hold_and_toggle_each_derive_take_facts_from_one_selected_arc() {
        let source = include_str!("mod.rs");
        let hold_signature = ["async fn schedule_hold_", "start"].concat();
        let toggle_signature = ["async fn start_toggle_", "recording"].concat();
        let stop_signature = ["async fn stop_toggle_", "and_adjudicate"].concat();
        let hold_start = source.find(&hold_signature).expect("hold start body");
        let toggle_start = source.find(&toggle_signature).expect("toggle start body");
        let stop_start = source[toggle_start..]
            .find(&stop_signature)
            .map(|offset| toggle_start + offset)
            .expect("toggle stop boundary");
        let hold_body = &source[hold_start..toggle_start];
        let toggle_body = &source[toggle_start..stop_start];

        let serial_lock = ["self.serial_lock", ".lock().await"].concat();
        let select_arc = ["self.runtime_settings_", "arc().await"].concat();
        let config_projection = ["runtime_settings", ".values()"].concat();
        let user_projection = ["runtime_settings", ".user_settings()"].concat();
        let recorder_binding = ["Arc::clone(&runtime_", "settings)"].concat();

        for body in [hold_body, toggle_body] {
            assert_eq!(body.matches(&select_arc).count(), 1);
            assert_eq!(body.matches(&config_projection).count(), 1);
            assert_eq!(body.matches(&user_projection).count(), 1);
            assert_eq!(body.matches(&recorder_binding).count(), 1);
            assert!(
                body.find(&serial_lock).expect("lifecycle lock")
                    < body.find(&select_arc).expect("settings Arc selection")
            );
        }
    }

    /// C15D falsifier: profile publish uses one snapshot's values and settings.
    #[test]
    fn recording_profile_uses_controller_snapshot_user_settings() {
        let snapshot = Config::load_runtime_snapshot_without_keychain()
            .expect("seal runtime settings");
        let enabled = apply_runtime_transcription_profile(
            snapshot.values(),
            snapshot.user_settings(),
            false,
        );
        assert_eq!(
            enabled,
            snapshot.values().transcription_overlay_enabled
        );
    }
}
